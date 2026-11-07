use super::{Directory, Error, PROTOCOL_VERSION, browse};
use crate::{command, host::Host};
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

const BINARY_LIMIT: u64 = 64 * 1024 * 1024;
const PROBE: &str = "set -eu; command -v git >/dev/null || { echo Git-is-required-on-the-remote-host >&2; exit 1; }; command -v sha256sum >/dev/null || { echo sha256sum-is-required-on-the-remote-host >&2; exit 1; }; printf '\n__HIVE_HOST__\\n'; uname -s; uname -m";

#[derive(Deserialize)]
struct Manifest {
    version: String,
    protocol: u32,
    artifacts: Vec<Artifact>,
}
#[derive(Deserialize)]
struct Artifact {
    architecture: String,
    file: String,
    sha256: String,
}

/// Verify Linux and Git, atomically install the bundled helper, and browse remote HOME.
/// Blocking; call on the background executor. Requires an already usable SSH key.
pub fn prepare(host: &Host, cancellation: &command::Cancellation) -> Result<Directory, Error> {
    ensure(host, cancellation)?;
    browse(host, Path::new("~"), cancellation)
}

pub(super) fn ensure(host: &Host, cancellation: &command::Cancellation) -> Result<(), Error> {
    static INSTALLED: Mutex<Option<HashSet<Host>>> = Mutex::new(None);
    if INSTALLED
        .lock()
        .map_err(connection)?
        .as_ref()
        .is_some_and(|hosts| hosts.contains(host))
    {
        return Ok(());
    }
    // Concurrent first requests may install the same checksum; atomic rename makes that safe.
    install(host, cancellation)?;
    INSTALLED
        .lock()
        .map_err(connection)?
        .get_or_insert_with(HashSet::new)
        .insert(host.clone());
    Ok(())
}

fn install(host: &Host, cancellation: &command::Cancellation) -> Result<(), Error> {
    host.validate()
        .map_err(|e| Error::Connection(e.to_string()))?;
    if host.is_local() {
        return Err(Error::Connection("Choose an SSH host".into()));
    }
    let output = run(host, PROBE, &[], cancellation)?;
    let architecture = architecture(&output)?;
    let directory = assets()?;
    let manifest_bytes = read_bounded(&directory.join("manifest.json"), 65536)?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes).map_err(connection)?;
    if manifest.version != env!("CARGO_PKG_VERSION") || manifest.protocol != PROTOCOL_VERSION {
        return Err(connection(
            "helper package does not match this Hive version",
        ));
    }
    let artifact = manifest
        .artifacts
        .iter()
        .find(|a| a.architecture == architecture)
        .ok_or_else(|| connection("no helper for this Linux architecture"))?;
    if artifact.sha256.len() != 64
        || !artifact.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        || Path::new(&artifact.file).components().count() != 1
        || artifact.file.starts_with('.')
    {
        return Err(connection("invalid helper manifest entry"));
    }
    let binary = directory.join(&artifact.file);
    if fs::metadata(&binary).map_err(connection)?.len() > BINARY_LIMIT {
        return Err(connection("helper exceeds 64 MiB"));
    }
    let checksum = command::run(
        Host::Local.command(
            "/usr/bin/shasum",
            &["-a", "256", "--", &binary.to_string_lossy()],
            None,
        ),
        &[],
        cancellation,
        Duration::from_secs(10),
        4096,
    )
    .map_err(connection)?;
    if checksum.code != Some(0)
        || !String::from_utf8_lossy(&checksum.stdout).starts_with(&format!("{} ", artifact.sha256))
    {
        return Err(connection("bundled helper checksum mismatch"));
    }
    let destination = format!("$HOME/.hive/bin/hive-remote-{}", env!("CARGO_PKG_VERSION"));
    let check = format!(
        "if test -f \"{destination}\" && test -x \"{destination}\"; then sha256sum \"{destination}\"; fi"
    );
    if !String::from_utf8_lossy(&run(host, &check, &[], cancellation)?)
        .lines()
        .any(|line| line.starts_with(&format!("{} ", artifact.sha256)))
    {
        let bytes = read_bounded(&binary, BINARY_LIMIT)?;
        let script = format!(
            "set -eu; umask 077; mkdir -p \"$HOME/.hive/bin\"; temporary=$(mktemp \"{destination}.tmp.XXXXXX\"); trap 'rm -f \"$temporary\"' EXIT HUP INT TERM; cat > \"$temporary\"; actual=$(sha256sum \"$temporary\"); test \"${{actual%% *}}\" = '{}'; chmod 700 \"$temporary\"; mv -f \"$temporary\" \"{destination}\"",
            artifact.sha256
        );
        run(host, &script, &bytes, cancellation)?;
    }
    Ok(())
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(connection)?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(connection)?;
    if bytes.len() as u64 > limit {
        return Err(connection("helper package exceeds its size limit"));
    }
    Ok(bytes)
}

fn assets() -> Result<PathBuf, Error> {
    if let Some(directory) = std::env::var_os("HIVE_REMOTE_ASSETS") {
        return Ok(directory.into());
    }
    let executable = std::env::current_exe().map_err(connection)?;
    if let Some(contents) = executable.parent().and_then(Path::parent) {
        let bundle = contents.join("Resources/remote");
        if bundle.is_dir() {
            return Ok(bundle);
        }
    }
    let development = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/remote");
    if development.is_dir() {
        return Ok(development);
    }
    Err(connection(
        "Remote helper package is missing. Use the packaged Hive app or prepare target/remote using scripts/package-remote.py.",
    ))
}

fn run(
    host: &Host,
    script: &str,
    input: &[u8],
    cancellation: &command::Cancellation,
) -> Result<Vec<u8>, Error> {
    let output = command::run(
        host.command("sh", &["-c", script], None),
        input,
        cancellation,
        Duration::from_secs(60),
        65536,
    )
    .map_err(connection)?;
    if output.code != Some(0) {
        return Err(connection(format!(
            "{host:?}: {}. Connect with ssh in a terminal to confirm the host key and unlock your key.",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

fn architecture(bytes: &[u8]) -> Result<&str, Error> {
    let text = std::str::from_utf8(bytes).map_err(connection)?;
    let (_, info) = text
        .split_once("__HIVE_HOST__\n")
        .ok_or_else(|| connection("missing host information"))?;
    let mut lines = info.lines();
    if lines.next() != Some("Linux") {
        return Err(connection("Remote projects currently require Linux"));
    }
    match lines.next() {
        Some("x86_64") => Ok("x86_64"),
        Some("aarch64" | "arm64") => Ok("aarch64"),
        _ => Err(connection(
            "Unsupported Linux architecture; use x86_64 or arm64",
        )),
    }
}

fn connection(error: impl std::fmt::Display) -> Error {
    Error::Connection(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn probe_requires_linux_and_known_architecture_after_marker() {
        assert_eq!(
            architecture(b"Welcome\n__HIVE_HOST__\nLinux\nx86_64\n").unwrap(),
            "x86_64"
        );
        assert_eq!(
            architecture(b"__HIVE_HOST__\nLinux\naarch64\n").unwrap(),
            "aarch64"
        );
        assert!(architecture(b"__HIVE_HOST__\nDarwin\narm64\n").is_err());
        assert!(architecture(b"__HIVE_HOST__\nLinux\nmips\n").is_err());
        assert!(architecture(b"Linux\nx86_64\n").is_err());
    }
}
