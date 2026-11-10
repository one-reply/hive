//! Loads an unmerged working file off-thread and writes its resolution back. The widget that
//! shows the conflicts and holds the user's choices belongs to the panel.
use super::file_io;
use super::git::{ConflictRequest, Git, GitEvent};
use gpui_kit::component::diff::DiffFile;
use gpui_kit::*;
use hive_core::files::{Access, FileError, Snapshot};
use hive_core::git::changes::Action;
use std::path::Path;

/// What the working tree holds for an unmerged path.
#[derive(Clone)]
pub(crate) enum Content {
    /// A text file and the conflicts its markers delimit; none once the markers are gone.
    Text { snapshot: Snapshot, file: DiffFile },
    /// Markers that do not pair up, which only editing the file can repair.
    Malformed(String),
    /// Nothing Hive can show as text: a binary, oversized or missing file.
    Unavailable(String),
}

impl Content {
    fn read(path: &str, result: Result<Snapshot, FileError>) -> Self {
        match result {
            Ok(snapshot) => Self::parse(path, snapshot),
            Err(error) => Self::Unavailable(error.to_string()),
        }
    }

    fn parse(path: &str, snapshot: Snapshot) -> Self {
        let text = snapshot.text();
        match DiffFile::parse_conflicts(path.to_owned(), &text) {
            // Without markers the file is shown whole, for a last look before staging.
            Ok(file) if file.conflicts().is_empty() => Self::Text {
                file: DiffFile::unchanged(path.to_owned(), &text),
                snapshot,
            },
            Ok(file) => Self::Text { snapshot, file },
            Err(error) => Self::Malformed(error.to_string()),
        }
    }

    /// Whether a later read found the same thing, so the widget can keep the user's choices.
    fn is_unchanged_by(&self, read: &Result<Snapshot, FileError>) -> bool {
        match (self, read) {
            (Self::Text { snapshot, .. }, Ok(new)) => snapshot == new,
            (Self::Unavailable(message), Err(error)) => *message == error.to_string(),
            _ => false,
        }
    }
}

