use super::super::TerminalView;
use super::{FADE_INTERVAL, HOLD_INTERVAL, IDLE_CYCLES};
use crate::state::terminal_session::TerminalSession;
use gpui_kit as gpui;
use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use hive_core::terminal::{Terminal, TerminalSize};

fn session() -> TerminalSession {
    let terminal = Terminal::spawn(None, Some("/bin/cat"), TerminalSize::new(80, 24)).unwrap();
    TerminalSession::for_test(terminal)
}

fn setup(cx: &mut TestAppContext) -> (Entity<TerminalView>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let session = cx.new(|_| session());
    let (view, cx) = cx.add_window_view(|window, cx| {
        let view = TerminalView::with_session(session, None, window, cx);
        view.focus_handle.focus(window, cx);
        view
    });
    cx.update(|window, cx| {
        window.activate_window();
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| {
        assert!(window.is_window_active());
        assert!(view.read(cx).focus_handle.is_focused(window));
        assert!(!cx.reduce_motion());
    });
    (view, cx)
}

/// Advances from one hold to the other: lit to dark, or dark to lit.
fn half_cycle(cx: &mut VisualTestContext) {
    for interval in [HOLD_INTERVAL, FADE_INTERVAL, FADE_INTERVAL, FADE_INTERVAL] {
        cx.executor().advance_clock(interval);
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
}

#[gpui::test]
fn cursor_fades_through_intermediate_opacities_with_quiet_holds(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    for (interval, expected) in [
        (HOLD_INTERVAL, 0.85),
        (FADE_INTERVAL, 0.5),
        (FADE_INTERVAL, 0.15),
        (FADE_INTERVAL, 0.),
        (HOLD_INTERVAL, 0.15),
        (FADE_INTERVAL, 0.5),
        (FADE_INTERVAL, 0.85),
        (FADE_INTERVAL, 1.),
    ] {
        cx.executor().advance_clock(interval);
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(
            view.read_with(cx, |view, _| view.cursor_opacity()),
            expected
        );
    }
}

#[gpui::test]
fn focused_cursor_blinks_and_typing_restarts_visible_interval(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    assert_eq!(view.read_with(cx, |view, _| view.cursor_opacity()), 1.);
    assert!(view.read_with(cx, |view, _| view.cursor_tick.is_some()));
    half_cycle(cx);
    assert_eq!(view.read_with(cx, |view, _| view.cursor_opacity()), 0.);
    half_cycle(cx);
    assert_eq!(view.read_with(cx, |view, _| view.cursor_opacity()), 1.);
    half_cycle(cx);
    cx.simulate_keystrokes("a");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, _| view.cursor_opacity()), 1.);
    half_cycle(cx);
    assert_eq!(view.read_with(cx, |view, _| view.cursor_opacity()), 0.);
}

#[gpui::test]
fn deactivation_cancels_blink_and_activation_restarts_it(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    half_cycle(cx);
    cx.deactivate_window();
    cx.run_until_parked();
    for _ in 0..3 {
        half_cycle(cx);
        view.read_with(cx, |view, _| {
            assert_eq!(view.cursor_opacity(), 1.);
            assert!(view.cursor_tick.is_none());
        });
    }
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    half_cycle(cx);
    assert_eq!(view.read_with(cx, |view, _| view.cursor_opacity()), 0.);
}

#[gpui::test]
fn blur_and_reduced_motion_keep_cursor_steady(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    half_cycle(cx);
    cx.update(|window, cx| window.blur(cx));
    cx.run_until_parked();
    half_cycle(cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.cursor_opacity(), 1.);
        assert!(view.cursor_tick.is_none());
    });
    cx.update(|window, cx| {
        cx.set_reduce_motion(true);
        let focus = view.read(cx).focus_handle.clone();
        focus.focus(window, cx);
    });
    cx.run_until_parked();
    half_cycle(cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.cursor_opacity(), 1.);
        assert!(view.cursor_tick.is_none());
    });
}

#[gpui::test]
fn steady_program_cursor_cancels_pending_blink(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    half_cycle(cx);
    view.update(cx, |view, cx| {
        view.session
            .read(cx)
            .terminal()
            .unwrap()
            .term()
            .lock()
            .set_options(Default::default());
        cx.notify();
    });
    cx.run_until_parked();
    half_cycle(cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.cursor_opacity(), 1.);
        assert!(view.cursor_tick.is_none());
    });
}

#[gpui::test]
fn exited_session_cancels_pending_blink(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    half_cycle(cx);
    view.update(cx, |view, cx| {
        view.session.update(cx, |session, cx| {
            session.exited = true;
            cx.notify();
        });
    });
    cx.run_until_parked();
    half_cycle(cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.cursor_opacity(), 1.);
        assert!(view.cursor_tick.is_none());
    });
}

#[gpui::test]
fn idle_cursor_stays_lit_after_the_timeout_until_the_next_input(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    for _ in 0..IDLE_CYCLES * 2 {
        half_cycle(cx);
    }
    for _ in 0..3 {
        view.read_with(cx, |view, _| {
            assert_eq!(view.cursor_opacity(), 1.);
            assert!(view.cursor_tick.is_none());
        });
        half_cycle(cx);
    }
    cx.simulate_keystrokes("a");
    cx.run_until_parked();
    half_cycle(cx);
    assert_eq!(view.read_with(cx, |view, _| view.cursor_opacity()), 0.);
}

#[gpui::test]
fn replacing_the_session_restarts_the_visible_interval(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    half_cycle(cx);
    assert_eq!(view.read_with(cx, |view, _| view.cursor_opacity()), 0.);
    let shell = cx.new(|_| session());
    view.update_in(cx, |view, window, cx| {
        view.shell = Some(shell);
        view.continue_in_shell(window, cx);
    });
    assert_eq!(view.read_with(cx, |view, _| view.cursor_opacity()), 1.);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    half_cycle(cx);
    assert_eq!(view.read_with(cx, |view, _| view.cursor_opacity()), 0.);
}
