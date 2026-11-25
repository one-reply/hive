use super::super::TerminalView;
use crate::state::terminal_session::TerminalSession;
use gpui_kit as gpui;
use gpui_kit::{
    AppContext as _, Entity, MouseButton, Pixels, Point, TestAppContext, VisualTestContext, point,
    px,
};
use hive_core::terminal::{Color, Line, Terminal, TerminalSize};
use std::time::Duration;

fn setup(cx: &mut TestAppContext) -> (Entity<TerminalView>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let terminal = Terminal::spawn(None, Some("/bin/cat"), TerminalSize::new(80, 24)).unwrap();
    terminal
        .term()
        .lock()
        .grid_mut()
        .scroll_up::<Color>(&(Line(0)..Line(24)), 300);
    let session = cx.new(|_| TerminalSession::for_test(terminal));
    let (view, cx) = cx.add_window_view(|window, cx| {
        let view = TerminalView::with_session(session, None, window, cx);
        view.focus_handle.focus(window, cx);
        view
    });
    cx.update(|window, cx| {
        window.activate_window();
        window.draw(cx).clear(cx);
    });
    (view, cx)
}

fn offset(view: &Entity<TerminalView>, cx: &VisualTestContext) -> usize {
    cx.read(|cx| {
        view.read(cx)
            .session
            .read(cx)
            .terminal()
            .unwrap()
            .term()
            .lock()
            .grid()
            .display_offset()
    })
}

fn tick(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.run_until_parked();
}

