use super::{
    Directory, Error,
    protocol::{self, Failure, Mutation, Request, Response},
};
use crate::{
    code_navigation::Navigator,
    files::{self, FileError},
    host::Host,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// Handle one bounded request using the same filesystem rules as the desktop app.
pub fn serve(reader: &mut impl Read, writer: &mut impl Write) -> Result<(), Error> {
    let request = protocol::read(reader)?;
    let response: Result<Response, Failure> = dispatch(request).map_err(Into::into);
    let output = protocol::encode(&response)?;
    writer
        .write_all(&output)
        .map_err(|e| Error::Protocol(e.to_string()))
}

fn dispatch(request: Request) -> Result<Response, FileError> {
    let host = &Host::Local;
    Ok(match request {
        Request::Browse { path } => Response::Directory(browse(&path)?),
        Request::List { root, path } => {
            Response::Entries(files::list_directory(host, &root, &path)?)
        }
        Request::Index { root } => {
            Response::Index(files::list_files(host, &root, &Default::default())?)
        }
        Request::Read { root, path } => Response::Snapshot(files::read(host, &root, &path)?),
        Request::Image { root, path } => {
            Response::Image(STANDARD.encode(files::images::read(host, &root, &path)?))
        }
        Request::Save {
            root,
            path,
            expected,
            text,
        } => Response::Snapshot(files::save(host, &root, &path, &expected, &text)?),
        Request::Mutate {
            root,
            path,
            mutation,
        } => {
            match mutation {
                Mutation::CreateFile => files::create_file(host, &root, &path)?,
                Mutation::CreateDirectory => files::create_directory(host, &root, &path)?,
                Mutation::Move { to } => files::move_entry(host, &root, &path, &to)?,
                Mutation::Delete => files::delete_entry(host, &root, &path)?,
            }
            Response::Done
        }
        Request::Definition {
            root,
            path,
            text,
            offset,
        } => {
            files::resolve(host, &root, &path)?;
            if text.len() > files::MAX_FILE_BYTES {
                return Err(files::unsupported(&path, "navigation text exceeds 1 MiB"));
            }
            Response::Definition(
                Navigator::default()
                    .definition(&root, &path, &text, offset)
                    .map_err(|e| FileError::Listing {
                        path,
                        message: e.to_string(),
                    })?,
            )
        }
    })
}

fn browse(input: &Path) -> Result<Directory, FileError> {
    let home = || {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| files::unsupported(input, "remote HOME is not set"))
    };
    let path = if input == Path::new("~") || input.as_os_str().is_empty() {
        home()?
    } else if let Ok(suffix) = input.strip_prefix("~") {
        home()?.join(suffix)
    } else {
        input.into()
    };
    if !path.is_absolute() {
        return Err(files::unsupported(
            input,
            "enter an absolute path or ~/folder",
        ));
    }
    let path =
        fs::canonicalize(&path).map_err(|e| files::io_error("resolve directory", &path, e))?;
    let mut children = Vec::new();
    for entry in fs::read_dir(&path).map_err(|e| files::io_error("browse directory", &path, e))? {
        let entry = entry.map_err(|e| files::io_error("browse directory", &path, e))?;
        if entry
            .file_type()
            .map_err(|e| files::io_error("inspect directory", &entry.path(), e))?
            .is_dir()
        {
            children.push(entry.path());
        }
        if children.len() >= 100_000 {
            return Err(files::unsupported(
                &path,
                "directory contains too many folders",
            ));
        }
    }
    children.sort();
    Ok(Directory { path, children })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    #[test]
    fn remote_roundtrip_preserves_conflicts_and_never_overwrites_moves() {
        let root = std::env::temp_dir().join(format!(
            "hive-remote-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let mut path = PathBuf::from(OsString::from_vec(b"notes-\xff-\xe4\xb8\x96.txt".to_vec()));
        if let Err(error) = fs::write(root.join(&path), b"original\r\n") {
            // APFS rejects non-UTF-8 names; Linux runs the exact byte-path filesystem case.
            assert_eq!(error.raw_os_error(), Some(libc::EILSEQ));
            path = "notes-世界.txt".into();
            fs::write(root.join(&path), b"original\r\n").unwrap();
        }
        let request = Request::Read {
            root: root.clone(),
            path: path.clone(),
        };
        let mut output = Vec::new();
        serve(
            &mut protocol::encode(&request).unwrap().as_slice(),
            &mut output,
        )
        .unwrap();
        let response: Result<Response, Failure> = protocol::decode_response(&output).unwrap();
        let Response::Snapshot(base) = response.ok().unwrap() else {
            panic!("snapshot expected")
        };
        fs::write(root.join(&path), b"external").unwrap();
        assert!(matches!(
            dispatch(Request::Save {
                root: root.clone(),
                path: path.clone(),
                expected: base,
                text: "mine".into()
            }),
            Err(FileError::Conflict(_))
        ));
        fs::write(root.join("existing"), b"keep").unwrap();
        assert!(
            dispatch(Request::Mutate {
                root: root.clone(),
                path: path.clone(),
                mutation: Mutation::Move {
                    to: "existing".into()
                }
            })
            .is_err()
        );
        assert_eq!(fs::read(root.join("existing")).unwrap(), b"keep");
        // A lost acknowledgement must leave the completed write visible to a subsequent read.
        struct LostConnection;
        impl Write for LostConnection {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let expected = files::read(&Host::Local, &root, &path).unwrap();
        let save = Request::Save {
            root: root.clone(),
            path: path.clone(),
            expected,
            text: "saved despite lost response".into(),
        };
        assert!(
            serve(
                &mut protocol::encode(&save).unwrap().as_slice(),
                &mut LostConnection
            )
            .is_err()
        );
        assert_eq!(
            files::read(&Host::Local, &root, &path).unwrap().text(),
            "saved despite lost response"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
