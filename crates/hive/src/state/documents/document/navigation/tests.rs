use super::{
    Document, OpenFile, STICKY_BORDER_WIDTH, STICKY_SCROLL_ROWS, Snapshot, Store,
    reveal_below_sticky, sticky_row_limit,
};
use gpui::{
    AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, WeakEntity, Window, div, point, px, size,
};
use gpui_kit as gpui;
use gpui_kit::component::input::{Editor, Position};
use hive_core::{host::Host, projects::Target};
use std::path::Path;

struct Harness(Entity<Document>);
impl Render for Harness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().w(px(400.)).h(px(140.)).child(
            Editor::new(&self.0.read(cx).editor)
                .bordered(false)
                .size_full(),
        )
    }
}

fn setup(cx: &mut TestAppContext, sticky: bool) -> (Entity<Document>, &mut VisualTestContext) {
    cx.update(gpui::init);
    let mut document = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let store = cx.new(|cx| {
            let mut store = Store::with_held_recovery(cx);
            store.state.settings.files.sticky_scroll = sticky;
            store.state.settings.files.language_tools = false;
            store
        });
        let request = OpenFile {
            target: Target {
                host: Host::Local,
                project: "/unused".into(),
                path: "/unused".into(),
            },
            path: "source.txt".into(),
            position: None,
            preview: false,
            placement: Default::default(),
        };
        let entity = cx.new(|cx| {
            Document::isolated_for_test(request, store, WeakEntity::new_invalid(), window, cx)
        });
        let source: String = (0..100)
            .map(|line| format!("line {line}: {}\n", "x".repeat(160)))
            .collect();
        entity.update(cx, |document, cx| {
            document.base =
                Some(Snapshot::parse(Path::new("source.txt"), source.as_bytes()).unwrap());
            document
                .editor
                .update(cx, |editor, cx| editor.set_value(source, window, cx));
        });
        document = Some(entity.clone());
        Harness(entity)
    });
    draw(cx);
    (document.unwrap(), cx)
}

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
    }
}

// TestAppContext draws at the end of update; deliver the first native frame callback
// inside the same update as goto so it cannot accidentally observe an already-painted layout.
fn navigate(document: &Entity<Document>, positions: &[Position], cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        for position in positions {
            document.update(cx, |document, cx| document.goto(*position, window, cx));
        }
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
    draw(cx);
}

#[gpui::test]
fn navigation_leaves_the_cursor_below_sticky_rows_after_scrolling_up(cx: &mut TestAppContext) {
    let (document, cx) = setup(cx, true);
    navigate(&document, &[Position::new(90, 0)], cx);
    navigate(&document, &[Position::new(20, 2)], cx);
    cx.read(|cx| {
        let editor = document.read(cx).editor.read(cx);
        let cursor = editor.cursor();
        let caret = editor.range_to_bounds(&(cursor..cursor)).unwrap();
        assert_eq!(editor.cursor_position(), Position::new(20, 2));
        assert!(
            caret.top()
                >= editor.input_bounds().top()
                    + editor.line_height().unwrap() * STICKY_SCROLL_ROWS
                    + STICKY_BORDER_WIDTH
        );
        assert!(caret.bottom() <= editor.input_bounds().bottom());
    });
}

#[gpui::test]
fn a_newer_navigation_keeps_its_cursor_and_horizontal_visibility(cx: &mut TestAppContext) {
    let (document, cx) = setup(cx, true);
    navigate(&document, &[Position::new(90, 0)], cx);
    navigate(
        &document,
        &[Position::new(20, 2), Position::new(40, 150)],
        cx,
    );
    cx.read(|cx| {
        let editor = document.read(cx).editor.read(cx);
        let cursor = editor.cursor();
        let caret = editor.range_to_bounds(&(cursor..cursor)).unwrap();
        assert_eq!(editor.cursor_position(), Position::new(40, 150));
        assert!(caret.left() >= editor.input_bounds().left());
        assert!(caret.right() <= editor.input_bounds().right());
    });
}

#[gpui::test]
fn navigation_without_sticky_scroll_keeps_the_editors_minimal_padding(cx: &mut TestAppContext) {
    let (document, cx) = setup(cx, false);
    navigate(&document, &[Position::new(90, 0)], cx);
    navigate(&document, &[Position::new(20, 2)], cx);
    cx.read(|cx| {
        let editor = document.read(cx).editor.read(cx);
        let cursor = editor.cursor();
        let caret = editor.range_to_bounds(&(cursor..cursor)).unwrap();
        assert!(
            caret.top()
                < editor.input_bounds().top() + editor.line_height().unwrap() * STICKY_SCROLL_ROWS
        );
    });
}

#[test]
fn sticky_reveal_preserves_visible_targets_and_fits_short_viewports() {
    assert_eq!(sticky_row_limit(px(100.), px(20.)), 2);
    assert_eq!(sticky_row_limit(px(41.), px(20.)), 1);
    assert_eq!(sticky_row_limit(px(30.), px(20.)), 0);
    assert_eq!(sticky_row_limit(px(10.), px(0.)), 0);
    let offset = point(px(-80.), px(-200.));
    let viewport = Bounds::new(point(px(0.), px(10.)), size(px(400.), px(100.)));
    let caret = Bounds::new(point(px(40.), px(70.)), size(px(0.), px(20.)));
    assert_eq!(
        reveal_below_sticky(offset, viewport, caret, px(20.)),
        offset
    );
    let viewport = Bounds::new(point(px(0.), px(10.)), size(px(400.), px(30.)));
    let caret = Bounds::new(point(px(40.), px(10.)), size(px(0.), px(20.)));
    assert_eq!(
        reveal_below_sticky(offset, viewport, caret, px(20.)),
        offset
    );
}
