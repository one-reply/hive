//! `git blame` of an editor buffer: which commit last changed each line.
//!
//! The buffer is passed with `--contents -`, so unsaved edits and inserted lines are blamed as
//! uncommitted instead of shifting every line below them onto the wrong commit.
use super::{GitError, READ_LIMIT, bytes, run};
use crate::files::Snapshot;
use crate::host::Host;
use chrono::{DateTime, FixedOffset};
use std::collections::HashMap;
use std::path::Path;

/// Who last changed each line of one buffer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blame {
    /// For each buffer line, an index into `origins`.
    lines: Vec<usize>,
    origins: Vec<Origin>,
}

/// Where a line comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Changed in the working tree or the unsaved buffer.
    Uncommitted,
    /// Last changed by this commit.
    Commit(Commit),
}

/// The commit that last changed a line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Commit {
    /// Full object ID.
    pub oid: String,
    /// Author name.
    pub author: String,
    /// Author email without angle brackets.
    pub email: String,
    /// ISO-8601 author date in the author's time zone; empty when Git gave none we could read.
    pub date: String,
    /// Subject line.
    pub summary: String,
}

impl From<Commit> for super::history::Commit {
    fn from(commit: Commit) -> Self {
        Self {
            oid: commit.oid,
            subject: commit.summary,
            author: commit.author,
            date: commit.date,
        }
    }
}

impl Blame {
    /// The origin of 0-based buffer `line`, `None` past the end.
    pub fn line(&self, line: usize) -> Option<&Origin> {
        self.origins.get(*self.lines.get(line)?)
    }
}

/// Blocking: blames the editor `text` as the current version of `path`, relative to `cwd`.
///
/// The text is encoded with the BOM and line endings of `base`, the version on disk, exactly as a
/// save would write it: the editor strips both, and Git would otherwise see every line of a CRLF
/// file, or the first line of a file with a BOM, as rewritten. Fails for a path Git does not
/// track at `HEAD`, as `git blame` itself does.
pub fn read(
    host: &Host,
    cwd: &Path,
    path: &Path,
    base: &Snapshot,
    text: &str,
) -> Result<Blame, GitError> {
    let path = path
        .to_str()
        .ok_or_else(|| GitError::Invalid(format!("Non-UTF-8 path {}", path.display())))?;
    let raw = bytes(
        host,
        cwd,
        &["blame", "--porcelain", "--contents", "-", "--", path],
        Some(base.encode(text).as_bytes()),
        READ_LIMIT,
        false,
    )?;
    parse(&raw)
}

/// Blocking: the full message of commit `oid`, as the hover card shows it.
pub fn message(host: &Host, cwd: &Path, oid: &str) -> Result<String, GitError> {
    if !is_oid(oid) {
        return Err(GitError::Invalid(format!("Not a commit id: {oid}")));
    }
    run(host, cwd, &["show", "-s", "--format=%B", oid, "--"])
}

fn parse(raw: &[u8]) -> Result<Blame, GitError> {
    let malformed = || GitError::Invalid("Malformed Git blame output".into());
    // Author names and line contents are not guaranteed UTF-8; a lossy name beats no blame.
    let text = String::from_utf8_lossy(raw);
    let mut blame = Blame::default();
    let mut known: HashMap<&str, usize> = HashMap::new();
    // The origin whose header was read and whose content line has not arrived yet.
    let mut open: Option<usize> = None;
    for line in text.split('\n') {
        if line.starts_with('\t') {
            blame.lines.push(open.take().ok_or_else(malformed)?);
            continue;
        }
        let Some(origin) = open else {
            if line.is_empty() {
                continue;
            }
            let mut fields = line.split(' ');
            let oid = fields
                .next()
                .filter(|oid| is_oid(oid))
                .ok_or_else(malformed)?;
            let final_line: usize = fields
                .nth(1)
                .and_then(|n| n.parse().ok())
                .ok_or_else(malformed)?;
            if final_line != blame.lines.len() + 1 {
                return Err(malformed());
            }
            let next = blame.origins.len();
            let origin = *known.entry(oid).or_insert(next);
            if origin == next {
                blame.origins.push(if oid.bytes().all(|b| b == b'0') {
                    Origin::Uncommitted
                } else {
                    Origin::Commit(Commit {
                        oid: oid.into(),
                        ..Commit::default()
                    })
                });
            }
            open = Some(origin);
            continue;
        };
        if let Origin::Commit(commit) = &mut blame.origins[origin] {
            apply_header(commit, line);
        }
    }
    if open.is_some() {
        return Err(malformed());
    }
    Ok(blame)
}

