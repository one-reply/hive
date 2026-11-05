use super::{AgentKind, Error};
pub use crate::command::Cancellation;
use std::{
    fs, io,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
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

pub(super) fn run(
    agent: AgentKind,
    command: Command,
    prompt: &str,
    cancellation: &Cancellation,
    timeout: Duration,
) -> Result<Vec<u8>, Error> {
    let output = crate::command::run(
        command,
        prompt.as_bytes(),
        cancellation,
        timeout,
        1024 * 1024,
    )
    .map_err(|error| match error {
        crate::command::Error::Io(source) => Error::CliIo { agent, source },
        crate::command::Error::Cancelled => Error::Invalid("Generation cancelled."),
        crate::command::Error::TimedOut => Error::Invalid("CLI generation timed out. Try again."),
        crate::command::Error::TooLarge(_) => Error::Invalid("CLI output exceeds 1 MiB"),
    })?;
    if output.code != Some(0) {
        return Err(Error::Invalid(
            "CLI generation failed. Check its login, model and supported version in your terminal.",
        ));
    }
    Ok(output.stdout)
}
#[cfg(test)]
mod tests;
