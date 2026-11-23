use super::{Directory, Error, PROTOCOL_VERSION, browse};
use crate::{command, host::Host};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    ffi::OsString,
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

/// Hosts this run of Hive has installed or found its helper on.
static INSTALLED: Mutex<Option<HashSet<Host>>> = Mutex::new(None);

pub(super) fn ensure(host: &Host, cancellation: &command::Cancellation) -> Result<(), Error> {
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

/// The helper is no longer where it was installed: the remote home was cleaned, or another
/// Hive removed it. The next [`ensure`] installs it again.
pub(super) fn forget(host: &Host) {
    if let Ok(mut installed) = INSTALLED.lock()
        && let Some(hosts) = installed.as_mut()
    {
        hosts.remove(host);
    }
}

/// Whether the shell that was to start the helper says it is missing or not executable.
pub(super) fn helper_missing(code: Option<i32>) -> bool {
    matches!(code, Some(126 | 127))
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
    // Read once: the bytes checked here are the bytes uploaded.
    let bytes = read_bounded(&directory.join(&artifact.file), BINARY_LIMIT)?;
    if !matches_checksum(&bytes, &artifact.sha256) {
        return Err(connection("bundled helper checksum mismatch"));
    }
    let current = format!("hive-remote-{}", env!("CARGO_PKG_VERSION"));
    let destination = format!("$HOME/.hive/bin/{current}");
    let check = format!(
        "if test -f \"{destination}\" && test -x \"{destination}\"; then sha256sum \"{destination}\"; fi"
    );
    if !String::from_utf8_lossy(&run(host, &check, &[], cancellation)?)
        .lines()
        .any(|line| line.starts_with(&format!("{} ", artifact.sha256)))
    {
        let script = format!(
            "set -eu; umask 077; mkdir -p \"$HOME/.hive/bin\"; temporary=$(mktemp \"{destination}.tmp.XXXXXX\"); trap 'rm -f \"$temporary\"' EXIT HUP INT TERM; cat > \"$temporary\"; actual=$(sha256sum \"$temporary\"); test \"${{actual%% *}}\" = '{}'; chmod 700 \"$temporary\"; mv -f \"$temporary\" \"{destination}\"; {}",
            artifact.sha256,
            prune(&current)
        );
        run(host, &script, &bytes, cancellation)?;
    }
    Ok(())
}

/// Removes the helpers of other releases once they are a month old, after the `current` one is
/// in place: each Hive update would otherwise leave one more behind for good. Another Hive
/// still on an older release gets its helper back on its next request (`forget`), once a month
/// at most. A temporary file older than a day belongs to an install that died. Never fails
/// the install: what it could not remove is removed next time.
fn prune(current: &str) -> String {
    format!(
        "find \"$HOME/.hive/bin\" -type f -name 'hive-remote-*' ! -name '{current}' \\( -name '*.tmp.*' -mtime +0 -o ! -name '*.tmp.*' -mtime +30 \\) -exec rm -f {{}} + 2>/dev/null || :"
    )
}

fn matches_checksum(bytes: &[u8], expected: &str) -> bool {
    crate::git::sha256_hex(&Sha256::digest(bytes)).eq_ignore_ascii_case(expected)
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
    if let Some(directory) = development_override(
        cfg!(debug_assertions),
        std::env::var_os("HIVE_REMOTE_ASSETS"),
    ) {
        return Ok(directory);
    }
    let executable = std::env::current_exe().map_err(connection)?;
    if let Some(contents) = executable.parent().and_then(Path::parent) {
        let bundle = contents.join("Resources/remote");
        if bundle.is_dir() {
            return Ok(bundle);
        }
    }
    let development = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/remote");
    if cfg!(debug_assertions) && development.is_dir() {
        return Ok(development);
    }
    Err(connection(
        "Remote helper package is missing. Use the packaged Hive app or prepare target/remote using scripts/package-remote.py.",
    ))
}

/// Where a development build takes the helper from instead of its bundle. A release ignores
/// the variable: whatever sets it would choose the program installed on every SSH host.
fn development_override(debug: bool, variable: Option<OsString>) -> Option<PathBuf> {
    variable.filter(|_| debug).map(PathBuf::from)
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

    #[test]
    fn the_uploaded_bytes_are_the_bytes_checked() {
        // SHA-256 of "abc", FIPS 180-2 appendix B.1.
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(matches_checksum(b"abc", abc));
        assert!(matches_checksum(b"abc", &abc.to_uppercase()));
        assert!(!matches_checksum(b"abd", abc));
    }

    #[test]
    fn only_a_development_build_takes_the_helper_from_the_environment() {
        let variable = || Some(OsString::from("/tmp/helpers"));
        assert_eq!(
            development_override(true, variable()),
            Some(PathBuf::from("/tmp/helpers"))
        );
        assert_eq!(development_override(false, variable()), None);
        assert_eq!(development_override(true, None), None);
    }

    #[test]
    fn a_missing_or_unusable_helper_is_told_apart_from_its_own_failure() {
        assert!(helper_missing(Some(127)));
        assert!(helper_missing(Some(126)));
        for code in [Some(0), Some(1), Some(255), None] {
            assert!(!helper_missing(code), "{code:?}");
        }
    }

    #[test]
    fn pruning_keeps_this_release_and_recent_ones_and_drops_month_old_ones() {
        use std::time::{Duration, SystemTime};
        let home = std::env::temp_dir().join(format!("hive-prune-{}", std::process::id()));
        let bin = home.join(".hive/bin");
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&bin).unwrap();
        let days = |n: u64| SystemTime::now() - Duration::from_secs(n * 24 * 60 * 60);
        let files = [
            ("hive-remote-9.9.9", days(40), true),
            ("hive-remote-1.0.1", days(40), false),
            ("hive-remote-1.0.2", days(10), true),
            ("hive-remote-1.0.1.tmp.abc123", days(2), false),
            ("hive-remote-1.0.3.tmp.def456", days(0), true),
            ("notes.txt", days(40), true),
        ];
        for (name, modified, _) in files {
            let file = fs::File::create(bin.join(name)).unwrap();
            file.set_modified(modified).unwrap();
        }
        let status = std::process::Command::new("/bin/sh")
            .args(["-c", &prune("hive-remote-9.9.9")])
            .env("HOME", &home)
            .status()
            .unwrap();
        assert!(status.success());
        for (name, _, kept) in files {
            assert_eq!(bin.join(name).exists(), kept, "{name}");
        }
        fs::remove_dir_all(home).unwrap();
    }
}
