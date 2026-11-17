//! Installed formatters on the document's host. Source is passed only through stdin.
use crate::{
    command::{self, Cancellation},
    host::Host,
};
use serde::Deserialize;
use std::{
    path::{Component, Path, PathBuf},
    time::Duration,
};

const LIMIT: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);

/// A formatter failure, with the source file and host retained for diagnosis.
#[derive(Debug, thiserror::Error)]
#[error("Format {path} on {host:?}: {message}")]
pub struct Error {
    path: PathBuf,
    host: Host,
    message: String,
}

/// Whether an installed formatter is supported for this source file.
pub fn supported(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx" | "rs")
    )
}

/// Formats an in-memory source without writing the source file or installing tools.
pub fn format(
    host: &Host,
    root: &Path,
    relative: &Path,
    text: &str,
    cancellation: &Cancellation,
) -> Result<String, Error> {
    let fail = |message: String| Error {
        path: relative.into(),
        host: host.clone(),
        message,
    };
    if !root.is_absolute()
        || relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|c| matches!(c, Component::Normal(name) if name != ".git"))
    {
        return Err(fail("File must stay inside its worktree".into()));
    }
    if !supported(relative) {
        return Err(fail("No formatter for this language".into()));
    }
    if text.len() > LIMIT {
        return Err(fail("Source exceeds 1 MiB".into()));
    }
    let path = root.join(relative);
    let directory = path
        .parent()
        .ok_or_else(|| fail("File has no parent".into()))?;
    let filename = path
        .to_str()
        .ok_or_else(|| fail("Formatter requires a UTF-8 path".into()))?;
    let script = if relative.extension().is_some_and(|ext| ext == "rs") {
        let metadata = run(host, directory, "d=$PWD; while [ ! -f \"$d/Cargo.toml\" ]; do [ \"$d\" = / ] && { printf '{}'; exit; }; d=${d%/*}; [ -n \"$d\" ] || d=/; done; exec cargo metadata --no-deps --offline --locked --format-version 1", "", cancellation).map_err(fail)?;
        // Cargo reports physical manifest paths, including when the worktree was opened via an alias.
        let physical = run(host, directory, "pwd -P", "", cancellation).map_err(fail)?;
        let physical = Path::new(physical.strip_suffix('\n').unwrap_or(&physical));
        if !physical.is_absolute() {
            return Err(fail(
                "Formatter could not resolve the source directory".into(),
            ));
        }
        let name = path
            .file_name()
            .ok_or_else(|| fail("Source has no filename".into()))?;
        let edition = edition(&metadata, &physical.join(name)).map_err(fail)?;
        let mut args = vec!["rustfmt".to_owned(), "--emit".into(), "stdout".into()];
        if let Some(edition) = edition {
            args.extend(["--edition".into(), edition]);
        }
        format!("exec {}", quote_args(&args).map_err(fail)?)
    } else {
        prettier_script(filename).map_err(fail)?
    };
    run(host, directory, &script, text, cancellation).map_err(fail)
}

fn quote_args(args: &[String]) -> Result<String, String> {
    shlex::try_join(args.iter().map(String::as_str)).map_err(|e| e.to_string())
}

fn prettier_script(path: &str) -> Result<String, String> {
    let path = shlex::try_quote(path).map_err(|e| e.to_string())?;
    Ok(format!(
        "d=$PWD; tool=prettier; while :; do if [ -x \"$d/node_modules/.bin/prettier\" ]; then tool=\"$d/node_modules/.bin/prettier\"; break; fi; [ \"$d\" = / ] && break; d=${{d%/*}}; [ -n \"$d\" ] || d=/; done; exec \"$tool\" --stdin-filepath {path}"
    ))
}

fn run(
    host: &Host,
    directory: &Path,
    script: &str,
    input: &str,
    cancellation: &Cancellation,
) -> Result<String, String> {
    // The inner POSIX shell also works when an SSH account uses fish or csh.
    let script = if host.is_local() {
        script.to_owned()
    } else {
        let action = quote_args(&["sh".into(), "-c".into(), script.into()])?;
        format!(
            "exec \"${{SHELL:-/bin/sh}}\" -lc {}",
            shlex::try_quote(&action).map_err(|e| e.to_string())?
        )
    };
    let output = command::run(
        host.command("sh", &["-c", &script], Some(directory)),
        input.as_bytes(),
        cancellation,
        TIMEOUT,
        LIMIT,
    )
    .map_err(|e| e.to_string())?;
    decode(output)
}

