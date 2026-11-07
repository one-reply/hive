//! One-shot filesystem requests over the user's OpenSSH connection.
//! No listener, daemon, credentials or persistent agent processes live here.
use crate::{command, files::FileError, host::Host};
use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

mod client;
mod install;
use crate::path_encoding as path;
mod protocol;
mod service;
pub use client::definition;
pub(crate) use client::{image, list_directory, list_files, mutate, read, save};
pub use install::prepare;
pub(crate) use protocol::Mutation;
use protocol::{Request, Response};
pub use service::serve;

/// Wire compatibility, independent of the application release version.
pub const PROTOCOL_VERSION: u32 = 1;
/// Maximum encoded message, including base64 payloads.
pub const MESSAGE_LIMIT: usize = 32 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);

/// A canonical remote directory and its directly browsable child directories.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Directory {
    /// Absolute directory selected on the remote machine.
    #[serde(with = "path")]
    pub path: PathBuf,
    /// Absolute paths; symlinks are not traversed by the picker.
    #[serde(with = "path::many")]
    pub children: Vec<PathBuf>,
}

/// An SSH, installation or wire failure. Mutations are never replayed automatically.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Transport or helper preparation failed.
    #[error("{0}")]
    Connection(String),
    /// A malformed, incompatible or oversized response.
    #[error("Invalid remote response: {0}")]
    Protocol(String),
    /// A remote filesystem operation failed.
    #[error(transparent)]
    File(#[from] FileError),
}

/// Resolve `~` or an absolute path on the host and list its child directories.
pub fn browse(
    host: &Host,
    directory: &Path,
    cancellation: &command::Cancellation,
) -> Result<Directory, Error> {
    match request_with(
        host,
        Request::Browse {
            path: directory.into(),
        },
        cancellation,
    )? {
        Response::Directory(directory) => Ok(directory),
        _ => Err(Error::Protocol("expected directory".into())),
    }
}

fn request_with(
    host: &Host,
    request: Request,
    cancellation: &command::Cancellation,
) -> Result<Response, Error> {
    host.validate()
        .map_err(|e| Error::Connection(e.to_string()))?;
    install::ensure(host, cancellation)?;
    let input = protocol::encode(&request)?;
    let script = format!(
        "exec \"$HOME/.hive/bin/hive-remote-{}\"",
        env!("CARGO_PKG_VERSION")
    );
    let output = command::run(
        host.command("sh", &["-c", &script], None),
        &input,
        cancellation,
        TIMEOUT,
        MESSAGE_LIMIT + protocol::PREAMBLE_LIMIT + 32,
    )
    .map_err(|e| transport_error(host, &request, e.to_string()))?;
    if output.code != Some(0) {
        return Err(transport_error(
            host,
            &request,
            String::from_utf8_lossy(&output.stderr).trim().into(),
        ));
    }
    let response: Result<Response, protocol::Failure> =
        protocol::decode_response(&output.stdout)
            .map_err(|error| transport_error(host, &request, error.to_string()))?;
    let response = response.map_err(|failure| Error::File(failure.into_file_error(host)))?;
    if !protocol::accepts(&request, &response) {
        return Err(transport_error(
            host,
            &request,
            "unexpected or unsafe response".into(),
        ));
    }
    Ok(response)
}

fn transport_error(host: &Host, request: &Request, message: String) -> Error {
    let guidance = if request.is_mutation() {
        "Outcome unknown. Refresh and inspect the remote files before retrying; nothing was retried."
    } else {
        "Check the connection. If authentication is needed, connect using ssh in a terminal first."
    };
    Error::Connection(format!("{host:?}: {message}. {guidance}"))
}

fn file_error(host: &Host, path: &Path, error: Error) -> FileError {
    match error {
        Error::File(error) => error,
        error => FileError::Io {
            operation: "access remote file",
            path: path.into(),
            source: io::Error::other(format!("{host:?}: {error}")),
        },
    }
}
