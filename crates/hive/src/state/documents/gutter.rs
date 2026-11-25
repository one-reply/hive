//! An immutable committed blob is reused as the user types. Only visible changed files need a diff.
use super::*;
use crate::state::git::{Git, Tracking};
use gpui_kit::component::input::Rope;
use hive_core::git::gutter::{self, Mark};

#[derive(Clone, PartialEq)]
struct Baseline {
    target: Target,
    path: PathBuf,
    oid: Option<String>,
}

pub(crate) struct Gutter {
    doc: Entity<Document>,
    git: Entity<Git>,
    baseline: Option<Baseline>,
    cached: Option<Arc<String>>,
    text: Option<Rope>,
    pub marks: Vec<Mark>,
    task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}
impl Gutter {
    pub(crate) fn new(doc: Entity<Document>, git: Entity<Git>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&doc, |this, _, cx| this.refresh(cx)),
            cx.observe(&git, |this, _, cx| this.refresh(cx)),
        ];
        let mut this = Self {
            doc,
            git,
            baseline: None,
            cached: None,
            text: None,
            marks: Vec::new(),
            task: None,
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    fn source(&self, cx: &App) -> Option<(Baseline, Rope)> {
        let doc = self.doc.read(cx);
        if !doc.visible || doc.base.is_none() || doc.content.shows_image() {
            return None;
        }
        let Tracking::Status(status) = self.git.read(cx).tracking(Some(&doc.target)) else {
            return None;
        };
        let entry = status
            .files
            .iter()
            .find(|f| std::path::Path::new(&f.path) == doc.path);
        if entry.is_none() && !doc.dirty {
            return None;
        }
        let path = entry
            .and_then(|f| f.old_path.as_ref())
            .map(PathBuf::from)
            .unwrap_or_else(|| doc.path.clone());
        Some((
            Baseline {
                target: doc.target.clone(),
                path,
                oid: status.oid.clone(),
            },
            doc.editor.read(cx).text().clone(),
        ))
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some((base, text)) = self.source(cx) else {
            self.task = None;
            self.text = None;
            if !self.marks.is_empty() {
                self.marks.clear();
                cx.notify();
            }
            return;
        };
        let same = self.baseline.as_ref() == Some(&base);
        if same && self.text.as_ref() == Some(&text) {
            return;
        }
        if !same {
            self.cached = None;
            if !self.marks.is_empty() {
                self.marks.clear();
                cx.notify();
            }
        }
        self.baseline = Some(base.clone());
        // GPUI tracks existing gutter ranges through typing until the replacement diff arrives.
        self.text = Some(text.clone());
        let cached = self.cached.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(200))
                .await;
            let work_text = text.clone();
            let work_base = base.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let original = match (cached, &work_base.oid) {
                        (Some(cached), _) => cached,
                        (_, Some(oid)) => Arc::new(gutter::baseline(
                            &work_base.target.host,
                            &work_base.target.path,
                            oid,
                            &work_base.path,
                        )?),
                        (_, None) => Arc::new(String::new()),
                    };
                    let marks = gutter::marks(&original, &work_text.to_string());
                    Ok::<_, hive_core::git::GitError>((original, marks))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.baseline.as_ref() != Some(&base) || this.text.as_ref() != Some(&text) {
                    return;
                }
                match result {
                    Ok((source, marks)) => {
                        this.cached = Some(source);
                        this.marks = marks;
                        cx.notify();
                    }
                    Err(e) => tracing::debug!(%e,"Git gutter unavailable"),
                }
            });
        }));
    }
}
