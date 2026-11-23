//! Every cancellable close route enters here before mutating layout or project state.
//!
//! The guard takes the unsaved documents of its scope one at a time: it saves the ones that
//! save on their own, and asks about the rest. A document that will not save is asked about
//! again with the reason, so a close never ends without the user knowing why.
use super::*;
use crate::state::documents::{Documents, OpenFile, SaveBlocker};
use gpui_kit::component::WindowExt as _;
use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

#[derive(Clone)]
pub(crate) enum Scope {
    Document(Entity<Document>),
    Project(hive_core::host::Host, std::path::PathBuf),
    Worktree(hive_core::host::Host, std::path::PathBuf),
    All,
}

impl Scope {
    fn dirty(&self, documents: &Entity<Documents>, cx: &App) -> Option<Entity<Document>> {
        if let Self::Document(document) = self {
            return document.read(cx).dirty.then(|| document.clone());
        }
        documents.read(cx).dirty(cx).into_iter().find(|document| {
            let target = &document.read(cx).target;
            match self {
                Self::Project(host, path) => &target.host == host && &target.project == path,
                Self::Worktree(host, path) => &target.host == host && &target.path == path,
                Self::All => true,
                Self::Document(_) => false,
            }
        })
    }
}

/// What the guard asks about one unsaved document. Discard and Cancel are always offered;
/// `safe` is the answer that loses nothing.
#[derive(Debug, PartialEq, Eq)]
struct Question {
    title: String,
    detail: String,
    safe: Safe,
}

/// The answer Enter takes. It never throws the buffer or the version on disk away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Safe {
    /// Write the buffer.
    Save,
    /// Write it again after a failure the user may have fixed meanwhile.
    Retry,
    /// Keep everything open and compare the buffer with the version on disk.
    Review,
}

impl Safe {
    fn label(self) -> &'static str {
        match self {
            Self::Save => "Save",
            Self::Retry => "Try Again",
            Self::Review => "Review Changes",
        }
    }
}

impl Question {
    /// A conflicted document is never offered a save: only an explicit overwrite, made while
    /// looking at both versions, replaces what another writer put on disk.
    fn about(path: &Path, blocker: Option<&SaveBlocker>) -> Self {
        let path = path.display();
        match blocker {
            None => Self {
                title: format!("Save changes to {path}?"),
                detail: "Your changes have not been saved to the project file.".into(),
                safe: Safe::Save,
            },
            Some(SaveBlocker::Conflict) => Self {
                title: format!("{path} changed on disk"),
                detail: "The file was changed outside the editor after you started editing it, \
                         so saving would replace that version. Review both versions, or discard \
                         your changes."
                    .into(),
                safe: Safe::Review,
            },
            Some(SaveBlocker::Failed(reason)) => Self {
                title: format!("Could not save {path}"),
                detail: reason.clone(),
                safe: Safe::Retry,
            },
        }
    }
}

type Complete = Box<dyn FnOnce(bool, &mut Window, &mut App)>;

/// One close being decided: the documents it covers and who is waiting for the answer.
#[derive(Clone)]
struct Closing {
    documents: Entity<Documents>,
    scope: Scope,
    /// Taken by the first answer. Dropped unanswered when the window goes away, which the
    /// quit request it may hold treats as "no".
    complete: Rc<Cell<Option<Complete>>>,
}

impl Closing {
    fn finish(&self, approved: bool, window: &mut Window, cx: &mut App) {
        if let Some(complete) = self.complete.take() {
            complete(approved, window, cx);
        }
    }
}

pub(crate) fn guard(
    scope: Scope,
    window: &mut Window,
    cx: &mut App,
    complete: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    guard_result(scope, window, cx, move |approved, window, cx| {
        if approved {
            complete(window, cx);
        }
    });
}

fn guard_result(
    scope: Scope,
    window: &mut Window,
    cx: &mut App,
    complete: impl FnOnce(bool, &mut Window, &mut App) + 'static,
) {
    let documents = AppState::get(cx).documents.clone();
    begin(documents, scope, window, cx, complete);
}

fn begin(
    documents: Entity<Documents>,
    scope: Scope,
    window: &mut Window,
    cx: &mut App,
    complete: impl FnOnce(bool, &mut Window, &mut App) + 'static,
) {
    let closing = Closing {
        documents,
        scope,
        complete: Rc::new(Cell::new(Some(Box::new(complete)))),
    };
    advance(closing, window, cx);
}

