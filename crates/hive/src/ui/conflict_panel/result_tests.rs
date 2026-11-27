//! Exercise the live editor callbacks, undo actions and the write/stage path together.
use super::{BasePanel as _, ConflictPanel, ConflictRequest, ConflictResolution, DockArea, Git};
use crate::state::{projects::Projects, store::Store};
use gpui_kit as gpui;
use gpui_kit::component::input::{Redo, Undo};
use gpui_kit::{AppContext as _, TestAppContext, VisualTestContext};
use hive_core::{
    git::testing,
    host::Host,
    projects::Target,
    state::{Access, StateError},
};
use std::{fs, path::PathBuf};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[gpui::test]
fn the_full_result_keeps_manual_edits_and_undoes_choices_even_when_the_text_is_identical(
    cx: &mut TestAppContext,
) {
    let fixture =
        Fixture(std::env::temp_dir().join(format!("hive-result-window-{}", std::process::id())));
    testing::init(&fixture.0);
    let path = fixture.0.join("file.md");
    let common = "common\r\n".repeat(12);
    let document = |a: &str, b: &str| format!("\u{feff}start\r\n{a}\r\n{common}{b}\r\nend");
    for side in ["base", "incoming", "current"] {
        match side {
            "incoming" => testing::run(&fixture.0, &["checkout", "-qb", "topic"]),
            "current" => testing::run(&fixture.0, &["checkout", "-q", "main"]),
            _ => String::new(),
        };
        fs::write(&path, document(side, side)).unwrap();
        testing::run(&fixture.0, &["add", "file.md"]);
        testing::run(&fixture.0, &["commit", "-qm", side]);
    }
    assert!(
        !testing::git(&fixture.0, &["merge", "topic"])
            .output()
            .unwrap()
            .status
            .success()
    );
    // Git's marker lines can have LF even for CRLF content. Use a uniform CRLF fixture
    // to exercise the snapshot's BOM and line-ending contract, including the last line.
    let markers = fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n")
        .replace('\n', "\r\n");
    fs::write(&path, &markers).unwrap();
    cx.update(gpui_kit::init);
    let store = cx.new(|cx| {
        Store::new(
            Err(StateError::Io {
                operation: "test",
                path: fixture.0.clone(),
                source: std::io::Error::other("read only"),
            }),
            Access::Held,
            cx,
        )
    });
    let projects = cx.new(|cx| Projects::new(&store, cx));
    let git = cx.new(|cx| Git::new(projects, cx));
    let request = ConflictRequest {
        target: Target {
            host: Host::Local,
            project: fixture.0.clone(),
            path: fixture.0.clone(),
        },
        path: "file.md".into(),
    };
    let (panel, cx) = cx.add_window_view(|window, cx| {
        let dock = cx.new(|cx| DockArea::new("test-result", None, window, cx));
        ConflictPanel::over(request, dock.downgrade(), git, window, cx)
    });
    cx.run_until_parked();
    let editor = panel.read_with(cx, |p, cx| {
        assert_eq!(p.resolutions(), [false, false]);
        let e = p.result.as_ref().unwrap().state.clone();
        assert_eq!(
            e.read(cx).text().to_string(),
            document("current", "current")
                .trim_start_matches('\u{feff}')
                .replace("\r\n", "\n")
        );
        e
    });
    let replace = |range: std::ops::Range<usize>, text: &str, cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            editor.update(cx, |e, cx| {
                e.set_selected_range(range, cx);
                e.replace(text.to_owned(), window, cx);
            })
        });
        cx.run_until_parked();
    };
    replace(0..5, "🙂", cx);
    assert_eq!(panel.read_with(cx, |p, _| p.resolutions()), [false, false]);
    cx.update(|window, cx| {
        panel.update(cx, |p, cx| {
            p.resolve_conflict(0, Some(ConflictResolution::Current), window, cx)
        })
    });
    cx.dispatch_action(Undo);
    assert_eq!(panel.read_with(cx, |p, _| p.resolutions()), [false, false]);
    cx.dispatch_action(Redo);
    assert_eq!(panel.read_with(cx, |p, _| p.resolutions()), [true, false]);

    // A bulk action changes only pending blocks, and one undo restores the whole action.
    cx.update(|window, cx| {
        panel.update(cx, |p, cx| {
            p.accept_all(ConflictResolution::Incoming, window, cx)
        })
    });
    assert_eq!(panel.read_with(cx, |p, _| p.resolutions()), [true, true]);
    assert!(editor.read_with(cx, |e, _| e.text().to_string().ends_with("incoming\nend")));
    cx.dispatch_action(Undo);
    assert_eq!(panel.read_with(cx, |p, _| p.resolutions()), [true, false]);

    let selection = editor.read_with(cx, |e, _| e.selected_range());
    cx.update(|window, cx| {
        panel.update(cx, |p, cx| {
            p.set_active(false, window, cx);
            p.set_active(true, window, cx);
        })
    });
    cx.run_until_parked();
    panel.read_with(cx, |p, cx| {
        assert_eq!(p.result.as_ref().unwrap().state, editor);
        assert_eq!(editor.read(cx).selected_range(), selection);
        assert_eq!(p.resolutions(), [true, false]);
    });

    let range = panel.read_with(cx, |p, _| {
        let blocks = &p.result.as_ref().unwrap().blocks;
        blocks[0].range.start..blocks[1].range.end
    });
    replace(range, "combined\n", cx);
    assert_eq!(panel.read_with(cx, |p, _| p.resolutions()), [true, true]);
    cx.dispatch_action(Undo);
    assert_eq!(panel.read_with(cx, |p, _| p.resolutions()), [true, false]);
    cx.dispatch_action(Redo);
    assert_eq!(
        editor.read_with(cx, |e, _| e.text().to_string()),
        "🙂\ncombined\nend"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), markers);
    cx.update(|window, cx| {
        panel.update(cx, |p, cx| {
            p.navigate(true, window, cx);
            p.toggle_collapse_unchanged(cx);
            p.resolve_conflict(1, Some(ConflictResolution::Incoming), window, cx);
            p.mark_resolved(cx);
        })
    });
    cx.run_until_parked();
    let exact = "\u{feff}🙂\r\ncombined\r\nincoming\r\nend";
    assert_eq!(fs::read_to_string(&path).unwrap(), exact);
    assert_eq!(
        testing::git(&fixture.0, &["show", ":file.md"])
            .output()
            .unwrap()
            .stdout,
        exact.as_bytes()
    );
}