fn apply_header(commit: &mut Commit, line: &str) {
    let (key, value) = line.split_once(' ').unwrap_or((line, ""));
    match key {
        "author" => commit.author = value.into(),
        "author-mail" => {
            commit.email = value.trim_start_matches('<').trim_end_matches('>').into();
        }
        // `author-time` precedes `author-tz`: keep the raw seconds until the zone arrives.
        "author-time" => commit.date = value.into(),
        "author-tz" => commit.date = iso_date(&commit.date, value).unwrap_or_default(),
        "summary" => commit.summary = value.into(),
        _ => {}
    }
}

/// `1790838000` and `+0300` → `2026-10-01T10:00:00+03:00`.
fn iso_date(seconds: &str, zone: &str) -> Option<String> {
    let (sign, digits) = match zone.split_at_checked(1)? {
        ("+", digits) => (1, digits),
        ("-", digits) => (-1, digits),
        _ => return None,
    };
    if digits.len() != 4 {
        return None;
    }
    let hours: i32 = digits[..2].parse().ok()?;
    let minutes: i32 = digits[2..].parse().ok()?;
    let offset = FixedOffset::east_opt(sign * (hours * 3600 + minutes * 60))?;
    let date = DateTime::from_timestamp(seconds.parse().ok()?, 0)?;
    Some(date.with_timezone(&offset).to_rfc3339())
}

