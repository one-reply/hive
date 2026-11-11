//! Bounded command IO, deadlines and cancellation shared by CLI adapters.
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    process::{self, Pid, Signal, WaitId, WaitIdOptions},
};
use std::{
    io::{self, Read, Write},
    os::{fd::AsFd, unix::process::CommandExt},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// How long a silent command is left alone before cancellation and the deadline are checked
/// again. Output, a writable stdin and a closed pipe all end the wait at once.
const WAIT_SLICE: Duration = Duration::from_millis(25);
/// First recheck of a child that closed its pipes but cannot be reaped yet; the wait doubles
/// up to `WAIT_SLICE` for one that lingers.
const EXIT_RECHECK: Duration = Duration::from_millis(1);

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

/// Reads what `stream` has ready and closes it at end of file, so it stops being waited on.
fn drain(stream: &mut Option<impl Read>, output: &mut Vec<u8>, limit: usize) -> Result<(), Error> {
    let Some(reader) = stream else {
        return Ok(());
    };
    let mut buffer = [0; 8192];
    // Bound each pass so a noisy child cannot starve cancellation or the deadline.
    for _ in 0..64 {
        match reader.read(&mut buffer) {
            Ok(0) => {
                *stream = None;
                break;
            }
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
    Ok(())
}

/// The child's pipes that are still open.
struct Pipes {
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
}

impl Pipes {
    /// Sleeps until a pipe can make progress or `limit` passes. Returns early on a signal;
    /// the caller loops anyway.
    fn wait(&self, limit: Duration) -> io::Result<()> {
        let mut pipes: Vec<PollFd<'_>> = Vec::with_capacity(3);
        if let Some(stdin) = &self.stdin {
            pipes.push(PollFd::new(stdin, PollFlags::OUT));
        }
        if let Some(stdout) = &self.stdout {
            pipes.push(PollFd::new(stdout, PollFlags::IN));
        }
        if let Some(stderr) = &self.stderr {
            pipes.push(PollFd::new(stderr, PollFlags::IN));
        }
        // A slice of milliseconds always fits; the fallback only keeps this infallible.
        let timeout = Timespec::try_from(limit).unwrap_or(Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        });
        match poll(&mut pipes, Some(&timeout)) {
            Ok(_) | Err(rustix::io::Errno::INTR) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn output_closed(&self) -> bool {
        self.stdout.is_none() && self.stderr.is_none()
    }
}

/// Blocking: service stdin and both output pipes until EOF, cancellation or the deadline.
pub fn run(
    command: Command,
    input: &[u8],
    cancellation: &Cancellation,
    timeout: Duration,
    limit: usize,
) -> Result<Output, Error> {
    run_with_progress(command, input, cancellation, timeout, limit, |_, _| {})
}

/// Like [`run`], reporting bounded cumulative stdout/stderr when either stream changes.
/// The callback must not block; authentication callers use it to expose a device code.
pub fn run_with_progress(
    mut command: Command,
    input: &[u8],
    cancellation: &Cancellation,
    timeout: Duration,
    limit: usize,
    mut progress: impl FnMut(&[u8], &[u8]),
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
    let mut pipes = Pipes {
        stdin: running.child.stdin.take(),
        stdout: running.child.stdout.take(),
        stderr: running.child.stderr.take(),
    };
    nonblocking(pipes.stdin.as_ref().expect("stdin is piped")).map_err(io_error)?;
    nonblocking(pipes.stdout.as_ref().expect("stdout is piped")).map_err(io_error)?;
    nonblocking(pipes.stderr.as_ref().expect("stderr is piped")).map_err(io_error)?;
    let (mut output, mut errors, mut written) = (Vec::new(), Vec::new(), 0);
    let deadline = Instant::now() + timeout;
    let mut exit_recheck = EXIT_RECHECK;
    loop {
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Error::TimedOut);
        }
        let previous = (output.len(), errors.len());
        drain(&mut pipes.stdout, &mut output, limit)?;
        drain(&mut pipes.stderr, &mut errors, limit)?;
        if previous != (output.len(), errors.len()) {
            progress(&output, &errors);
        }
        if let Some(pipe) = &mut pipes.stdin {
            match pipe.write(&input[written..]) {
                Ok(count) => written += count,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                    pipes.stdin = None;
                }
                Err(e) => return Err(io_error(e)),
            }
            if written == input.len() {
                pipes.stdin = None;
            }
        }
        let exited = process::waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        )
        .map_err(|e| io_error(e.into()))?;
        if let Some(status) = exited
            && (status.exited() || status.killed() || status.dumped())
            && pipes.output_closed()
        {
            running.exited = true;
            return Ok(Output {
                code: status.exit_status(),
                stdout: output,
                stderr: errors,
            });
        }
        if pipes.output_closed() {
            // Nothing announces that a child became reapable, so look again shortly.
            std::thread::sleep(exit_recheck.min(remaining));
            exit_recheck = (exit_recheck * 2).min(WAIT_SLICE);
        } else {
            pipes.wait(WAIT_SLICE.min(remaining)).map_err(io_error)?;
        }
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
    fn progress_is_available_before_a_waiting_child_completes_and_can_cancel_it() {
        let cancellation = Cancellation::default();
        let mut seen = false;
        let result = run_with_progress(
            Host::Local.command(
                "/bin/sh",
                &["-c", "printf code >&2; while :; do :; done"],
                None,
            ),
            &[],
            &cancellation,
            Duration::from_secs(5),
            1024,
            |_, errors| {
                if errors == b"code" {
                    seen = true;
                    cancellation.cancel();
                }
            },
        );
        assert!(seen);
        assert!(matches!(result, Err(Error::Cancelled)));
    }

    #[test]
    fn cancelled_command_never_spawns() {
        let cancellation = Cancellation::default();
        cancellation.cancel();
        let result = run(
            std::process::Command::new("hive-must-not-spawn"),
            &[],
            &cancellation,
            Duration::ZERO,
            1,
        );
        assert!(matches!(result, Err(Error::Cancelled)));
        assert!(cancellation.group.lock().unwrap().is_none());
    }

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

    fn shell(script: &str, input: &[u8], limit: usize) -> Result<Output, Error> {
        run(
            Host::Local.command("/bin/sh", &["-c", script], None),
            input,
            &Cancellation::default(),
            Duration::from_secs(10),
            limit,
        )
    }

    #[test]
    fn stdout_and_stderr_are_collected_separately_with_the_exit_code() {
        let output = shell("printf out; printf err >&2; exit 3", b"", 1024).unwrap();
        assert_eq!(output.stdout, b"out");
        assert_eq!(output.stderr, b"err");
        assert_eq!(output.code, Some(3));
    }

    #[test]
    fn output_written_after_a_silent_pause_is_collected() {
        let output = shell("sleep 0.1; printf late", b"", 1024).unwrap();
        assert_eq!(output.stdout, b"late");
        assert_eq!(output.code, Some(0));
    }

    #[test]
    fn child_that_closes_its_output_early_still_reports_its_exit_code() {
        let output = shell("exec >&- 2>&-; sleep 0.1; exit 7", b"", 1024).unwrap();
        assert!(output.stdout.is_empty());
        assert_eq!(output.code, Some(7));
    }

    #[test]
    fn input_the_child_never_reads_does_not_fail_the_command() {
        let input = vec![b'x'; 256 * 1024];
        let output = shell("exec <&-; printf done", &input, 1024).unwrap();
        assert_eq!(output.stdout, b"done");
        assert_eq!(output.code, Some(0));
    }

    #[test]
    fn output_beyond_the_limit_is_rejected() {
        let result = shell("printf 12345", b"", 4);
        assert!(matches!(result, Err(Error::TooLarge(4))));
    }

    #[test]
    fn cancelling_a_running_command_returns_cancelled() {
        let control = Cancellation::default();
        let result = std::thread::scope(|scope| {
            let running = scope.spawn(|| {
                run(
                    Host::Local.command("/bin/sleep", &["30"], None),
                    b"",
                    &control,
                    Duration::from_secs(30),
                    1024,
                )
            });
            while control.group.lock().unwrap().is_none() {
                std::thread::yield_now();
            }
            control.cancel();
            running.join().unwrap()
        });
        assert!(matches!(result, Err(Error::Cancelled)));
        assert!(control.group.lock().unwrap().is_none());
    }
}
