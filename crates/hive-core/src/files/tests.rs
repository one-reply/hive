use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hive-files-{}-{}",
            std::process::id(),
            NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn read(&self, name: &str) -> Result<Snapshot, FileError> {
        read(&Host::Local, &self.0, Path::new(name))
    }
    fn save(&self, name: &str, base: &Snapshot, text: &str) -> Result<Snapshot, FileError> {
        save(&Host::Local, &self.0, Path::new(name), base, text)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn preserves_bom_crlf_permissions_and_missing_final_newline() {
    let f = Fixture::new();
    let path = f.0.join("code.rs");
    fs::write(&path, "\u{feff}привіт\r\nworld").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o751)).unwrap();
    let base = f.read("code.rs").unwrap();
    assert_eq!(base.text(), "привіт\nworld");
    assert!(base.matches("привіт\nworld"));
    let saved = f.save("code.rs", &base, "привіт\n世界").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "\u{feff}привіт\r\n世界");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o751
    );
    assert!(saved.matches("привіт\n世界"));
}

#[test]
fn stale_save_and_missing_file_never_overwrite_or_recreate() {
    let f = Fixture::new();
    let path = f.0.join("a");
    fs::write(&path, "base").unwrap();
    let base = f.read("a").unwrap();
    fs::write(&path, "agent").unwrap();
    assert!(matches!(
        f.save("a", &base, "mine"),
        Err(FileError::Conflict(_))
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), "agent");
    fs::remove_file(&path).unwrap();
    assert!(f.save("a", &base, "mine").is_err());
    assert!(!path.exists());
}

#[test]
fn rejects_unsafe_paths_symlinks_and_unsupported_content() {
    let f = Fixture::new();
    fs::write(f.0.join("a"), "ok").unwrap();
    symlink("a", f.0.join("link")).unwrap();
    fs::create_dir(f.0.join("dir")).unwrap();
    symlink("dir", f.0.join("linked-dir")).unwrap();
    fs::write(f.0.join("dir/file"), "ok").unwrap();
    for name in [
        "../a",
        "/tmp/a",
        ".git/config",
        "link",
        "linked-dir/file",
        "dir",
    ] {
        assert!(f.read(name).is_err(), "{name}");
    }
    for bytes in [vec![0], vec![0xff], vec![b'x'; MAX_FILE_BYTES + 1]] {
        fs::write(f.0.join("bad"), bytes).unwrap();
        assert!(f.read("bad").is_err());
    }
    fs::write(f.0.join("empty"), "").unwrap();
    assert_eq!(f.read("empty").unwrap().text(), "");
}

#[test]
fn failed_final_check_preserves_original_and_removes_temporary_file() {
    let f = Fixture::new();
    let path = f.0.join("a");
    fs::write(&path, "original").unwrap();
    assert!(
        atomic_write(&path, b"replacement", None, || Err(FileError::Conflict(
            path.clone()
        )))
        .is_err()
    );
    assert_eq!(fs::read_to_string(path).unwrap(), "original");
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
}

#[test]
fn recovery_roundtrip_private_permissions_corruption_and_removal() {
    let f = Fixture::new();
    fs::write(f.0.join("a"), "base").unwrap();
    let dir = f.0.join("recovery");
    let draft = recovery::Draft {
        host: Host::Local,
        project: f.0.clone(),
        root: f.0.clone(),
        path: "a".into(),
        base: f.read("a").unwrap(),
        text: "draft".into(),
    };
    recovery::write(&dir, &draft.key(), &draft).unwrap();
    let (records, errors) = recovery::load(&dir);
    assert!(errors.is_empty());
    assert_eq!(records[0].draft.text, "draft");
    assert_eq!(
        fs::metadata(dir.join(format!("{}.json", draft.key())))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    fs::write(dir.join("bad.json"), "{").unwrap();
    let (records, errors) = recovery::load(&dir);
    assert_eq!(records.len(), 1);
    assert_eq!(errors.len(), 1);
    recovery::remove(&dir, &draft.key()).unwrap();
    assert!(recovery::load(&dir).0.is_empty());
    assert!(dir.join("bad.json").exists());
}

#[test]
fn directory_listing_keeps_dotfiles_but_not_git_or_symlink_traversal() {
    let f = Fixture::new();
    fs::create_dir(f.0.join(".git")).unwrap();
    fs::create_dir(f.0.join("src")).unwrap();
    fs::write(f.0.join(".env"), "x").unwrap();
    symlink("src", f.0.join("link")).unwrap();
    let entries = list_directory(&Host::Local, &f.0, Path::new("")).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].path, PathBuf::from("src"));
    assert!(
        entries
            .iter()
            .any(|e| e.path == Path::new("link") && e.unsupported)
    );
}

