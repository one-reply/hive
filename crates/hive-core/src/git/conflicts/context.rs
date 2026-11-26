//! Labels and progress for the Git operation that stopped in this worktree.
use super::{
    GitError, Host, INSPECT_OUTPUT_LIMIT, Operation, READ_TIMEOUT, Side, operation_marker,
    resolve_git_paths, run,
};
use crate::command::{self, Cancellation};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

/// Reads the first line of each readable path, separated by NUL bytes, in one remote call.
const FIRST_LINES_SCRIPT: &str = r#"for path do if [ -f "$path" ] && [ -r "$path" ]; then IFS= read -r line < "$path" || true; printf '%s\0' "$line"; else printf '\0'; fi; done"#;

/// One side of an active merge, rebase, cherry-pick, or revert.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideInfo {
    /// Human-readable branch, ref, or short commit OID (for example `"main"`, `"develop"`).
    pub label: String,
    /// Short commit OID and subject when available (for example `"8f3a1b2 Fix snap"`).
    pub commit: Option<String>,
}

/// Active Git operation together with the branches/commits involved and merge direction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationContext {
    /// Which Git operation stopped for conflicts.
    pub operation: Operation,
    /// The checked-out / target side (`Ours`, index stage 2).
    ///
    /// During rebase this is the `onto` branch/commit being built upon; during merge,
    /// cherry-pick, or revert this is `HEAD`.
    pub current: SideInfo,
    /// The incoming / replayed side (`Theirs`, index stage 3).
    ///
    /// During rebase this is the branch being rebased; during merge this is `MERGE_HEAD`.
    pub incoming: SideInfo,
    /// Rebase progress `(step, total)` when known.
    pub step: Option<(usize, usize)>,
}

impl OperationContext {
    /// Full human-readable description of what is being merged/rebased into what.
    pub fn direction_summary(&self) -> String {
        match self.operation {
            Operation::Merge => format!("Merging {} → {}", self.incoming.label, self.current.label),
            Operation::Rebase => match self.step {
                Some((step, total)) => format!(
                    "Rebasing {} onto {} (step {step}/{total})",
                    self.incoming.label, self.current.label
                ),
                None => format!(
                    "Rebasing {} onto {}",
                    self.incoming.label, self.current.label
                ),
            },
            Operation::CherryPick => {
                format!(
                    "Cherry-picking {} → {}",
                    self.incoming.label, self.current.label
                )
            }
            Operation::Revert => {
                format!("Revert on {} ({})", self.current.label, self.incoming.label)
            }
        }
    }

    /// Label for one side of the operation.
    pub fn side(&self, side: Side) -> &SideInfo {
        match side {
            Side::Ours => &self.current,
            Side::Theirs => &self.incoming,
        }
    }
}

/// Blocking: the active operation together with the branch or commit on each side.
pub fn operation_context(host: &Host, cwd: &Path) -> Result<Option<OperationContext>, GitError> {
    Ok(operation_marker(host, cwd)?.map(|(op, marker)| match op {
        Operation::Rebase => read_rebase_context(host, cwd, marker),
        _ => read_head_context(host, cwd, op, marker),
    }))
}

fn read_rebase_context(host: &Host, cwd: &Path, dir: &str) -> OperationContext {
    let (step_a, step_b) = if dir == "rebase-merge" {
        ("msgnum", "end")
    } else {
        ("next", "last")
    };
    let head_name_key = format!("{dir}/head-name");
    let onto_key = format!("{dir}/onto");
    let step_a_key = format!("{dir}/{step_a}");
    let step_b_key = format!("{dir}/{step_b}");
    let values = read_git_first_lines(
        host,
        cwd,
        &[&head_name_key, &onto_key, &step_a_key, &step_b_key],
    );

    let incoming_label = values
        .first()
        .and_then(|v| v.as_deref())
        .map(strip_ref_prefix)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "HEAD".to_owned());
    let current_label = values
        .get(1)
        .and_then(Option::as_deref)
        .filter(|value| crate::git::blame::is_oid(value))
        .map(|oid| resolve_oid_label(host, cwd, oid))
        .unwrap_or_else(|| "rebase target".into());
    // Earlier steps can have advanced HEAD past the original onto commit.
    let current_commit = commit_summary(host, cwd, "HEAD");
    let incoming_commit = commit_summary(host, cwd, "REBASE_HEAD");
    let step = parse_step(
        values.get(2).and_then(|v| v.as_deref()),
        values.get(3).and_then(|v| v.as_deref()),
    );
    OperationContext {
        operation: Operation::Rebase,
        current: SideInfo {
            label: current_label,
            commit: current_commit,
        },
        incoming: SideInfo {
            label: incoming_label,
            commit: incoming_commit,
        },
        step,
    }
}