fn start_drag(
    view: &Entity<TerminalView>,
    cx: &mut VisualTestContext,
) -> (Point<Pixels>, Point<Pixels>) {
    let grid = cx.read(|cx| view.read(cx).grid.unwrap());
    let inside = grid.bounds.center();
    let outside = point(inside.x, grid.bounds.top() - px(60.));
    cx.simulate_mouse_down(inside, MouseButton::Left, Default::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_mouse_move(outside, MouseButton::Left, Default::default());
    (inside, outside)
}

#[gpui::test]
fn drag_keeps_scrolling_outside_then_stops_inside_and_on_release(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let (inside, outside) = start_drag(&view, cx);
    for _ in 0..8 {
        tick(cx);
    }
    let scrolled = offset(&view, cx);
    assert!(scrolled > 0);
    cx.simulate_mouse_move(inside, MouseButton::Left, Default::default());
    for _ in 0..8 {
        tick(cx);
    }
    assert_eq!(offset(&view, cx), scrolled);
    cx.simulate_mouse_move(outside, MouseButton::Left, Default::default());
    for _ in 0..8 {
        tick(cx);
    }
    assert!(offset(&view, cx) > scrolled);
    cx.simulate_mouse_up(outside, MouseButton::Left, Default::default());
    let released = offset(&view, cx);
    for _ in 0..8 {
        tick(cx);
    }
    assert_eq!(offset(&view, cx), released);
    cx.read(|cx| {
        assert!(!view.read(cx).selecting);
        assert!(!view.read(cx).selection_scroll.is_active());
    });
}

#[gpui::test]
fn deactivating_the_window_cancels_drag_autoscroll(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    start_drag(&view, cx);
    for _ in 0..8 {
        tick(cx);
    }
    assert!(offset(&view, cx) > 0);
    cx.deactivate_window();
    let stopped = offset(&view, cx);
    for _ in 0..8 {
        tick(cx);
    }
    assert_eq!(offset(&view, cx), stopped);
    cx.read(|cx| assert!(!view.read(cx).selection_scroll.is_active()));
}

#[gpui::test]
fn exiting_the_session_stops_drag_autoscroll(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    start_drag(&view, cx);
    tick(cx);
    let stopped = offset(&view, cx);
    cx.update(|_, cx| {
        let session = view.read(cx).session.clone();
        session.update(cx, |session, _| session.exited = true);
    });
    for _ in 0..8 {
        tick(cx);
    }
    assert_eq!(offset(&view, cx), stopped);
    cx.read(|cx| assert!(!view.read(cx).selection_scroll.is_active()));
}

fn with_url(view: &Entity<TerminalView>, cx: &mut VisualTestContext) -> &'static str {
    use hive_core::terminal::{Column, Point as GridPoint};
    let url = "https://example.com/x";
    cx.read(|cx| {
        let session = view.read(cx).session.read(cx);
        let mut term = session.terminal().unwrap().term().lock();
        for (ix, ch) in url.chars().enumerate() {
            term.grid_mut()[GridPoint::new(Line(0), Column(ix))].c = ch;
        }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    url
}

fn cell_center(view: &Entity<TerminalView>, column: f32, cx: &VisualTestContext) -> Point<Pixels> {
    let grid = cx.read(|cx| view.read(cx).grid.unwrap());
    grid.bounds.origin + point(grid.cell.width * (column + 0.5), grid.cell.height * 0.5)
}

#[gpui::test]
fn a_plain_click_on_a_url_opens_it(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let url = with_url(&view, cx);
    let at = cell_center(&view, 2., cx);
    cx.simulate_mouse_move(at, None, Default::default());
    assert!(cx.read(|cx| view.read(cx).hovered_link().is_some()));
    cx.simulate_click(at, Default::default());
    assert_eq!(cx.opened_url().as_deref(), Some(url));
}

#[gpui::test]
fn dragging_across_a_url_selects_without_opening_it(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    with_url(&view, cx);
    let (from, to) = (cell_center(&view, 2., cx), cell_center(&view, 9., cx));
    cx.simulate_mouse_down(from, MouseButton::Left, Default::default());
    cx.simulate_mouse_move(to, MouseButton::Left, Default::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Default::default());
    assert_eq!(cx.opened_url(), None);
    let selected = cx.read(|cx| {
        view.read(cx)
            .session
            .read(cx)
            .terminal()
            .unwrap()
            .term()
            .lock()
            .selection_to_string()
    });
    assert_eq!(selected.as_deref(), Some("ps://ex"));
}

#[gpui::test]
fn a_click_on_a_codex_document_label_reaches_the_application(cx: &mut TestAppContext) {
    use alacritty_terminal::vte::ansi::Processor;

    let (view, cx) = setup(cx);
    let session = cx.read(|cx| view.read(cx).session.clone());
    let feed = |bytes: &[u8], cx: &VisualTestContext| {
        cx.read(|cx| {
            let terminal = session.read(cx).terminal().unwrap();
            let mut parser: Processor = Processor::new();
            parser.advance(&mut *terminal.term().lock(), bytes);
        });
    };
    // Codex owns the file destination. Its label is styled text with no OSC 8 target.
    feed(
        b"\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h\x1b[4mSource\x1b[0m",
        cx,
    );
    let at = cell_center(&view, 2., cx);
    let elsewhere = cell_center(&view, 5., cx);
    cx.simulate_mouse_down(at, MouseButton::Left, Default::default());
    cx.simulate_mouse_move(elsewhere, MouseButton::Left, Default::default());
    cx.simulate_mouse_move(at, MouseButton::Left, Default::default());
    cx.simulate_mouse_up(at, MouseButton::Left, Default::default());
    assert!(cx.read(|cx| session.read(cx).written.is_empty()));

    cx.simulate_click(
        at,
        gpui_kit::Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    cx.simulate_mouse_down(at, MouseButton::Left, Default::default());
    feed(b"\x1b[?1003l", cx);
    cx.simulate_mouse_up(at, MouseButton::Left, Default::default());
    assert!(cx.read(|cx| session.read(cx).written.is_empty()));

    feed(b"\x1b[?1003h", cx);
    cx.simulate_click(at, Default::default());
    assert_eq!(
        cx.read(|cx| session.read(cx).written.clone()),
        b"\x1b[<0;3;1M\x1b[<0;3;1m"
    );
    assert_eq!(cx.opened_url(), None);
}
