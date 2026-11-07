use super::{
    Error, Mutation, file_error,
    protocol::{Request, Response},
    request_with,
};
use crate::command::Cancellation;
use crate::{
    code_navigation::Definition,
    files::{Entry, FileError, FileIndex, Snapshot},
    host::Host,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::path::Path;

pub(crate) fn read(
    host: &Host,
    root: &Path,
    path: &Path,
    cancellation: &Cancellation,
) -> Result<Snapshot, FileError> {
    match request_with(
        host,
        Request::Read {
            root: root.into(),
            path: path.into(),
        },
        cancellation,
    )
    .map_err(|e| file_error(host, path, e))?
    {
        Response::Snapshot(snapshot) => Ok(snapshot),
        _ => Err(unexpected(host, path)),
    }
}

pub(crate) fn image(
    host: &Host,
    root: &Path,
    path: &Path,
    cancellation: &Cancellation,
) -> Result<Vec<u8>, FileError> {
    match request_with(
        host,
        Request::Image {
            root: root.into(),
            path: path.into(),
        },
        cancellation,
    )
    .map_err(|e| file_error(host, path, e))?
    {
        Response::Image(encoded) => {
            let bytes = STANDARD
                .decode(encoded)
                .map_err(|_| unexpected(host, path))?;
            if bytes.len() > crate::files::images::MAX_IMAGE_BYTES {
                return Err(unexpected(host, path));
            }
            Ok(bytes)
        }
        _ => Err(unexpected(host, path)),
    }
}

pub(crate) fn list_directory(
    host: &Host,
    root: &Path,
    path: &Path,
    cancellation: &Cancellation,
) -> Result<Vec<Entry>, FileError> {
    match request_with(
        host,
        Request::List {
            root: root.into(),
            path: path.into(),
        },
        cancellation,
    )
    .map_err(|e| file_error(host, path, e))?
    {
        Response::Entries(entries) => Ok(entries),
        _ => Err(unexpected(host, path)),
    }
}

pub(crate) fn list_files(
    host: &Host,
    root: &Path,
    cancellation: &Cancellation,
) -> Result<FileIndex, FileError> {
    match request_with(host, Request::Index { root: root.into() }, cancellation)
        .map_err(|e| file_error(host, root, e))?
    {
        Response::Index(index) => Ok(index),
        _ => Err(unexpected(host, root)),
    }
}

pub(crate) fn save(
    host: &Host,
    root: &Path,
    path: &Path,
    expected: &Snapshot,
    text: &str,
    cancellation: &Cancellation,
) -> Result<Snapshot, FileError> {
    match request_with(
        host,
        Request::Save {
            root: root.into(),
            path: path.into(),
            expected: expected.clone(),
            text: text.into(),
        },
        cancellation,
    )
    .map_err(|e| file_error(host, path, e))?
    {
        Response::Snapshot(snapshot) => Ok(snapshot),
        _ => Err(unexpected(host, path)),
    }
}

pub(crate) fn mutate(
    host: &Host,
    root: &Path,
    path: &Path,
    mutation: Mutation,
    cancellation: &Cancellation,
) -> Result<(), FileError> {
    match request_with(
        host,
        Request::Mutate {
            root: root.into(),
            path: path.into(),
            mutation,
        },
        cancellation,
    )
    .map_err(|e| file_error(host, path, e))?
    {
        Response::Done => Ok(()),
        _ => Err(unexpected(host, path)),
    }
}

/// Resolve a JS/TS import beside its remote files, including unsaved requesting text.
pub fn definition(
    host: &Host,
    root: &Path,
    path: &Path,
    text: &str,
    offset: usize,
    cancellation: &Cancellation,
) -> Result<Option<Definition>, Error> {
    match request_with(
        host,
        Request::Definition {
            root: root.into(),
            path: path.into(),
            text: text.into(),
            offset,
        },
        cancellation,
    )? {
        Response::Definition(definition) => Ok(definition),
        _ => Err(Error::Protocol("expected definition".into())),
    }
}

fn unexpected(host: &Host, path: &Path) -> FileError {
    file_error(
        host,
        path,
        Error::Protocol("unexpected response kind".into()),
    )
}
