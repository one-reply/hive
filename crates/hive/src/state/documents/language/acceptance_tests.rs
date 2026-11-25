//! Exercise the actual editor, deferred LSP response and undo stack with captured server output.
use super::{Document, Family, LanguageSession, OpenFile, Session, Store, Target};
use crate::state::documents::language_features;
use gpui::component::input::{Editor, EditorState, RopeExt as _};
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, TestAppContext, WeakEntity, Window, div,
};
use gpui_kit as gpui;
use hive_core::{files::Snapshot, host::Host, language_service::Outbox};
use lsp_types::ServerCapabilities;
use serde_json::{Value, json};
use std::path::Path;

struct Harness(Entity<EditorState>, Entity<Document>);
impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .capture_action(crate::ui::editor_panel::import_menu::accept_on_enter(
                self.1.clone(),
            ))
            .capture_action(crate::ui::editor_panel::suggestions::accept_fix_on_enter(
                self.0.clone(),
            ))
            .child(Editor::new(&self.0))
    }
}

fn mount<'a>(
    source: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Document>, Outbox, &'a mut gpui::VisualTestContext) {
    cx.update(gpui::init);
    let (client, outbox) = Session::detached(32);
    let mut doc = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let capabilities = serde_json::from_value::<ServerCapabilities>(json!({
            "textDocumentSync": 1,
            "codeActionProvider": {"resolveProvider": true},
            "completionProvider": {"resolveProvider": true, "triggerCharacters": ["."]}
        }))
        .unwrap();
        let session = cx.new(|_| LanguageSession {
            host: Host::Local,
            root: "/project/ts".into(),
            family: Family::TypeScript,
            capabilities: Some(capabilities),
            error: None,
            diagnostics: Default::default(),
            diagnostics_limited: false,
            client,
            documents: Default::default(),
            unsent: Default::default(),
            _events: None,
            _resync: None,
        });
        let store = cx.new(Store::with_held_recovery);
        let request = OpenFile {
            target: Target {
                host: Host::Local,
                project: "/project/ts".into(),
                path: "/project/ts".into(),
            },
            path: "src/Demo.tsx".into(),
            position: None,
            preview: false,
            placement: Default::default(),
        };
        let document = cx.new(|cx| {
            Document::isolated_for_test(request, store, WeakEntity::new_invalid(), window, cx)
        });
        document.update(cx, |doc, cx| {
            doc.base = Some(Snapshot::parse(Path::new("src/Demo.tsx"), source.as_bytes()).unwrap());
            doc.editor
                .update(cx, |editor, cx| editor.set_value(source, window, cx));
            doc.attach_language(session, cx);
        });
        language_features::install(&document, WeakEntity::new_invalid(), cx);
        doc = Some(document.clone());
        let view = cx.new(|cx| Harness(document.read(cx).editor.clone(), document.clone()));
        gpui::base::Root::new(view, window, cx)
    });
    let doc = doc.unwrap();
    assert_eq!(outbox.take(), Some("textDocument/didOpen"));
    (doc, outbox, cx)
}

#[gpui::test]
fn a_deferred_autoimport_is_one_undo_and_a_stale_reply_cannot_overwrite_typing(
    cx: &mut TestAppContext,
) {
    let fixture: Value = serde_json::from_str(include_str!(
        "../language_features/fixtures/typescript-autoimport.json"
    ))
    .unwrap();
    let source = fixture["source"].as_str().unwrap();
    let (doc, outbox, cx) = mount(&source[..source.len() - 1], cx);
    let editor = cx.read(|cx| doc.read(cx).editor.clone());
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.focus(window, cx);
            editor.set_cursor_position(
                editor.text().offset_to_position(source.len() - 1),
                window,
                cx,
            );
        });
        window.draw(cx).clear(cx);
    });
    for stale in [false, true] {
        if stale {
            cx.update(|window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.set_cursor_position(
                        editor.text().offset_to_position(source.len()),
                        window,
                        cx,
                    );
                    editor.show_completions(window, cx);
                });
            });
        } else {
            cx.simulate_keystrokes("t");
            assert_eq!(outbox.take(), Some("textDocument/didChange"));
        }
        let (method, params) = outbox.respond(json!([fixture["initial"]])).unwrap();
        assert_eq!(method, "textDocument/completion");
        assert_eq!(params["context"], json!({"triggerKind":1}));
        cx.run_until_parked();
        assert!(cx.read(|cx| doc.read(cx).import_menu.is_some()));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        if stale {
            cx.update(|window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.set_value("a".repeat(source.len()), window, cx);
                    editor.set_cursor_position(
                        editor.text().offset_to_position(source.len()),
                        window,
                        cx,
                    );
                })
            });
        }
        let (method, params) = outbox.respond(fixture["resolved"].clone()).unwrap();
        assert_eq!(method, "completionItem/resolve");
        assert_eq!(params["data"], fixture["initial"]["data"]);
        cx.run_until_parked();
        if stale {
            cx.read(|cx| assert_eq!(editor.read(cx).text().to_string(), "a".repeat(source.len())));
        } else {
            cx.read(|cx| {
                assert_eq!(
                    editor.read(cx).text().to_string(),
                    "import { Button } from \"./Button\";\n\nexport const Demo = () => <Button"
                )
            });
            cx.simulate_keystrokes("cmd-z");
            cx.read(|cx| assert_eq!(editor.read(cx).text().to_string(), source));
        }
    }
}

#[gpui::test]
fn a_resolved_typescript_quick_fix_through_the_editor_requires_one_undo(cx: &mut TestAppContext) {
    let capture: Value = serde_json::from_str(include_str!(
        "../language_features/fixtures/typescript-fix.json"
    ))
    .unwrap();
    let source = "export const Demo = () => Button(\"a\", 1);";
    let (doc, outbox, cx) = mount("", cx);
    let editor = cx.read(|cx| doc.read(cx).editor.clone());
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.set_value(source, window, cx);
            editor.focus(window, cx);
            editor.set_cursor_position(
                editor
                    .text()
                    .offset_to_position(source.find("Button").unwrap()),
                window,
                cx,
            );
        });
        doc.update(cx, |doc, _| {
            doc.diagnostics = serde_json::from_value(capture["diagnostics"].clone()).unwrap();
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.dispatch_action(gpui::base::input::ToggleCodeActions);
    cx.run_until_parked();
    assert_eq!(outbox.take(), Some("textDocument/didChange"));
    let unresolved = json!({"title":capture["title"], "kind":"quickfix", "data":{"fix":42}});
    let (method, params) = outbox.respond(json!([unresolved])).unwrap();
    assert_eq!(method, "textDocument/codeAction");
    assert_eq!(params["context"]["diagnostics"][0]["code"], 2304);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.read(|cx| editor.read(cx).code_action_menu_state().open));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let (method, params) = outbox.respond(capture).unwrap();
    assert_eq!(method, "codeAction/resolve");
    assert_eq!(params["data"]["fix"], 42);
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            editor.read(cx).text().to_string(),
            format!("import {{ Button }} from \"./Button\";\n\n{source}")
        )
    });
    cx.simulate_keystrokes("cmd-z");
    cx.read(|cx| assert_eq!(editor.read(cx).text().to_string(), source));
}