fn decode(output: command::Output) -> Result<String, String> {
    if output.code != Some(0) {
        return Err(format!(
            "Installed formatter failed (exit {:?}): {}",
            output.code,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "Formatter returned invalid UTF-8".to_owned())?;
    if text.contains('\0') {
        return Err("Formatter returned binary data".into());
    }
    Ok(text)
}

#[derive(Deserialize, Default)]
struct Metadata {
    #[serde(default)]
    packages: Vec<Package>,
}
#[derive(Deserialize)]
struct Package {
    manifest_path: PathBuf,
    edition: String,
}

fn edition(metadata: &str, path: &Path) -> Result<Option<String>, String> {
    let metadata: Metadata =
        serde_json::from_str(metadata).map_err(|e| format!("Invalid cargo metadata: {e}"))?;
    let package = metadata
        .packages
        .into_iter()
        .filter(|p| {
            p.manifest_path
                .parent()
                .is_some_and(|root| path.starts_with(root))
        })
        .max_by_key(|p| p.manifest_path.components().count());
    package
        .map(|p| match p.edition.as_str() {
            "2015" | "2018" | "2021" | "2024" => Ok(p.edition),
            other => Err(format!("Unsupported Rust edition: {other}")),
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rust_formatting_uses_the_package_edition_through_a_symlink_root() {
        use std::{fs, os::unix::fs::symlink};
        let root = std::env::temp_dir().join(format!("hive-format-alias-{}", std::process::id()));
        fs::create_dir_all(root.join("actual/src")).unwrap();
        fs::write(
            root.join("actual/Cargo.toml"),
            "[package]\nname = 'alias-fixture'\nversion = '0.0.0'\nedition = '2021'\n[workspace]\n",
        )
        .unwrap();
        fs::write(
            root.join("actual/Cargo.lock"),
            "version = 4\n[[package]]\nname = 'alias-fixture'\nversion = '0.0.0'\n",
        )
        .unwrap();
        fs::write(root.join("actual/rustfmt.toml"), "newline_style = 'Unix'\n").unwrap();
        let source = "pub async fn example(){}\n";
        fs::write(root.join("actual/src/lib.rs"), source).unwrap();
        symlink(root.join("actual"), root.join("alias")).unwrap();
        let canonical = fs::canonicalize(root.join("actual")).unwrap();
        let direct = format(
            &Host::Local,
            &canonical,
            Path::new("src/lib.rs"),
            source,
            &Cancellation::default(),
        )
        .unwrap();
        let aliased = format(
            &Host::Local,
            &root.join("alias"),
            Path::new("src/lib.rs"),
            source,
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(aliased, direct);
        assert!(aliased.contains("async fn example() {}"));
        assert_eq!(
            fs::read_to_string(root.join("actual/src/lib.rs")).unwrap(),
            source
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn project_formatter_receives_unsaved_stdin_and_failure_preserves_disk() {
        use std::{
            fs,
            os::unix::fs::PermissionsExt as _,
            sync::atomic::{AtomicUsize, Ordering},
        };
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "hive-format-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
        fs::create_dir(root.join("src")).unwrap();
        let tool = root.join("node_modules/.bin/prettier");
        let path = Path::new("src/a ' $().ts");
        fs::write(root.join(path), "saved").unwrap();
        fs::write(&tool, "#!/bin/sh\n[ \"$1\" = --stdin-filepath ] || exit 7\nprintf '%s' \"$2\" > received-path\ncat\n").unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        let text = "const café = '😀';\n";
        assert_eq!(
            format(&Host::Local, &root, path, text, &Cancellation::default()).unwrap(),
            text
        );
        assert_eq!(
            fs::read_to_string(root.join("src/received-path")).unwrap(),
            root.join(path).to_str().unwrap()
        );
        fs::write(&tool, "#!/bin/sh\nprintf 'bad output'; exit 1\n").unwrap();
        assert!(format(&Host::Local, &root, path, text, &Cancellation::default()).is_err());
        assert_eq!(fs::read_to_string(root.join(path)).unwrap(), "saved");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn rust_edition_comes_from_the_nearest_package_and_unknown_fields_are_allowed() {
        let metadata = r#"{"packages":[{"manifest_path":"/work/Cargo.toml","edition":"2018"},{"manifest_path":"/work/nested/Cargo.toml","edition":"2024","other":true}],"version":1}"#;
        assert_eq!(
            edition(metadata, Path::new("/work/nested/src/lib.rs"))
                .unwrap()
                .as_deref(),
            Some("2024")
        );
        assert_eq!(edition("{}", Path::new("/standalone.rs")).unwrap(), None);
        assert!(edition("{", Path::new("/work/a.rs")).is_err());
    }
    #[test]
    fn formatter_failure_and_binary_output_are_not_source() {
        for (code, stdout) in [
            (Some(1), b"source".to_vec()),
            (None, vec![]),
            (Some(0), vec![255]),
            (Some(0), vec![0]),
        ] {
            assert!(
                decode(command::Output {
                    code,
                    stdout,
                    stderr: b"error".to_vec()
                })
                .is_err()
            );
        }
        assert_eq!(
            decode(command::Output {
                code: Some(0),
                stdout: b"a\n".to_vec(),
                stderr: vec![]
            })
            .unwrap(),
            "a\n"
        );
    }
    #[test]
    fn unsafe_paths_fail_before_any_process() {
        for relative in ["../bad.ts", "/bad.ts", ".git/config", ""] {
            assert!(
                format(
                    &Host::Local,
                    Path::new("/work"),
                    Path::new(relative),
                    "x",
                    &Cancellation::default()
                )
                .is_err()
            );
        }
        let args = vec!["prettier".into(), "a ' $() `x`.ts".into()];
        assert_eq!(shlex::split(&quote_args(&args).unwrap()).unwrap(), args);
    }
}
