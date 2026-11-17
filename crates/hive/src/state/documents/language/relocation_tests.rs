use super::{Document, Family, LanguageSession, OpenFile, Session, Store, Synced, Target};
use gpui::component::input::Rope;
use gpui::{
    AppContext as _, Context, Entity, IntoElement, Render, Task, TestAppContext, WeakEntity,
    Window, div,
};
use gpui_kit as gpui;
use hive_core::{files::Snapshot, host::Host, language_service::position};
use lsp_types::{ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind};
use std::{collections::HashMap, fs, os::unix::fs::PermissionsExt, path::Path};

struct Harness(Entity<Document>);
impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let _ = &self.0;
        div()
    }
}

#[gpui::test]
fn unlocking_a_renamed_document_opens_its_new_uri_without_another_session_event(
    cx: &mut TestAppContext,
) {
    cx.update(gpui::init);
    let root = std::env::temp_dir().join(format!("hive-rename-sync-{}", std::process::id()));
    let bin = root.join("node_modules/.bin/typescript-language-server");
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    // No responses: the shared session must not emit an event that accidentally repairs the bug.
    fs::write(&bin, "#!/bin/sh\ncat >/dev/null\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
    let target = Target {
        host: Host::Local,
        project: root.clone(),
        path: root.clone(),
    };
    let old_uri = position::uri(&root.join("old.ts")).unwrap();
    let new_uri = position::uri(&root.join("new.ts")).unwrap();
    let other_uri = position::uri(&root.join("other.ts")).unwrap();
    let mut captured = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let store = cx.new(Store::with_held_recovery);
        let (client, _events) = Session::start(Host::Local, root.clone(), Family::TypeScript);
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
            documents: HashMap::<_, Synced>::new(),
            _events: None,
            _resync: None,
        });
        let make = |path: &str| OpenFile {
            target: target.clone(),
            path: path.into(),
            position: None,
            preview: false,
            placement: Default::default(),
        };
        let document = cx.new(|cx| {
            Document::isolated_for_test(
                make("old.ts"),
                store.clone(),
                WeakEntity::new_invalid(),
                window,
                cx,
            )
        });
        let other = cx.new(|cx| {
            Document::isolated_for_test(
                make("other.ts"),
                store.clone(),
                WeakEntity::new_invalid(),
                window,
                cx,
            )
        });
        for document in [&document, &other] {
            document.update(cx, |d, cx| {
                d.base = Some(Snapshot::parse(Path::new("source"), b"").unwrap());
                d.attach_language(session.clone(), cx);
            });
        }
        document.update(cx, |d, cx| {
            d.lock_path(cx).unwrap();
            d.close_language(cx);
            d.path = "new.ts".into();
            d.attach_language(session.clone(), cx);
        });
        let recovery = store
            .read(cx)
            .replace_recovery("new".into(), None, vec!["old".into()]);
        let completion: Task<()> = document.update(cx, |_, cx| {
            cx.spawn(async move |this, cx| {
                recovery.recv().await.unwrap().unwrap();
                this.update(cx, |d, cx| d.unlock_path(cx)).unwrap();
            })
        });
        captured = Some((document.clone(), other, store, session, completion));
        Harness(document)
    });
    let (document, _other, store, session, _completion) = captured.unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(document.read(cx).path_busy);
        let session = session.read(cx);
        assert!(!session.documents.contains_key(&old_uri));
        assert!(!session.documents.contains_key(&new_uri));
        assert!(session.documents.contains_key(&other_uri));
    });
    store.read_with(cx, |store, _| store.complete_held_recovery());
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(!document.read(cx).path_busy);
        let session = session.read(cx);
        assert_eq!(session.documents[&new_uri].text, Rope::from(""));
        assert!(session.documents.contains_key(&other_uri));
    });
    fs::remove_dir_all(root).unwrap();
}
