//! Saves against real files in a temporary worktree: what a failed write leaves behind.
use super::{Document, DocumentError, SaveBlocker, Snapshot};
use crate::state::documents::{Documents, OpenFile};
use crate::state::store::Store;
use gpui::{
    AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, VisualTestContext,
    Window, div,
};
use gpui_kit as gpui;
use hive_core::{host::Host, projects::Target};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Worktree(PathBuf);
impl Worktree {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hive-document-save-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        // The temporary folder is reached through a link on macOS; documents name real paths.
        Self(fs::canonicalize(root).unwrap())
    }
}
impl Drop for Worktree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Harness {
    _documents: Entity<Documents>,
}
impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// `name` on disk as `disk`, open in an editor that read `read` and now holds `buffer`.
fn open<'a>(
    cx: &'a mut TestAppContext,
    worktree: &Worktree,
    name: &str,
    [disk, read, buffer]: [&str; 3],
) -> (Entity<Document>, &'a mut VisualTestContext) {
    cx.update(gpui::init);
    fs::write(worktree.0.join(name), disk).unwrap();
    let request = OpenFile {
        target: Target {
            host: Host::Local,
            project: worktree.0.clone(),
            path: worktree.0.clone(),
        },
        path: name.into(),
        position: None,
        preview: false,
        placement: Default::default(),
    };
    let base = Snapshot::parse(Path::new(name), read.as_bytes()).unwrap();
    let mut document = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let store = cx.new(Store::with_held_recovery);
        let documents = cx.new(|cx| Documents::new(store, Default::default(), cx));
        document = Some(documents.update(cx, |documents, cx| {
            documents.open_edited_for_test(request, base, buffer, window, cx)
        }));
        Harness {
            _documents: documents,
        }
    });
    (document.unwrap(), cx)
}

fn save(document: &Entity<Document>, cx: &mut VisualTestContext) -> bool {
    let saved = cx.update(|window, cx| document.update(cx, |d, cx| d.save(None, window, cx)));
    cx.run_until_parked();
    saved.try_recv() == Ok(true)
}

#[gpui::test]
fn a_save_writes_the_buffer_and_leaves_nothing_in_its_way(cx: &mut TestAppContext) {
    let worktree = Worktree::new();
    let (document, cx) = open(cx, &worktree, "a.txt", ["old\n", "old\n", "mine\n"]);

    assert!(save(&document, cx));

    assert_eq!(
        fs::read_to_string(worktree.0.join("a.txt")).unwrap(),
        "mine\n"
    );
    cx.read(|cx| {
        let document = document.read(cx);
        assert!(!document.dirty);
        assert_eq!(document.save_blocker(), None);
    });
}

#[gpui::test]
fn a_file_changed_on_disk_fails_the_save_and_becomes_a_conflict(cx: &mut TestAppContext) {
    let worktree = Worktree::new();
    // An agent rewrote the file after the editor read it.
    let (document, cx) = open(cx, &worktree, "a.txt", ["theirs\n", "old\n", "mine\n"]);

    assert!(!save(&document, cx));

    assert_eq!(
        fs::read_to_string(worktree.0.join("a.txt")).unwrap(),
        "theirs\n",
        "the other writer's version is not replaced"
    );
    cx.read(|cx| {
        let document = document.read(cx);
        assert!(document.dirty, "the buffer is kept");
        // The read that follows a failed write has found what changed.
        assert_eq!(document.save_blocker(), Some(SaveBlocker::Conflict));
    });
    // Asking again cannot help, and says so at once.
    assert!(!save(&document, cx));
    assert_eq!(
        fs::read_to_string(worktree.0.join("a.txt")).unwrap(),
        "theirs\n"
    );
}