/// SHA-1 or SHA-256 object ID; also keeps a value Git would read as an option out of argv.
fn is_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = include_bytes!("fixtures/blame-porcelain.txt");

    fn commit(blame: &Blame, line: usize) -> &Commit {
        match blame.line(line) {
            Some(Origin::Commit(commit)) => commit,
            other => panic!("line {line} is {other:?}"),
        }
    }

    #[test]
    fn parses_captured_blame_of_an_edited_buffer() {
        let blame = parse(SAMPLE).unwrap();
        let first = commit(&blame, 0);
        assert_eq!(first.oid, "b4eb94f00ec0aaa2554e0307c2d8f85833458738");
        assert_eq!(first.author, "Ada Lovelace");
        assert_eq!(first.email, "ada@example.com");
        assert_eq!(first.date, "2026-10-01T10:00:00+03:00");
        assert_eq!(first.summary, "Add numbers");
        assert_eq!(commit(&blame, 1).author, "Grace Hopper");
        assert_eq!(commit(&blame, 1).date, "2026-11-01T12:00:00+00:00");
        // A repeated commit carries no headers and must still resolve to the full record.
        assert_eq!(commit(&blame, 2), first);
        assert_eq!(blame.line(3), Some(&Origin::Uncommitted));
        assert_eq!(blame.line(4), Some(&Origin::Uncommitted));
        assert_eq!(blame.line(5), None);
    }

    #[test]
    fn blamed_commit_opens_as_a_history_commit() {
        let blame = parse(SAMPLE).unwrap();
        let history: crate::git::history::Commit = commit(&blame, 0).clone().into();
        assert_eq!(history.oid, "b4eb94f00ec0aaa2554e0307c2d8f85833458738");
        assert_eq!(history.subject, "Add numbers");
        assert_eq!(history.author, "Ada Lovelace");
        assert_eq!(history.date, "2026-10-01T10:00:00+03:00");
    }

    #[test]
    fn empty_buffer_has_no_lines() {
        assert_eq!(parse(b"").unwrap().line(0), None);
    }

    #[test]
    fn truncated_output_is_rejected() {
        let cut = SAMPLE
            .iter()
            .rposition(|b| *b == b'\t')
            .expect("sample has content lines");
        assert!(matches!(parse(&SAMPLE[..cut]), Err(GitError::Invalid(_))));
    }

    #[test]
    fn malformed_headers_are_rejected() {
        for raw in [
            &b"not-an-oid 1 1 1\n\tline\n"[..],
            b"b4eb94f00ec0aaa2554e0307c2d8f85833458738 1\n\tline\n",
            b"b4eb94f00ec0aaa2554e0307c2d8f85833458738 1 2 1\n\tline\n",
            b"\tline without header\n",
        ] {
            assert!(matches!(parse(raw), Err(GitError::Invalid(_))), "{raw:?}");
        }
    }

    #[test]
    fn non_utf8_author_and_bad_dates_degrade() {
        let raw = b"b4eb94f00ec0aaa2554e0307c2d8f85833458738 1 1 1\nauthor Ren\xe9\nauthor-time soon\nauthor-tz +0300\n\tline\n";
        let blame = parse(raw).unwrap();
        assert_eq!(commit(&blame, 0).author, "Ren\u{fffd}");
        assert_eq!(commit(&blame, 0).date, "");
    }

    #[test]
    fn dates_keep_the_author_zone() {
        assert_eq!(
            iso_date("1790838000", "-0130").as_deref(),
            Some("2026-10-01T05:30:00-01:30")
        );
        assert_eq!(iso_date("1790838000", "0300"), None);
        assert_eq!(iso_date("1790838000", "+03"), None);
    }

    /// A throwaway repository with one committed file, removed on drop.
    struct Repo(std::path::PathBuf);

    impl Repo {
        fn with_committed(name: &str, raw: &[u8]) -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "hive-blame-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            let repo = Self(path);
            for args in [
                &["init", "-q"][..],
                &["config", "user.name", "Hive Test"],
                &["config", "user.email", "hive@example.invalid"],
                &["config", "commit.gpgsign", "false"],
                &["config", "core.hooksPath", "/dev/null"],
                // Store the bytes as written, whatever the developer's global setting.
                &["config", "core.autocrlf", "false"],
            ] {
                run(&Host::Local, &repo.0, args).unwrap();
            }
            std::fs::write(repo.0.join(name), raw).unwrap();
            run(&Host::Local, &repo.0, &["add", "--", name]).unwrap();
            run(&Host::Local, &repo.0, &["commit", "-q", "-m", "fixture"]).unwrap();
            repo
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn unchanged_crlf_file_with_bom_is_blamed_on_its_commit() {
        let raw = "\u{feff}one\r\ntwo\r\n";
        let repo = Repo::with_committed("a.txt", raw.as_bytes());
        let base = Snapshot::parse(Path::new("a.txt"), raw.as_bytes()).unwrap();
        let blame = read(
            &Host::Local,
            &repo.0,
            Path::new("a.txt"),
            &base,
            &base.text(),
        )
        .unwrap();
        for line in 0..2 {
            assert!(
                matches!(blame.line(line), Some(Origin::Commit(_))),
                "line {line} is {:?}",
                blame.line(line)
            );
        }
    }

    #[test]
    fn edited_line_of_a_crlf_file_alone_is_uncommitted() {
        let raw = "one\r\ntwo\r\n";
        let repo = Repo::with_committed("a.txt", raw.as_bytes());
        let base = Snapshot::parse(Path::new("a.txt"), raw.as_bytes()).unwrap();
        let blame = read(
            &Host::Local,
            &repo.0,
            Path::new("a.txt"),
            &base,
            "one\nTWO\n",
        )
        .unwrap();
        assert!(matches!(blame.line(0), Some(Origin::Commit(_))));
        assert_eq!(blame.line(1), Some(&Origin::Uncommitted));
    }

    #[test]
    fn only_object_ids_reach_git_show() {
        assert!(is_oid("b4eb94f00ec0aaa2554e0307c2d8f85833458738"));
        assert!(!is_oid("--output=/tmp/x"));
        assert!(matches!(
            message(&Host::Local, Path::new("/"), "--help"),
            Err(GitError::Invalid(_))
        ));
    }
}
