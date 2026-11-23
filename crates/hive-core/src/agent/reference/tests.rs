use super::*;
use crate::git::diff::parse;
use std::path::PathBuf;

const OID: &str = "0123456789abcdef0123456789abcdef01234567";

fn reference(path: &str, lines: RangeInclusive<usize>, version: Version) -> CodeReference {
    CodeReference::new(
        Host::Local,
        Path::new("/work/tree"),
        Path::new(path),
        lines,
        version,
    )
    .unwrap()
}

fn three_hunks() -> FileDiff {
    parse(include_str!("../../git/fixtures/diff-three-hunks.patch").into()).unwrap()
}

#[test]
fn selected_lines_count_whole_lines_and_an_empty_selection_as_its_line() {
    let text = "one\ntwo\nthree";
    assert_eq!(selected_lines(text, 4..7), Some(2..=2));
    assert_eq!(selected_lines(text, 0..8), Some(1..=2));
    assert_eq!(selected_lines(text, 2..10), Some(1..=3));
    assert_eq!(selected_lines(text, 8..8), Some(3..=3));
    assert_eq!(selected_lines(text, 0..0), Some(1..=1));
}

#[test]
fn selected_lines_reject_ranges_outside_text_or_inside_a_character() {
    assert_eq!(selected_lines("é", 1..2), None);
    assert_eq!(selected_lines("abc", 2..9), None);
}

#[test]
fn worktree_lines_become_claude_mentions_with_an_absolute_path() {
    assert_eq!(
        reference("src/app.js", 133..=141, Version::Worktree).to_text(),
        "@/work/tree/src/app.js#L133-141 "
    );
    assert_eq!(
        reference("src/app.js", 7..=7, Version::Worktree).to_text(),
        "@/work/tree/src/app.js#L7 "
    );
    // A selection made upwards ends above where it started.
    let (anchor, cursor) = (9, 3);
    assert_eq!(
        reference("src/app.js", anchor..=cursor, Version::Worktree).to_text(),
        "@/work/tree/src/app.js#L3-9 "
    );
}

#[test]
fn the_label_names_the_file_from_the_worktree_root() {
    assert_eq!(
        reference("src/app.js", 1..=2, Version::Worktree).label(),
        "@src/app.js#L1-2"
    );
    assert_eq!(
        reference("src/app.js", 1..=1, Version::Head).label(),
        "src/app.js#L1 (HEAD)"
    );
}

#[test]
fn paths_with_spaces_are_quoted_mentions() {
    assert_eq!(
        reference("my dir/app.js", 1..=2, Version::Worktree).to_text(),
        "@\"/work/tree/my dir/app.js#L1-2\" "
    );
}

#[test]
fn other_versions_are_named_instead_of_mentioned() {
    let text = |version| reference("src/app.js", 3..=4, version).to_text();
    assert_eq!(text(Version::Index), "/work/tree/src/app.js#L3-4 (index) ");
    assert_eq!(text(Version::Head), "/work/tree/src/app.js#L3-4 (HEAD) ");
    assert_eq!(
        text(Version::Commit(OID.into())),
        "/work/tree/src/app.js#L3-4 (0123456789ab) "
    );
    assert_eq!(
        text(Version::Parent(OID.into())),
        "/work/tree/src/app.js#L3-4 (0123456789ab^) "
    );
    assert_eq!(
        text(Version::MergeBase("main".into())),
        "/work/tree/src/app.js#L3-4 (merge base of main and HEAD) "
    );
}

#[test]
fn a_terminal_of_the_same_worktree_gets_a_path_relative_to_its_directory() {
    let reference = reference("src/app.js", 1..=2, Version::Worktree);
    let worktree = Path::new("/work/tree");
    let text = |directory| reference.to_text_in(&Host::Local, worktree, Path::new(directory));
    assert_eq!(text("/work/tree"), "@src/app.js#L1-2 ");
    assert_eq!(text("/work/tree/src"), "@app.js#L1-2 ");
    assert_eq!(text("/work/tree/docs"), "@../src/app.js#L1-2 ");
}

#[test]
fn a_terminal_of_another_worktree_or_host_gets_the_absolute_path() {
    let reference = reference("src/app.js", 1..=2, Version::Worktree);
    let absolute = "@/work/tree/src/app.js#L1-2 ";
    let other = Path::new("/work/other");
    assert_eq!(reference.to_text_in(&Host::Local, other, other), absolute);
    let remote = Host::Ssh {
        target: "devbox".into(),
    };
    let worktree = Path::new("/work/tree");
    assert_eq!(reference.to_text_in(&remote, worktree, worktree), absolute);
    // A directory Hive could not read keeps a path that works from anywhere.
    assert_eq!(
        reference.to_text_in(&Host::Local, worktree, Path::new("relative")),
        absolute
    );
}