#[gpui::test]
fn a_file_deleted_on_disk_fails_the_save_with_the_reason(cx: &mut TestAppContext) {
    let worktree = Worktree::new();
    let (document, cx) = open(cx, &worktree, "a.txt", ["old\n", "old\n", "mine\n"]);
    fs::remove_file(worktree.0.join("a.txt")).unwrap();

    assert!(!save(&document, cx));

    assert!(
        !worktree.0.join("a.txt").exists(),
        "a deleted file is not created again"
    );
    cx.read(|cx| {
        let document = document.read(cx);
        assert!(document.dirty);
        assert!(matches!(document.error, Some(DocumentError::Read(_))));
        let Some(SaveBlocker::Failed(reason)) = document.save_blocker() else {
            panic!("expected the failure to be reported");
        };
        assert!(reason.contains("a.txt"), "{reason}");
    });
}

#[gpui::test]
fn an_upgrade_rewrites_only_the_requirement_its_row_offers(cx: &mut TestAppContext) {
    use hive_core::package_lens::{Change, Channel, Lens, Manifest, Upgrade, Version, Versions};
    let worktree = Worktree::new();
    let source = r#"{"name":"тест","dependencies":{"a":"^1.0.0","b":"1"}}"#;
    let (doc, cx) = open(cx, &worktree, "package.json", [source, source, source]);
    let entries = Manifest::parse(source).unwrap().entries;
    let (a, b) = (&entries[0], &entries[1]);
    // The entry where `a` stood before a byte was typed in front of it.
    let mut moved = a.clone();
    moved.pair = moved.pair.start + 1..moved.pair.end + 1;
    moved.value = moved.value.start + 1..moved.value.end + 1;
    cx.update(|window, cx| {
        doc.update(cx, |d, cx| {
            d.packages.answer(Lens {
                dependency: a.clone(),
                resolved: None,
                versions: Versions::Found(vec![Upgrade {
                    channel: Channel::Latest,
                    version: Version::parse("2.0.0").unwrap(),
                    change: Some(Change::Major),
                    spec: "^2.0.0".into(),
                }]),
                advisories: Vec::new(),
                more_advisories: false,
            });
            d.apply_upgrade(&moved, "^2.0.0", window, cx);
            d.apply_upgrade(a, "^9.0.0", window, cx);
            d.apply_upgrade(b, "2", window, cx);
            assert_eq!(d.text(cx), source, "no row offers these");
            d.apply_upgrade(a, "^2.0.0", window, cx);
            assert_eq!(d.text(cx), source.replace("^1.0.0", "^2.0.0"));
        })
    });
    assert_eq!(
        fs::read_to_string(worktree.0.join("package.json")).unwrap(),
        source,
        "an upgrade edits the buffer; saving stays explicit"
    );
}

#[gpui::test]
fn failed_package_save_never_queues_an_install(cx: &mut TestAppContext) {
    let worktree = Worktree::new();
    let (doc, cx) = open(cx, &worktree, "package.json", ["{}", "{ }", "{"]);
    cx.update(|_, cx| {
        let store = doc.read(cx).store.clone();
        let mut project = hive_core::state::ProjectRef::new(worktree.0.clone(), Host::Local);
        project.install_on_save = true;
        store.update(cx, |s, _| s.state.projects.push(project));
    });
    assert!(!save(&doc, cx));
    cx.read(|cx| assert!(doc.read(cx).packages.install_status.is_none()));
    assert_eq!(
        fs::read_to_string(worktree.0.join("package.json")).unwrap(),
        "{}"
    );
}

#[gpui::test]
fn saving_invalid_json_reports_install_skipped_and_preserves_the_written_buffer(
    cx: &mut TestAppContext,
) {
    let worktree = Worktree::new();
    let (doc, cx) = open(cx, &worktree, "package.json", ["{}", "{}", "{"]);
    cx.update(|_, cx| {
        let store = doc.read(cx).store.clone();
        let mut project = hive_core::state::ProjectRef::new(worktree.0.clone(), Host::Local);
        project.install_on_save = true;
        store.update(cx, |s, _| s.state.projects.push(project));
    });
    assert!(save(&doc, cx));
    cx.read(|cx| {
        assert!(
            doc.read(cx)
                .packages
                .install_status
                .as_deref()
                .unwrap()
                .starts_with("Install skipped")
        )
    });
    assert_eq!(
        fs::read_to_string(worktree.0.join("package.json")).unwrap(),
        "{"
    );
}