fn read_head_context(
    host: &Host,
    cwd: &Path,
    operation: Operation,
    head_ref: &str,
) -> OperationContext {
    let current_label = current_branch_name(host, cwd).unwrap_or_else(|| "HEAD".to_owned());
    let current_commit = commit_summary(host, cwd, "HEAD");
    let (decorated, incoming_commit) = commit_decoration_and_summary(host, cwd, head_ref);

    let incoming_label = match operation {
        Operation::Merge => {
            let msg_line = read_git_first_lines(host, cwd, &["MERGE_MSG"])
                .into_iter()
                .next()
                .flatten();
            msg_line
                .as_deref()
                .and_then(parse_merge_msg_source)
                .or(decorated)
                .or_else(|| short_oid(host, cwd, head_ref))
                .unwrap_or_else(|| "Incoming".to_owned())
        }
        Operation::CherryPick => decorated
            .or_else(|| short_oid(host, cwd, head_ref))
            .unwrap_or_else(|| "Cherry-pick".to_owned()),
        Operation::Revert => short_oid(host, cwd, head_ref)
            .map(|oid| format!("before {oid}"))
            .unwrap_or_else(|| "before reverted change".into()),
        Operation::Rebase => unreachable!("rebase handled separately"),
    };

    OperationContext {
        operation,
        current: SideInfo {
            label: current_label,
            commit: current_commit,
        },
        incoming: SideInfo {
            label: incoming_label,
            // Stage 3 contains the inverse change, not REVERT_HEAD itself.
            commit: incoming_commit.filter(|_| operation != Operation::Revert),
        },
        step: None,
    }
}

fn current_branch_name(host: &Host, cwd: &Path) -> Option<String> {
    if let Ok(branch) = run(host, cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        let trimmed = branch.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }
    short_oid(host, cwd, "HEAD")
}

fn short_oid(host: &Host, cwd: &Path, rev: &str) -> Option<String> {
    let out = run(host, cwd, &["rev-parse", "--short", "--quiet", rev]).ok()?;
    let trimmed = out.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn resolve_oid_label(host: &Host, cwd: &Path, oid: &str) -> String {
    if let Ok(output) = run(
        host,
        cwd,
        &[
            "for-each-ref",
            "--points-at",
            oid,
            "--format=%(refname)%00%(refname:short)",
        ],
    ) && let Some(best) = pick_best_ref(&output)
    {
        return best;
    }
    oid.get(..7).unwrap_or(oid).to_owned()
}

/// Prefers a local branch over a remote branch (excluding `*/HEAD`), then any tag.
fn pick_best_ref(output: &str) -> Option<String> {
    let mut remote = None;
    let mut tag = None;
    for line in output.lines() {
        let Some((full, short)) = line.split_once('\0') else {
            continue;
        };
        if short.is_empty() || full.ends_with("/HEAD") {
            continue;
        }
        if full.starts_with("refs/heads/") {
            return Some(short.to_owned());
        }
        if full.starts_with("refs/remotes/") && remote.is_none() {
            remote = Some(short.to_owned());
        } else if tag.is_none() {
            tag = Some(short.to_owned());
        }
    }
    remote.or(tag)
}

fn commit_summary(host: &Host, cwd: &Path, rev: &str) -> Option<String> {
    commit_decoration_and_summary(host, cwd, rev).1
}

fn commit_decoration_and_summary(
    host: &Host,
    cwd: &Path,
    rev: &str,
) -> (Option<String>, Option<String>) {
    let Ok(output) = run(host, cwd, &["log", "-1", "--format=%D%x00%h%x00%s", rev]) else {
        return (None, None);
    };
    let mut parts = output.splitn(3, '\0');
    let decoration = parts.next().and_then(pick_decoration);
    let sha = parts.next().map(str::trim).unwrap_or("");
    let subject = parts.next().map(str::trim).unwrap_or("");
    let summary = match (sha.is_empty(), subject.is_empty()) {
        (true, _) => None,
        (false, true) => Some(sha.to_owned()),
        (false, false) => Some(format!("{sha} {subject}")),
    };
    (decoration, summary)
}

/// Extracts the most useful branch/tag name from `git log --format=%D` output.
fn pick_decoration(raw: &str) -> Option<String> {
    raw.split(',')
        .map(str::trim)
        .map(|item| item.strip_prefix("HEAD -> ").unwrap_or(item))
        .find(|item| !item.is_empty() && !item.ends_with("/HEAD") && !item.starts_with("tag: "))
        .map(ToOwned::to_owned)
        .or_else(|| {
            raw.split(',')
                .map(str::trim)
                .find_map(|item| item.strip_prefix("tag: "))
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(ToOwned::to_owned)
        })
}

/// Extracts the source branch, remote branch, or tag from Git's `MERGE_MSG` first line.
fn parse_merge_msg_source(first_line: &str) -> Option<String> {
    let line = first_line.trim();
    for prefix in [
        "Merge branch '",
        "Merge remote-tracking branch '",
        "Merge tag '",
        "Merge branches '",
    ] {
        if let Some(rest) = line.strip_prefix(prefix)
            && let Some((name, _)) = rest.split_once('\'')
            && !name.is_empty()
        {
            return Some(name.to_owned());
        }
    }
    None
}

fn strip_ref_prefix(raw: &str) -> String {
    let trimmed = raw.trim();
    trimmed
        .strip_prefix("refs/heads/")
        .or_else(|| trimmed.strip_prefix("refs/remotes/"))
        .unwrap_or(trimmed)
        .to_owned()
}

fn parse_step(current: Option<&str>, total: Option<&str>) -> Option<(usize, usize)> {
    let current = current?.trim().parse::<usize>().ok()?;
    let total = total?.trim().parse::<usize>().ok()?;
    (current > 0 && total > 0).then_some((current, total))
}

fn read_git_first_lines(host: &Host, cwd: &Path, entries: &[&str]) -> Vec<Option<String>> {
    let Ok(resolved) = resolve_git_paths(host, cwd, entries) else {
        return vec![None; entries.len()];
    };
    match host {
        Host::Local => resolved
            .into_iter()
            .map(|path| {
                let path = Path::new(&path);
                let full = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    cwd.join(path)
                };
                if !full.is_file() {
                    return None;
                }
                let file = std::fs::File::open(full).ok()?;
                let mut line = String::new();
                BufReader::new(file.take(INSPECT_OUTPUT_LIMIT as u64))
                    .read_line(&mut line)
                    .ok()?;
                let line = line.trim();
                (!line.is_empty()).then(|| line.to_owned())
            })
            .collect(),
        Host::Ssh { .. } => {
            let mut args = vec!["-c", FIRST_LINES_SCRIPT, "sh"];
            args.extend(resolved.iter().map(String::as_str));
            let Ok(output) = command::run(
                host.command("sh", &args, Some(cwd)),
                &[],
                &Cancellation::default(),
                READ_TIMEOUT,
                INSPECT_OUTPUT_LIMIT,
            ) else {
                return vec![None; entries.len()];
            };
            if output.code != Some(0) {
                return vec![None; entries.len()];
            }
            parse_first_lines(&output.stdout, entries.len())
                .unwrap_or_else(|| vec![None; entries.len()])
        }
    }
}

