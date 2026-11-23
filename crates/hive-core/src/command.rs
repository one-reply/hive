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
/// How long output is still read after the process exits. Everything it wrote is already in
/// the pipe; only another holder of the pipe keeps it open past that: a server the command left
/// running, or on macOS a sibling spawned while the pipe was not yet close-on-exec. Neither
/// may keep a finished command waiting for its deadline.
const EXIT_GRACE: Duration = Duration::from_secs(1);
/// A command allowed to run this long hides Hive's other descriptors from itself (see
/// [`keep_only_standard_streams`]): it could hold one that long. Shorter ones, every read of
/// Git among them, keep the faster `posix_spawn`.
const LONG_LIVED: Duration = Duration::from_secs(60);

/// Failure while supervising a command. Adapters add command, host and path context.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The process could not be started, so nothing of the command ran.
    #[error("{0}")]
    Spawn(#[source] io::Error),
    /// Pipe or process IO failed after the process started.
    #[error("{0}")]
    Io(#[from] io::Error),
    /// The owner cancelled this command before its process was started, so nothing of it ran.
    #[error("Command cancelled before it started")]
    CancelledBeforeStart,
    /// The owner cancelled this command while its process ran; the process was ended.
    #[error("Command cancelled")]
    Cancelled,
    /// The deadline elapsed while the process still ran.
    #[error("Command timed out")]
    TimedOut,
    /// A stream exceeded its memory budget.
    #[error("Command output exceeds {0} bytes")]
    TooLarge(usize),
}

impl Error {
    /// Whether the process was never started, so that nothing of the command ran. Any other
    /// failure ended a process that may already have acted.
    pub fn never_started(&self) -> bool {
        match self {
            Self::Spawn(_) | Self::CancelledBeforeStart => true,
            Self::Io(_) | Self::Cancelled | Self::TimedOut | Self::TooLarge(_) => false,
        }
    }
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

/// How a supervised command ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum End {
    /// The process exited, without a code when a signal ended it.
    Exited(Option<i32>),
    /// The deadline passed while the process still ran; its process group was terminated.
    TimedOut,
}

/// What a supervised command printed and how it ended. A command that ran out of time keeps
/// its output too: the end of its log is the only clue to what it was stuck on.
pub(crate) struct Log {
    pub(crate) end: End,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

impl Log {
    /// The output of a command that finished; a timeout stays an error for callers that need
    /// complete output.
    fn into_output(self) -> Result<Output, Error> {
        match self.end {
            End::Exited(code) => Ok(Output {
                code,
                stdout: self.stdout,
                stderr: self.stderr,
            }),
            End::TimedOut => Err(Error::TimedOut),
        }
    }

    /// Standard output followed by standard error, as one text for people and agents.
    #[cfg(feature = "desktop")]
    pub(crate) fn text(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.stdout).into_owned();
        if !self.stderr.is_empty() {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&String::from_utf8_lossy(&self.stderr));
        }
        text
    }
}

/// Thread-safe cancellation; a registered child is killed immediately, including at app quit.
#[derive(Default)]
pub struct Cancellation {
    cancelled: AtomicBool,
    /// The process groups of the commands running under it: one operation may run several
    /// at once, and stopping it stops them all.
    groups: Mutex<Vec<Pid>>,
}

