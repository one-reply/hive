//! Short reads made before a confirmation opens: what a force push would discard, whether a
//! branch is merged, what a request starts from.
use super::*;

impl Git {
    /// Awaits `read` for a confirmation that is about to open on `target`. Until it is done
    /// the worktree counts as busy, so the panel shows the click was taken and nothing else
    /// is started on it; on a slow host that is seconds in which the user would otherwise
    /// see nothing and go back to typing.
    ///
    /// The receiver gets what was read. `None` when the worktree is busy already.
    pub(crate) fn prepare<T: 'static>(
        &mut self,
        target: &Target,
        read: Task<T>,
        cx: &mut Context<Self>,
    ) -> Option<async_channel::Receiver<T>> {
        if self.is_busy_at(target) {
            return None;
        }
        self.preparing.insert(target.clone());
        cx.notify();
        let (done, value) = async_channel::bounded(1);
        let target = target.clone();
        // Not kept: the read is short, and it has to end the busy state even when whoever
        // asked for it is gone.
        cx.spawn(async move |this, cx| {
            let read = read.await;
            let _ = this.update(cx, |this, cx| {
                this.preparing.remove(&target);
                cx.notify();
            });
            // The waiting side may have gone away; then nobody needs the answer.
            let _ = done.try_send(read);
        })
        .detach();
        Some(value)
    }

    /// Whether a confirmation is being prepared for the active worktree.
    pub(crate) fn is_preparing(&self) -> bool {
        self.target
            .as_ref()
            .is_some_and(|target| self.is_preparing_at(target))
    }

    pub(crate) fn is_preparing_at(&self, target: &Target) -> bool {
        self.preparing.contains(target)
    }

    /// Starts checking whether the local branch `name` of `target` is merged into its HEAD,
    /// which decides how deleting it is confirmed. `None` when the worktree is busy.
    pub(crate) fn check_merged(
        &mut self,
        target: &Target,
        name: &str,
        cx: &mut Context<Self>,
    ) -> Option<async_channel::Receiver<Result<bool, git::GitError>>> {
        let (host, path, name) = (target.host.clone(), target.path.clone(), name.to_owned());
        let check = cx
            .background_executor()
            .spawn(async move { git::branches::is_merged(&host, &path, &name) });
        self.prepare(target, check, cx)
    }
}

#[cfg(test)]
mod tests {
    use crate::state::git::tests::source_control;
    use gpui_kit as gpui;
    use gpui_kit::TestAppContext;
    use hive_core::{host::Host, projects::Target};

    #[gpui::test]
    fn a_worktree_is_busy_while_a_confirmation_is_prepared_and_free_afterwards(
        cx: &mut TestAppContext,
    ) {
        let (_, git) = source_control(cx);
        let worktree = |path: &str| Target {
            host: Host::Local,
            project: "/unused".into(),
            path: path.into(),
        };
        let (shown, other) = (worktree("/unused"), worktree("/unused/other"));
        // The empty project list settles first; its notification would clear the target.
        cx.run_until_parked();
        let (release, held) = async_channel::bounded::<()>(1);
        let answer = git.update(cx, |git, cx| {
            git.target = Some(shown.clone());
            let read = cx.background_executor().spawn(async move {
                let _ = held.recv().await;
                7
            });
            let answer = git.prepare(&shown, read, cx).expect("an idle worktree");
            assert!(git.is_preparing() && git.is_busy());
            assert!(!git.is_preparing_at(&other) && !git.is_busy_at(&other));
            // A second click while the first read is under way starts nothing.
            let again = cx.background_executor().spawn(async { 8 });
            assert!(git.prepare(&shown, again, cx).is_none());
            answer
        });
        cx.run_until_parked();
        assert!(git.read_with(cx, |git, _| git.is_preparing()));
        assert!(answer.try_recv().is_err());

        release.try_send(()).expect("the read is waiting");
        cx.run_until_parked();
        assert_eq!(answer.try_recv().ok(), Some(7));
        git.read_with(cx, |git, _| {
            assert!(!git.is_preparing() && !git.is_busy());
        });
    }

    #[gpui::test]
    fn a_dropped_listener_does_not_leave_the_worktree_busy(cx: &mut TestAppContext) {
        let (_, git) = source_control(cx);
        let target = Target {
            host: Host::Local,
            project: "/unused".into(),
            path: "/unused".into(),
        };
        cx.run_until_parked();
        git.update(cx, |git, cx| {
            git.target = Some(target.clone());
            let read = cx.background_executor().spawn(async { 7 });
            // The window that asked was closed before the answer came.
            drop(git.prepare(&target, read, cx));
            assert!(git.is_busy());
        });
        cx.run_until_parked();
        assert!(!git.read_with(cx, |git, _| git.is_busy()));
    }
}
