use super::{Error, Family, Message, wire};
use crate::host::Host;
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    process::{Pid, Signal, kill_process_group},
};
use std::{
    io::{BufReader, Read, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, ChildStdin, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    time::{Duration, Instant},
};

/// The first stderr bytes say why a server could not start (rustup's missing component, a
/// shell's "not found"); later output may quote source and is never kept.
const STDERR_HEAD: usize = 1024;

pub(super) struct Process {
    child: Child,
    input: ChildStdin,
    stopped: Arc<AtomicBool>,
    stderr_head: Receiver<Vec<u8>>,
}

impl Process {
    pub(super) fn start(
        host: &Host,
        root: &Path,
        family: Family,
        generation: u8,
        sender: SyncSender<Message>,
        stopped: Arc<AtomicBool>,
    ) -> Result<Self, Error> {
        let script = match family {
            // A rustup proxy must report a missing toolchain, never download one.
            Family::Rust => "export RUSTUP_AUTO_INSTALL=0; exec rust-analyzer",
            // Only the worktree's own install or PATH: a parent directory is outside the project.
            Family::TypeScript => {
                "if [ -x node_modules/.bin/typescript-language-server ]; then exec node_modules/.bin/typescript-language-server --stdio; fi; exec typescript-language-server --stdio"
            }
        };
        let script = if host.is_local() {
            script.to_owned()
        } else {
            let action = shlex::try_join(["sh", "-c", script])
                .map_err(|e| Error::Protocol(e.to_string()))?;
            format!(
                "exec \"${{SHELL:-/bin/sh}}\" -lc {}",
                shlex::try_quote(&action).map_err(|e| Error::Protocol(e.to_string()))?
            )
        };
        let fail = |error: std::io::Error| Error::Session {
            host: host.clone(),
            root: root.into(),
            message: error.to_string(),
        };
        let mut child = host
            .command("sh", &["-c", &script], Some(root))
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(fail)?;
        let Some(input) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Unavailable("server has no stdin"));
        };
        let (head_sender, stderr_head) = sync_channel(1);
        let mut process = Self {
            child,
            input,
            stopped,
            stderr_head,
        };
        fcntl_setfl(
            &process.input,
            fcntl_getfl(&process.input).map_err(|e| fail(e.into()))? | OFlags::NONBLOCK,
        )
        .map_err(|e| fail(e.into()))?;
        let output = process
            .child
            .stdout
            .take()
            .ok_or(Error::Unavailable("server has no stdout"))?;
        let stderr = process
            .child
            .stderr
            .take()
            .ok_or(Error::Unavailable("server has no stderr"))?;
        std::thread::Builder::new()
            .name("hive-lsp-read".into())
            .spawn(move || {
                let mut reader = BufReader::new(output);
                loop {
                    let result = wire::read(&mut reader);
                    let ended = !matches!(result, Ok(Some(_)));
                    if sender.send(Message::Incoming(generation, result)).is_err() || ended {
                        break;
                    }
                }
            })
            .map_err(fail)?;
        std::thread::Builder::new()
            .name("hive-lsp-stderr".into())
            .spawn(move || {
                // Drain without retaining arbitrary server logs or source-bearing diagnostics.
                let mut reader = stderr;
                let mut buffer = [0; 4096];
                let mut head = Vec::new();
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => {
                            let kept = count.min(STDERR_HEAD - head.len());
                            head.extend_from_slice(&buffer[..kept]);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(error) => {
                            tracing::debug!(%error,"language-server stderr closed");
                            break;
                        }
                    }
                }
                // The session may already be gone; nobody is waiting then.
                let _ = head_sender.send(head);
            })
            .map_err(fail)?;
        Ok(process)
    }

    /// What the server printed first, once it has exited; `None` while it still runs.
    pub(super) fn startup_output(&self) -> Option<String> {
        let head = self
            .stderr_head
            .recv_timeout(Duration::from_millis(200))
            .ok()?;
        let text: String = String::from_utf8_lossy(&head)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .filter(|character| !character.is_control())
            .collect();
        (!text.is_empty()).then_some(text)
    }

    pub(super) fn write(&mut self, message: &impl serde::Serialize) -> Result<(), Error> {
        self.write_with_deadline(message, Duration::from_secs(5), true)
    }

    pub(super) fn closing(&mut self, message: &impl serde::Serialize) -> Result<(), Error> {
        self.write_with_deadline(message, Duration::from_millis(100), false)
    }

    fn write_with_deadline(
        &mut self,
        message: &impl serde::Serialize,
        timeout: Duration,
        cancellable: bool,
    ) -> Result<(), Error> {
        let bytes = wire::encode(message)?;
        let deadline = Instant::now() + timeout;
        let mut remaining = bytes.as_slice();
        while !remaining.is_empty() {
            if cancellable && self.stopped.load(Ordering::Acquire) {
                return Err(Error::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(Error::TimedOut);
            }
            match self.input.write(remaining) {
                Ok(0) => return Err(Error::Unavailable("server closed stdin")),
                Ok(count) => remaining = &remaining[count..],
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    let timeout = Timespec::try_from(Duration::from_millis(25))
                        .map_err(|e| Error::Protocol(e.to_string()))?;
                    // A signal only ends this wait early; the loop checks the deadline again.
                    match poll(
                        &mut [PollFd::new(&self.input, PollFlags::OUT)],
                        Some(&timeout),
                    ) {
                        Ok(_) | Err(rustix::io::Errno::INTR) => {}
                        Err(error) => return Err(Error::Protocol(error.to_string())),
                    }
                }
                Err(e) => return Err(Error::Protocol(e.to_string())),
            }
        }
        Ok(())
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // Keep the child unreaped until its group is signalled, avoiding PID reuse.
        if let Some(pid) = Pid::from_raw(self.child.id() as i32)
            && let Err(error) = kill_process_group(pid, Signal::KILL)
            && error != rustix::io::Errno::SRCH
        {
            tracing::debug!(%error,"language-server group cleanup");
        }
        if let Err(error) = self.child.wait() {
            tracing::warn!(%error,"could not reap language server");
        }
    }
}
