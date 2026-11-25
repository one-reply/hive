//! A full queue to the language server refuses a notification. An edit is sent again as the
//! document's full text on the next sync; a close or a save would be gone for good.
use super::{LanguageSession, RESYNC_DELAY};
use gpui::{AppContext as _, Entity, TestAppContext};
use gpui_kit as gpui;
use gpui_kit::component::input::Rope;
use hive_core::host::Host;
use hive_core::language_service::{Error, Family, Outbox, Session, position};
use lsp_types::{
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions,
};
use std::path::Path;

/// A session whose server queue holds one message at a time.
fn busy_session(cx: &mut TestAppContext) -> (Entity<LanguageSession>, Outbox) {
    let (client, outbox) = Session::detached(1);
    let session = cx.new(|_| LanguageSession {
        host: Host::Local,
        root: "/repo".into(),
        family: Family::TypeScript,
        capabilities: Some(ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Options(
                TextDocumentSyncOptions {
                    open_close: Some(true),
                    change: Some(TextDocumentSyncKind::FULL),
                    save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                    ..Default::default()
                },
            )),
            ..Default::default()
        }),
        error: None,
        diagnostics: Default::default(),
        diagnostics_limited: false,
        client,
        generation: 0,
        documents: Default::default(),
        unsent: Default::default(),
        _events: None,
        _resync: None,
    });
    (session, outbox)
}

fn wait_out_the_resync(cx: &mut TestAppContext) {
    cx.executor().advance_clock(RESYNC_DELAY);
    cx.run_until_parked();
}

#[gpui::test]
fn a_save_and_a_close_the_full_queue_refused_reach_the_server_in_order(cx: &mut TestAppContext) {
    let (session, outbox) = busy_session(cx);
    let uri = position::uri(Path::new("/repo/a.ts")).unwrap();
    let path = Path::new("a.ts");
    let sync = |session: &mut LanguageSession| session.sync(uri.clone(), path, &Rope::from("one"));
    session.update(cx, |session, _| {
        sync(session).unwrap();
    });
    session.update(cx, |session, cx| {
        session.saved(uri.clone(), None, cx);
        session.close(&uri, cx);
    });
    assert_eq!(outbox.take(), Some("textDocument/didOpen"));

    // Opened again while the queue has room but the session still owes the save and the
    // close: those go first, and the open waits for a later sync.
    session.update(cx, |session, _| {
        assert!(matches!(sync(session), Err(Error::Busy)));
    });
    assert_eq!(outbox.take(), Some("textDocument/didSave"));
    wait_out_the_resync(cx);
    assert_eq!(outbox.take(), Some("textDocument/didClose"));
    wait_out_the_resync(cx);
    assert_eq!(outbox.take(), None);

    session.update(cx, |session, _| {
        sync(session).unwrap();
    });
    assert_eq!(outbox.take(), Some("textDocument/didOpen"));
}
