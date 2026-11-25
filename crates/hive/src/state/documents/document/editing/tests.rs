use super::{Command, Document, Indentation, OpenFile, Snapshot, Store, next_occurrence};
use crate::ui::{
    editor_panel::{DuplicateLinesDown, MoveLinesUp, SelectNextOccurrence},
    keymap::{FILE_EDITOR_INPUT_CONTEXT, SHORTCUTS},
};
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, KeyBinding, Modifiers,
    MouseButton, ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext,
    WeakEntity, Window, div,
};
use gpui_kit as gpui;
use gpui_kit::base::input::AddCursorBelow;
use gpui_kit::component::input::{Editor, InputEditsEvent};
use hive_core::{host::Host, projects::Target};
use std::path::Path;

struct Harness(Entity<Document>);
impl Render for Harness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let document = self.0.clone();
        div()
            .capture_any_mouse_down(move |event, _, cx| {
                if event.button == MouseButton::Left {
                    document.update(cx, |d, cx| d.native_selections(event.modifiers.alt, cx));
                }
            })
            .capture_action(cx.listener(|this, _: &AddCursorBelow, _, cx| {
                this.0.update(cx, |d, cx| d.vertical_cursor(false, cx));
                cx.propagate();
            }))
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "down" {
                    this.0.update(cx, |d, cx| d.native_selection_motion(cx));
                }
            }))
            .size_full()
            .key_context("EditorPanel")
            .on_action(cx.listener(|this, _: &SelectNextOccurrence, _, cx| {
                this.0.update(cx, |d, cx| d.select_next_occurrence(cx))
            }))
            .on_action(cx.listener(|this, _: &MoveLinesUp, w, cx| {
                this.0
                    .update(cx, |d, cx| d.edit_lines(Command::MoveUp, w, cx))
            }))
            .on_action(cx.listener(|this, _: &DuplicateLinesDown, w, cx| {
                this.0
                    .update(cx, |d, cx| d.edit_lines(Command::DuplicateDown, w, cx))
            }))
            .child(Editor::new(&self.0.read(cx).editor).size_full())
    }
}

fn setup<'a>(
    cx: &'a mut TestAppContext,
    source: &'static str,
    cursor: usize,
) -> (Entity<Document>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        gpui::init(cx);
        let shortcuts = SHORTCUTS
            .iter()
            .filter(|s| s.context == FILE_EDITOR_INPUT_CONTEXT)
            .map(|s| {
                KeyBinding::load(
                    s.default,
                    s.action(),
                    Some(std::rc::Rc::new(
                        gpui::KeyBindingContextPredicate::parse(s.context).unwrap(),
                    )),
                    false,
                    None,
                    cx,
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        cx.bind_keys(shortcuts);
    });
    let mut document = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let store = cx.new(Store::with_held_recovery);
        let request = OpenFile {
            target: Target {
                host: Host::Local,
                project: "/unused".into(),
                path: "/unused".into(),
            },
            path: "a.rs".into(),
            position: None,
            preview: false,
            placement: Default::default(),
        };
        let entity = cx.new(|cx| {
            Document::isolated_for_test(request, store, WeakEntity::new_invalid(), window, cx)
        });
        entity.update(cx, |d, cx| {
            assert_eq!(d.indentation, Indentation::Four);
            d._subscriptions.push(
                cx.subscribe(&d.editor, |this, _, event: &InputEditsEvent, _| {
                    this.selection_commands.edited(event)
                }),
            );
            d.base = Some(Snapshot::parse(Path::new("a.rs"), source.as_bytes()).unwrap());
            d.editor.update(cx, |e, cx| {
                e.set_value(source, window, cx);
                e.set_selected_range(cursor..cursor, cx);
                e.focus(window, cx);
            });
        });
        document = Some(entity.clone());
        Harness(entity)
    });
    (document.unwrap(), cx)
}

#[gpui::test]
fn real_input_applies_occurrences_as_one_undo_and_repeats_line_commands(cx: &mut TestAppContext) {
    let (d, cx) = setup(cx, "λ foo foo\nbar\nend", 4);
    cx.simulate_keystrokes("cmd-d cmd-d");
    cx.read(|cx| assert_eq!(d.read(cx).cursor_count(), Some(2)));
    cx.simulate_keystrokes("x");
    cx.read(|cx| {
        assert_eq!(
            d.read(cx).editor.read(cx).text().to_string(),
            "λ x x\nbar\nend"
        )
    });
    cx.simulate_keystrokes("cmd-z");
    cx.update(|_, cx| {
        d.update(cx, |d, cx| {
            assert_eq!(d.editor.read(cx).text().to_string(), "λ foo foo\nbar\nend");
            d.native_selections(false, cx);
            d.editor
                .update(cx, |e, cx| e.set_selected_range(12..12, cx));
        });
    });
    cx.simulate_keystrokes("alt-shift-down alt-up alt-up");
    cx.read(|cx| {
        assert_eq!(
            d.read(cx).editor.read(cx).text().to_string(),
            "bar\nλ foo foo\nbar\nend"
        )
    });
    cx.update(|_, cx| {
        d.update(cx, |d, cx| {
            d.set_indentation(Indentation::Tabs, cx);
            d.editor.update(cx, |e, cx| e.set_selected_range(0..0, cx));
        })
    });
    cx.simulate_keystrokes("tab");
    cx.read(|cx| {
        assert!(
            d.read(cx)
                .editor
                .read(cx)
                .text()
                .to_string()
                .starts_with("\tbar")
        )
    });
}

