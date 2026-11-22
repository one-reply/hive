use super::*;
use crate::files::mutations::tests::Fixture;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

/// A worktree and, beside it, a folder standing in for Finder's selection.
struct Paste {
    fixture: Fixture,
}
impl Paste {
    fn new() -> Self {
        let fixture = Fixture::new();
        fs::create_dir(fixture.0.join("worktree")).unwrap();
        fs::create_dir(fixture.0.join("finder")).unwrap();
        Self { fixture }
    }
    fn root(&self) -> PathBuf {
        self.fixture.0.join("worktree")
    }
    fn source(&self, name: &str) -> PathBuf {
        self.fixture.0.join("finder").join(name)
    }
    fn paste(&self, directory: &str, sources: &[PathBuf]) -> Result<Vec<PathBuf>, FileError> {
        paste_entries(
            &Host::Local,
            &self.root(),
            Path::new(directory),
            sources,
            &Cancellation::default(),
        )
    }
    fn names(&self, directory: &str) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(self.root().join(directory))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

#[test]
fn paste_name_follows_finder_copy_suffixes() {
    assert_eq!(paste_name("report.pdf", false, 0), "report.pdf");
    assert_eq!(paste_name("report.pdf", false, 1), "report copy.pdf");
    assert_eq!(paste_name("report.pdf", false, 2), "report copy 2.pdf");
    assert_eq!(
        paste_name("archive.tar.gz", false, 1),
        "archive.tar copy.gz"
    );
    assert_eq!(paste_name("Makefile", false, 1), "Makefile copy");
    assert_eq!(paste_name(".env", false, 1), ".env copy");
    assert_eq!(paste_name(".env.local", false, 3), ".env copy 3.local");
    assert_eq!(paste_name("notes.", false, 1), "notes. copy");
    assert_eq!(paste_name("v1.2", true, 1), "v1.2 copy");
    assert_eq!(paste_name("нотатки.md", false, 1), "нотатки copy.md");
}

#[test]
fn pastes_files_and_folders_recursively_without_following_links() {
    let paste = Paste::new();
    fs::write(paste.source("a b.rs"), "fn main() {}").unwrap();
    fs::set_permissions(paste.source("a b.rs"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir_all(paste.source("папка/nested")).unwrap();
    fs::write(paste.source("папка/nested/deep.txt"), "deep").unwrap();
    fs::write(paste.fixture.0.join("outside"), "secret").unwrap();
    symlink("../../outside", paste.source("папка/link")).unwrap();
    fs::create_dir(paste.root().join("src")).unwrap();

    let pasted = paste
        .paste("src", &[paste.source("a b.rs"), paste.source("папка")])
        .unwrap();

    assert_eq!(
        pasted,
        [PathBuf::from("src/a b.rs"), PathBuf::from("src/папка")]
    );
    let src = paste.root().join("src");
    assert_eq!(
        fs::read_to_string(src.join("a b.rs")).unwrap(),
        "fn main() {}"
    );
    let mode = fs::metadata(src.join("a b.rs"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o100, 0o100, "the executable bit is kept");
    assert_eq!(
        fs::read_to_string(src.join("папка/nested/deep.txt")).unwrap(),
        "deep"
    );
    let link = src.join("папка/link");
    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(fs::read_link(&link).unwrap(), Path::new("../../outside"));
    assert_eq!(
        fs::read_to_string(paste.source("a b.rs")).unwrap(),
        "fn main() {}",
        "the source stays where it was"
    );
}

#[test]
fn a_pasted_link_stays_a_link_and_its_target_is_not_copied() {
    let paste = Paste::new();
    fs::create_dir(paste.source("secrets")).unwrap();
    fs::write(paste.source("secrets/key"), "private").unwrap();
    symlink("secrets", paste.source("shortcut")).unwrap();
    symlink("/nonexistent/target", paste.source("dangling")).unwrap();

    let pasted = paste
        .paste("", &[paste.source("shortcut"), paste.source("dangling")])
        .unwrap();

    assert_eq!(
        pasted,
        [PathBuf::from("shortcut"), PathBuf::from("dangling")]
    );
    for (name, target) in [("shortcut", "secrets"), ("dangling", "/nonexistent/target")] {
        let link = paste.root().join(name);
        assert!(fs::symlink_metadata(&link).unwrap().is_symlink(), "{name}");
        assert_eq!(fs::read_link(&link).unwrap(), Path::new(target));
    }
    assert_eq!(paste.names(""), ["dangling", "shortcut"]);
}

#[test]
fn taken_names_get_a_copy_suffix_and_existing_entries_are_untouched() {
    let paste = Paste::new();
    fs::write(paste.root().join("notes.md"), "mine").unwrap();
    fs::write(paste.root().join("notes copy.md"), "mine too").unwrap();
    fs::write(paste.source("notes.md"), "theirs").unwrap();
    fs::create_dir(paste.fixture.0.join("other")).unwrap();
    fs::write(paste.fixture.0.join("other/notes.md"), "second").unwrap();

    let pasted = paste
        .paste(
            "",
            &[
                paste.source("notes.md"),
                paste.fixture.0.join("other/notes.md"),
            ],
        )
        .unwrap();

    assert_eq!(
        pasted,
        [
            PathBuf::from("notes copy 2.md"),
            PathBuf::from("notes copy 3.md")
        ]
    );
    let root = paste.root();
    assert_eq!(fs::read_to_string(root.join("notes.md")).unwrap(), "mine");
    assert_eq!(
        fs::read_to_string(root.join("notes copy.md")).unwrap(),
        "mine too"
    );
    assert_eq!(
        fs::read_to_string(root.join("notes copy 2.md")).unwrap(),
        "theirs"
    );
    assert_eq!(
        fs::read_to_string(root.join("notes copy 3.md")).unwrap(),
        "second"
    );
}

#[test]
fn a_folder_whose_name_is_taken_is_pasted_beside_it_not_merged_into_it() {
    let paste = Paste::new();
    fs::create_dir(paste.root().join("assets")).unwrap();
    fs::write(paste.root().join("assets/logo.svg"), "mine").unwrap();
    fs::create_dir(paste.source("assets")).unwrap();
    fs::write(paste.source("assets/logo.svg"), "theirs").unwrap();

    let pasted = paste.paste("", &[paste.source("assets")]).unwrap();

    assert_eq!(pasted, [PathBuf::from("assets copy")]);
    assert_eq!(paste.names("assets"), ["logo.svg"]);
    assert_eq!(
        fs::read_to_string(paste.root().join("assets/logo.svg")).unwrap(),
        "mine"
    );
    assert_eq!(
        fs::read_to_string(paste.root().join("assets copy/logo.svg")).unwrap(),
        "theirs"
    );
}

#[test]
fn a_name_the_volume_treats_as_taken_never_replaces_the_existing_entry() {
    // One entry on APFS, two on a volume that compares bytes: another case, and the same
    // letter composed and decomposed. Either way both contents must survive.
    for (existing, pasted_as) in [
        ("README.md", "readme.md"),
        ("cafe\u{301}.txt", "caf\u{e9}.txt"),
    ] {
        let paste = Paste::new();
        fs::write(paste.root().join(existing), "mine").unwrap();
        fs::write(paste.source(pasted_as), "theirs").unwrap();

        let pasted = paste.paste("", &[paste.source(pasted_as)]).unwrap();

        assert_eq!(
            fs::read_to_string(paste.root().join(existing)).unwrap(),
            "mine",
            "{existing}"
        );
        assert_eq!(
            fs::read_to_string(paste.root().join(&pasted[0])).unwrap(),
            "theirs",
            "{existing}"
        );
        assert_eq!(paste.names("").len(), 2, "{existing}");
    }
}

#[test]
fn pasting_a_worktree_file_into_its_own_folder_duplicates_it() {
    let paste = Paste::new();
    fs::create_dir(paste.root().join("src")).unwrap();
    fs::write(paste.root().join("src/lib.rs"), "lib").unwrap();

    let pasted = paste
        .paste("src", &[paste.root().join("src/lib.rs")])
        .unwrap();

    assert_eq!(pasted, [PathBuf::from("src/lib copy.rs")]);
    assert_eq!(
        fs::read_to_string(paste.root().join("src/lib copy.rs")).unwrap(),
        "lib"
    );
}

#[test]
fn a_folder_is_never_pasted_inside_itself() {
    let paste = Paste::new();
    fs::create_dir_all(paste.root().join("src/inner")).unwrap();
    // Finder may name the same folder through a linked parent; resolved paths are compared.
    symlink(paste.root(), paste.fixture.0.join("alias")).unwrap();

    for (directory, source) in [
        ("src", paste.root().join("src")),
        ("src/inner", paste.root().join("src")),
        ("src/inner", paste.fixture.0.join("alias/src")),
    ] {
        let error = paste.paste(directory, &[source]).unwrap_err();
        assert!(error.to_string().contains("inside itself"), "{error}");
    }
    assert_eq!(paste.names("src"), ["inner"]);
    assert!(paste.names("src/inner").is_empty());
}

#[test]
fn refused_sources_leave_nothing_behind() {
    let paste = Paste::new();
    fs::create_dir_all(paste.source("repo/.git")).unwrap();
    fs::write(paste.source("repo/README"), "readme").unwrap();
    fs::create_dir(paste.source(".git")).unwrap();
    fs::write(paste.source(".hive-write-1"), "temporary").unwrap();
    let socket = paste.source("socket");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();

    for source in [
        paste.source("repo"),
        paste.source(".git"),
        paste.source(".hive-write-1"),
        socket,
    ] {
        assert!(
            paste.paste("", std::slice::from_ref(&source)).is_err(),
            "{source:?}"
        );
    }
    assert!(paste.paste("", &[PathBuf::from("relative.txt")]).is_err());
    assert!(paste.paste("", &[PathBuf::from("/")]).is_err());
    assert!(paste.names("").is_empty());
}

#[test]
fn a_source_that_fails_after_others_were_pasted_says_how_many_are_in_place() {
    let paste = Paste::new();
    fs::write(paste.source("first.txt"), "first").unwrap();
    fs::write(paste.source("third.txt"), "third").unwrap();

    let error = paste
        .paste(
            "",
            &[
                paste.source("first.txt"),
                paste.source("gone.txt"),
                paste.source("third.txt"),
            ],
        )
        .unwrap_err();

    assert!(
        matches!(
            error,
            FileError::PartialPaste {
                pasted: 1,
                total: 3,
                ..
            }
        ),
        "{error}"
    );
    assert!(
        error.to_string().starts_with("Pasted 1 of 3 items"),
        "{error}"
    );
    // What was pasted stays; nothing after the failure is attempted.
    assert_eq!(paste.names(""), ["first.txt"]);
}

#[test]
fn a_folder_that_fails_halfway_is_removed_and_earlier_entries_are_reported() {
    let paste = Paste::new();
    fs::write(paste.source("first.txt"), "first").unwrap();
    fs::create_dir(paste.source("folder")).unwrap();
    fs::write(paste.source("folder/readable"), "ok").unwrap();
    fs::write(paste.source("folder/unreadable"), "no").unwrap();
    let unreadable = paste.source("folder/unreadable");
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
    // Root reads any file; the scenario needs a read that the system refuses.
    let refused = fs::File::open(&unreadable).is_err();

    let result = paste.paste("", &[paste.source("first.txt"), paste.source("folder")]);
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();

    if !refused {
        return;
    }
    let error = result.unwrap_err();
    assert!(
        matches!(
            error,
            FileError::PartialPaste {
                pasted: 1,
                total: 2,
                ..
            }
        ),
        "{error}"
    );
    assert_eq!(
        fs::read_to_string(paste.root().join("first.txt")).unwrap(),
        "first"
    );
    assert_eq!(
        paste.names(""),
        ["first.txt"],
        "the half-copied folder is gone"
    );
}

#[test]
fn a_missing_first_source_reports_its_own_error() {
    let paste = Paste::new();
    let error = paste.paste("", &[paste.source("gone.txt")]).unwrap_err();
    assert!(matches!(error, FileError::Io { .. }), "{error}");
}

#[test]
fn cancellation_stops_before_creating_anything() {
    let paste = Paste::new();
    fs::write(paste.source("a.txt"), "a").unwrap();
    let cancellation = Cancellation::default();
    cancellation.cancel();

    let result = paste_entries(
        &Host::Local,
        &paste.root(),
        Path::new(""),
        &[paste.source("a.txt")],
        &cancellation,
    );

    assert!(result.is_err());
    assert!(paste.names("").is_empty());
}

#[test]
fn destination_must_be_a_folder_inside_a_local_worktree() {
    let paste = Paste::new();
    fs::write(paste.source("a.txt"), "a").unwrap();
    fs::write(paste.root().join("file"), "").unwrap();
    fs::create_dir(paste.fixture.0.join("elsewhere")).unwrap();
    symlink(
        paste.fixture.0.join("elsewhere"),
        paste.root().join("linked"),
    )
    .unwrap();
    for directory in ["../finder", ".git", "missing", "file", "linked"] {
        assert!(
            paste.paste(directory, &[paste.source("a.txt")]).is_err(),
            "{directory}"
        );
    }
    assert!(
        fs::read_dir(paste.fixture.0.join("elsewhere"))
            .unwrap()
            .next()
            .is_none(),
        "a linked folder is not a way out of the worktree"
    );
    let remote = Host::Ssh {
        target: "home".into(),
    };
    let error = paste_entries(
        &remote,
        Path::new("/srv/repo"),
        Path::new(""),
        &[paste.source("a.txt")],
        &Cancellation::default(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("only in local worktrees"),
        "{error}"
    );
}
