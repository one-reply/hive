use super::changed_display;
use hive_core::{
    git::diff::{self, DiffSource},
    host::Host,
};
use std::{fs, path::PathBuf};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn unchanged_patch_skips_display_but_status_does_not_hide_content_changes() {
    let fixture =
        Fixture(std::env::temp_dir().join(format!("hive-diff-display-{}", std::process::id())));
    hive_core::git::testing::init(&fixture.0);
    let read = || {
        diff::read(
            &Host::Local,
            &fixture.0,
            "sample.txt",
            &DiffSource::Untracked,
            None,
        )
        .unwrap()
    };
    fs::write(fixture.0.join("sample.txt"), "first\n").unwrap();
    let initial = read();
    let status = hive_core::git::status::read(&Host::Local, &fixture.0).unwrap();
    assert!(changed_display(&initial, None).unwrap().is_some());
    assert!(changed_display(&read(), Some(&initial)).unwrap().is_none());

    fs::write(fixture.0.join("sample.txt"), "second\n").unwrap();
    assert_eq!(
        status,
        hive_core::git::status::read(&Host::Local, &fixture.0).unwrap()
    );
    let updated = read();
    assert!(changed_display(&updated, Some(&initial)).unwrap().is_some());
    assert!(changed_display(&read(), Some(&updated)).unwrap().is_none());
    assert!(
        diff::read(
            &Host::Local,
            &fixture.0.join("missing"),
            "sample.txt",
            &DiffSource::Untracked,
            None
        )
        .is_err()
    );
    // Refresh clears the previous patch on I/O failure, so recovery must parse again.
    assert!(changed_display(&read(), None).unwrap().is_some());
}

#[test]
fn unmerged_file_points_to_its_resolver_instead_of_a_parser_error() {
    let fixture =
        Fixture(std::env::temp_dir().join(format!("hive-diff-conflict-{}", std::process::id())));
    hive_core::git::testing::init(&fixture.0);
    let git = |args: &[&str]| {
        hive_core::git::testing::git(&fixture.0, args)
            .output()
            .unwrap()
            .status
            .success()
    };
    let commit = |content: &str| {
        fs::write(fixture.0.join("sample.txt"), content).unwrap();
        assert!(git(&["add", "-A"]));
        assert!(git(&["commit", "-q", "-m", "fixture"]));
    };
    commit("base\n");
    assert!(git(&["checkout", "-q", "-b", "feature"]));
    commit("feature\n");
    assert!(git(&["checkout", "-q", "main"]));
    commit("main\n");
    assert!(!git(&["merge", "feature"]));
    let patch = diff::read(
        &Host::Local,
        &fixture.0,
        "sample.txt",
        &DiffSource::WorkingTree,
        None,
    )
    .unwrap();
    assert!(patch.patch().starts_with("diff --cc "));
    let error = changed_display(&patch, None).err().expect("no display");
    assert!(error.contains("merge conflicts"), "{error}");
}

#[test]
fn local_sections_keep_both_patches_and_historical_reviews_use_their_own_inventory() {
    use super::{catalog, read_block};
    use crate::state::git::DiffRequest;
    use hive_core::{git::testing, projects::Target};
    let fixture =
        Fixture(std::env::temp_dir().join(format!("hive-review-sections-{}", std::process::id())));
    let path = &fixture.0;
    testing::init(path);
    fs::write(path.join("both"), "original\n").unwrap();
    testing::run(path, &["add", "both"]);
    testing::run(path, &["commit", "-qm", "base"]);
    testing::run(path, &["checkout", "-qb", "topic"]);
    fs::write(path.join("both"), "committed\n").unwrap();
    testing::run(path, &["commit", "-qam", "topic"]);
    fs::write(path.join("both"), "index\n").unwrap();
    testing::run(path, &["add", "both"]);
    fs::write(path.join("both"), "disk\n").unwrap();
    fs::write(path.join("new"), "new\n").unwrap();
    let mut request = DiffRequest {
        target: Target {
            host: Host::Local,
            path: path.clone(),
            project: path.clone(),
        },
        source: DiffSource::Staged,
        path: "both".into(),
        old_path: None,
        whole_file_only: false,
    };
    let entries = catalog::read(&request).unwrap();
    let keys = entries
        .iter()
        .map(|e| e.request.block_key().to_string())
        .collect::<Vec<_>>();
    assert_eq!(keys, ["staged/both", "changes/both", "untracked/new"]);
    let load = |request: &DiffRequest| {
        read_block(request, &request.block_key(), None)
            .unwrap()
            .unwrap()
    };
    let (index, staged, staged_revision) = load(&entries[0].request);
    let (work, disk, disk_revision) = load(&entries[1].request);
    assert_ne!(index.path(), work.path());
    assert_ne!(staged_revision.unwrap(), disk_revision.unwrap());
    assert!(staged.patch().contains("-committed\n+index"));
    assert!(disk.patch().contains("-index\n+disk"));
    for source in [
        DiffSource::Compare("main".into()),
        DiffSource::Commit(testing::run(path, &["rev-parse", "HEAD"])),
    ] {
        request.source = source;
        let entries = catalog::read(&request).unwrap();
        assert_eq!(entries.len(), 1);
        let (_, patch, _) = load(&entries[0].request);
        assert!(patch.patch().contains("-original\n+committed"));
    }
    hive_core::git::diff::apply_hunk(
        &Host::Local,
        path,
        "both",
        &DiffSource::WorkingTree,
        &disk,
        0,
    )
    .unwrap();
    assert_eq!(testing::run(path, &["show", ":both"]), "disk");
    assert!(
        hive_core::git::diff::apply_hunk(
            &Host::Local,
            path,
            "both",
            &DiffSource::Staged,
            &staged,
            0
        )
        .is_err()
    );
}

#[test]
fn an_oversized_file_keeps_its_header_and_can_recover_with_a_binary_patch() {
    use super::{Block, catalog, read_block};
    use crate::state::git::DiffRequest;
    use hive_core::{git::testing, projects::Target};

    let fixture =
        Fixture(std::env::temp_dir().join(format!("hive-review-notice-{}", std::process::id())));
    testing::init(&fixture.0);
    let path = fixture.0.join("sample.bin");
    fs::write(&path, vec![b'x'; diff::PREVIEW_LIMIT + 1]).unwrap();
    let request = DiffRequest {
        target: Target {
            host: Host::Local,
            project: fixture.0.clone(),
            path: fixture.0.clone(),
        },
        path: "sample.bin".into(),
        old_path: None,
        source: DiffSource::Untracked,
        whole_file_only: false,
    };
    let entry = catalog::read(&request).unwrap().remove(0);
    let key = request.block_key();
    let mut block = Block::new(entry, &key);
    assert!(block.install(&key, read_block(&request, &key, None)));
    assert_eq!(block.file.path(), &key);
    assert!(block.error.is_some() && block.patch.is_none());
    fs::write(path, [0, 1, 2]).unwrap();
    assert!(block.install(&key, read_block(&request, &key, None)));
    assert!(block.error.is_none() && block.file.is_binary());
}
