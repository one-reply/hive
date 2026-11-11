//! Pinned vendor downloads, extracted to a private staging directory and atomically installed.
use super::{Error, Provider};
use crate::{
    command::{self, Cancellation},
    host::Host,
};
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

const BINARY_LIMIT: usize = 128 * 1024 * 1024;
const DOWNLOAD_LIMIT: &str = "134217728";
struct Release {
    url: &'static str,
    sha256: &'static str,
    member: &'static str,
}
fn release(provider: Provider) -> Release {
    match provider {
        Provider::GitHub => Release {
            url: "https://github.com/cli/cli/releases/download/v2.103.3/gh_2.103.3_macOS_arm64.zip",
            sha256: "d700207eae9ae98fe2b89535c2c087c07823d4628c56364695a5a07ef311c062",
            member: "gh_2.103.3_macOS_arm64/bin/gh",
        },
        Provider::GitLab => Release {
            url: "https://gitlab.com/api/v4/projects/gitlab-org%2Fcli/packages/generic/glab/1%2E130%2E0/glab_1%2E130%2E0_darwin_arm64%2Etar%2Egz",
            sha256: "5791b652e774e4eca85afba1ebfd9db8498138c3311e0ed0a39d19590311f232",
            member: "bin/glab",
        },
    }
}
struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            tracing::warn!(%error, "could not remove CLI installer staging directory");
        }
    }
}
fn io_error(error: std::io::Error) -> Error {
    Error::Install(error.to_string())
}
fn staging(parent: &Path) -> Result<Staging, Error> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fs::create_dir_all(parent).map_err(io_error)?;
    for _ in 0..100 {
        let path = parent.join(format!(
            ".install-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => return Ok(Staging(path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error(error)),
        }
    }
    Err(Error::Install(
        "Could not create a private staging directory.".into(),
    ))
}
fn run(
    program: &str,
    args: &[&str],
    cancellation: &Cancellation,
    timeout: Duration,
    limit: usize,
) -> Result<command::Output, Error> {
    let output = command::run(
        Host::Local.command(program, args, None),
        &[],
        cancellation,
        timeout,
        limit,
    )?;
    if output.code != Some(0) {
        return Err(Error::Install(format!(
            "{} failed. Check your network and available disk space, then retry.",
            Path::new(program)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        )));
    }
    Ok(output)
}
fn checksum_matches(output: &[u8], expected: &str) -> bool {
    std::str::from_utf8(output)
        .ok()
        .and_then(|s| s.split_whitespace().next())
        .is_some_and(|s| s == expected)
}
fn validate_binary(bytes: &[u8]) -> Result<(), Error> {
    // Only a thin 64-bit arm64 Mach-O executable is eligible for this macOS-only installer.
    if bytes.len() < 32
        || bytes[..4] != [0xcf, 0xfa, 0xed, 0xfe]
        || bytes[4..8] != [12, 0, 0, 1]
        || bytes[12..16] != [2, 0, 0, 0]
    {
        return Err(Error::Install(
            "Vendor archive did not contain an Apple Silicon executable.".into(),
        ));
    }
    Ok(())
}