#[test]
fn occurrence_search_wraps_and_skips_selected_ranges() {
    let s = "β x β x β";
    assert_eq!(
        next_occurrence(s, std::slice::from_ref(&(5..7))),
        Some(10..12)
    );
    assert_eq!(next_occurrence(s, &[5..7, 10..12]), Some(0..2));
    assert_eq!(next_occurrence(s, &[5..7, 10..12, 0..2]), None);
}

#[gpui::test]
fn selecting_the_next_word_keeps_a_native_alt_click_caret(cx: &mut TestAppContext) {
    let (d, cx) = setup(cx, "foo foo\nbar", 1);
    let at = cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        d.read(cx)
            .editor
            .read(cx)
            .range_to_bounds(&(9..9))
            .unwrap()
            .center()
    });
    cx.simulate_click(
        at,
        Modifiers {
            alt: true,
            ..Default::default()
        },
    );
    cx.simulate_keystrokes("cmd-d cmd-d");
    cx.read(|cx| assert!(d.read(cx).multiple_cursors_without_count()));
    cx.simulate_keystrokes("x");
    cx.read(|cx| {
        assert_eq!(d.read(cx).editor.read(cx).text().to_string(), "x x\nbxar");
        assert_eq!(d.read(cx).cursor_count(), Some(3));
    });
}

#[gpui::test]
fn touching_occurrences_merge_and_keep_the_original_search_word(cx: &mut TestAppContext) {
    let (d, cx) = setup(cx, "λλ λ", 0);
    cx.update(|_, cx| {
        d.read(cx)
            .editor
            .clone()
            .update(cx, |e, cx| e.set_selected_range(0..2, cx))
    });
    cx.simulate_keystrokes("cmd-d");
    cx.read(|cx| assert_eq!(d.read(cx).cursor_count(), Some(1)));
    cx.simulate_keystrokes("cmd-d");
    cx.read(|cx| assert_eq!(d.read(cx).cursor_count(), Some(2)));
    cx.simulate_keystrokes("x");
    cx.read(|cx| assert_eq!(d.read(cx).editor.read(cx).text().to_string(), "x x"));
}

#[test]
fn stale_or_empty_occurrences_do_not_index_outside_text() {
    assert_eq!(next_occurrence("x", &[0..1, 3..4]), None);
    assert_eq!(next_occurrence("x", std::slice::from_ref(&(0..0))), None);
    assert_eq!(next_occurrence("ø", std::slice::from_ref(&(0..1))), None);
}

#[gpui::test]
fn vertical_cursor_shortcuts_count_distinct_rows_and_stop_at_eof(cx: &mut TestAppContext) {
    let (d, cx) = setup(cx, "a\nb\nc", 0);
    cx.update(|w, cx| w.draw(cx).clear(cx));
    for count in [2, 3, 3] {
        cx.simulate_keystrokes("cmd-alt-down");
        cx.read(|cx| assert_eq!(d.read(cx).cursor_count(), Some(count)));
    }
    cx.simulate_keystrokes("x");
    cx.read(|cx| assert_eq!(d.read(cx).editor.read(cx).text().to_string(), "xa\nxb\nxc"));
}

#[test]
fn indentation_detection_requires_consistent_evidence() {
    for (text, expected) in [
        (
            "function f() {\n    a();\n    b();\n}\n",
            Some(Indentation::Four),
        ),
        ("if foo {\n  a();\n    b();\n}", Some(Indentation::Two)),
        ("main:\n\tfirst\n\tsecond", Some(Indentation::Tabs)),
        ("fn f() {}", None),
        ("  aligned", None),
        ("a\n    x\n      y", None),
        ("a\n  x\n\tx", None),
        ("a\n\t  x\n\t  y", None),
        ("", None),
    ] {
        assert_eq!(Indentation::detect(text), expected, "{text:?}");
    }
}

#[gpui::test]
fn first_load_detects_indentation_and_explicit_choice_survives_reload(cx: &mut TestAppContext) {
    let (d, cx) = setup(cx, "", 0);
    cx.update(|w, cx| {
        d.update(cx, |d, cx| {
            let snapshot = |text: &[u8]| Snapshot::parse(Path::new("sample.tsx"), text).unwrap();
            d.install(snapshot(b"if(x){\n  x();\n  y();\n}"), w, cx);
            assert_eq!(d.indentation, Indentation::Two);
            d.set_indentation(Indentation::Tabs, cx);
            d.install(snapshot(b"if(x){\n    x();\n    y();\n}"), w, cx);
            assert_eq!(d.indentation, Indentation::Tabs);
        })
    });
}
