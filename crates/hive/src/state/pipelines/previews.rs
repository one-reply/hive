//! What the launch form shows of a repository at a base ref: the commit a run would start
//! from, its branches and whether the checkout has local changes. Git is read in the
//! background, and an answer is kept only while no newer read of the same thing has been
//! started, so a slow read cannot put back what a later one replaced.
use super::*;
use workspace::BasePreview;

/// One thing to read: a registered project, the checkout whose local changes matter, and the
/// base ref.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PreviewKey {
    pub(crate) project: PathBuf,
    pub(crate) checkout: PathBuf,
    pub(crate) reference: String,
}

/// Where the read of one base stands.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Preview<'a> {
    /// No answer yet. A base that was read before keeps showing that answer instead.
    Reading,
    /// Why the last read failed.
    Failed(&'a str),
    Ready(&'a BasePreview),
}

#[derive(Default)]
struct Slot {
    answer: Option<Result<BasePreview, String>>,
    /// Stamp of the read in flight; an answer carrying another stamp is dropped.
    reading: Option<u64>,
}

#[derive(Default)]
pub(super) struct Previews {
    slots: HashMap<PreviewKey, Slot>,
}

impl Previews {
    /// Starts read `job`, which replaces one still in flight. A good answer stays on screen
    /// until the new one arrives; a failure is forgotten, so a retry shows as reading.
    fn begin(&mut self, key: PreviewKey, job: u64) {
        let slot = self.slots.entry(key).or_default();
        slot.reading = Some(job);
        if matches!(slot.answer, Some(Err(_))) {
            slot.answer = None;
        }
    }

    /// Keeps the answer of read `job` unless a newer read was started since. Returns whether
    /// it was kept.
    fn finish(&mut self, key: &PreviewKey, job: u64, answer: Result<BasePreview, String>) -> bool {
        let Some(slot) = self.slots.get_mut(key) else {
            return false;
        };
        if slot.reading != Some(job) {
            return false;
        }
        slot.reading = None;
        slot.answer = Some(answer);
        true
    }

    /// Records a failure known without reading; a read in flight no longer counts.
    fn fail(&mut self, key: PreviewKey, error: String) {
        let slot = self.slots.entry(key).or_default();
        slot.reading = None;
        slot.answer = Some(Err(error));
    }

    fn get(&self, key: &PreviewKey) -> Preview<'_> {
        match self.slots.get(key).and_then(|slot| slot.answer.as_ref()) {
            Some(Ok(preview)) => Preview::Ready(preview),
            Some(Err(error)) => Preview::Failed(error),
            None => Preview::Reading,
        }
    }

    fn is_reading(&self, key: &PreviewKey) -> bool {
        self.slots
            .get(key)
            .is_some_and(|slot| slot.reading.is_some())
    }

    /// The read of `project` that resolved its base to `commit`, so a run starts only from a
    /// commit Hive itself read.
    pub(super) fn resolved(&self, project: &Path, commit: &str) -> Option<&BasePreview> {
        self.slots.iter().find_map(|(key, slot)| {
            let preview = slot.answer.as_ref()?.as_ref().ok()?;
            (key.project == project && preview.commit == commit).then_some(preview)
        })
    }
}

impl Pipelines {
    /// Reads the base again, whatever is already known about it. A project removed from Hive
    /// fails at once instead of staying unread.
    pub(crate) fn refresh_preview(&mut self, key: PreviewKey, cx: &mut Context<Self>) {
        if !self.registered(&key.project, cx) {
            let error = format!(
                "{} is no longer a project in Hive. Choose another repository.",
                key.project.display()
            );
            self.previews.fail(key, error);
            cx.notify();
            return;
        }
        let job = self.next_job();
        self.previews.begin(key.clone(), job);
        cx.notify();
        let (checkout, reference) = (key.checkout.clone(), key.reference.clone());
        let work = cx.background_executor().spawn(async move {
            workspace::preview(&checkout, &reference).map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let answer = work.await;
            let _ = this.update(cx, |pipelines, cx| {
                if pipelines.previews.finish(&key, job, answer) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn preview(&self, key: &PreviewKey) -> Preview<'_> {
        self.previews.get(key)
    }

    /// Whether a read of the base is in flight, also while an earlier answer is still shown.
    pub(crate) fn is_reading_preview(&self, key: &PreviewKey) -> bool {
        self.previews.is_reading(key)
    }
}

#[cfg(test)]
mod tests {
    use super::{BasePreview, Preview, PreviewKey, Previews};

    fn key() -> PreviewKey {
        PreviewKey {
            project: "/repo".into(),
            checkout: "/repo".into(),
            reference: "refs/heads/develop".into(),
        }
    }

    fn at(commit: &str) -> Result<BasePreview, String> {
        Ok(BasePreview {
            worktrees_dir: "/repo-worktrees".into(),
            commit: commit.into(),
            dirty: false,
            branches: Vec::new(),
            checks: Vec::new(),
        })
    }

    fn commit(previews: &Previews) -> Option<&str> {
        match previews.get(&key()) {
            Preview::Ready(preview) => Some(&preview.commit),
            _ => None,
        }
    }

    #[test]
    fn a_slow_read_does_not_replace_the_answer_of_a_newer_one() {
        let mut previews = Previews::default();
        previews.begin(key(), 1);
        previews.begin(key(), 2);
        assert!(!previews.finish(&key(), 1, at("old")));
        assert!(matches!(previews.get(&key()), Preview::Reading));
        assert!(previews.is_reading(&key()));
        assert!(previews.finish(&key(), 2, at("new")));
        assert_eq!(commit(&previews), Some("new"));
        assert!(!previews.is_reading(&key()));
        // The same holds when the slow answer arrives last.
        assert!(!previews.finish(&key(), 1, at("old")));
        assert_eq!(commit(&previews), Some("new"));
    }

    #[test]
    fn a_good_answer_stays_while_it_is_read_again_and_a_failure_does_not() {
        let mut previews = Previews::default();
        previews.begin(key(), 1);
        previews.finish(&key(), 1, at("first"));
        previews.begin(key(), 2);
        assert_eq!(commit(&previews), Some("first"));
        assert!(previews.is_reading(&key()));
        previews.finish(&key(), 2, Err("fatal: bad revision".into()));
        assert!(matches!(
            previews.get(&key()),
            Preview::Failed("fatal: bad revision")
        ));
        assert!(!previews.is_reading(&key()));
        previews.begin(key(), 3);
        assert!(matches!(previews.get(&key()), Preview::Reading));
    }

    #[test]
    fn a_failure_known_without_reading_outlives_the_read_it_interrupts() {
        let mut previews = Previews::default();
        previews.begin(key(), 1);
        previews.fail(key(), "gone".into());
        assert!(!previews.is_reading(&key()));
        assert!(!previews.finish(&key(), 1, at("late")));
        assert!(matches!(previews.get(&key()), Preview::Failed("gone")));
    }

    #[test]
    fn a_run_starts_only_from_a_commit_read_for_its_own_project() {
        let mut previews = Previews::default();
        previews.begin(key(), 1);
        previews.finish(&key(), 1, at("abc"));
        assert!(previews.resolved("/repo".as_ref(), "abc").is_some());
        assert!(previews.resolved("/repo".as_ref(), "def").is_none());
        assert!(previews.resolved("/other".as_ref(), "abc").is_none());
    }
}
