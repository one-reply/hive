//! macOS PTY teardown. Keep the shell unreaped until all session signals are sent.
use super::Listener;
use super::events::Publisher;
use super::program_status::{self, ProgramStatus, Scanner};
use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite};
use polling::os::kqueue::{PollerKqueueExt, Process, ProcessOps, Timer};
use polling::{Event, PollMode, Poller};
use rustix::process::{self, Pid, Signal, WaitId, WaitIdOptions};
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

// EventLoop in alacritty_terminal 0.26 dispatches these private Unix token values.
const PTY_READ_WRITE_TOKEN: usize = 0;
const PTY_CHILD_EVENT_TOKEN: usize = 1;
/// Stands in for the exit note of a shell that was exiting before it could be watched.
const EXITING: Timer = Timer {
    id: PTY_CHILD_EVENT_TOKEN,
    timeout: Duration::ZERO,
};

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

/// The PTY's output as alacritty reads it, with OSC 7501 reports picked out on the way: its
/// parser would drop them. The support query is answered from here too, ahead of any reply the
/// parser queues for the same chunk, which is the order a probing program expects.
pub(super) struct StatusReader {
    file: File,
    scanner: Scanner,
    status: ProgramStatus,
    listener: Listener,
}

impl Read for StatusReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.file.read(buf)?;
        let Self {
            scanner,
            status,
            listener,
            ..
        } = self;
        scanner.feed(&buf[..read], |signal| match signal {
            program_status::Signal::Report(state) => status.push(state),
            program_status::Signal::Query => listener.write(program_status::QUERY_REPLY.to_vec()),
        });
        Ok(read)
    }
}

/// What tells the event loop that the shell is exiting.
#[derive(Clone, Copy)]
enum ExitWatch {
    /// The kernel's exit note for the shell.
    Process,
    /// A timer that fires at once, for a shell that was already exiting at registration: the
    /// kernel then refuses to watch it with `ESRCH`.
    Timer,
}

pub(super) struct ManagedPty {
    child: std::process::Child,
    reader: StatusReader,
    completion: Arc<Completion>,
    events: Publisher,
    exit_watch: ExitWatch,
}

impl ManagedPty {
    pub(super) fn new(
        child: std::process::Child,
        file: File,
        completion: Arc<Completion>,
        events: Publisher,
        listener: Listener,
        status: ProgramStatus,
    ) -> Self {
        Self {
            child,
            reader: StatusReader {
                file,
                scanner: Scanner::default(),
                status,
                listener,
            },
            completion,
            events,
            exit_watch: ExitWatch::Process,
        }
    }

    fn pid(&self) -> Pid {
        Pid::from_child(&self.child)
    }

    fn process_filter(&self) -> Process<'_> {
        // EventLoop deregisters before dropping the PTY; we never reap the child here.
        unsafe { Process::new(&self.child, ProcessOps::Exit) }
    }

    fn delete_exit_watch(&self, poll: &Poller) -> io::Result<()> {
        match self.exit_watch {
            ExitWatch::Process => poll.delete_filter(self.process_filter()),
            ExitWatch::Timer => poll.delete_filter(EXITING),
        }
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
    type Reader = StatusReader;
    type Writer = File;

    unsafe fn register(
        &mut self,
        poll: &Arc<Poller>,
        mut event: Event,
        mode: PollMode,
    ) -> io::Result<()> {
        event.key = PTY_READ_WRITE_TOKEN;
        // The exit watch goes first, so a shell already exiting is seen exiting before its
        // output is read, the same order as when the exit note overtakes the last read; the
        // drain reads that output either way.
        match poll.add_filter(
            self.process_filter(),
            PTY_CHILD_EVENT_TOKEN,
            PollMode::Oneshot,
        ) {
            // A quick command can be exiting already, held by output nobody has read yet.
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {
                self.exit_watch = ExitWatch::Timer;
                poll.add_filter(EXITING, PTY_CHILD_EVENT_TOKEN, PollMode::Oneshot)?;
            }
            watched => watched?,
        }
        // The file remains alive through EventLoop's deregistration.
        if let Err(error) = unsafe { poll.add_with_mode(&self.reader.file, event, mode) } {
            let _ = self.delete_exit_watch(poll);
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
        poll.modify_with_mode(&self.reader.file, event, mode)
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        let file_result = poll.delete(&self.reader.file);
        file_result.and(self.delete_exit_watch(poll))
    }

    fn reader(&mut self) -> &mut StatusReader {
        &mut self.reader
    }
    fn writer(&mut self) -> &mut File {
        &mut self.reader.file
    }
}

impl EventedPty for ManagedPty {
    /// Only the exit watch calls this, and it fires once: the shell is exiting. It may not be
    /// waitable yet, because Darwin holds an exiting process until its terminal output is
    /// read, and that read is the drain the event loop does next. Waiting for the exit status
    /// here instead left the exit unnoticed for good and the reader spinning on end-of-file.
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        Some(ChildEvent::Exited(None))
    }
}

impl OnResize for ManagedPty {
    fn on_resize(&mut self, size: WindowSize) {
        if let Err(error) =
            rustix::termios::tcsetwinsize(&self.reader.file, super::process::winsize(size))
        {
            tracing::warn!(%error, "failed to resize PTY");
        }
    }
}

impl Drop for ManagedPty {
    fn drop(&mut self) {
        self.terminate_session();
        // Darwin can keep a killed child in tty drain after the reader stops consuming output.
        if let Err(error) =
            rustix::termios::tcflush(&self.reader.file, rustix::termios::QueueSelector::IOFlush)
        {
            tracing::warn!(%error, "failed to flush closing PTY");
        }
        // Keep the PID owned until every session signal has been sent.
        if let Err(error) = self.child.wait() {
            tracing::error!(%error, "failed to reap PTY child");
        }
        self.events.close();
        self.completion.finish();
    }
}

fn session_members(session: Pid) -> Vec<Pid> {
    let mut members = vec![session];
    match crate::agent::ProcessTable::snapshot() {
        Ok(table) => members.extend(
            table
                .pids()
                .filter_map(|pid| i32::try_from(pid).ok().and_then(Pid::from_raw))
                .filter(|pid| *pid != session && process::getsid(Some(*pid)).ok() == Some(session)),
        ),
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