#[test]
fn references_reject_paths_that_cannot_be_named_safely() {
    let new = |worktree: &str, path: &str| {
        CodeReference::new(
            Host::Local,
            Path::new(worktree),
            Path::new(path),
            1..=1,
            Version::Worktree,
        )
    };
    assert!(new("work/tree", "a.rs").is_err());
    assert!(new("/work/tree", "../a.rs").is_err());
    assert!(new("/work/tree", "/etc/passwd").is_err());
    assert!(new("/work/tree", "").is_err());
    assert!(new("/work/tree", "a\u{1b}[31m.rs").is_err());
    let non_utf8 = {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        PathBuf::from(OsString::from_vec(b"a\xff.rs".to_vec()))
    };
    assert!(
        CodeReference::new(
            Host::Local,
            Path::new("/work"),
            &non_utf8,
            1..=1,
            Version::Worktree
        )
        .is_err()
    );
}

#[test]
fn references_round_trip_through_clipboard_json() {
    let reference = reference("src/app.js", 3..=4, Version::Parent(OID.into()));
    let json = serde_json::to_string(&reference).unwrap();
    assert_eq!(
        serde_json::from_str::<CodeReference>(&json).unwrap(),
        reference
    );
    assert!(serde_json::from_str::<CodeReference>("[1, 2]").is_err());
}

#[test]
fn each_diff_side_names_the_version_it_shows() {
    let commit = DiffSource::Commit(OID.into());
    let compare = DiffSource::Compare("main".into());
    assert_eq!(
        Version::original(&DiffSource::WorkingTree),
        Some(Version::Index)
    );
    assert_eq!(Version::original(&DiffSource::Staged), Some(Version::Head));
    assert_eq!(
        Version::original(&commit),
        Some(Version::Parent(OID.into()))
    );
    assert_eq!(
        Version::original(&compare),
        Some(Version::MergeBase("main".into()))
    );
    assert_eq!(Version::original(&DiffSource::Untracked), None);
    assert_eq!(
        Version::modified(&DiffSource::WorkingTree),
        Version::Worktree
    );
    assert_eq!(Version::modified(&DiffSource::Untracked), Version::Worktree);
    assert_eq!(Version::modified(&DiffSource::Staged), Version::Index);
    assert_eq!(Version::modified(&commit), Version::Commit(OID.into()));
    assert_eq!(Version::modified(&compare), Version::Head);
}

#[test]
fn selected_diff_lines_on_one_side_keep_that_side() {
    let diff = three_hunks();
    let source = DiffSource::WorkingTree;
    assert_eq!(
        diff_lines(&diff, &source, Line::Original(15), Line::Original(13)),
        Some((Version::Index, 13..=15))
    );
    assert_eq!(
        diff_lines(&diff, &source, Line::Modified(25), Line::Modified(26)),
        Some((Version::Worktree, 25..=26))
    );
    assert_eq!(
        diff_lines(
            &diff,
            &DiffSource::Untracked,
            Line::Original(1),
            Line::Original(2)
        ),
        None
    );
}

#[test]
fn a_selection_across_sides_names_the_modified_lines_it_covers() {
    let diff = three_hunks();
    let source = DiffSource::WorkingTree;
    assert_eq!(
        diff_lines(&diff, &source, Line::Original(3), Line::Modified(3)),
        Some((Version::Worktree, 3..=3))
    );
    // From "line 12" down to "line 18", which is line 17 after the removed "line 15".
    assert_eq!(
        diff_lines(&diff, &source, Line::Modified(17), Line::Original(12)),
        Some((Version::Worktree, 12..=17))
    );
    assert_eq!(
        diff_lines(&diff, &source, Line::Original(99), Line::Modified(3)),
        None
    );
}

#[test]
fn selected_text_found_once_names_its_lines() {
    let diff = three_hunks();
    let find = |text| find_in_diff(&diff, &DiffSource::Staged, text);
    assert_eq!(find("line 3 changed"), Some((Version::Index, 3..=3)));
    assert_eq!(
        find("added one\nadded two\n"),
        Some((Version::Index, 25..=26))
    );
    assert_eq!(find("3 changed\nline"), Some((Version::Index, 3..=4)));
    // Context lines are on both sides; the modified side wins.
    assert_eq!(find("line 13\nline 14"), Some((Version::Index, 13..=14)));
    // Only the original side has a removed line.
    assert_eq!(find("line 15"), Some((Version::Head, 15..=15)));
}

#[test]
fn selected_text_without_a_single_place_names_no_lines() {
    let diff = three_hunks();
    let find = |text| find_in_diff(&diff, &DiffSource::WorkingTree, text);
    // "line 2" is also the start of "line 23" … "line 29".
    assert_eq!(find("line 2"), None);
    assert_eq!(find("line 6\nline 12"), None);
    assert_eq!(find("not in the diff"), None);
    assert_eq!(find(" \n"), None);
    assert_eq!(find(""), None);
    let overlapping = parse("--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+aaa\n".into()).unwrap();
    assert_eq!(
        find_in_diff(&overlapping, &DiffSource::WorkingTree, "aa"),
        None
    );
}
