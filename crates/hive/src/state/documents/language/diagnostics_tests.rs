use super::{Document, Family, LanguageSession, OpenFile, Session, Store, Target};
use gpui::{
    AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, WeakEntity, Window, div,
};
use gpui_kit as gpui;
use hive_core::{files::Snapshot, host::Host, language_service::position};
use lsp_types::{
    Diagnostic, Position, PublishDiagnosticsParams, Range, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind,
};
use std::{
    collections::HashMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Harness(Entity<Document>);
impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let _ = &self.0;
        div()
    }
}

/// How the server published the one diagnostic on line 0 of the source.
#[derive(Clone, Copy)]
enum Published {
    /// For the opened document's version, as rust-analyzer does.
    Versioned,
    /// With no version after the document was opened, as typescript-language-server does.
    Unversioned,
    /// With no version before the document was opened, as rust-analyzer does for
    /// `cargo check` results in files no editor has open.
    UnversionedBeforeOpen,
}

const TAIL_EDIT: &str = "const broken: string = 123;\n\n// changed tail\n";
const FIXED_LINE: &str = "const broken: string = 'fixed';\n\n// tail\n";
const LINE_ABOVE: &str = "// added\nconst broken: string = 123;\n\n// tail\n";

#[gpui::test]
fn unchanged_diagnostics_survive_a_synced_edit_without_a_new_server_event(cx: &mut TestAppContext) {
    check_after_edit(cx, Published::Versioned, TAIL_EDIT, 1);
}

#[gpui::test]
fn editing_a_diagnosed_line_does_not_restore_its_old_diagnostic(cx: &mut TestAppContext) {
    check_after_edit(cx, Published::Versioned, FIXED_LINE, 0);
}

#[gpui::test]
fn an_unversioned_diagnostic_survives_an_edit_that_leaves_its_line(cx: &mut TestAppContext) {
    check_after_edit(cx, Published::Unversioned, TAIL_EDIT, 1);
}

#[gpui::test]
fn an_unversioned_diagnostic_does_not_return_after_its_line_is_fixed(cx: &mut TestAppContext) {
    check_after_edit(cx, Published::Unversioned, FIXED_LINE, 0);
}

#[gpui::test]
fn an_unversioned_diagnostic_does_not_land_on_the_line_an_edit_moved_under_it(
    cx: &mut TestAppContext,
) {
    check_after_edit(cx, Published::Unversioned, LINE_ABOVE, 0);
}

#[gpui::test]
fn a_diagnostic_published_before_the_document_opened_follows_later_edits(cx: &mut TestAppContext) {
    check_after_edit(cx, Published::UnversionedBeforeOpen, FIXED_LINE, 0);
}

fn check_after_edit(cx: &mut TestAppContext, published: Published, edited: &str, expected: usize) {
    cx.update(gpui::init);
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "hive-diagnostics-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let fixture = Fixture(root.clone());
    let bin = root.join("node_modules/.bin/typescript-language-server");
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    fs::write(&bin, "#!/bin/sh\nexec cat >/dev/null\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
    let source = "const broken: string = 123;\n\n// tail\n";
    let uri = position::uri(&root.join("a.ts")).unwrap();
    let mut captured = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let store = cx.new(Store::with_held_recovery);
        let (client, _events) = Session::start(Host::Local, root.clone(), Family::TypeScript);
        // No event listener: diagnostics below are the only publication, as when
        // rust-analyzer suppresses an unchanged set after an unrelated edit.
        let session = cx.new(|_| LanguageSession {
            host: Host::Local,
            root: root.clone(),
            family: Family::TypeScript,
            capabilities: Some(ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                ..Default::default()
            }),
            error: None,
            diagnostics: HashMap::new(),
            diagnostics_limited: false,
            client,
            generation: 0,
            documents: HashMap::new(),
            unsent: Default::default(),
            _events: None,
            _resync: None,
        });
        let request = OpenFile {
            target: Target {
                host: Host::Local,
                project: root.clone(),
                path: root.clone(),
            },
            path: "a.ts".into(),
            position: None,
            preview: false,
            placement: Default::default(),
        };
        let document = cx.new(|cx| {
            Document::isolated_for_test(request, store, WeakEntity::new_invalid(), window, cx)
        });
        let publish = |session: &Entity<LanguageSession>, cx: &mut Context<Harness>| {
            let version = matches!(published, Published::Versioned).then_some(1);
            session.update(cx, |session, cx| {
                session.receive_diagnostics(PublishDiagnosticsParams::new(
                    uri.clone(),
                    vec![Diagnostic {
                        range: Range::new(Position::new(0, 6), Position::new(0, 12)),
                        message: "Expected string, found number".into(),
                        ..Default::default()
                    }],
                    version,
                ));
                cx.notify();
            });
        };
        if matches!(published, Published::UnversionedBeforeOpen) {
            publish(&session, cx);
        }
        document.update(cx, |document, cx| {
            document.base = Some(Snapshot::parse(Path::new("a.ts"), source.as_bytes()).unwrap());
            document
                .editor
                .update(cx, |editor, cx| editor.set_value(source, window, cx));
            document.attach_language(session.clone(), cx);
        });
        if !matches!(published, Published::UnversionedBeforeOpen) {
            publish(&session, cx);
        }
        captured = Some((document.clone(), session));
        Harness(document)
    });
    let (document, session) = captured.unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            document.read(cx).diagnostics.len(),
            1,
            "initial diagnostic is shown"
        )
    });
    document.update_in(cx, |document, window, cx| {
        document
            .editor
            .update(cx, |editor, cx| editor.set_value(edited, window, cx));
        // isolated_for_test disables production InputEvent subscriptions; run the
        // same path its Change handler calls, including the actual debounce task.
        document.schedule_language(cx);
    });
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    cx.read(|cx| {
        let document = document.read(cx);
        let text = document.editor.read(cx).text();
        let session = session.read(cx);
        assert_eq!(session.documents[&uri].version, 2, "the edit reached the session");
        assert_eq!(session.current_diagnostics(&uri, text).len(), expected, "the cached publication describes only lines the edit left in place");
        assert_eq!(document.diagnostics.len(), expected, "the editor must restore still-valid diagnostics without waiting for another server event");
    });
    drop(fixture);
}
