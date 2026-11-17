//! Persisted acknowledgement of an exact, complete text diff, scoped to its source and host.
use super::diff::{DiffSource, FileDiff};
use crate::host::Host;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const LIMIT: usize = 500;

/// A compact revision token. Only a freshly loaded diff can establish that it is current.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    host: Host,
    #[serde(with = "crate::path_encoding::persisted")]
    worktree: PathBuf,
    path: String,
    source: DiffSource,
    fingerprint: String,
}

impl Revision {
    /// Refuses binary, empty, or abbreviated-object previews whose revision is incomplete.
    pub fn from_diff(
        host: &Host,
        worktree: &Path,
        path: &str,
        source: &DiffSource,
        diff: &FileDiff,
    ) -> Option<Self> {
        let patch = diff.patch();
        if !complete(patch) {
            return None;
        }
        Some(Self {
            host: host.clone(),
            worktree: worktree.into(),
            path: path.into(),
            source: source.clone(),
            fingerprint: format!("{:x}", Sha256::digest(patch.as_bytes())),
        })
    }

    fn same_file(&self, other: &Self) -> bool {
        self.host == other.host
            && self.worktree == other.worktree
            && self.path == other.path
            && self.source == other.source
    }
}

/// Sets one acknowledgement, retiring its older revision and bounding retained history.
pub fn set(records: &mut Vec<Revision>, revision: Revision, reviewed: bool) -> bool {
    let already = records.contains(&revision);
    if already == reviewed {
        return false;
    }
    records.retain(|old| !old.same_file(&revision));
    if reviewed {
        records.push(revision);
        if records.len() > LIMIT {
            records.drain(..records.len() - LIMIT);
        }
    }
    true
}

fn complete(patch: &str) -> bool {
    let mut indexes = 0;
    for line in patch.lines() {
        if line.starts_with("Binary files ")
            || line.starts_with("GIT binary patch")
            || line.starts_with("diff --cc ")
            || line.starts_with("diff --combined ")
        {
            return false;
        }
        if let Some(ids) = line.strip_prefix("index ") {
            let Some((old, new)) = ids
                .split_whitespace()
                .next()
                .and_then(|ids| ids.split_once(".."))
            else {
                return false;
            };
            let valid = |id: &str| {
                matches!(id.len(), 40 | 64) && id.bytes().all(|byte| byte.is_ascii_hexdigit())
            };
            if !valid(old) || !valid(new) {
                return false;
            }
            indexes += 1;
        }
    }
    indexes > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    fn revision(hash: &str) -> Revision {
        Revision {
            host: Host::Local,
            worktree: "/work".into(),
            path: "a.rs".into(),
            source: DiffSource::WorkingTree,
            fingerprint: hash.into(),
        }
    }
    #[test]
    fn an_edited_file_and_another_source_are_not_reviewed_after_restart() {
        let mut records = Vec::new();
        assert!(set(&mut records, revision("old"), true));
        let records: Vec<Revision> =
            serde_json::from_str(&serde_json::to_string(&records).unwrap()).unwrap();
        assert!(records.contains(&revision("old")));
        assert!(!records.contains(&revision("new")));
        let mut staged = revision("old");
        staged.source = DiffSource::Staged;
        assert!(!records.contains(&staged));
        let mut remote = revision("old");
        remote.host = Host::Ssh {
            target: "server".into(),
        };
        assert!(!records.contains(&remote));
    }
    #[test]
    fn new_acknowledgements_replace_old_revisions_and_can_be_removed() {
        let mut records = vec![revision("old")];
        assert!(set(&mut records, revision("new"), true));
        assert_eq!(records, [revision("new")]);
        assert!(!set(&mut records, revision("new"), true));
        assert!(set(&mut records, revision("new"), false));
        assert!(records.is_empty());
    }
    #[test]
    fn only_full_object_ids_establish_a_complete_revision() {
        let full = format!(
            "diff --git a/a b/a\nindex {}..{} 100644\n",
            "0".repeat(40),
            "a".repeat(40)
        );
        assert!(complete(&full));
        for patch in ["", "index 1234567..abcdef0 100644", "new file mode 100644"] {
            assert!(!complete(patch));
        }
        assert!(!complete(&(full + "Binary files a/a and b/a differ\n")));
    }
}
