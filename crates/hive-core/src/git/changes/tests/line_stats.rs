use super::*;
use crate::git::status::{FileChange, LineStats};

fn counts(added: usize, removed: usize) -> Option<LineStats> {
    Some(LineStats { added, removed })
}

fn file<'a>(files: &'a [FileChange], path: &str) -> &'a FileChange {
    files.iter().find(|f| f.path == path).unwrap()
}

#[test]
fn counts_follow_index_worktree_untracked_and_immutable_reviews() {
    let repo = Repo::new();
    repo.write("tracked.txt", "one\ntwo\nthree\n");
    repo.commit();
    repo.git(&["branch", "base"]);
    repo.write("tracked.txt", "one\nTWO\nthree\nfour\n");
    repo.git(&["add", "tracked.txt"]);
    repo.write("tracked.txt", "one\nTWO\nthree\nfour\nfive\n");
    repo.write("-new\tфайл\n.txt", "new\nlast");
    repo.write("empty.txt", "");
    repo.write("binary.bin", "new\0bytes");

    let status = status::read_with_stats(&Host::Local, &repo.0).unwrap();
    let tracked = file(&status.files, "tracked.txt");
    assert_eq!(tracked.staged_stats, counts(2, 1));
    assert_eq!(tracked.unstaged_stats, counts(1, 0));
    assert_eq!(
        file(&status.files, "-new\tфайл\n.txt").unstaged_stats,
        counts(2, 0)
    );
    assert_eq!(
        file(&status.files, "empty.txt").unstaged_stats,
        counts(0, 0)
    );
    assert_eq!(file(&status.files, "binary.bin").unstaged_stats, None);

    // The status letters stay MM, but the counters must still refresh.
    repo.write("tracked.txt", "one\nTWO\nthree\nfour\nfive\nsix\n");
    let refreshed = status::read_with_stats(&Host::Local, &repo.0).unwrap();
    assert_eq!(
        file(&refreshed.files, "tracked.txt").unstaged_stats,
        counts(2, 0)
    );
    assert_ne!(status, refreshed);
    assert!(
        status::read(&Host::Local, &repo.0)
            .unwrap()
            .files
            .iter()
            .all(|f| f.staged_stats.is_none() && f.unstaged_stats.is_none())
    );

    repo.git(&["add", "tracked.txt"]);
    repo.git(&["commit", "-m", "change tracked"]);
    for files in [
        history::commit_files(&Host::Local, &repo.0, "HEAD").unwrap(),
        history::compare_files(&Host::Local, &repo.0, "base").unwrap(),
    ] {
        assert_eq!(file(&files, "tracked.txt").staged_stats, counts(4, 1));
    }
    let root = history::commit_files(&Host::Local, &repo.0, "base").unwrap();
    assert_eq!(file(&root, "tracked.txt").staged_stats, counts(3, 0));
}

#[test]
fn counts_handle_unborn_renames_deletions_and_binary_changes() {
    let repo = Repo::new();
    repo.write("old\tname.txt", "one\ntwo\nthree\nfour\n");
    repo.write("deleted.txt", "one\ntwo\nthree");
    repo.write("binary.bin", "old\0bytes");
    repo.git(&["add", "-A"]);
    let unborn = status::read_with_stats(&Host::Local, &repo.0).unwrap();
    assert_eq!(
        file(&unborn.files, "old\tname.txt").staged_stats,
        counts(4, 0)
    );
    assert_eq!(file(&unborn.files, "binary.bin").staged_stats, None);
    repo.commit();
    repo.git(&["mv", "old\tname.txt", "new\nname.txt"]);
    repo.write("new\nname.txt", "one\ntwo\nthree\nfive\nsix\n");
    repo.write("binary.bin", "new\0bytes");
    repo.git(&["rm", "deleted.txt"]);
    repo.git(&["add", "-A"]);
    let status = status::read_with_stats(&Host::Local, &repo.0).unwrap();
    assert_eq!(
        file(&status.files, "new\nname.txt").old_path.as_deref(),
        Some("old\tname.txt")
    );
    assert_eq!(
        file(&status.files, "new\nname.txt").staged_stats,
        counts(2, 1)
    );
    assert_eq!(
        file(&status.files, "deleted.txt").staged_stats,
        counts(0, 3)
    );
    assert_eq!(file(&status.files, "binary.bin").staged_stats, None);
    repo.commit();
    let files = history::commit_files(&Host::Local, &repo.0, "HEAD").unwrap();
    assert_eq!(file(&files, "new\nname.txt").staged_stats, counts(2, 1));
    assert_eq!(file(&files, "deleted.txt").staged_stats, counts(0, 3));
    assert_eq!(file(&files, "binary.bin").staged_stats, None);
}