impl Cancellation {
    /// Cancels this operation and terminates its CLI process groups, if started.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let groups = self.groups.lock().unwrap_or_else(|e| e.into_inner());
        for group in groups.iter() {
            kill(*group, false);
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
    /// The command exited and its output ended: nothing of it holds the pipes, and what it
    /// left in its process group was detached on purpose, such as a hook's `ctags >/dev/null &`.
    settled: bool,
}
impl Drop for Running<'_> {
    fn drop(&mut self) {
        let group = Pid::from_child(&self.child);
        // Unregistered before the child is reaped, so cancellation cannot signal a reused
        // PID; the lock is not held while reaping, so a cancel never waits for it.
        self.cancellation
            .groups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|registered| *registered != group);
        if !self.settled {
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

#[derive(Clone, Copy)]
enum Capture {
    Exact(usize),
    LogTail(usize),
}

impl Capture {
    fn append(self, output: &mut Vec<u8>, bytes: &[u8]) -> Result<(), Error> {
        match self {
            Self::Exact(limit) if output.len().saturating_add(bytes.len()) > limit => {
                Err(Error::TooLarge(limit))
            }
            Self::LogTail(limit) if output.len().saturating_add(bytes.len()) > limit => {
                let start = output
                    .len()
                    .saturating_sub(limit.saturating_sub(bytes.len()));
                output.drain(..start);
                output.extend_from_slice(&bytes[bytes.len().saturating_sub(limit)..]);
                Ok(())
            }
            _ => {
                output.extend_from_slice(bytes);
                Ok(())
            }
        }
    }
}

/// How a command is supervised besides its input: how long it may run, what of its output is
/// kept, and what becomes of the jobs it leaves behind.
#[derive(Clone, Copy)]
struct Terms {
    timeout: Duration,
    capture: Capture,
    leftovers: Leftovers,
}

/// What becomes of the rest of a command's process group once the command exits.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Leftovers {
    /// Left alone when they let go of its output, as a shell leaves them: a Git hook's
    /// `ctags >/dev/null &` goes on after the commit.
    Detached,
    /// Ended with the command, as a CI step ends: a check must not leave a server holding a
    /// port for the next one.
    #[cfg(feature = "desktop")]
    Ended,
}

/// Reads what `stream` has ready and closes it at end of file, so it stops being waited on.
fn drain(
    stream: &mut Option<impl Read>,
    mut consume: impl FnMut(&[u8]) -> Result<(), Error>,
) -> Result<(), Error> {
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
            Ok(count) => consume(&buffer[..count])?,
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

/// Blocking: service stdin and both output pipes until the process exits and its output ends,
/// or until cancellation or the deadline. Output still open after the exit is read for at most
/// `EXIT_GRACE`, then whatever still holds the pipe is terminated with the process group. A
/// job the command started with its output elsewhere goes on, as it would from a shell.
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
    command: Command,
    input: &[u8],
    cancellation: &Cancellation,
    timeout: Duration,
    limit: usize,
    progress: impl FnMut(&[u8], &[u8]),
) -> Result<Output, Error> {
    supervise(
        command,
        input,
        cancellation,
        Terms {
            timeout,
            capture: Capture::Exact(limit),
            leftovers: Leftovers::Detached,
        },
        |output, bytes| Capture::Exact(limit).append(output, bytes),
        progress,
    )
    .and_then(Log::into_output)
}

/// Like [`run`] for a command that is run for what it changes: past `limit` each stream keeps
/// only its end, and the command goes on. Ending it for printing too much, as [`run`] does,
/// would stop it halfway through the change.
pub(crate) fn run_keeping_tail(
    command: Command,
    input: &[u8],
    cancellation: &Cancellation,
    timeout: Duration,
    limit: usize,
) -> Result<Output, Error> {
    supervise(
        command,
        input,
        cancellation,
        Terms {
            timeout,
            capture: Capture::LogTail(limit),
            leftovers: Leftovers::Detached,
        },
        |output, bytes| Capture::LogTail(limit).append(output, bytes),
        |_, _| {},
    )
    .and_then(Log::into_output)
}

/// A test command can legitimately print megabytes. Retain its diagnostic tail without
/// changing its exit status, and keep that tail when the command runs out of time. Like a CI
/// step, it ends whatever the command left running in its process group.
#[cfg(feature = "desktop")]
pub(crate) fn run_with_log_tail(
    command: Command,
    cancellation: &Cancellation,
    timeout: Duration,
    limit: usize,
) -> Result<Log, Error> {
    run_with_observed_log_tail(command, cancellation, timeout, limit, |_| {})
}

/// Like [`run_with_log_tail`], handing every stdout chunk to `observe` before the tail drops
/// it: a report at the start of a long log is otherwise gone by the time the command exits.
#[cfg(feature = "desktop")]
pub(crate) fn run_with_observed_log_tail(
    command: Command,
    cancellation: &Cancellation,
    timeout: Duration,
    limit: usize,
    mut observe: impl FnMut(&[u8]),
) -> Result<Log, Error> {
    supervise(
        command,
        &[],
        cancellation,
        Terms {
            timeout,
            capture: Capture::LogTail(limit),
            leftovers: Leftovers::Ended,
        },
        |output, bytes| {
            observe(bytes);
            Capture::LogTail(limit).append(output, bytes)
        },
        |_, _| {},
    )
}

/// Feed large read-only command output to a bounded incremental consumer and retain the tail
/// of stderr: warnings, one per file in some configurations, must not fail the command.
#[cfg(feature = "desktop")]
pub(crate) fn stream_stdout(
    command: Command,
    cancellation: &Cancellation,
    timeout: Duration,
    stderr_limit: usize,
    mut sink: impl FnMut(&[u8]),
) -> Result<Output, Error> {
    supervise(
        command,
        &[],
        cancellation,
        Terms {
            timeout,
            capture: Capture::LogTail(stderr_limit),
            leftovers: Leftovers::Detached,
        },
        |_, bytes| {
            sink(bytes);
            Ok(())
        },
        |_, _| {},
    )
    .and_then(Log::into_output)
}

/// Leaves a long-lived child nothing of Hive's but its standard streams.
///
/// macOS cannot create a pipe or a pty that is close-on-exec from the start, so a child
/// spawned at the wrong instant inherits a descriptor another thread was still marking. A
/// short command only delays that pipe's end of file. A pane or a language server holds it
/// for as long as it lives, and whoever reads the pipe waits out its whole deadline.
///
/// Call it last: a `pre_exec` closure added afterwards could open a descriptor that stays.
pub(crate) fn keep_only_standard_streams(command: &mut Command) {
    let descriptors = descriptor_limit();
    // SAFETY: between fork and exec the closure only calls `fcntl`, which is
    // async-signal-safe, and marking a descriptor in the child leaves the parent's untouched.
    unsafe {
        command.pre_exec(move || {
            for descriptor in 3..descriptors {
                libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            Ok(())
        });
    }
}

/// macOS allows a process 61 440 descriptors by default (`kern.maxfilesperproc`).
const DESCRIPTOR_SCAN: libc::rlim_t = 65_536;

/// One past the highest descriptor number this process can hold. Read before the fork:
/// only the loop over it is async-signal-safe.
fn descriptor_limit() -> libc::c_int {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid, writable `rlimit`.
    let known = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } == 0;
    // ponytail: an unlimited or unreadable limit is scanned up to this bound; list `/dev/fd`
    // in the child if a profile ever shows the scan.
    let bound = if known {
        limit.rlim_cur.min(DESCRIPTOR_SCAN)
    } else {
        DESCRIPTOR_SCAN
    };
    libc::c_int::try_from(bound).unwrap_or(libc::c_int::MAX)
}

/// A descriptor left inheritable, the way another thread's pipe is on macOS until it is
/// marked, parked on a number no shell opens for itself.
#[cfg(all(test, feature = "desktop"))]
pub(crate) fn inheritable_descriptor() -> std::os::fd::OwnedFd {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let file = std::fs::File::open("/dev/null").expect("/dev/null opens");
    // SAFETY: duplicates a descriptor this function owns; `F_DUPFD` leaves the copy inheritable.
    let parked = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD, 100) };
    assert!(parked >= 100, "{}", io::Error::last_os_error());
    // SAFETY: `fcntl` just returned this descriptor and nothing else owns it.
    unsafe { OwnedFd::from_raw_fd(parked) }
}

