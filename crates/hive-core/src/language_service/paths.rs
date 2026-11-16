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

pub(super) fn rebase(value: &mut Value, from: &Path, to: &Path) {
    if from == to {
        return;
    }
    match value {
        Value::Array(values) => {
            for value in values {
                rebase(value, from, to);
            }
        }
        Value::Object(fields) => {
            for (key, value) in fields {
                if matches!(key.as_str(), "uri" | "targetUri" | "rootUri") {
                    let mapped = value
                        .as_str()
                        .and_then(|text| text.parse().ok())
                        .and_then(|uri| position::path(&uri))
                        .and_then(|path| {
                            path.strip_prefix(from)
                                .ok()
                                .map(|relative| to.join(relative))
                        })
                        .and_then(|path| position::uri(&path));
                    if let Some(uri) = mapped {
                        *value = Value::String(uri.as_str().into());
                    }
                } else {
                    rebase(value, from, to);
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
}