/// Saves or asks about the next unsaved document, and approves the close once none is left.
fn advance(closing: Closing, window: &mut Window, cx: &mut App) {
    if !closing.documents.read(cx).mutating_paths.is_empty() {
        use gpui_kit::component::notification::Notification;
        window.push_notification(
            Notification::info("Wait for the file operation to finish before closing."),
            cx,
        );
        closing.finish(false, window, cx);
        return;
    }
    // Re-read the whole scope after each async save: another buffer may have changed meanwhile.
    let Some(document) = closing.scope.dirty(&closing.documents, cx) else {
        window.defer(cx, move |window, cx| closing.finish(true, window, cx));
        return;
    };
    if document.read(cx).saves_on_focus_change(cx) {
        save(closing, document, window, cx);
    } else {
        ask(closing, document, window, cx);
    }
}

/// Writes `document` and goes on with the close, or asks about the document that would not
/// save: a conflict or a failed write still needs the user's choice, and its reason.
fn save(closing: Closing, document: Entity<Document>, window: &mut Window, cx: &mut App) {
    let saved = document.update(cx, |d, cx| d.save(None, window, cx));
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        let saved = saved.recv().await == Ok(true);
        let _ = handle.update(cx, |_, window, cx| {
            if saved {
                advance(closing, window, cx);
            } else {
                ask(closing, document, window, cx);
            }
        });
    })
    .detach();
}

/// Brings `document` to the front and asks what to do with its changes.
fn ask(closing: Closing, document: Entity<Document>, window: &mut Window, cx: &mut App) {
    show(&closing.documents, &document, cx);
    // The tab takes the focus when it comes to the front; the question opens after that.
    window.defer(cx, move |window, cx| {
        let question = {
            let document = document.read(cx);
            Question::about(&document.path, document.save_blocker().as_ref())
        };
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (safe, safely) = (question.safe, (closing.clone(), document.clone()));
            let discarding = (closing.clone(), document.clone());
            let cancelling = closing.clone();
            crate::ui::destructive::choice(
                alert
                    .confirm()
                    .title(question.title.clone())
                    .description(question.detail.clone()),
                (safe.label(), move |window: &mut Window, cx: &mut App| {
                    let (closing, document) = safely.clone();
                    // The next question, if there is one, opens once this one has closed.
                    window.defer(cx, move |window, cx| match safe {
                        Safe::Save | Safe::Retry => save(closing, document, window, cx),
                        Safe::Review => {
                            closing.finish(false, window, cx);
                            actions::resolve_conflict(&document, window, cx);
                        }
                    });
                    true
                }),
                ("Discard", move |window: &mut Window, cx: &mut App| {
                    let (closing, document) = discarding.clone();
                    document.update(cx, |d, cx| d.discard(window, cx));
                    window.defer(cx, move |window, cx| advance(closing, window, cx));
                    true
                }),
                move |window, cx| cancelling.finish(false, window, cx),
            )
        });
    });
}

/// Selects the tab of `document`, so the question is about what the user is looking at.
fn show(documents: &Entity<Documents>, document: &Entity<Document>, cx: &mut App) {
    let request = {
        let document = document.read(cx);
        OpenFile {
            target: document.target.clone(),
            path: document.path.clone(),
            position: None,
            preview: false,
            placement: Default::default(),
        }
    };
    documents.update(cx, |documents, cx| documents.request(request, cx));
}

pub(crate) fn all(
    window: &mut Window,
    cx: &mut App,
    complete: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    guard(Scope::All, window, cx, complete);
}

pub(crate) fn quit(request: crate::quit::Request, window: &mut Window, cx: &mut App) {
    guard_result(Scope::All, window, cx, move |approved, _, cx| {
        request.reply(approved, cx)
    });
}

pub(crate) fn recovered(
    record: hive_core::files::recovery::Record,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui_kit::component::input::EditorState;
    let editor = cx.new(|cx| EditorState::new(window, cx).default_value(record.draft.text.clone()));
    window.open_alert_dialog(cx, move |dialog, window, cx| {
        let (copy, key) = (record.draft.text.clone(), record.key.clone());
        let [copy_hover, delete_hover] =
            ["recovery-copy", "recovery-delete"].map(|id| Hover::new(id, window, cx));
        dialog
            .title(format!("Recovered draft · {}", record.draft.path.display()))
            .child(format!(
                "Project unavailable: {}",
                record.draft.project.display()
            ))
            .child(Editor::new(&editor).readonly(true).h(px(350.)))
            .child(
                Button::new("recovery-copy")
                    .cursor_pointer()
                    .label("Copy")
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                    })
                    .smooth_hover(ButtonLook::Ghost, &copy_hover, cx),
            )
            .child(
                Button::new("recovery-delete")
                    .cursor_pointer()
                    .label("Delete draft")
                    .on_click(move |_, window, cx| {
                        AppState::get(cx)
                            .store
                            .read(cx)
                            .save_recovery(key.clone(), None);
                        AppState::get(cx)
                            .documents
                            .clone()
                            .update(cx, |documents, _| {
                                documents.recovered.retain(|r| r.key != key)
                            });
                        window.close_dialog(cx);
                    })
                    .smooth_hover(ButtonLook::Outline, &delete_hover, cx),
            )
    });
}

