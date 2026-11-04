//! macOS PTY teardown. Keep the shell unreaped until all session signals are sent.
use super::events::Publisher;
use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{self, ChildEvent, EventedPty, EventedReadWrite};
use polling::os::kqueue::{PollerKqueueExt, Process, ProcessOps};
use polling::{Event, PollMode, Poller};
use rustix::process::{self, Pid, Signal, WaitId, WaitIdOptions};
use std::collections::HashSet;
use std::fs::File;
use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

// EventLoop in alacritty_terminal 0.26 dispatches these private Unix token values.
const PTY_READ_WRITE_TOKEN: usize = 0;
const PTY_CHILD_EVENT_TOKEN: usize = 1;

#[derive(Default)]
pub(super) struct Completion {
    closed: Mutex<bool>,
    changed: Condvar,
}

impl Completion {
    pub(super) fn is_closed(&self) -> bool {
        *self.closed.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn wait_until(&self, deadline: Instant) -> bool {
        let closed = self.closed.lock().unwrap_or_else(|e| e.into_inner());
        let (closed, _) = self
            .changed
            .wait_timeout_while(
                closed,
                deadline.saturating_duration_since(Instant::now()),
                |closed| !*closed,
            )
            .unwrap_or_else(|e| e.into_inner());
        *closed
    }

    fn finish(&self) {
        *self.closed.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.changed.notify_all();
    }
}

pub(super) struct ManagedPty {
    pty: Option<tty::Pty>,
    completion: Arc<Completion>,
    events: Publisher,
}

impl ManagedPty {
    pub(super) fn new(pty: tty::Pty, completion: Arc<Completion>, events: Publisher) -> Self {
        Self {
            pty: Some(pty),
            completion,
            events,
        }
    }

    fn inner(&self) -> &tty::Pty {
        self.pty.as_ref().expect("PTY exists until teardown")
    }

    fn pid(&self) -> Pid {
        Pid::from_child(self.inner().child())
    }

    fn process_filter(&self) -> Process<'_> {
        // EventLoop deregisters before dropping the PTY; we never reap the child here.
        unsafe { Process::new(self.inner().child(), ProcessOps::Exit) }
    }

    fn has_exited(&self) -> io::Result<bool> {
        process::waitid(
            WaitId::Pid(self.pid()),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        )
        // Darwin can report stopped children despite WEXITED; do not treat that as exit.
        .map(|status| {
            status.is_some_and(|status| status.exited() || status.killed() || status.dumped())
        })
        .map_err(Into::into)
    }

    fn terminate_session(&self) {
        let session = self.pid();
        let members = session_members(session);
        signal_groups(session, &members, Signal::HUP);
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            let shell_running = !self.has_exited().unwrap_or_else(|error| {
                tracing::warn!(pid = session.as_raw_nonzero().get(), %error, "could not inspect PTY child");
                false
            });
            let other_running = members
                .iter()
                .any(|pid| *pid != session && process::getsid(Some(*pid)).ok() == Some(session));
            if !shell_running && !other_running {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Re-scan after the grace period: a signal handler may have started another job.
        signal_groups(session, &session_members(session), Signal::KILL);
    }
}

impl EventedReadWrite for ManagedPty {
    type Reader = File;
    type Writer = File;

    unsafe fn register(
        &mut self,
        poll: &Arc<Poller>,
        mut event: Event,
        mode: PollMode,
    ) -> io::Result<()> {
        event.key = PTY_READ_WRITE_TOKEN;
        // The file remains alive through EventLoop's deregistration.
        unsafe {
            poll.add_with_mode(self.inner().file(), event, mode)?;
        }
        if let Err(error) = poll.add_filter(
            self.process_filter(),
            PTY_CHILD_EVENT_TOKEN,
            PollMode::Oneshot,
        ) {
            let _ = poll.delete(self.inner().file());
            return Err(error);
        }
        Ok(())
    }

    fn reregister(
        &mut self,
        poll: &Arc<Poller>,
        mut event: Event,
        mode: PollMode,
    ) -> io::Result<()> {
        event.key = PTY_READ_WRITE_TOKEN;
        poll.modify_with_mode(self.inner().file(), event, mode)
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        let file_result = poll.delete(self.inner().file());
        let process_result = poll.delete_filter(self.process_filter());
        file_result.and(process_result)
    }

    fn reader(&mut self) -> &mut File {
        self.pty
            .as_mut()
            .expect("PTY exists until teardown")
            .reader()
    }
    fn writer(&mut self) -> &mut File {
        self.pty
            .as_mut()
            .expect("PTY exists until teardown")
            .writer()
    }
}

impl EventedPty for ManagedPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        match self.has_exited() {
            Ok(true) => Some(ChildEvent::Exited(None)),
            Ok(false) => None,
            Err(error) => {
                tracing::error!(%error, "failed to inspect PTY child exit");
                None
            }
        }
    }
}

impl OnResize for ManagedPty {
    fn on_resize(&mut self, size: WindowSize) {
        self.pty
            .as_mut()
            .expect("PTY exists until teardown")
            .on_resize(size);
    }
}

impl Drop for ManagedPty {
    fn drop(&mut self) {
        self.terminate_session();
        // Darwin can keep a killed child in tty drain after the reader stops consuming output.
        if let Err(error) =
            rustix::termios::tcflush(self.inner().file(), rustix::termios::QueueSelector::IOFlush)
        {
            tracing::warn!(%error, "failed to flush closing PTY");
        }
        // Upstream sends HUP and waits here. The owned PID cannot be reused until this wait.
        drop(self.pty.take());
        self.events.close();
        self.completion.finish();
    }
}

fn session_members(session: Pid) -> Vec<Pid> {
    let result = crate::host::Host::Local
        .command("/bin/ps", &["-ax", "-o", "pid="], None)
        .output();
    let mut members = vec![session];
    match result {
        Ok(output) if output.status.success() => {
            members.extend(
                String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .filter_map(|pid| pid.parse().ok().and_then(Pid::from_raw))
                    .filter(|pid| {
                        *pid != session && process::getsid(Some(*pid)).ok() == Some(session)
                    }),
            );
        }
        Ok(output) => {
            tracing::error!(stderr = %String::from_utf8_lossy(&output.stderr), "failed to list PTY session processes")
        }
        Err(error) => tracing::error!(%error, "failed to list PTY session processes"),
    }
    members
}

fn signal_groups(session: Pid, members: &[Pid], signal: Signal) {
    let groups: HashSet<_> = members
        .iter()
        .filter(|pid| process::getsid(Some(**pid)).ok() == Some(session))
        .filter_map(|pid| process::getpgid(Some(*pid)).ok())
        .collect();
    for group in groups {
        if let Err(error) = process::kill_process_group(group, signal)
            && error != rustix::io::Errno::SRCH
        {
            tracing::error!(pid = group.as_raw_nonzero().get(), %error, ?signal, "failed to terminate PTY process group");
        }
    }
}
