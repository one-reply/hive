//! Keep server-canonical file URIs in the workspace's user-facing path namespace.
use super::position;
use crate::{
    command::{self, Cancellation},
    host::{Host, preamble},
};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) fn physical_root(host: &Host, root: &Path) -> PathBuf {
    let resolved = if host.is_local() {
        std::fs::canonicalize(root).map_err(|e| e.to_string())
    } else {
        command::run(
            root_command(host, root),
            b"",
            &Cancellation::default(),
            Duration::from_secs(5),
            4096 + preamble::OVERHEAD,
        )
        .map_err(|e| e.to_string())
        .and_then(root_output)
    };
    resolved.unwrap_or_else(|error| {
        tracing::warn!(%error, "using configured language-server root");
        root.into()
    })
}

fn root_command(host: &Host, root: &Path) -> std::process::Command {
    host.login_command("pwd -P", Some(root))
}

fn root_output(output: command::Output) -> Result<PathBuf, String> {
    let stdout = preamble::strip(&output.stdout).map_err(|e| e.to_string())?;
    let path = std::str::from_utf8(stdout).map_err(|e| e.to_string())?;
    let path = PathBuf::from(path.strip_suffix('\n').unwrap_or(path));
    if output.code == Some(0)
        && path.is_absolute()
        && !path.as_os_str().as_encoded_bytes().contains(&0)
    {
        Ok(path)
    } else {
        Err("Cannot resolve language-server worktree".into())
    }
}

/// Outbound: file URIs under the user-facing root move into the server's physical namespace.
pub(super) fn rebase(value: &mut Value, from: &Path, to: &Path) {
    if from == to {
        return;
    }
    map_uris(value, &|path| {
        path.strip_prefix(from)
            .ok()
            .map(|relative| to.join(relative))
    });
}

/// Inbound: back into the user-facing namespace, and into the one spelling [`position::uri`]
/// writes. Servers percent-encode differently; documents and diagnostics are matched by URI.
pub(super) fn inbound(value: &mut Value, physical: &Path, root: &Path) {
    map_uris(value, &|path| {
        Some(match path.strip_prefix(physical) {
            Ok(relative) => root.join(relative),
            Err(_) => path,
        })
    });
}

fn map_uris(value: &mut Value, map: &dyn Fn(PathBuf) -> Option<PathBuf>) {
    match value {
        Value::Array(values) => {
            for value in values {
                map_uris(value, map);
            }
        }
        Value::Object(fields) => {
            for (key, value) in fields {
                if matches!(key.as_str(), "uri" | "targetUri" | "rootUri") {
                    let mapped = value
                        .as_str()
                        .and_then(position::decode)
                        .and_then(map)
                        .and_then(|path| position::uri(&path));
                    if let Some(uri) = mapped {
                        *value = Value::String(uri.as_str().into());
                    }
                } else {
                    map_uris(value, map);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn remote_root_discards_login_banners_and_resolves_a_symlink() {
        use std::{
            fs,
            os::unix::fs::{PermissionsExt as _, symlink},
        };
        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let scratch =
            Scratch(std::env::temp_dir().join(format!("hive-root-banner-{}", std::process::id())));
        let (physical, home, alias) = (
            scratch.0.join("physical"),
            scratch.0.join("home"),
            scratch.0.join("work tree's"),
        );
        fs::create_dir_all(&physical).unwrap();
        fs::create_dir_all(&home).unwrap();
        symlink(&physical, &alias).unwrap();
        let shell = scratch.0.join("login-shell");
        fs::write(&shell, "#!/bin/sh\necho 'Welcome to build-01'\ncase $1 in -c|-lc) exec /bin/sh -c \"$2\" ;; esac\nexit 64\n").unwrap();
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
        let host = Host::Ssh {
            target: "build-01".into(),
        };
        let output = command::run(
            crate::host::as_login_shell(&root_command(&host, &alias), &shell, &home),
            b"",
            &Cancellation::default(),
            Duration::from_secs(5),
            4096 + preamble::OVERHEAD,
        )
        .unwrap();
        assert!(output.stdout.starts_with(b"Welcome to build-01\n"));
        assert_eq!(
            root_output(output).unwrap(),
            fs::canonicalize(physical).unwrap()
        );
    }

    #[test]
    fn remote_root_rejects_missing_markers_failed_commands_and_invalid_paths() {
        for (code, stdout) in [
            (Some(0), b"/unmarked\n".to_vec()),
            (
                Some(1),
                [preamble::MARKER.as_bytes(), b"/failed\n"].concat(),
            ),
            (
                Some(0),
                [preamble::MARKER.as_bytes(), b"relative\n"].concat(),
            ),
            (
                Some(0),
                [preamble::MARKER.as_bytes(), b"/bad\0path\n"].concat(),
            ),
            (
                Some(0),
                [preamble::MARKER.as_bytes(), b"/bad\xff\n"].concat(),
            ),
            (Some(0), preamble::MARKER.as_bytes().to_vec()),
            (
                Some(0),
                [
                    vec![b'x'; preamble::LIMIT + 1],
                    preamble::MARKER.as_bytes().to_vec(),
                    b"/path\n".to_vec(),
                ]
                .concat(),
            ),
        ] {
            assert!(
                root_output(command::Output {
                    code,
                    stdout,
                    stderr: vec![]
                })
                .is_err()
            );
        }
    }
    #[test]
    fn maps_only_file_uri_fields_inside_the_root_in_both_directions() {
        let mut payload = json!({"textDocument":{"uri":"file:///visible/src/caf%C3%A9.ts"}, "text":"file:///visible/src/a.ts", "other":{"uri":"file:///visible-other/a.ts"}});
        let original = payload.clone();
        rebase(&mut payload, Path::new("/visible"), Path::new("/physical"));
        assert_eq!(
            payload["textDocument"]["uri"],
            "file:///physical/src/caf%C3%A9.ts"
        );
        assert_eq!(payload["text"], original["text"]);
        assert_eq!(payload["other"], original["other"]);
        rebase(&mut payload, Path::new("/physical"), Path::new("/visible"));
        assert_eq!(payload, original);
    }
    #[test]
    fn inbound_uris_get_one_spelling_even_when_the_root_is_not_an_alias() {
        let root = Path::new("/work/rs (proj)+@[x]");
        let mut payload = json!({"uri":"file:///work/rs%20(proj)+@[x]/src/a.rs", "items":[{"targetUri":"file:///elsewhere/b%20(c).rs"}], "other":{"uri":"untitled:a"}, "text":"file:///work/rs%20(proj)/x"});
        inbound(&mut payload, root, root);
        assert_eq!(
            payload["uri"],
            position::uri(&root.join("src/a.rs")).unwrap().as_str()
        );
        assert_eq!(
            payload["items"][0]["targetUri"],
            "file:///elsewhere/b%20%28c%29.rs"
        );
        assert_eq!(payload["other"]["uri"], "untitled:a");
        assert_eq!(payload["text"], "file:///work/rs%20(proj)/x");
        let mut aliased = json!({"uri":"file:///physical/a%20(b).rs"});
        inbound(&mut aliased, Path::new("/physical"), Path::new("/visible"));
        assert_eq!(aliased["uri"], "file:///visible/a%20%28b%29.rs");
    }
}
