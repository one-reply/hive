use std::path::{Path, PathBuf};

/// How many recently opened files a worktree's search prefers.
const LIMIT: usize = 10;

/// Moves `path` to the front of `recent`, newest first.
pub(super) fn remember(recent: &mut Vec<PathBuf>, path: &Path) {
    recent.retain(|old| old != path);
    recent.insert(0, path.into());
    recent.truncate(LIMIT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recents_are_bounded_and_deduplicated_newest_first() {
        let mut recent = Vec::new();
        for n in 0..15 {
            remember(&mut recent, Path::new(&format!("{n}.rs")));
        }
        remember(&mut recent, Path::new("7.rs"));
        assert_eq!(recent.len(), LIMIT);
        assert_eq!(recent[0], PathBuf::from("7.rs"));
        assert_eq!(recent[1], PathBuf::from("14.rs"));
        assert_eq!(
            recent
                .iter()
                .filter(|path| *path == Path::new("7.rs"))
                .count(),
            1
        );
    }
}
