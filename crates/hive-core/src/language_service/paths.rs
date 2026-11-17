//! Keep server-canonical file URIs in the workspace's user-facing path namespace.
use super::position;
use crate::{
    command::{self, Cancellation},
    host::Host,
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
            host.command("pwd", &["-P"], Some(root)),
            b"",
            &Cancellation::default(),
            Duration::from_secs(5),
            4096,
        )
        .map_err(|e| e.to_string())
        .and_then(|output| {
            let path = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
            let path = PathBuf::from(path.strip_suffix('\n').unwrap_or(&path));
            if output.code == Some(0) && path.is_absolute() {
                Ok(path)
            } else {
                Err("Cannot resolve language-server worktree".into())
            }
        })
    };
    resolved.unwrap_or_else(|error| {
        tracing::warn!(%error, "using configured language-server root");
        root.into()
    })
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
