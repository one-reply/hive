use hive_core::projects::Target;
use std::path::{Path, PathBuf};

pub(super) fn remember(recent: &mut Vec<(Target, Vec<PathBuf>)>, target: &Target, path: &Path) {
    let index = recent
        .iter()
        .position(|(key, _)| key == target)
        .unwrap_or_else(|| {
            recent.push((target.clone(), Vec::new()));
            recent.len() - 1
        });
    let paths = &mut recent[index].1;
    paths.retain(|old| old != path);
    paths.insert(0, path.into());
    paths.truncate(10);
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_core::host::Host;
    #[test]
    fn recents_are_bounded_deduplicated_and_isolated_by_worktree_and_host() {
        let target = Target {
            host: Host::Local,
            project: "/project".into(),
            path: "/project".into(),
        };
        let remote = Target {
            host: Host::Ssh {
                target: "example".into(),
            },
            ..target.clone()
        };
        let branch = Target {
            path: "/worktree".into(),
            ..target.clone()
        };
        let mut recent = Vec::new();
        for n in 0..15 {
            remember(&mut recent, &target, Path::new(&format!("{n}.rs")));
        }
        remember(&mut recent, &remote, Path::new("a.rs"));
        remember(&mut recent, &branch, Path::new("b.rs"));
        remember(&mut recent, &target, Path::new("7.rs"));
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].1.len(), 10);
        assert_eq!(recent[0].1[0], PathBuf::from("7.rs"));
        assert_eq!(recent[1].1, [PathBuf::from("a.rs")]);
        assert_eq!(recent[2].1, [PathBuf::from("b.rs")]);
    }
}
