use super::{Directory, Error, MESSAGE_LIMIT, PROTOCOL_VERSION};
use crate::path_encoding as path;
use crate::{
    code_navigation::Definition,
    files::{Entry, FileError, FileIndex, Snapshot},
    host::Host,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    io::{self, Read},
    path::PathBuf,
};

mod snapshot;

const MAGIC: &[u8] = b"HIVE-REMOTE\n";
pub(super) const PREAMBLE_LIMIT: usize = 8192;

#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub(super) enum Request {
    Browse {
        #[serde(with = "path")]
        path: PathBuf,
    },
    List {
        #[serde(with = "path")]
        root: PathBuf,
        #[serde(with = "path")]
        path: PathBuf,
    },
    Index {
        #[serde(with = "path")]
        root: PathBuf,
    },
    Read {
        #[serde(with = "path")]
        root: PathBuf,
        #[serde(with = "path")]
        path: PathBuf,
    },
    Image {
        #[serde(with = "path")]
        root: PathBuf,
        #[serde(with = "path")]
        path: PathBuf,
    },
    Save {
        #[serde(with = "path")]
        root: PathBuf,
        #[serde(with = "path")]
        path: PathBuf,
        #[serde(with = "snapshot")]
        expected: Snapshot,
        text: String,
    },
    Mutate {
        #[serde(with = "path")]
        root: PathBuf,
        #[serde(with = "path")]
        path: PathBuf,
        mutation: Mutation,
    },
    Definition {
        #[serde(with = "path")]
        root: PathBuf,
        #[serde(with = "path")]
        path: PathBuf,
        text: String,
        offset: usize,
    },
}

impl Request {
    pub(super) fn is_mutation(&self) -> bool {
        matches!(self, Self::Save { .. } | Self::Mutate { .. })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Mutation {
    CreateFile,
    CreateDirectory,
    Move {
        #[serde(with = "path")]
        to: PathBuf,
    },
    Delete,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(super) enum Response {
    Directory(Directory),
    Entries(Vec<Entry>),
    Index(FileIndex),
    Snapshot(#[serde(with = "snapshot")] Snapshot),
    Image(String),
    Definition(Option<Definition>),
    Done,
}

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    version: u32,
    value: T,
}

pub(super) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    let json = serde_json::to_vec(&Envelope {
        version: PROTOCOL_VERSION,
        value,
    })
    .map_err(protocol)?;
    if json.len() > MESSAGE_LIMIT {
        return Err(protocol("message exceeds 32 MiB"));
    }
    let mut result = Vec::with_capacity(MAGIC.len() + 4 + json.len());
    result.extend_from_slice(MAGIC);
    result.extend_from_slice(&(json.len() as u32).to_be_bytes());
    result.extend(json);
    Ok(result)
}

pub(super) fn read<T: DeserializeOwned>(reader: &mut impl Read) -> Result<T, Error> {
    let mut magic = [0; MAGIC.len()];
    reader.read_exact(&mut magic).map_err(protocol)?;
    if magic != MAGIC {
        return Err(protocol("missing message header"));
    }
    let mut size = [0; 4];
    reader.read_exact(&mut size).map_err(protocol)?;
    let size = u32::from_be_bytes(size) as usize;
    if size > MESSAGE_LIMIT {
        return Err(protocol("message exceeds 32 MiB"));
    }
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes).map_err(protocol)?;
    let envelope: Envelope<T> = serde_json::from_slice(&bytes).map_err(protocol)?;
    if envelope.version != PROTOCOL_VERSION {
        return Err(protocol("incompatible helper; reconnect to update it"));
    }
    Ok(envelope.value)
}

pub(super) fn decode_response<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Error> {
    let start = bytes
        .windows(MAGIC.len())
        .take(PREAMBLE_LIMIT + 1)
        .position(|s| s == MAGIC)
        .ok_or_else(|| protocol("missing response header"))?;
    let mut remaining = &bytes[start..];
    let value = read(&mut remaining)?;
    if !remaining.is_empty() {
        return Err(protocol("unexpected trailing data"));
    }
    Ok(value)
}

fn protocol(error: impl std::fmt::Display) -> Error {
    Error::Protocol(error.to_string())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FailureKind {
    Conflict,
    NotFound,
    PermissionDenied,
    AlreadyExists,
    PartialDelete,
    Other,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Failure {
    kind: FailureKind,
    #[serde(with = "path")]
    path: PathBuf,
    message: String,
}

impl From<FileError> for Failure {
    fn from(error: FileError) -> Self {
        let message = error.to_string();
        let (kind, path) = match error {
            FileError::Conflict(path) => (FailureKind::Conflict, path),
            FileError::PartialDelete { path, .. } => (FailureKind::PartialDelete, path),
            FileError::Io { path, source, .. } => (
                match source.kind() {
                    io::ErrorKind::NotFound => FailureKind::NotFound,
                    io::ErrorKind::PermissionDenied => FailureKind::PermissionDenied,
                    io::ErrorKind::AlreadyExists => FailureKind::AlreadyExists,
                    _ => FailureKind::Other,
                },
                path,
            ),
            FileError::Unsupported { path, .. }
            | FileError::Listing { path, .. }
            | FileError::Recovery { path, .. } => (FailureKind::Other, path),
        };
        Self {
            kind,
            path,
            message,
        }
    }
}

impl Failure {
    pub(super) fn into_file_error(self, host: &Host) -> FileError {
        if matches!(self.kind, FailureKind::Conflict) {
            return FileError::Conflict(self.path);
        }
        let kind = match self.kind {
            FailureKind::NotFound => io::ErrorKind::NotFound,
            FailureKind::PermissionDenied => io::ErrorKind::PermissionDenied,
            FailureKind::AlreadyExists => io::ErrorKind::AlreadyExists,
            _ => io::ErrorKind::Other,
        };
        let error = FileError::Io {
            operation: "access remote file",
            path: self.path.clone(),
            source: io::Error::new(kind, format!("{host:?}: {}", self.message)),
        };
        if matches!(self.kind, FailureKind::PartialDelete) {
            FileError::PartialDelete {
                path: self.path,
                source: Box::new(error),
            }
        } else {
            error
        }
    }
}

pub(super) fn accepts(request: &Request, response: &Response) -> bool {
    let relative = |path: &std::path::Path| {
        !path.as_os_str().is_empty()
            && path
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(name) if name != ".git"))
    };
    match (request, response) {
        (Request::Browse { .. }, Response::Directory(directory)) => {
            directory.path.is_absolute()
                && directory.children.len() <= 100_000
                && directory.children.iter().all(|child| {
                    child.parent() == Some(directory.path.as_path()) && child.file_name().is_some()
                })
        }
        (Request::List { .. }, Response::Entries(entries)) => {
            entries.iter().all(|entry| relative(&entry.path))
        }
        (Request::Index { .. }, Response::Index(index)) => {
            index.paths.len() <= 100_000 && index.paths.iter().all(|path| relative(path))
        }
        (Request::Read { .. } | Request::Save { .. }, Response::Snapshot(_)) => true,
        (Request::Image { .. }, Response::Image(_)) | (Request::Mutate { .. }, Response::Done) => {
            true
        }
        (Request::Definition { .. }, Response::Definition(definition)) => definition
            .as_ref()
            .is_none_or(|definition| relative(&definition.path)),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