fn supervise(
    mut command: Command,
    input: &[u8],
    cancellation: &Cancellation,
    terms: Terms,
    mut consume: impl FnMut(&mut Vec<u8>, &[u8]) -> Result<(), Error>,
    mut progress: impl FnMut(&[u8], &[u8]),
) -> Result<Log, Error> {
    let Terms {
        timeout,
        capture,
        leftovers,
    } = terms;
    let io_error = Error::Io;
    // The one look before the process exists. A caller that looked for itself first would
    // only move the moment a cancellation can arrive unseen, not remove it.
    if cancellation.is_cancelled() {
        return Err(Error::CancelledBeforeStart);
    }
    command
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if timeout >= LONG_LIVED {
        keep_only_standard_streams(&mut command);
    }
    let child = command.spawn().map_err(Error::Spawn)?;
    let pid = Pid::from_child(&child);
    {
        let mut groups = cancellation
            .groups
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        groups.push(pid);
        if cancellation.is_cancelled() {
            kill(pid, false);
        }
    }
    let mut running = Running {
        child,
        cancellation,
        exited: false,
        settled: false,
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
    // The exit code, and until when output is still awaited, once the process has exited.
    let mut exit: Option<(Option<i32>, Instant)> = None;
    loop {
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let previous = (output.len(), errors.len());
        drain(&mut pipes.stdout, |chunk| consume(&mut output, chunk))?;
        drain(&mut pipes.stderr, |chunk| {
            capture.append(&mut errors, chunk)
        })?;
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
        if exit.is_none() {
            let status = process::waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            )
            .map_err(|e| io_error(e.into()))?;
            if let Some(status) = status
                && (status.exited() || status.killed() || status.dumped())
            {
                running.exited = true;
                // Not bounded by the deadline: what the process wrote just before exiting is
                // read on the next pass.
                exit = Some((status.exit_status(), Instant::now() + EXIT_GRACE));
            }
        }
        let now = Instant::now();
        let end = match exit {
            Some((code, until)) if pipes.output_closed() || now >= until => Some(End::Exited(code)),
            None if now >= deadline => Some(End::TimedOut),
            _ => None,
        };
        if let Some(end) = end {
            if cancellation.is_cancelled() {
                return Err(Error::Cancelled);
            }
            running.settled = leftovers == Leftovers::Detached
                && matches!(end, End::Exited(_))
                && pipes.output_closed();
            return Ok(Log {
                end,
                stdout: output,
                stderr: errors,
            });
        }
        let until = exit.map_or(deadline, |(_, until)| until);
        wait_for_progress(
            &pipes,
            until.saturating_duration_since(now),
            &mut exit_recheck,
        )
        .map_err(io_error)?;
    }
}

