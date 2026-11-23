//! Typing into a terminal the way the window delivers it: a key first, then, if the
//! terminal left the key alone, its text through the registered input handler.
use super::super::TerminalView;
use crate::state::terminal_session::TerminalSession;
use crate::terminal::input::TerminalInputHandler;
use gpui_kit as gpui;
use gpui_kit::{
    AppContext as _, Entity, InputHandler as _, KeyDownEvent, Keystroke, Modifiers, TestAppContext,
    VisualTestContext,
};
use hive_core::terminal::{Terminal, TerminalSize};

fn setup(cx: &mut TestAppContext) -> (Entity<TerminalView>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let terminal = Terminal::spawn(None, Some("/bin/cat"), TerminalSize::new(80, 24)).unwrap();
    let session = cx.new(|_| TerminalSession::for_test(terminal));
    let (view, cx) = cx.add_window_view(|window, cx| {
        let view = TerminalView::with_session(session, None, window, cx);
        view.focus_handle.focus(window, cx);
        view
    });
    draw(cx);
    (view, cx)
}

/// A frame: the focused terminal registers its input handler while it paints.
fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.activate_window());
    // Focus changes are reported only between frames of an active window.
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// Everything the program has been sent.
fn written(view: &Entity<TerminalView>, cx: &mut VisualTestContext) -> Vec<u8> {
    cx.read(|cx| view.read(cx).session.read(cx).written.clone())
}

fn marked(view: &Entity<TerminalView>, cx: &mut VisualTestContext) -> Option<String> {
    cx.read(|cx| view.read(cx).marked_text().map(str::to_owned))
}

/// A key as macOS reports it, with the character it types; nothing follows it up.
fn press(key: &str, key_char: &str, alt: bool, cx: &mut VisualTestContext) {
    cx.simulate_event(KeyDownEvent {
        keystroke: Keystroke {
            modifiers: Modifiers {
                alt,
                ..Modifiers::default()
            },
            key: key.into(),
            key_char: Some(key_char.into()),
        },
        is_held: false,
        prefer_character_input: false,
    });
    cx.run_until_parked();
}

fn handler(view: &Entity<TerminalView>) -> TerminalInputHandler {
    TerminalInputHandler::new(view.clone(), None, None)
}

#[gpui::test]
fn typed_text_reaches_the_program_once_through_the_text_input(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);

    cx.simulate_keystrokes("l s space - l enter");

    assert_eq!(
        written(&view, cx),
        b"ls -l\r",
        "each character once, and Enter as the carriage return of its own key"
    );
}

#[gpui::test]
fn cyrillic_and_emoji_are_sent_as_the_text_the_system_delivers(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);

    cx.simulate_input("привіт");
    // The emoji panel and dictation insert text with no key at all.
    cx.update(|window, cx| handler(&view).replace_text_in_range(None, " 😀", window, cx));

    assert_eq!(written(&view, cx), "привіт 😀".as_bytes());
}

#[gpui::test]
fn a_dead_key_is_composed_instead_of_sent_as_a_bare_accent(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);

    // The acute accent of a German layout, and Option+E of a US one.
    press("´", "´", false, cx);
    press("e", "´", true, cx);
    assert_eq!(
        written(&view, cx),
        b"",
        "the key is left for the input method to compose"
    );

    // What the input method then does with it.
    cx.update(|window, cx| {
        handler(&view).replace_and_mark_text_in_range(None, "´", Some(1..1), window, cx)
    });
    assert_eq!(marked(&view, cx).as_deref(), Some("´"));
    assert_eq!(written(&view, cx), b"", "a composition is not sent");
    draw(cx);

    cx.update(|window, cx| handler(&view).replace_text_in_range(None, "é", window, cx));
    assert_eq!(marked(&view, cx), None);
    assert_eq!(written(&view, cx), "é".as_bytes());
}

#[gpui::test]
fn a_composition_is_reported_so_that_enter_and_backspace_go_to_the_input_method(
    cx: &mut TestAppContext,
) {
    let (view, cx) = setup(cx);
    let range = |view: &Entity<TerminalView>, cx: &mut VisualTestContext| {
        cx.update(|window, cx| handler(view).marked_text_range(window, cx))
    };
    assert_eq!(range(&view, cx), None);

    // Kana before their conversion; 😀 is two UTF-16 units.
    cx.update(|window, cx| {
        handler(&view).replace_and_mark_text_in_range(None, "にほ😀", None, window, cx)
    });
    assert_eq!(range(&view, cx), Some(0..4));

    // Backspace inside the input method shortens the composition; the last one ends it.
    cx.update(|window, cx| {
        handler(&view).replace_and_mark_text_in_range(None, "", None, window, cx)
    });
    assert_eq!(range(&view, cx), None);
    assert_eq!(written(&view, cx), b"", "nothing of it was sent");
}

#[gpui::test]
fn a_cancelled_or_abandoned_composition_is_dropped_unsent(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);

    cx.update(|window, cx| {
        handler(&view).replace_and_mark_text_in_range(None, "か", None, window, cx)
    });
    cx.update(|window, cx| handler(&view).unmark_text(window, cx));
    assert_eq!(marked(&view, cx), None);

    // The keyboard leaves the terminal in the middle of a composition.
    cx.update(|window, cx| {
        handler(&view).replace_and_mark_text_in_range(None, "か", None, window, cx)
    });
    draw(cx);
    cx.update(|window, cx| window.blur(cx));
    draw(cx);
    assert_eq!(marked(&view, cx), None);
    assert_eq!(written(&view, cx), b"");
}

#[gpui::test]
fn the_candidate_window_follows_the_cursor_cell(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let cell = gpui_kit::Bounds {
        origin: gpui_kit::point(gpui_kit::px(40.), gpui_kit::px(60.)),
        size: gpui_kit::size(gpui_kit::px(8.), gpui_kit::px(17.)),
    };

    let at_cursor = cx.update(|window, cx| {
        TerminalInputHandler::new(view.clone(), Some(cell), None).bounds_for_range(0..0, window, cx)
    });
    assert_eq!(at_cursor, Some(cell));

    // A cursor scrolled out of view gives the system nothing to anchor to.
    let hidden = cx.update(|window, cx| handler(&view).bounds_for_range(0..0, window, cx));
    assert_eq!(hidden, None);
}

#[gpui::test]
fn text_for_an_exited_program_is_dropped(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, _| session.exited = true)
        })
    });

    cx.simulate_keystrokes("a enter");

    assert_eq!(written(&view, cx), b"");
}
