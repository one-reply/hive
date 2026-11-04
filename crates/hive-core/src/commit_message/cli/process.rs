use super::{AgentKind, Error};
use rustix::{
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    process::{self, Pid, Signal, WaitId, WaitIdOptions},
};
use std::{
    fs,
    io::{self, Read, Write},
    os::{
        fd::AsFd,
        unix::{fs::DirBuilderExt, process::CommandExt},
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 1024 * 1024;
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

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
            kill(group);
        }
    }
    /// Whether the owner has cancelled this generation.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

fn kill(group: Pid) {
    if let Err(error) = process::kill_process_group(group, Signal::KILL)
        && error != rustix::io::Errno::SRCH
    {
        tracing::warn!(%error, "could not terminate commit generator");
    }
}

pub(super) struct Directory(pub PathBuf);
impl Directory {
    pub(super) fn new() -> io::Result<Self> {
        loop {
            let path = std::env::temp_dir().join(format!(
                "hive-commit-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            tracing::warn!(%error, "could not remove generator temporary directory");
        }
    }
}

struct Running<'a> {
    child: Child,
    cancellation: &'a Cancellation,
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
            kill(group);
        }
        if let Err(error) = self.child.wait() {
            tracing::warn!(%error, "could not reap commit generator");
        }
    }
}

fn nonblocking(fd: &impl AsFd) -> io::Result<()> {
    fcntl_setfl(fd, fcntl_getfl(fd)? | OFlags::NONBLOCK).map_err(Into::into)
}

fn drain(reader: &mut impl Read, output: &mut Vec<u8>) -> io::Result<()> {
    let mut buffer = [0; 8192];
    // Bound each pass so a noisy child cannot starve cancellation or the deadline.
    for _ in 0..64 {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                if output.len() + count > OUTPUT_LIMIT {
                    return Err(io::Error::other("CLI output exceeds 1 MiB"));
                }
                output.extend_from_slice(&buffer[..count]);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

pub(super) fn run(
    agent: AgentKind,
    mut command: Command,
    prompt: &str,
    cancellation: &Cancellation,
    timeout: Duration,
) -> Result<Vec<u8>, Error> {
    let io_error = |source| Error::CliIo { agent, source };
    if cancellation.is_cancelled() {
        return Err(Error::Invalid("Generation cancelled."));
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
            kill(pid);
        }
    }
    let mut running = Running {
        child,
        cancellation,
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
            return Err(Error::Invalid("Generation cancelled."));
        }
        if Instant::now() >= deadline {
            return Err(Error::Invalid("CLI generation timed out. Try again."));
        }
        drain(&mut stdout, &mut output).map_err(io_error)?;
        drain(&mut stderr, &mut errors).map_err(io_error)?;
        if let Some(input) = &mut stdin {
            match input.write(&prompt.as_bytes()[written..]) {
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
            if written == prompt.len() {
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
        {
            drain(&mut stdout, &mut output).map_err(io_error)?;
            if status.exit_status() != Some(0) {
                return Err(Error::Invalid(
                    "CLI generation failed. Check its login, model and supported version in your terminal.",
                ));
            }
            return Ok(output);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests;