/// Sleeps until a pipe can make progress, or briefly when both outputs are closed but the
/// child cannot be reaped yet, never past `remaining`.
fn wait_for_progress(
    pipes: &Pipes,
    remaining: Duration,
    exit_recheck: &mut Duration,
) -> io::Result<()> {
    if pipes.output_closed() {
        // Nothing announces that a child became reapable, so look again shortly.
        std::thread::sleep((*exit_recheck).min(remaining));
        *exit_recheck = (*exit_recheck * 2).min(WAIT_SLICE);
        Ok(())
    } else {
        pipes.wait(WAIT_SLICE.min(remaining))
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

    #[cfg(feature = "desktop")]
    #[test]
    fn verbose_checks_keep_their_true_exit_code_and_diagnostic_tail() {
        let result = run_with_log_tail(
            Host::Local.command(
                "/bin/sh",
                &["-c", "printf abcdef; printf 123456 >&2; exit 19"],
                None,
            ),
            &Cancellation::default(),
            Duration::from_secs(5),
            3,
        )
        .unwrap();
        assert_eq!(result.end, End::Exited(Some(19)));
        assert_eq!(result.stdout, b"def");
        assert_eq!(result.stderr, b"456");
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn a_log_that_runs_out_of_time_keeps_what_the_command_printed() {
        let control = Cancellation::default();
        let log = run_with_log_tail(
            Host::Local.command(
                "/bin/sh",
                &["-c", "printf started; printf waiting >&2; exec sleep 600"],
                None,
            ),
            &control,
            Duration::from_secs(3),
            1024,
        )
        .unwrap();
        assert_eq!(log.end, End::TimedOut);
        assert_eq!(log.stdout, b"started");
        assert_eq!(log.stderr, b"waiting");
        assert_eq!(log.text(), "started\nwaiting");
        assert!(control.groups.lock().unwrap().is_empty());
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn streamed_output_survives_more_warnings_than_the_stderr_budget() {
        let mut streamed = Vec::new();
        let result = stream_stdout(
            Host::Local.command(
                "/bin/sh",
                &["-c", "printf patch; printf warning-warning >&2"],
                None,
            ),
            &Cancellation::default(),
            Duration::from_secs(5),
            7,
            |chunk| streamed.extend_from_slice(chunk),
        )
        .unwrap();
        assert_eq!(result.code, Some(0));
        assert_eq!(streamed, b"patch");
        assert_eq!(result.stderr, b"warning");
    }

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
    fn cancellation_during_a_completed_output_pass_is_not_reported_as_success() {
        let cancellation = Cancellation::default();
        let mut command = crate::host::Host::Local.command("/bin/sleep", &["30"], None);
        // spawn returns only after exec: both output streams are already closed when
        // run_with_progress begins draining, but the child is still running.
        unsafe {
            command.pre_exec(|| {
                let ready = b"ready";
                if libc::write(libc::STDOUT_FILENO, ready.as_ptr().cast(), ready.len())
                    != ready.len() as isize
                {
                    return Err(io::Error::last_os_error());
                }
                if libc::close(libc::STDOUT_FILENO) != 0 || libc::close(libc::STDERR_FILENO) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut seen = false;
        let result = run_with_progress(
            command,
            &[],
            &cancellation,
            Duration::from_secs(5),
            1024,
            |output, _| {
                assert_eq!(output, b"ready");
                seen = true;
                let pid = cancellation.groups.lock().unwrap()[0];
                cancellation.cancel();
                // Wait for our cancellation to take effect without reaping the child;
                // run_with_progress remains its owner and can inspect the same status.
                let status = process::waitid(
                    WaitId::Pid(pid),
                    WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
                )
                .unwrap()
                .unwrap();
                assert!(status.killed());
            },
        );
        assert!(seen);
        match result {
            Err(Error::Cancelled) => {}
            Err(error) => panic!("expected Cancelled, got {error:?}"),
            Ok(output) => panic!(
                "expected Cancelled, got Ok with exit code {:?}",
                output.code
            ),
        }
    }

    #[test]
    fn a_command_cancelled_before_its_process_exists_says_that_it_never_started() {
        let cancellation = Cancellation::default();
        cancellation.cancel();
        // Every entry point shares the one look; a program by this name would fail to spawn.
        let never_spawned = || std::process::Command::new("hive-must-not-spawn");
        let exact = run(never_spawned(), &[], &cancellation, Duration::ZERO, 1).err();
        let tail = run_keeping_tail(never_spawned(), &[], &cancellation, Duration::ZERO, 1).err();
        for error in [exact, tail].map(|error| error.expect("nothing ran")) {
            assert!(matches!(error, Error::CancelledBeforeStart), "{error:?}");
            assert!(error.never_started());
        }
        assert!(cancellation.groups.lock().unwrap().is_empty());
    }

    #[test]
    fn only_a_process_that_was_never_started_is_known_to_have_done_nothing() {
        let missing = run(
            std::process::Command::new("hive-must-not-spawn"),
            &[],
            &Cancellation::default(),
            Duration::ZERO,
            1,
        )
        .err()
        .expect("nothing by that name runs");
        assert!(matches!(missing, Error::Spawn(_)), "{missing:?}");
        assert!(missing.never_started());
        for started in [
            Error::Io(io::Error::other("pipe")),
            Error::Cancelled,
            Error::TimedOut,
            Error::TooLarge(1),
        ] {
            assert!(!started.never_started(), "{started:?}");
        }
    }

    #[test]
    fn a_command_still_running_at_the_deadline_times_out() {
        let control = Cancellation::default();
        let began = Instant::now();
        let result = run(
            Host::Local.command("/bin/sh", &["-c", "printf partial; exec sleep 30"], None),
            b"",
            &control,
            Duration::from_millis(50),
            1024,
        );
        assert!(matches!(result, Err(Error::TimedOut)));
        assert!(began.elapsed() < Duration::from_secs(2));
        assert!(control.groups.lock().unwrap().is_empty());
    }

    #[test]
    fn a_process_left_holding_the_pipe_does_not_hold_a_finished_command() {
        let control = Cancellation::default();
        let began = Instant::now();
        let output = run(
            Host::Local.command("/bin/sh", &["-c", "printf done; sleep 30 & exit 4"], None),
            b"",
            &control,
            Duration::from_secs(60),
            1024,
        )
        .unwrap();
        assert_eq!(output.code, Some(4));
        assert_eq!(output.stdout, b"done");
        // The grace is one second; the deadline and the holder both last far longer.
        assert!(began.elapsed() < Duration::from_secs(10));
        assert!(control.groups.lock().unwrap().is_empty());
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
            while control.groups.lock().unwrap().is_empty() {
                std::thread::yield_now();
            }
            control.cancel();
            running.join().unwrap()
        });
        assert!(matches!(result, Err(Error::Cancelled)));
        assert!(control.groups.lock().unwrap().is_empty());
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn only_a_command_that_may_run_long_hides_hives_other_descriptors() {
        use std::os::fd::AsRawFd;
        let descriptor = inheritable_descriptor();
        let parked = descriptor.as_raw_fd();
        let sees_it = |timeout| {
            let script = format!("test -e /dev/fd/{parked}");
            run(
                Host::Local.command("/bin/sh", &["-c", &script], None),
                b"",
                &Cancellation::default(),
                timeout,
                64,
            )
            .unwrap()
            .code
                == Some(0)
        };
        assert!(
            !sees_it(LONG_LIVED),
            "a check or a hook would hold it for its whole run"
        );
        assert!(
            sees_it(Duration::from_secs(5)),
            "a short read keeps posix_spawn"
        );
    }

    #[test]
    fn a_finished_command_leaves_the_jobs_it_detached_running() {
        let output = run(
            Host::Local.command(
                "/bin/sh",
                &["-c", "sleep 30 </dev/null >/dev/null 2>&1 & printf %s $!"],
                None,
            ),
            b"",
            &Cancellation::default(),
            Duration::from_secs(10),
            64,
        )
        .unwrap();
        let job = std::str::from_utf8(&output.stdout)
            .unwrap()
            .parse()
            .unwrap();
        let job = Pid::from_raw(job).unwrap();
        // A killed orphan is gone within milliseconds, once launchd reaps it; half a second of
        // it still being there means nobody killed it.
        let watched_until = Instant::now() + Duration::from_millis(500);
        let mut running = true;
        while running && Instant::now() < watched_until {
            running = process::test_kill_process(job).is_ok();
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = process::kill_process(job, Signal::KILL);
        assert!(running, "a hook's background job outlives the hook");
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn a_check_ends_the_jobs_it_left_running_like_a_ci_step() {
        let log = run_with_log_tail(
            Host::Local.command(
                "/bin/sh",
                &["-c", "sleep 30 </dev/null >/dev/null 2>&1 & printf %s $!"],
                None,
            ),
            &Cancellation::default(),
            Duration::from_secs(10),
            64,
        )
        .unwrap();
        let job = std::str::from_utf8(&log.stdout).unwrap().parse().unwrap();
        let job = Pid::from_raw(job).unwrap();
        // Killed already; launchd reaps the orphan a moment later.
        let deadline = Instant::now() + Duration::from_secs(5);
        while process::test_kill_process(job).is_ok() {
            assert!(
                Instant::now() < deadline,
                "the server a check started outlives it"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn cancelling_stops_every_command_running_under_it() {
        let control = Cancellation::default();
        let results: (usize, Duration, Vec<_>) = std::thread::scope(|scope| {
            let runs: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        run(
                            Host::Local.command("/bin/sleep", &["30"], None),
                            b"",
                            &control,
                            Duration::from_secs(30),
                            1024,
                        )
                    })
                })
                .collect();
            // Bounded: a cancellation that kept one group would never list two.
            let deadline = Instant::now() + Duration::from_secs(10);
            while control.groups.lock().unwrap().len() < 2 && Instant::now() < deadline {
                std::thread::yield_now();
            }
            let registered = control.groups.lock().unwrap().len();
            control.cancel();
            let cancelled = Instant::now();
            let results = runs.into_iter().map(|run| run.join().unwrap()).collect();
            (registered, cancelled.elapsed(), results)
        });
        let (registered, took, results) = results;
        assert_eq!(registered, 2);
        // `sleep 30` that nobody killed ends only with its own sleep.
        assert!(took < Duration::from_secs(10), "{took:?}");
        for result in results {
            assert!(matches!(result, Err(Error::Cancelled)));
        }
        assert!(control.groups.lock().unwrap().is_empty());
    }
}
