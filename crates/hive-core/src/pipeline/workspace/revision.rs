//! Hash complete Git patches as streams: a large patch may exceed the display budget.
use super::*;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

const DIFF_FLAGS: &[&str] = &[
    "--literal-pathspecs",
    "diff",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--no-renames",
    "--full-index",
    "--ignore-submodules=none",
];

/// Identity of every tracked change and untracked file in the run, independent of the UI preview budget.
/// Each Git patch carries the file mode and full object IDs even for a binary file or a symlink.
pub fn revision(run: u64, lanes: &[Lane], stop: &Cancellation) -> Result<String, GitError> {
    let mut hash = Sha256::new();
    let mut seen = HashSet::new();
    for lane in lanes.iter().filter(|lane| seen.insert(&lane.worktree)) {
        verify(run, lane)?;
        let base = super::review::pinned_base(lane)?;
        digest_checkout(&mut hash, &lane.worktree, &base, stop)?;
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn digest_checkout(
    hash: &mut Sha256,
    cwd: &Path,
    base: &str,
    stop: &Cancellation,
) -> Result<(), GitError> {
    frame(hash, cwd.as_os_str().as_bytes());
    frame(hash, base.as_bytes());
    digest_patch(hash, cwd, [OsStr::new(base), OsStr::new("--")], false, stop)?;
    let files = git::bytes(
        &Host::Local,
        cwd,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        None,
        16 * 1024 * 1024,
        false,
    )?;
    for file in files.split(|&b| b == 0).filter(|p| !p.is_empty()) {
        frame(hash, file);
        if file.ends_with(b"/") {
            // Git lists an embedded repository as one path. Hash its checkout, never its .git metadata.
            digest_nested(hash, cwd, file, stop)?;
        } else {
            digest_patch(
                hash,
                cwd,
                [
                    OsStr::new("--no-index"),
                    OsStr::new("--"),
                    OsStr::new("/dev/null"),
                    OsStr::from_bytes(file),
                ],
                true,
                stop,
            )?;
        }
    }
    // A superproject patch only says "-dirty" for every edit inside the same gitlink.
    // Inspect changed links even when the repository is configured to hide dirty submodules.
    let raw = git::bytes(
        &Host::Local,
        cwd,
        &[
            "diff",
            "--raw",
            "--no-renames",
            "--ignore-submodules=none",
            "-z",
            base,
            "--",
        ],
        None,
        16 * 1024 * 1024,
        false,
    )?;
    let mut records = raw.split(|&b| b == 0).filter(|record| !record.is_empty());
    while let Some(header) = records.next() {
        let path = records
            .next()
            .ok_or_else(|| GitError::Invalid("Incomplete Git raw diff.".into()))?;
        if header.split(|&b| b == b' ').nth(1) == Some(b"160000") {
            digest_nested(hash, cwd, path, stop)?;
        }
    }
    Ok(())
}

fn digest_nested(
    hash: &mut Sha256,
    cwd: &Path,
    path: &[u8],
    stop: &Cancellation,
) -> Result<(), GitError> {
    let path = Path::new(OsStr::from_bytes(path));
    if !path
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        return Err(GitError::Invalid("Unexpected Git submodule path.".into()));
    }
    let nested = cwd.join(path);
    if nested
        .join(".git")
        .try_exists()
        .map_err(|e| GitError::Invalid(format!("{}: {e}", nested.display())))?
    {
        // A symlink swap or removed checkout could otherwise send Git back to an ancestor.
        let top = git::bytes(
            &Host::Local,
            &nested,
            &["rev-parse", "--show-toplevel"],
            None,
            64 * 1024,
            false,
        )?;
        let root = Path::new(OsStr::from_bytes(top.strip_suffix(b"\n").unwrap_or(&top)));
        if root != nested {
            return Err(GitError::Invalid(format!(
                "The checkout at {} changed during review.",
                nested.display()
            )));
        }
        let head = git::branches::resolve(&Host::Local, &nested, "HEAD")?;
        digest_checkout(hash, &nested, &head, stop)?;
    }
    Ok(())
}

fn frame(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_be_bytes());
    hash.update(value);
}

fn digest_patch<const N: usize>(
    combined: &mut Sha256,
    cwd: &Path,
    args: [&OsStr; N],
    allow_difference: bool,
    stop: &Cancellation,
) -> Result<(), GitError> {
    let mut cmd = Host::Local.command("git", DIFF_FLAGS, Some(cwd));
    cmd.args(args)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0");
    let mut digest = Sha256::new();
    let out = command::stream_stdout(cmd, stop, Duration::from_secs(60), CHECK_OUTPUT, |chunk| {
        digest.update(chunk)
    })
    .map_err(|source| GitError::Interrupted {
        command: "diff".into(),
        host: Host::Local,
        cwd: cwd.into(),
        source,
        mutation: false,
        guidance: "Review could not verify the current files.",
    })?;
    if out.code != Some(0) && !(allow_difference && out.code == Some(1)) {
        return Err(GitError::Failed {
            command: "diff".into(),
            host: Host::Local,
            cwd: cwd.into(),
            code: out.code,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    combined.update(digest.finalize());
    Ok(())
}
