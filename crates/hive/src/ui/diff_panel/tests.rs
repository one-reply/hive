use super::DiffPanel;
use crate::state::{
    git::{DiffRequest, Git},
    projects::Projects,
    store::Store,
};
use gpui_kit as gpui;
use gpui_kit::component::diff::{DiffFile, DiffLineRange, DiffSide, DiffState};
use gpui_kit::component::dock::DockArea;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, ScrollDelta,
    ScrollEvent, Styled as _, TestAppContext, VisualTestContext, Window, div, point, px,
};
use hive_core::{
    git::{diff::DiffSource, testing},
    host::Host,
    projects::Target,
    state::{Access, StateError},
};
use std::{fs, path::PathBuf, time::Duration};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

struct WindowHost(Entity<DiffPanel>);
impl Render for WindowHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(650.)).h(px(280.)).child(self.0.clone())
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
}

#[gpui::test]
fn loading_a_neighbor_preserves_a_code_selection_until_its_own_patch_changes(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    let patch = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n";
    let file = DiffFile::parse(patch).unwrap().remove(0);
    let display = cx.new(|cx| DiffState::new([file.clone()], cx));
    display.update(cx, |d, cx| {
        d.set_selected_lines(Some(DiffLineRange::new("a", DiffSide::Modified, 1, 1)), cx);
        d.set_review_files(vec![file, DiffFile::notice("b", "Loading changes…")], cx);
        assert_eq!(d.selected_text(cx), "new\n");
        d.previous_change(cx);
        assert_eq!(
            d.file_at_row(d.list().logical_scroll_top().item_ix)
                .unwrap()
                .path()
                .as_str(),
            "b"
        );
        d.set_review_files(
            DiffFile::parse(&patch.replace("+new", "+changed")).unwrap(),
            cx,
        );
        assert!(d.selected_lines().is_none());
    });
}

#[gpui::test]
fn the_shared_view_loads_visible_files_and_a_manual_scroll_cancels_the_sidebar_target(
    cx: &mut TestAppContext,
) {
    let fixture =
        Fixture(std::env::temp_dir().join(format!("hive-review-window-{}", std::process::id())));
    testing::init(&fixture.0);
    testing::run(&fixture.0, &["commit", "-qm", "base", "--allow-empty"]);
    for i in 0..24 {
        fs::write(fixture.0.join(format!("file-{i:02}")), "code\n".repeat(28)).unwrap();
    }
    let request = |i| DiffRequest {
        target: Target {
            host: Host::Local,
            project: fixture.0.clone(),
            path: fixture.0.clone(),
        },
        path: format!("file-{i:02}"),
        old_path: None,
        source: DiffSource::Untracked,
        whole_file_only: false,
    };
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
    let (host, cx) = cx.add_window_view(|window, cx| {
        let dock = cx.new(|cx| DockArea::new("test-review", None, window, cx));
        WindowHost(
            cx.new(|cx| DiffPanel::over(request(0), dock.downgrade(), git, store, window, cx)),
        )
    });
    let panel = host.read_with(cx, |w, _| w.0.clone());
    let review = panel.read_with(cx, |p, _| p.review.clone());
    review.update(cx, |r, cx| r.set_active(true, cx));
    draw(cx);
    let last = request(23).block_key();
    review.read_with(cx, |r, _| {
        assert_eq!(r.order.len(), 24);
        assert!(r.blocks[&request(0).block_key()].patch.is_some());
        assert!(r.blocks[&last].patch.is_none());
    });
    panel.update(cx, |p, cx| p.reveal(request(23), cx));
    draw(cx);
    cx.executor().advance_clock(Duration::from_millis(300));
    draw(cx);
    panel.read_with(cx, |p, cx| {
        let d = p.display.read(cx);
        assert_eq!(
            d.file_at_row(d.list().logical_scroll_top().item_ix)
                .unwrap()
                .path(),
            &last
        );
    });
    panel.update(cx, |p, cx| p.reveal(request(0), cx));
    draw(cx);
    let bounds = panel.read_with(cx, |p, cx| p.display.read(cx).list().viewport_bounds());
    assert!(bounds.size.height > px(0.));
    cx.simulate_event(ScrollEvent {
        position: bounds.origin + point(px(40.), px(40.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(80.))),
        ..Default::default()
    });
    draw(cx);
    panel.read_with(cx, |p, cx| {
        assert!(p.reveal.is_none());
        let d = p.display.read(cx);
        let top = d
            .file_at_row(d.list().logical_scroll_top().item_ix)
            .unwrap();
        assert_eq!(p.review.read(cx).request.block_key(), *top.path());
    });
    let mut empty = request(0);
    empty.source = DiffSource::Compare("main".into());
    panel.update(cx, |p, cx| p.reveal(empty, cx));
    draw(cx);
    review.read_with(cx, |r, _| {
        assert!(r.listed && r.order.is_empty() && r.error.is_none())
    });
}
