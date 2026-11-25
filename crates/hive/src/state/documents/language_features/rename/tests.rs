use super::{
    Change, Disk, Documents, OpenFile, Path, Target, apply_reviewed, position, relative_uri,
    verify_disks,
};
use crate::state::store::Store;
use gpui::{AppContext as _, Context, IntoElement, Render, TestAppContext, Window, div};
use gpui_kit as gpui;
use hive_core::{command::Cancellation, host::Host};
use std::fs;

#[test]
fn rename_rejects_git_parent_and_sibling_worktrees() {
    let root = Path::new("/work");
    for bad in [
        "/work/.git/config",
        "/work/.GiT/config",
        "/work/../a",
        "/work2/file",
        "/work",
    ] {
        assert!(relative_uri(root, &position::uri(Path::new(bad)).unwrap()).is_none());
    }
    assert_eq!(
        relative_uri(root, &position::uri(Path::new("/work/src/a.tsx")).unwrap()).unwrap(),
        Path::new("src/a.tsx")
    );
}

#[test]
fn rename_only_uses_unsaved_text_known_to_the_requesting_server() {
    let disk = Disk::parse(Path::new("a.ts"), b"disk").unwrap();
    let mut source = super::OpenVersion {
        text: "unsaved".into(),
        disk: disk.clone(),
        version: None,
    };
    assert!(super::rename_source(&disk, Some(&source), None).is_err());
    source.text = "disk".into();
    assert!(super::rename_source(&disk, Some(&source), Some(1)).is_err());
    assert_eq!(
        super::rename_source(&disk, Some(&source), None).unwrap(),
        "disk"
    );
    source.text = "unsaved".into();
    source.version = Some(1);
    assert_eq!(
        super::rename_source(&disk, Some(&source), Some(1)).unwrap(),
        "unsaved"
    );
}

struct Harness;
impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[gpui::test]
fn a_changed_second_buffer_rejects_the_entire_preview_and_preserves_unsaved_text(
    cx: &mut TestAppContext,
) {
    cx.update(gpui::init);
    let root = std::env::temp_dir().join(format!("hive-reviewed-rename-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let target = Target {
        host: Host::Local,
        project: root.clone(),
        path: root.clone(),
    };
    let (_, cx) = cx.add_window_view(|_, _| Harness);
    cx.update(|window, cx| {
        let store = cx.new(Store::with_held_recovery);
        let registry = cx.new(|cx| Documents::new(store, Default::default(), cx));
        let mut docs = Vec::new();
        let mut changes = Vec::new();
        for (name, buffer) in [("a.txt", "old mine"), ("b.txt", "concurrent")] {
            let path = std::path::PathBuf::from(name);
            fs::write(root.join(&path), "old").unwrap();
            let disk = Disk::parse(&path, b"old").unwrap();
            let request = OpenFile {
                target: target.clone(),
                path: path.clone(),
                position: None,
                preview: false,
                placement: Default::default(),
            };
            docs.push(registry.update(cx, |reg, cx| {
                reg.open_edited_for_test(request, disk.clone(), buffer, window, cx)
            }));
            changes.push(Change {
                path,
                before: if name == "a.txt" { "old mine" } else { "old" }.into(),
                after: if name == "a.txt" { "new mine" } else { "new" }.into(),
                disk,
                edits: Vec::new(),
            });
        }
        assert!(verify_disks(&target, &changes, &Cancellation::default()).is_ok());
        assert!(
            apply_reviewed(&target, Path::new("a.txt"), &registry, &changes, window, cx).is_err()
        );
        assert_eq!(docs[0].read(cx).text(cx), "old mine");
        assert_eq!(docs[1].read(cx).text(cx), "concurrent");
        docs[1]
            .read(cx)
            .editor
            .clone()
            .update(cx, |e, cx| e.set_value("old", window, cx));
        apply_reviewed(&target, Path::new("a.txt"), &registry, &changes, window, cx).unwrap();
        assert_eq!(docs[0].read(cx).text(cx), "new mine");
        assert_eq!(docs[1].read(cx).text(cx), "new");
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "old");
        fs::write(root.join("b.txt"), "agent edit").unwrap();
        assert!(verify_disks(&target, &changes, &Cancellation::default()).is_err());
        assert_eq!(
            fs::read_to_string(root.join("b.txt")).unwrap(),
            "agent edit"
        );
    });
    fs::remove_dir_all(root).unwrap();
}
