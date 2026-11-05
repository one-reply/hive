use super::*;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hive-file-mutations-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn create_move_and_delete_preserve_content_and_never_replace() {
    let f = Fixture::new();
    create_directory(&Host::Local, &f.0, Path::new("папка")).unwrap();
    create_file(&Host::Local, &f.0, Path::new("папка/a b.rs")).unwrap();
    fs::write(f.0.join("папка/a b.rs"), "draft").unwrap();
    assert!(create_file(&Host::Local, &f.0, Path::new("папка/a b.rs")).is_err());
    move_entry(
        &Host::Local,
        &f.0,
        Path::new("папка/a b.rs"),
        Path::new("moved.rs"),
    )
    .unwrap();
    fs::write(f.0.join("exists"), "safe").unwrap();
    assert!(
        move_entry(
            &Host::Local,
            &f.0,
            Path::new("moved.rs"),
            Path::new("exists")
        )
        .is_err()
    );
    assert_eq!(fs::read_to_string(f.0.join("exists")).unwrap(), "safe");
    assert_eq!(fs::read_to_string(f.0.join("moved.rs")).unwrap(), "draft");
    delete_entry(&Host::Local, &f.0, Path::new("папка")).unwrap();
    delete_entry(&Host::Local, &f.0, Path::new("moved.rs")).unwrap();
}

#[test]
fn invalid_roots_symlinks_and_nested_repositories_are_protected() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("safe")).unwrap();
    symlink("safe", f.0.join("link")).unwrap();
    for path in ["", "../outside", ".git/config", "/absolute", "link/file"] {
        assert!(
            create_file(&Host::Local, &f.0, Path::new(path)).is_err(),
            "{path}"
        );
    }
    assert!(delete_entry(&Host::Local, &f.0, Path::new("")).is_err());
    assert!(delete_entry(&Host::Local, &f.0, Path::new("link")).is_err());
    fs::create_dir_all(f.0.join("nested/.git")).unwrap();
    fs::write(f.0.join("nested/keep"), "safe").unwrap();
    assert!(delete_entry(&Host::Local, &f.0, Path::new("nested")).is_err());
    assert!(f.0.join("nested/keep").exists());
    assert!(
        move_entry(
            &Host::Local,
            &f.0,
            Path::new("safe"),
            Path::new("safe/child")
        )
        .is_err()
    );
}

#[test]
fn recursive_delete_never_follows_symlinks() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("keep")).unwrap();
    fs::write(f.0.join("keep/a"), "safe").unwrap();
    fs::create_dir(f.0.join("remove")).unwrap();
    symlink("../keep", f.0.join("remove/link")).unwrap();
    delete_entry(&Host::Local, &f.0, Path::new("remove")).unwrap();
    assert_eq!(fs::read_to_string(f.0.join("keep/a")).unwrap(), "safe");
}

#[test]
fn case_only_rename_preserves_data() {
    let f = Fixture::new();
    fs::write(f.0.join("lower.rs"), "source").unwrap();
    move_entry(
        &Host::Local,
        &f.0,
        Path::new("lower.rs"),
        Path::new("LOWER.rs"),
    )
    .unwrap();
    assert_eq!(fs::read_to_string(f.0.join("LOWER.rs")).unwrap(), "source");
    assert!(
        fs::read_dir(&f.0)
            .unwrap()
            .any(|p| p.unwrap().file_name() == "LOWER.rs")
    );
}

#[test]
fn inline_names_do_not_become_paths() {
    for name in [
        "",
        ".",
        "..",
        "../escape",
        "a/b",
        ".GIT",
        "a\0b",
        ".hive-write-x",
    ] {
        assert!(entry_path(Path::new("src"), name).is_err(), "{name:?}");
    }
    assert_eq!(
        entry_path(Path::new("src"), "тест file.rs").unwrap(),
        Path::new("src/тест file.rs")
    );
}

#[test]
fn relocated_file_paths_have_no_trailing_separator() {
    let from = Path::new("src/old.rs");
    let to = Path::new("src/new.ts");
    assert_eq!(
        relocated_path(from, from, to).unwrap().as_os_str(),
        to.as_os_str()
    );
    assert_eq!(
        relocated_path(
            Path::new("src/deep/file.rs"),
            Path::new("src"),
            Path::new("lib")
        )
        .unwrap(),
        Path::new("lib/deep/file.rs")
    );
    assert_eq!(
        relocated_path(
            Path::new("src-other/file.rs"),
            Path::new("src"),
            Path::new("lib")
        ),
        None
    );
}

#[test]
fn inaccessible_parent_and_partial_delete_keep_surviving_data() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let locked = f.0.join("folder/locked");
    fs::create_dir_all(&locked).unwrap();
    fs::write(locked.join("keep"), "survivor").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    let create = create_file(&Host::Local, &f.0, Path::new("folder/locked/new"));
    let delete = delete_entry(&Host::Local, &f.0, Path::new("folder"));
    // Restore permissions before assertions so a failed check still cleans its fixture.
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(create.is_err());
    assert!(matches!(delete, Err(FileError::PartialDelete { .. })));
    assert_eq!(fs::read_to_string(locked.join("keep")).unwrap(), "survivor");
}

#[test]
fn batched_ignore_rules_preserve_spaces_and_unicode() {
    let f = Fixture::new();
    let git = |args: &[&str]| {
        assert!(
            Host::Local
                .command("git", args, Some(&f.0))
                .output()
                .unwrap()
                .status
                .success()
        );
    };
    git(&["init", "-q"]);
    fs::write(f.0.join(".gitignore"), "ignored/\n*.cache\n").unwrap();
    fs::create_dir(f.0.join("ignored")).unwrap();
    fs::write(f.0.join("ignored/з пробілом.rs"), "x").unwrap();
    fs::write(f.0.join("tracked.cache"), "x").unwrap();
    git(&["add", "-f", "tracked.cache"]);
    let paths = [
        "ignored",
        "ignored/з пробілом.rs",
        "tracked.cache",
        ".gitignore",
    ]
    .map(PathBuf::from);
    let ignored = super::super::decorations::ignored(&Host::Local, &f.0, &paths).unwrap();
    assert_eq!(
        ignored,
        [paths[0].clone(), paths[1].clone()].into_iter().collect()
    );
}
