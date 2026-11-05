//! Bounded command IO, deadlines and cancellation shared by CLI adapters.
use rustix::{
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    process::{self, Pid, Signal, WaitId, WaitIdOptions},
};
use std::{
    io::{self, Read, Write},
    os::{fd::AsFd, unix::process::CommandExt},
    process::{Child, Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Failure while supervising a command. Adapters add command, host and path context.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Process or pipe IO failed.
    #[error("{0}")]
    Io(#[from] io::Error),
    /// The owner cancelled this command.
    #[error("Command cancelled")]
    Cancelled,
    /// The deadline elapsed, including pipe draining.
    #[error("Command timed out")]
    TimedOut,
    /// A stream exceeded its memory budget.
    #[error("Command output exceeds {0} bytes")]
    TooLarge(usize),
}

/// Completed process output. A signal termination has no exit code.
pub struct Output {
    /// Process exit code, when it exited normally.
    pub code: Option<i32>,
    /// Raw standard output, without trimming patches or binary data.
    pub stdout: Vec<u8>,
    /// Raw standard error, bounded independently of stdout.
    pub stderr: Vec<u8>,
}
/// Thread-safe cancellation; a registered child is killed immediately, including at app quit.
#[derive(Default)]
pub struct Cancellation {
    cancelled: AtomicBool,
    group: Mutex<Option<Pid>>,
}

impl Cancellation {
    /// Cancels this operation and terminates its CLI process group, if started.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let group = self.group.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(group) = *group {
            kill(group, false);
        }
    }
    /// Whether the owner has cancelled this operation.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

fn kill(group: Pid, exited: bool) {
    match process::kill_process_group(group, Signal::KILL) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => {}
        // macOS reports EPERM for a group containing only the unreaped zombie.
        Err(error) if exited && error == rustix::io::Errno::PERM => {
            tracing::debug!(%error, "completed command group has no signalable process");
        }
        Err(error) => tracing::warn!(%error, "could not terminate command"),
    }
}

struct Running<'a> {
    child: Child,
    cancellation: &'a Cancellation,
    exited: bool,
}
impl Drop for Running<'_> {
    fn drop(&mut self) {
        // Keep the child unreaped until cancellation can no longer signal a reused PID.
        let mut group = self
            .cancellation
            .group
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(group) = group.take() {
            kill(group, self.exited);
        }
        if let Err(error) = self.child.wait() {
            tracing::warn!(%error, "could not reap command");
        }
    }
}

fn nonblocking(fd: &impl AsFd) -> io::Result<()> {
    fcntl_setfl(fd, fcntl_getfl(fd)? | OFlags::NONBLOCK).map_err(Into::into)
}

fn drain(reader: &mut impl Read, output: &mut Vec<u8>, limit: usize) -> Result<bool, Error> {
    let mut buffer = [0; 8192];
    // Bound each pass so a noisy child cannot starve cancellation or the deadline.
    for _ in 0..64 {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(count) => {
                if output.len() + count > limit {
                    return Err(Error::TooLarge(limit));
                }
                output.extend_from_slice(&buffer[..count]);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Ok(false)
}

/// Blocking: service stdin and both output pipes until EOF, cancellation or the deadline.
pub fn run(
    mut command: Command,
    input: &[u8],
    cancellation: &Cancellation,
    timeout: Duration,
    limit: usize,
) -> Result<Output, Error> {
    let io_error = Error::Io;
    if cancellation.is_cancelled() {
        return Err(Error::Cancelled);
    }
    command
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().map_err(io_error)?;
    let pid = Pid::from_child(&child);
    {
        let mut group = cancellation.group.lock().unwrap_or_else(|e| e.into_inner());
        *group = Some(pid);
        if cancellation.is_cancelled() {
            kill(pid, false);
        }
    }
    let mut running = Running {
        child,
        cancellation,
        exited: false,
    };
    let mut stdin = running.child.stdin.take();
    let mut stdout = running.child.stdout.take().expect("stdout is piped");
    let mut stderr = running.child.stderr.take().expect("stderr is piped");
    nonblocking(stdin.as_ref().expect("stdin is piped")).map_err(io_error)?;
    nonblocking(&stdout).map_err(io_error)?;
    nonblocking(&stderr).map_err(io_error)?;
    let (mut output, mut errors, mut written) = (Vec::new(), Vec::new(), 0);
    let deadline = Instant::now() + timeout;
    loop {
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(Error::TimedOut);
        }
        let output_done = drain(&mut stdout, &mut output, limit)?;
        let errors_done = drain(&mut stderr, &mut errors, limit)?;
        if let Some(pipe) = &mut stdin {
            match pipe.write(&input[written..]) {
                Ok(count) => written += count,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                    stdin = None;
                }
                Err(e) => return Err(io_error(e)),
            }
            if written == input.len() {
                stdin = None;
            }
        }
        let exited = process::waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        )
        .map_err(|e| io_error(e.into()))?;
        if let Some(status) = exited
            && (status.exited() || status.killed() || status.dumped())
            && output_done
            && errors_done
        {
            running.exited = true;
            return Ok(Output {
                code: status.exit_status(),
                stdout: output,
                stderr: errors,
            });
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Blocking: find a CLI on the login shell's PATH or standard desktop installation paths.
pub fn executable(name: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    let mut directories: Vec<_> = crate::host::environment::path()
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    directories.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    if let Some(home) = std::env::var_os("HOME") {
        directories.push(PathBuf::from(home).join(".local/bin"));
    }
    directories
        .into_iter()
        .filter(|p| p.is_absolute())
        .map(|p| p.join(name))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Host;

    #[test]
    fn orphaned_pipe_holders_cannot_outlive_the_deadline() {
        let control = Cancellation::default();
        let began = Instant::now();
        let result = run(
            Host::Local.command("/bin/sh", &["-c", "sleep 30 & exit 0"], None),
            b"",
            &control,
            Duration::from_millis(50),
            1024,
        );
        assert!(matches!(result, Err(Error::TimedOut)));
        assert!(began.elapsed() < Duration::from_secs(2));
        assert!(control.group.lock().unwrap().is_none());
    }

    #[test]
    fn complete_output_is_drained_after_child_exit() {
        let bytes = vec![b'x'; 900_000];
        let output = run(
            Host::Local.command("cat", &[], None),
            &bytes,
            &Cancellation::default(),
            Duration::from_secs(5),
            1_000_000,
        )
        .unwrap();
        assert_eq!(output.stdout, bytes);
        assert_eq!(output.code, Some(0));
    }
}