#[test]
fn save_completion_and_overwrite_use_the_captured_version() {
    let f = Fixture::new();
    fs::write(f.0.join("a"), "base\n").unwrap();
    let saved = f.save("a", &f.read("a").unwrap(), "saved\n").unwrap();
    assert!(!saved.matches("saved\nmore typing"));
    assert!(saved.matches("saved\n"));
    fs::write(f.0.join("a"), "shown disk").unwrap();
    let shown = f.read("a").unwrap();
    fs::write(f.0.join("a"), "newer disk").unwrap();
    assert!(matches!(
        f.save("a", &shown, "overwrite"),
        Err(FileError::Conflict(_))
    ));
    assert_eq!(f.read("a").unwrap().text(), "newer disk");
}

#[test]
fn exact_limit_empty_save_readonly_and_mixed_line_endings() {
    let f = Fixture::new();
    for text in ["", "a\nb", "a\r\nb\nc", "\u{feff}"] {
        fs::write(f.0.join("a"), text).unwrap();
        let base = f.read("a").unwrap();
        f.save("a", &base, &base.text()).unwrap();
        assert_eq!(fs::read_to_string(f.0.join("a")).unwrap(), text);
    }
    fs::write(f.0.join("a"), "x".repeat(MAX_FILE_BYTES)).unwrap();
    let base = f.read("a").unwrap();
    assert!(f.save("a", &base, &"x".repeat(MAX_FILE_BYTES + 1)).is_err());
    fs::set_permissions(f.0.join("a"), fs::Permissions::from_mode(0o444)).unwrap();
    assert!(f.save("a", &base, "blocked").is_err());
    assert_eq!(f.read("a").unwrap(), base);
}

#[test]
fn git_index_excludes_ignored_but_tree_keeps_them() {
    let f = Fixture::new();
    let git = |args: &[&str]| {
        let output = Host::Local
            .command("git", args, Some(&f.0))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q"]);
    fs::write(f.0.join(".gitignore"), "ignored\n").unwrap();
    fs::write(f.0.join("ignored"), "ignored").unwrap();
    fs::write(f.0.join("tracked"), "tracked").unwrap();
    fs::write(f.0.join("untracked\nUnicode世界"), "new").unwrap();
    git(&["add", "tracked"]);
    let index = list_files(&Host::Local, &f.0).unwrap();
    assert!(!index.truncated);
    assert!(index.paths.contains(&"tracked".into()));
    assert!(index.paths.contains(&"untracked\nUnicode世界".into()));
    assert!(!index.paths.contains(&"ignored".into()));
    assert!(
        list_directory(&Host::Local, &f.0, Path::new(""))
            .unwrap()
            .iter()
            .any(|entry| entry.path == Path::new("ignored"))
    );
}

#[test]
fn same_relative_path_in_other_worktree_has_independent_recovery() {
    let first = Fixture::new();
    let second = Fixture::new();
    assert_ne!(
        recovery::key(&Host::Local, &first.0, Path::new("a")),
        recovery::key(&Host::Local, &second.0, Path::new("a"))
    );
}

#[test]
fn recovery_preserves_byte_paths_and_distinguishes_hosts_with_legacy_defaults() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let fixture = Fixture::new();
    let dir = fixture.0.join("recovery");
    let path = PathBuf::from(OsString::from_vec(b"notes-\xff.txt".to_vec()));
    let base = Snapshot::parse(&path, b"original\r\n").unwrap();
    for host in [
        Host::Local,
        Host::Ssh {
            target: "home".into(),
        },
        Host::Ssh {
            target: "other".into(),
        },
    ] {
        let draft = recovery::Draft {
            host,
            project: "/remote".into(),
            root: "/remote".into(),
            path: path.clone(),
            base: base.clone(),
            text: "changed".into(),
        };
        recovery::write(&dir, &draft.key(), &draft).unwrap();
    }
    let (records, errors) = recovery::load(&dir);
    assert!(errors.is_empty());
    assert_eq!(records.len(), 3);
    assert!(records.iter().all(|record| record.draft.path == path));
    let mut old = serde_json::to_value(&records[0].draft).unwrap();
    old.as_object_mut().unwrap().remove("host");
    old["path"] = serde_json::json!("notes.txt");
    old["future"] = serde_json::json!(true);
    let old: recovery::Draft = serde_json::from_value(old).unwrap();
    assert_eq!(old.host, Host::Local);
    assert_eq!(old.path, Path::new("notes.txt"));
}