pub(crate) struct GitConflict {
    pub request: ConflictRequest,
    content: Option<Content>,
    /// Changes whenever `content` does, so a panel can skip reloading its widget.
    generation: u64,
    /// Why the last resolution was not written.
    pub error: Option<String>,
    pub loading: bool,
    saving: bool,
    /// Whether a panel shows this file; only an active tab follows Git refreshes.
    active: bool,
    /// A refresh requested while reading or writing; served when that finishes.
    pending: bool,
    git: Entity<Git>,
    _load: Option<Task<()>>,
    _save: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl GitConflict {
    pub fn new(request: ConflictRequest, git: Entity<Git>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.subscribe(&git, |this, git, event, cx| {
            // The file is read on every refresh: an editor or an agent may have rewritten it.
            if let GitEvent::Refreshed { target, .. } = event
                && this.active
                && git.read(cx).window_active
                && target == &this.request.target
            {
                this.refresh(cx);
            }
        })];
        let mut this = Self {
            request,
            content: None,
            generation: 0,
            error: None,
            loading: false,
            saving: false,
            active: false,
            pending: false,
            git,
            _load: None,
            _save: None,
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    pub fn content(&self) -> Option<&Content> {
        self.content.as_ref()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn is_saving(&self) -> bool {
        self.saving
    }

    pub fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.saving {
            self.pending = true;
            return;
        }
        self.loading = true;
        let request = self.request.clone();
        let previous = self.content.clone();
        let executor = cx.background_executor().clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            let content = file_io::run(executor, move |cancellation| {
                let read = Access::new(&request.target.host, cancellation)
                    .read(&request.target.path, Path::new(&request.path));
                if previous.is_some_and(|content| content.is_unchanged_by(&read)) {
                    return None;
                }
                Some(Content::read(&request.path, read))
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                if let Some(content) = content {
                    this.set_content(content);
                }
                cx.notify();
                this.serve_pending(cx);
            });
        }));
        cx.notify();
    }

    /// Writes `text` over the file as it was loaded, then stages it. A file that changed on
    /// disk in the meantime is left alone and reloaded.
    pub fn apply(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(Content::Text { snapshot, .. }) = self.content.clone() else {
            return;
        };
        if self.saving {
            return;
        }
        self.saving = true;
        self.error = None;
        let request = self.request.clone();
        let executor = cx.background_executor().clone();
        self._save = Some(cx.spawn(async move |this, cx| {
            let saved = file_io::run(executor, move |cancellation| {
                Access::new(&request.target.host, cancellation)
                    .save(
                        &request.target.path,
                        Path::new(&request.path),
                        &snapshot,
                        &text,
                    )
                    .map(|snapshot| Content::parse(&request.path, snapshot))
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                match saved {
                    Ok(content) => {
                        this.set_content(content);
                        this.mark_resolved(cx);
                    }
                    Err(error) => {
                        tracing::warn!(path = %this.request.path, %error, "conflict resolution was not written");
                        this.error = Some(match error {
                            FileError::Conflict(_) => "The file changed on disk after it was loaded, so nothing was written. Review the reloaded conflicts and resolve them again.".into(),
                            error => error.to_string(),
                        });
                        this.pending = true;
                    }
                }
                cx.notify();
                this.serve_pending(cx);
            });
        }));
        cx.notify();
    }

    /// Stages the working file as it is, which is how Git records a conflict as resolved.
    pub fn mark_resolved(&mut self, cx: &mut Context<Self>) {
        let target = self.request.target.clone();
        let action = Action::Stage(vec![self.request.path.clone()]);
        self.git
            .update(cx, |git, cx| git.perform_at(target, action, cx));
    }

    fn set_content(&mut self, content: Content) {
        self.content = Some(content);
        self.generation += 1;
    }

    fn serve_pending(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.pending) {
            self.refresh(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Content, GitConflict};
    use crate::state::{
        git::{ConflictRequest, Git},
        projects::Projects,
        store::Store,
    };
    use gpui_kit as gpui;
    use gpui_kit::{AppContext, Entity, TestAppContext};
    use hive_core::files::{FileError, Snapshot};
    use hive_core::{host::Host, projects::Target, state::StateError};
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    /// A repository where merging `feature` into `main` left `a.txt` unmerged.
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    impl Fixture {
        fn new(base: &str, ours: &str, theirs: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "hive-conflict-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            let fixture = Self(path.canonicalize().unwrap());
            fixture.git(&["init", "-q", "-b", "main", "--template="]);
            fixture.commit(base);
            fixture.git(&["checkout", "-q", "-b", "feature"]);
            fixture.commit(theirs);
            fixture.git(&["checkout", "-q", "main"]);
            fixture.commit(ours);
            assert!(!fixture.run(&["merge", "feature"]).status.success());
            fixture
        }
        fn run(&self, args: &[&str]) -> std::process::Output {
            Command::new("git")
                .args([
                    "-c",
                    "user.name=Hive Test",
                    "-c",
                    "user.email=hive@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.autocrlf=false",
                ])
                .args(args)
                .current_dir(&self.0)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap()
        }
        fn git(&self, args: &[&str]) -> String {
            let output = self.run(args);
            assert!(output.status.success(), "git {args:?}: {output:?}");
            String::from_utf8(output.stdout).unwrap()
        }
        fn commit(&self, content: &str) {
            std::fs::write(self.0.join("a.txt"), content).unwrap();
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", "fixture"]);
        }
        fn read(&self) -> String {
            std::fs::read_to_string(self.0.join("a.txt")).unwrap()
        }
        fn request(&self) -> ConflictRequest {
            ConflictRequest {
                target: Target {
                    host: Host::Local,
                    project: self.0.clone(),
                    path: self.0.clone(),
                },
                path: "a.txt".into(),
            }
        }
    }

    fn git(cx: &mut TestAppContext) -> Entity<Git> {
        let store = cx.new(|cx| {
            Store::new(
                Err(StateError::Io {
                    operation: "test",
                    path: "/unused".into(),
                    source: std::io::Error::other("persistence disabled for test"),
                }),
                cx,
            )
        });
        let projects = cx.new(|cx| Projects::new(&store, cx));
        cx.new(|cx| Git::new(projects, cx))
    }

    fn conflicts(content: Option<&Content>) -> usize {
        match content {
            Some(Content::Text { file, .. }) => file.conflicts().len(),
            other => panic!("expected text, found {}", describe(other)),
        }
    }

    fn describe(content: Option<&Content>) -> &'static str {
        match content {
            None => "nothing",
            Some(Content::Text { .. }) => "text",
            Some(Content::Malformed(_)) => "malformed markers",
            Some(Content::Unavailable(_)) => "unavailable",
        }
    }

    fn snapshot(text: &str) -> Snapshot {
        Snapshot::parse(Path::new("a.txt"), text.as_bytes()).unwrap()
    }

    #[test]
    fn markers_decide_how_a_working_file_is_shown() {
        // Captured from Git 2.54 with the default and `merge.conflictStyle=zdiff3` styles.
        let two_way = "top\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> feature\nend\n";
        let diff3 = "<<<<<<< HEAD\nours\n||||||| 1a2b3c4\nbase\n=======\ntheirs\n>>>>>>> feature\n";
        assert_eq!(
            conflicts(Some(&Content::parse("a.txt", snapshot(two_way)))),
            1
        );
        assert_eq!(
            conflicts(Some(&Content::parse("a.txt", snapshot(diff3)))),
            1
        );
        // A resolved file is still text, with nothing left to choose.
        assert_eq!(
            conflicts(Some(&Content::parse("a.txt", snapshot("plain\n")))),
            0
        );
        assert_eq!(conflicts(Some(&Content::parse("a.txt", snapshot("")))), 0);
        for broken in [
            "<<<<<<< HEAD\nours\n",
            "<<<<<<< HEAD\n<<<<<<< HEAD\n",
            ">>>>>>> feature\n",
        ] {
            let content = Content::parse("a.txt", snapshot(broken));
            assert_eq!(describe(Some(&content)), "malformed markers", "{broken:?}");
        }
        let binary = Snapshot::parse(Path::new("a.bin"), b"\0\x01");
        let content = Content::read("a.bin", binary);
        assert_eq!(describe(Some(&content)), "unavailable");
    }

    #[test]
    fn an_identical_reread_keeps_the_loaded_content() {
        let text = "<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> feature\n";
        let content = Content::parse("a.txt", snapshot(text));
        assert!(content.is_unchanged_by(&Ok(snapshot(text))));
        assert!(!content.is_unchanged_by(&Ok(snapshot("resolved\n"))));
        let missing = || FileError::Conflict("a.txt".into());
        assert!(!content.is_unchanged_by(&Err(missing())));
        let unavailable = Content::read("a.txt", Err(missing()));
        assert!(unavailable.is_unchanged_by(&Err(missing())));
        assert!(!unavailable.is_unchanged_by(&Ok(snapshot(text))));
    }

    #[gpui::test]
    fn apply_writes_the_resolution_in_the_files_own_line_endings_and_stages_it(
        cx: &mut TestAppContext,
    ) {
        let fixture = Fixture::new("base\r\n", "ours\r\n", "theirs\r\n");
        let git = git(cx);
        let conflict = cx.new(|cx| GitConflict::new(fixture.request(), git, cx));
        cx.run_until_parked();
        conflict.update(cx, |conflict, cx| {
            assert_eq!(conflicts(conflict.content()), 1);
            let generation = conflict.generation();
            // The widget hands back LF text for a CRLF file.
            conflict.apply("ours\ntheirs\n".into(), cx);
            assert!(conflict.is_saving());
            assert_eq!(conflict.generation(), generation);
        });
        cx.run_until_parked();
        assert_eq!(fixture.read(), "ours\r\ntheirs\r\n");
        assert_eq!(fixture.git(&["status", "--porcelain"]), "M  a.txt\n");
        conflict.update(cx, |conflict, _| {
            assert!(!conflict.is_saving());
            assert_eq!(conflict.error, None);
            assert_eq!(conflicts(conflict.content()), 0);
        });
    }

    #[gpui::test]
    fn widget_choices_become_the_text_that_is_written(cx: &mut TestAppContext) {
        use gpui_kit::component::diff::{DiffConflictResolution, DiffState};
        cx.update(gpui_kit::init);
        // A CRLF file with a BOM, as Git leaves it after a conflicting merge.
        let raw = "\u{feff}top\r\n<<<<<<< HEAD\r\nours\r\n=======\r\ntheirs\r\n>>>>>>> feature\r\nmiddle\r\n<<<<<<< HEAD\r\nkeep\r\n=======\r\ndrop\r\n>>>>>>> feature\r\n";
        let Content::Text { snapshot, file } = Content::parse("a.txt", snapshot(raw)) else {
            panic!("expected text");
        };
        let display = cx.new(|cx| DiffState::new([file], cx));
        display.update(cx, |display, cx| {
            display.resolve_conflict("a.txt", 0, Some(DiffConflictResolution::Both), cx);
            // One undecided conflict leaves nothing to write.
            assert_eq!(display.resolved_text("a.txt"), None);
            display.resolve_conflict("a.txt", 1, Some(DiffConflictResolution::Current), cx);
            let text = display
                .resolved_text("a.txt")
                .expect("every conflict decided");
            assert_eq!(text, "top\nours\ntheirs\nmiddle\nkeep\n");
            assert_eq!(
                snapshot.encode(&text),
                "\u{feff}top\r\nours\r\ntheirs\r\nmiddle\r\nkeep\r\n"
            );
            // Undo makes the file undecided again.
            display.resolve_conflict("a.txt", 1, None, cx);
            assert_eq!(display.resolved_text("a.txt"), None);
        });
    }

    #[gpui::test]
    fn a_file_changed_after_loading_is_not_overwritten(cx: &mut TestAppContext) {
        let fixture = Fixture::new("base\n", "ours\n", "theirs\n");
        let git = git(cx);
        let conflict = cx.new(|cx| GitConflict::new(fixture.request(), git, cx));
        cx.run_until_parked();
        let edited = "<<<<<<< HEAD\nours, edited elsewhere\n=======\ntheirs\n>>>>>>> feature\n";
        std::fs::write(fixture.0.join("a.txt"), edited).unwrap();
        conflict.update(cx, |conflict, cx| conflict.apply("ours\n".into(), cx));
        cx.run_until_parked();
        assert_eq!(fixture.read(), edited);
        assert_eq!(fixture.git(&["status", "--porcelain"]), "UU a.txt\n");
        conflict.update(cx, |conflict, _| {
            assert!(
                conflict
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("changed on disk")
            );
            // The reload shows what is on disk now, ready to be resolved again.
            match conflict.content() {
                Some(Content::Text { snapshot, .. }) => assert_eq!(snapshot.text(), edited),
                other => panic!("expected text, found {}", describe(other)),
            }
        });
    }

    #[gpui::test]
    fn marking_resolved_stages_a_file_edited_by_hand(cx: &mut TestAppContext) {
        let fixture = Fixture::new("base\n", "ours\n", "theirs\n");
        std::fs::write(fixture.0.join("a.txt"), "merged by hand\n").unwrap();
        let git = git(cx);
        let conflict = cx.new(|cx| GitConflict::new(fixture.request(), git, cx));
        cx.run_until_parked();
        conflict.update(cx, |conflict, cx| {
            assert_eq!(conflicts(conflict.content()), 0);
            conflict.mark_resolved(cx);
        });
        cx.run_until_parked();
        assert_eq!(fixture.read(), "merged by hand\n");
        assert_eq!(fixture.git(&["status", "--porcelain"]), "M  a.txt\n");
    }
}