fn parse_first_lines(stdout: &[u8], expected: usize) -> Option<Vec<Option<String>>> {
    let mut fields: Vec<Option<String>> = stdout
        .split(|b| *b == 0)
        .map(|chunk| {
            let s = String::from_utf8_lossy(chunk).trim().to_owned();
            (!s.is_empty()).then_some(s)
        })
        .collect();
    // Only complete, NUL-terminated fields belong to this response.
    stdout.strip_suffix(b"\0")?;
    fields.pop();
    (fields.len() == expected).then_some(fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_lines_script_reads_readable_files_and_marks_missing_ones_empty() {
        let dir = std::env::temp_dir().join(format!("hive-first-lines-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let present = dir.join("head-name");
        std::fs::write(&present, "refs/heads/feature\nsecond line\n").unwrap();
        let missing = dir.join("missing");
        let output = std::process::Command::new("sh")
            .args(["-c", FIRST_LINES_SCRIPT, "sh"])
            .arg(&present)
            .arg(&missing)
            .output()
            .expect("sh");
        let parsed = parse_first_lines(&output.stdout, 2);
        assert_eq!(parse_first_lines(b"incomplete", 1), None);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            parsed,
            Some(vec![Some("refs/heads/feature".to_owned()), None])
        );
    }

    #[test]
    fn merge_msg_and_ref_helpers_extract_human_branch_names() {
        assert_eq!(
            parse_merge_msg_source("Merge branch 'develop' into main"),
            Some("develop".to_owned())
        );
        assert_eq!(
            parse_merge_msg_source("Merge remote-tracking branch 'origin/release'"),
            Some("origin/release".to_owned())
        );
        assert_eq!(
            parse_merge_msg_source("Merge tag 'v1.2.0'"),
            Some("v1.2.0".to_owned())
        );
        assert_eq!(parse_merge_msg_source("Custom message"), None);

        assert_eq!(
            pick_best_ref(
                "refs/remotes/origin/HEAD\0origin/HEAD\nrefs/remotes/origin/main\0origin/main\nrefs/heads/main\0main\n"
            ),
            Some("main".to_owned())
        );
        assert_eq!(
            pick_decoration("HEAD -> feature, origin/feature, tag: v1"),
            Some("feature".to_owned())
        );
    }

    #[test]
    fn operation_context_formats_merge_and_rebase_directions() {
        let merge = OperationContext {
            operation: Operation::Merge,
            current: SideInfo {
                label: "main".into(),
                commit: None,
            },
            incoming: SideInfo {
                label: "develop".into(),
                commit: None,
            },
            step: None,
        };
        assert_eq!(merge.direction_summary(), "Merging develop → main");

        let rebase = OperationContext {
            operation: Operation::Rebase,
            current: SideInfo {
                label: "develop".into(),
                commit: None,
            },
            incoming: SideInfo {
                label: "feature/snap".into(),
                commit: None,
            },
            step: Some((2, 5)),
        };
        assert_eq!(
            rebase.direction_summary(),
            "Rebasing feature/snap onto develop (step 2/5)"
        );
    }
}