/// Blocking: install a pinned, checksum-verified official CLI under Hive's private tools directory.
/// Callers should reuse existing system CLIs. No Homebrew, shell script, root access, or credentials are needed.
pub fn install(provider: Provider, cancellation: &Cancellation) -> Result<(), Error> {
    if cancellation.is_cancelled() {
        return Err(command::Error::Cancelled.into());
    }
    if std::env::consts::OS != "macos" || std::env::consts::ARCH != "aarch64" {
        return Err(Error::Install(
            "Automatic installation requires macOS on Apple Silicon.".into(),
        ));
    }
    install_at(provider, &provider.managed_path(), cancellation)
}
fn install_at(
    provider: Provider,
    destination: &Path,
    cancellation: &Cancellation,
) -> Result<(), Error> {
    let parent = destination
        .parent()
        .ok_or_else(|| Error::Install("Invalid installation path.".into()))?;
    let staging = staging(parent)?;
    let archive = staging.0.join("archive");
    let release = release(provider);
    run(
        "/usr/bin/curl",
        &[
            "--disable",
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "15",
            "--max-time",
            "180",
            "--max-filesize",
            DOWNLOAD_LIMIT,
            "--output",
            &archive.to_string_lossy(),
            release.url,
        ],
        cancellation,
        Duration::from_secs(190),
        16 * 1024,
    )?;
    let sum = run(
        "/usr/bin/shasum",
        &["-a", "256", "--", &archive.to_string_lossy()],
        cancellation,
        Duration::from_secs(30),
        4096,
    )?;
    if !checksum_matches(&sum.stdout, release.sha256) {
        return Err(Error::Install(
            "Download checksum mismatch. No executable was installed; retry the download.".into(),
        ));
    }
    // Extract one pinned member to stdout: archive paths/symlinks never write to the filesystem.
    let output = run(
        "/usr/bin/tar",
        &["-xOf", &archive.to_string_lossy(), release.member],
        cancellation,
        Duration::from_secs(30),
        BINARY_LIMIT,
    )?;
    write_binary(&output.stdout, &staging.0, destination, cancellation)
}

fn write_binary(
    bytes: &[u8],
    staging: &Path,
    destination: &Path,
    cancellation: &Cancellation,
) -> Result<(), Error> {
    validate_binary(bytes)?;
    let binary = staging.join("executable");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&binary)
        .map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    if cancellation.is_cancelled() {
        return Err(command::Error::Cancelled.into());
    }
    fs::rename(&binary, destination).map_err(io_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejected_or_cancelled_install_preserves_existing_binary_and_cleans_staging() {
        use std::os::unix::fs::PermissionsExt;
        let parent =
            std::env::temp_dir().join(format!("hive-cli-install-test-{}", std::process::id()));
        let stage = staging(&parent).unwrap();
        let stage_path = stage.0.clone();
        assert_eq!(
            fs::metadata(&stage_path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let destination = stage_path.join("existing");
        fs::write(&destination, b"old").unwrap();
        assert!(write_binary(b"bad", &stage_path, &destination, &Cancellation::default()).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"old");
        let mut bytes = vec![0; 32];
        bytes[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        bytes[4..8].copy_from_slice(&[12, 0, 0, 1]);
        bytes[12..16].copy_from_slice(&[2, 0, 0, 0]);
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            write_binary(&bytes, &stage_path, &destination, &cancel),
            Err(Error::Command(command::Error::Cancelled))
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"old");
        drop(stage);
        assert!(!stage_path.exists());
        fs::remove_dir(parent).unwrap();
    }

    #[test]
    fn pinned_downloads_are_https_with_exact_hashes_and_named_members() {
        for provider in Provider::ALL {
            let release = release(provider);
            assert!(release.url.starts_with("https://"));
            assert_eq!(release.sha256.len(), 64);
            assert!(release.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(release.member.ends_with(provider.cli()));
            assert!(checksum_matches(
                format!("{}  archive\n", release.sha256).as_bytes(),
                release.sha256
            ));
            assert!(!checksum_matches(b"bad  archive", release.sha256));
        }
    }
    #[test]
    fn non_arm64_or_truncated_download_cannot_become_executable() {
        let mut bytes = vec![0; 32];
        assert!(validate_binary(&bytes).is_err());
        bytes[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        bytes[4..8].copy_from_slice(&[12, 0, 0, 1]);
        bytes[12..16].copy_from_slice(&[2, 0, 0, 0]);
        assert!(validate_binary(&bytes).is_ok());
        assert!(validate_binary(&bytes[..20]).is_err());
        bytes[4] = 7;
        assert!(validate_binary(&bytes).is_err());
    }
}
