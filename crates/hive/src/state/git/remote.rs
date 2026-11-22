//! Progress and a short-lived confirmation for fetch, pull and push.
use super::*;
use hive_core::git::changes::{ForcePushPreview, Synced};

/// How long the confirmation of a finished remote operation stays in the panel.
const SYNCED_VISIBLE: Duration = Duration::from_secs(5);

/// The remote operation running on a target, which its button shows as in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteOperation {
    Fetch,
    Pull,
    Push,
    Publish,
}

/// A force push that waits for its confirmation.
pub(crate) struct ForcePushRequest {
    pub target: Target,
    /// The remote the branch tracks.
    pub remote: String,
    /// What the push would replace there, once it is read off the foreground thread.
    pub preview: async_channel::Receiver<Result<ForcePushPreview, git::GitError>>,
}

impl RemoteOperation {
    /// Commit & Push is left out: the commit button already shows its own progress.
    pub(super) fn of(action: &Action) -> Option<Self> {
        match action {
            Action::Fetch(_) => Some(Self::Fetch),
            Action::Pull => Some(Self::Pull),
            Action::Push { publish: false, .. } => Some(Self::Push),
            Action::Push { publish: true, .. } => Some(Self::Publish),
            _ => None,
        }
    }
}

impl Git {
    /// The remote operation running on the active worktree.
    pub(crate) fn remote_operation(&self) -> Option<RemoteOperation> {
        self.remote_operations.get(self.target.as_ref()?).copied()
    }

    /// Starts reading what a force push of the active worktree would replace on the remote
    /// its branch tracks; the worktree is busy meanwhile. `None` when the branch has no
    /// upstream on a remote or the worktree is busy already.
    pub(crate) fn preview_force_push(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<ForcePushRequest> {
        let target = self.target.clone()?;
        let Some(Action::Push {
            remote,
            publish: false,
        }) = self.push_action()
        else {
            return None;
        };
        let (host, path, name) = (target.host.clone(), target.path.clone(), remote.clone());
        let preview = cx
            .background_executor()
            .spawn(async move { git::changes::force_push_preview(&host, &path, &name) });
        let preview = self.prepare(&target, preview, cx)?;
        Some(ForcePushRequest {
            target,
            remote,
            preview,
        })
    }

    /// What the last remote operation on the active worktree changed, while it is fresh.
    pub(crate) fn synced(&self) -> Option<&Synced> {
        let (target, synced) = self.synced.as_ref()?;
        (self.target.as_ref() == Some(target)).then_some(synced)
    }

    pub(super) fn start_remote_operation(&mut self, target: &Target, action: Option<&Action>) {
        if self.synced.as_ref().is_some_and(|(t, _)| t == target) {
            self.synced = None;
            self._synced_timeout = None;
        }
        if let Some(operation) = action.and_then(RemoteOperation::of) {
            self.remote_operations.insert(target.clone(), operation);
        }
    }

    pub(super) fn finish_remote_operation(
        &mut self,
        target: &Target,
        synced: Option<Synced>,
        cx: &mut Context<Self>,
    ) {
        self.remote_operations.remove(target);
        let Some(synced) = synced else {
            return;
        };
        tracing::info!(path = ?target.path, %synced, "remote operation finished");
        self.synced = Some((target.clone(), synced));
        self._synced_timeout = Some(cx.spawn(async |this, cx| {
            cx.background_executor().timer(SYNCED_VISIBLE).await;
            let _ = this.update(cx, |this, cx| {
                this.synced = None;
                cx.notify();
            });
        }));
    }
}