#[cfg(test)]
mod tests {
    use super::{Question, Safe, Scope, begin};
    use crate::state::documents::{Document, Documents, OpenFile, SaveBlocker};
    use crate::state::store::Store;
    use crate::ui::destructive::testing::{is_open, press, settle, window};
    use gpui_kit as gpui;
    use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};
    use hive_core::{files::Snapshot, host::Host, projects::Target};
    use std::cell::Cell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn a_conflicted_document_is_offered_a_review_and_never_a_save() {
        let path = Path::new("src/main.rs");
        let plain = Question::about(path, None);
        assert_eq!(plain.safe, Safe::Save);
        assert_eq!(plain.title, "Save changes to src/main.rs?");

        let conflict = Question::about(path, Some(&SaveBlocker::Conflict));
        assert_eq!(conflict.safe, Safe::Review);
        assert_eq!(conflict.title, "src/main.rs changed on disk");
        assert!(conflict.detail.contains("changed outside the editor"));

        let reason = "Cannot write src/main.rs: Permission denied";
        let failed = Question::about(path, Some(&SaveBlocker::Failed(reason.into())));
        assert_eq!(failed.safe, Safe::Retry);
        assert_eq!(failed.title, "Could not save src/main.rs");
        assert_eq!(
            failed.detail, reason,
            "the reason is shown as it was reported"
        );
        assert_eq!(
            [Safe::Save, Safe::Retry, Safe::Review].map(Safe::label),
            ["Save", "Try Again", "Review Changes"]
        );
    }

    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// A temporary worktree and the registry of the documents open from it.
    struct Fixture {
        root: PathBuf,
        documents: Entity<Documents>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl Fixture {
        fn new(autosave: bool, cx: &mut VisualTestContext) -> Self {
            let root = std::env::temp_dir().join(format!(
                "hive-close-guard-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).unwrap();
            let documents = cx.update(|_, cx| {
                let store = cx.new(|cx| {
                    let mut store = Store::with_held_recovery(cx);
                    store.state.settings.files.save_on_focus_change = autosave;
                    store
                });
                cx.new(|cx| Documents::new(store, Default::default(), cx))
            });
            Self {
                // Documents name real paths; the temporary folder is behind a link on macOS.
                root: std::fs::canonicalize(root).unwrap(),
                documents,
            }
        }

        /// `name` on disk as `disk`, open in a tab that read `read` and now holds `buffer`.
        fn open(
            &self,
            name: &str,
            [disk, read, buffer]: [&str; 3],
            cx: &mut VisualTestContext,
        ) -> Entity<Document> {
            std::fs::write(self.root.join(name), disk).unwrap();
            let request = OpenFile {
                target: Target {
                    host: Host::Local,
                    project: self.root.clone(),
                    path: self.root.clone(),
                },
                path: name.into(),
                position: None,
                preview: false,
                placement: Default::default(),
            };
            let base = Snapshot::parse(Path::new(name), read.as_bytes()).unwrap();
            cx.update(|window, cx| {
                self.documents.update(cx, |documents, cx| {
                    documents.open_edited_for_test(request, base, buffer, window, cx)
                })
            })
        }

        /// Starts a close of `scope` and returns where its answer will appear.
        fn close(&self, scope: Scope, cx: &mut VisualTestContext) -> Rc<Cell<Option<bool>>> {
            let answer = Rc::new(Cell::new(None));
            let answered = answer.clone();
            let documents = self.documents.clone();
            cx.update(|window, cx| {
                begin(documents, scope, window, cx, move |approved, _, _| {
                    answered.set(Some(approved))
                })
            });
            settle(cx);
            answer
        }

        fn disk(&self, name: &str) -> String {
            std::fs::read_to_string(self.root.join(name)).unwrap()
        }
    }

    /// Reaches the red button (Tab, Tab) and presses it.
    fn discard(cx: &mut VisualTestContext) {
        cx.simulate_keystrokes("tab tab");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        press("space", cx);
        settle(cx);
    }

    fn is_dirty(document: &Entity<Document>, cx: &mut VisualTestContext) -> bool {
        cx.read(|cx| document.read(cx).dirty)
    }

    #[gpui::test]
    fn clean_and_saveable_documents_close_without_a_question(cx: &mut TestAppContext) {
        let cx = window(cx);
        let fixture = Fixture::new(true, cx);
        let document = fixture.open("a.txt", ["old\n", "old\n", "mine\n"], cx);

        let answer = fixture.close(Scope::All, cx);

        assert_eq!(answer.get(), Some(true));
        assert!(!is_open(cx));
        assert_eq!(fixture.disk("a.txt"), "mine\n");
        assert!(!is_dirty(&document, cx));
    }

    #[gpui::test]
    fn a_conflicted_document_is_asked_about_and_reviewing_it_cancels_the_close(
        cx: &mut TestAppContext,
    ) {
        let cx = window(cx);
        let fixture = Fixture::new(true, cx);
        // An agent rewrote the file; the editor has noticed and holds both versions.
        let document = fixture.open("a.txt", ["theirs\n", "old\n", "mine\n"], cx);
        cx.update(|window, cx| document.update(cx, |d, cx| d.refresh(window, cx)));
        cx.run_until_parked();
        assert_eq!(
            cx.read(|cx| document.read(cx).save_blocker()),
            Some(SaveBlocker::Conflict)
        );

        let answer = fixture.close(Scope::Document(document.clone()), cx);

        assert_eq!(answer.get(), None, "the close waits for an answer");
        assert!(is_open(cx), "the conflict is asked about, not saved over");
        assert_eq!(fixture.disk("a.txt"), "theirs\n");

        // Enter takes the safe answer, which for a conflict is to look at both versions.
        cx.simulate_keystrokes("enter");
        settle(cx);
        assert_eq!(answer.get(), Some(false));
        assert!(is_open(cx), "the two versions are shown side by side");
        assert_eq!(fixture.disk("a.txt"), "theirs\n");
        assert!(is_dirty(&document, cx), "the buffer is kept");
    }

    #[gpui::test]
    fn a_save_that_fails_asks_again_with_the_reason_instead_of_ending_the_close(
        cx: &mut TestAppContext,
    ) {
        let cx = window(cx);
        let fixture = Fixture::new(false, cx);
        // The file changed on disk and the editor has not read it since.
        let document = fixture.open("a.txt", ["theirs\n", "old\n", "mine\n"], cx);

        let answer = fixture.close(Scope::Document(document.clone()), cx);
        assert!(is_open(cx));
        cx.simulate_keystrokes("enter");
        settle(cx);

        assert_eq!(
            answer.get(),
            None,
            "a failed save is not an answer: the close neither ends nor goes on"
        );
        assert!(is_open(cx), "the document is asked about again");
        assert_eq!(fixture.disk("a.txt"), "theirs\n");
        assert!(is_dirty(&document, cx));
        assert!(cx.read(|cx| document.read(cx).save_blocker()).is_some());

        discard(cx);
        assert_eq!(answer.get(), Some(true));
        assert!(!is_dirty(&document, cx));
        assert_eq!(fixture.disk("a.txt"), "theirs\n", "discarding never writes");
    }

    #[gpui::test]
    fn every_unsaved_document_is_settled_before_a_window_or_the_app_closes(
        cx: &mut TestAppContext,
    ) {
        let cx = window(cx);
        let fixture = Fixture::new(true, cx);
        let first = fixture.open("a.txt", ["old a\n", "old a\n", "mine a\n"], cx);
        let stuck = fixture.open("b.txt", ["theirs\n", "old b\n", "mine b\n"], cx);
        let last = fixture.open("c.txt", ["old c\n", "old c\n", "mine c\n"], cx);

        let answer = fixture.close(Scope::All, cx);

        // The first document saved on its own; the second could not and is asked about.
        assert_eq!(fixture.disk("a.txt"), "mine a\n");
        assert!(!is_dirty(&first, cx));
        assert_eq!(answer.get(), None);
        assert!(is_open(cx));
        assert_eq!(fixture.disk("b.txt"), "theirs\n");
        assert_eq!(
            fixture.disk("c.txt"),
            "old c\n",
            "the rest waits for the answer"
        );

        discard(cx);
        assert_eq!(answer.get(), Some(true));
        assert!(!is_open(cx));
        assert!(!is_dirty(&stuck, cx) && !is_dirty(&last, cx));
        assert_eq!(fixture.disk("b.txt"), "theirs\n");
        assert_eq!(fixture.disk("c.txt"), "mine c\n");
    }

    #[gpui::test]
    fn dismissing_the_question_refuses_the_close_and_keeps_the_buffer(cx: &mut TestAppContext) {
        let cx = window(cx);
        let fixture = Fixture::new(false, cx);
        let document = fixture.open("a.txt", ["old\n", "old\n", "mine\n"], cx);

        let answer = fixture.close(Scope::All, cx);
        assert_eq!(answer.get(), None);
        cx.simulate_keystrokes("escape");
        settle(cx);

        assert_eq!(answer.get(), Some(false));
        assert!(!is_open(cx));
        assert!(is_dirty(&document, cx));
        assert_eq!(cx.read(|cx| document.read(cx).text(cx)), "mine\n");
        assert_eq!(fixture.disk("a.txt"), "old\n");
    }
}
