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
