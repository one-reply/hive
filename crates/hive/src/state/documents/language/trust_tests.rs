//! Language servers and formatters run programs the worktree controls. These tests put a
//! fake tool into a worktree that leaves a marker file when executed, and check that opening
//! or formatting a file runs it only while the user allows language tools.
use super::{Document, Documents, LanguageStatus, OpenFile, Store, Target};
use gpui::{
    AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, VisualTestContext,
    Window, div,
};
use gpui_kit as gpui;
use hive_core::{files::Snapshot, host::Host};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// A real process starts on its own thread; this only bounds a broken test.
const MARKER_DEADLINE: Duration = Duration::from_secs(30);

struct Harness;
impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

const SERVER: &str = "typescript-language-server";
const FORMATTER: &str = "prettier";

struct Worktree {
    root: PathBuf,
}

impl Worktree {
    /// A worktree whose `node_modules/.bin` server and formatter record that they ran. The
    /// server never answers, so no session event arrives from its thread mid-test; the
    /// formatter echoes stdin, changing nothing. Neither falls back to an installed tool.
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("hive-language-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let bin = root.join("node_modules/.bin");
        fs::create_dir_all(&bin).unwrap();
        let worktree = Self { root };
        for (tool, run) in [(SERVER, "exec cat >/dev/null"), (FORMATTER, "exec cat")] {
            let marker = worktree.marker(tool);
            let marker = marker.to_str().unwrap();
            assert!(!marker.contains('\''), "the script single-quotes the path");
            let script = bin.join(tool);
            fs::write(&script, format!("#!/bin/sh\n: > '{marker}'\n{run}\n")).unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        }
        worktree
    }

    fn marker(&self, tool: &str) -> PathBuf {
        self.root.join(format!("{tool}-ran"))
    }

    fn target(&self) -> Target {
        Target {
            host: Host::Local,
            project: self.root.clone(),
            path: self.root.clone(),
        }
    }

    fn ran(&self, tool: &str) -> bool {
        self.marker(tool).exists()
    }

    fn assert_ran(&self, tool: &str) {
        let started = Instant::now();
        while !self.ran(tool) {
            assert!(started.elapsed() < MARKER_DEADLINE, "{tool} never ran");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Opened {
    store: Entity<Store>,
    documents: Entity<Documents>,
    document: Entity<Document>,
}

/// Opens `path` the way `Documents::get` does: register the document, then attach it.
fn open<'a>(
    cx: &'a mut TestAppContext,
    worktree: &Worktree,
    path: &str,
    preview: bool,
    language_tools: bool,
) -> (Opened, &'a mut VisualTestContext) {
    cx.update(gpui::init);
    let request = OpenFile {
        target: worktree.target(),
        path: path.into(),
        position: None,
        preview,
        placement: Default::default(),
    };
    let mut opened = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let store = cx.new(Store::with_held_recovery);
        set_language_tools(&store, language_tools, cx);
        let documents = cx.new(|cx| Documents::new(store.clone(), Default::default(), cx));
        let registry = documents.downgrade();
        let document = cx.new(|cx| {
            let mut document =
                Document::isolated_for_test(request, store.clone(), registry, window, cx);
            document.base = Some(Snapshot::parse(Path::new("source"), b"").unwrap());
            document
        });
        documents.update(cx, |documents, cx| {
            documents.open.push(document.downgrade());
            documents.attach_language(&document, cx);
        });
        opened = Some(Opened {
            store,
            documents,
            document,
        });
        Harness
    });
    cx.run_until_parked();
    (opened.unwrap(), cx)
}

fn set_language_tools(store: &Entity<Store>, enabled: bool, cx: &mut gpui::App) {
    store.update(cx, |store, cx| {
        let mut files = store.state.settings.files.clone();
        files.language_tools = enabled;
        store.set_files_settings(files, cx);
    });
}

fn has_server(opened: &Opened, cx: &VisualTestContext) -> bool {
    cx.read(|cx| {
        let attached = opened.document.read(cx).language.is_some();
        let running = opened
            .documents
            .read(cx)
            .language_sessions
            .iter()
            .any(|(_, _, _, session)| session.upgrade().is_some());
        assert_eq!(attached, running, "a session outlived its documents");
        attached
    })
}

#[gpui::test]
fn opening_a_file_runs_the_worktree_server_only_while_language_tools_are_on(
    cx: &mut TestAppContext,
) {
    let worktree = Worktree::new("server");
    let (opened, cx) = open(cx, &worktree, "a.ts", false, false);
    assert!(!has_server(&opened, cx));
    assert_eq!(
        cx.read(|cx| opened.document.read(cx).language_status(cx)),
        LanguageStatus::Disabled
    );
    assert!(!worktree.ran(SERVER) && !worktree.ran(FORMATTER));

    cx.update(|_, cx| set_language_tools(&opened.store, true, cx));
    cx.run_until_parked();
    assert!(has_server(&opened, cx));
    worktree.assert_ran(SERVER);

    // Turning tools off again lets go of the session, which stops its server.
    cx.update(|_, cx| set_language_tools(&opened.store, false, cx));
    cx.run_until_parked();
    assert!(!has_server(&opened, cx));
}

#[gpui::test]
fn a_preview_tab_starts_no_server_until_the_user_keeps_it(cx: &mut TestAppContext) {
    let worktree = Worktree::new("preview");
    let (opened, cx) = open(cx, &worktree, "a.ts", true, true);
    assert!(!has_server(&opened, cx));
    assert_eq!(
        cx.read(|cx| opened.document.read(cx).language_status(cx)),
        LanguageStatus::Preview
    );
    assert!(!worktree.ran(SERVER) && !worktree.ran(FORMATTER));

    opened.document.update(cx, |document, cx| document.pin(cx));
    cx.run_until_parked();
    assert!(has_server(&opened, cx));
    worktree.assert_ran(SERVER);
}

#[gpui::test]
fn formatting_runs_the_worktree_formatter_only_while_language_tools_are_on(
    cx: &mut TestAppContext,
) {
    let worktree = Worktree::new("formatter");
    let (opened, cx) = open(cx, &worktree, "a.ts", false, false);
    opened
        .document
        .update_in(cx, |document, window, cx| document.format(window, cx));
    cx.run_until_parked();
    cx.read(|cx| {
        let document = opened.document.read(cx);
        assert!(!document.formatting);
        assert!(
            document
                .format_error
                .as_deref()
                .is_some_and(|error| error.contains("Settings → Files")),
            "the user is told why nothing was formatted"
        );
    });
    assert!(!worktree.ran(SERVER) && !worktree.ran(FORMATTER));

    cx.update(|_, cx| set_language_tools(&opened.store, true, cx));
    opened
        .document
        .update_in(cx, |document, window, cx| document.format(window, cx));
    cx.run_until_parked();
    worktree.assert_ran(FORMATTER);
    cx.read(|cx| assert!(opened.document.read(cx).format_error.is_none()));
}
