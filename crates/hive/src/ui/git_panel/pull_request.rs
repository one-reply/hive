//! Opening a pull or merge request for the checked-out branch: the dialog that shows it
//! first, and the notice once the hosting service has it.
use super::*;
use crate::state::git::Git;
use crate::ui::pull_request_dialog;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::{Notification, NotificationType};
use hive_core::projects::Head;

impl GitPanel {
    /// What is known about the request of the branch `git` has checked out.
    pub(super) fn request(&self, git: &Git, cx: &App) -> Request {
        let branch = git
            .status
            .as_ref()
            .and_then(|status| status.branch.as_ref());
        let (Some(target), Some(branch)) = (&git.target, branch) else {
            return Request::Unknown;
        };
        let requests = self.pull_requests.read(cx);
        let head = Head::Branch(branch.clone());
        Request::of(
            requests.is_opening(target),
            requests.status(&target.host, &target.path, &head),
        )
    }

    /// Shows the request before it is opened. With `push_first` the branch is pushed, or
    /// published, before the hosting service is asked; the dialog says so, which makes it
    /// the confirmation of that push as well.
    pub(super) fn create_pull_request(
        &self,
        push_first: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.git.read(cx);
        let (Some(target), Some(branch)) = (
            state.target.clone(),
            state
                .status
                .as_ref()
                .and_then(|status| status.branch.clone()),
        ) else {
            return;
        };
        let Some(Action::Push { remote, publish }) = state.push_action() else {
            return;
        };
        let base = request_base(state.base.as_deref(), &remote);
        let push = push_first.then_some(Action::Push { remote, publish });
        let hint = request_hint(&branch, push.as_ref());
        let prefill = self.pull_requests.read(cx).prefill(&target, base, cx);
        let (git, requests) = (self.git.clone(), self.pull_requests.clone());
        // The worktree shows busy until the dialog is there to be seen.
        let Some(prefill) = git.update(cx, |git, cx| git.prepare(&target, prefill, cx)) else {
            return;
        };
        window
            .spawn(cx, async move |cx| {
                // The sender goes away only with source control itself.
                let Ok(draft) = prefill.recv().await else {
                    return;
                };
                cx.update(|window, cx| {
                    pull_request_dialog::open(hint, &draft, window, cx, move |draft, _, cx| {
                        requests.update(cx, |requests, cx| {
                            requests.open(git.clone(), target.clone(), draft, push.clone(), cx)
                        });
                    });
                })
                .ok();
            })
            .detach();
    }
}

/// Tells the user their request exists and offers to show it.
pub(super) fn announce(event: &PullRequestEvent, window: &mut Window, cx: &mut App) {
    let PullRequestEvent::Opened { branch, url } = event;
    let address: SharedString = url.clone().into();
    window.push_notification(
        Notification::new()
            .id1::<PullRequests>(address.clone())
            .with_type(NotificationType::Success)
            .title("Pull request opened")
            .message(branch.clone())
            .placement(Anchor::BottomRight)
            .content(move |_, window, cx| {
                let hover = Hover::new("view-opened-request", window, cx);
                let address = address.clone();
                h_flex()
                    .justify_end()
                    .pt_2()
                    .child(
                        Button::new("view-opened-request")
                            .cursor_pointer()
                            .small()
                            .icon(Lucide::ExternalLink)
                            .label("View")
                            .on_click(move |_, _, cx| cx.open_url(&address))
                            .smooth_hover(ButtonLook::Outline, &hover, cx),
                    )
                    .into_any_element()
            }),
        cx,
    );
}

/// The branch a request merges into: the review base when it lives on `remote`, the remote
/// the branch is pushed to. Anything else leaves the choice to the hosting service.
fn request_base(base: Option<&str>, remote: &str) -> String {
    base.and_then(|base| {
        base.strip_prefix("refs/remotes/")?
            .strip_prefix(remote)?
            .strip_prefix('/')
    })
    .unwrap_or_default()
    .to_owned()
}

/// What the dialog says happens to `branch` before its request is opened.
fn request_hint(branch: &str, push: Option<&Action>) -> String {
    const SIGN_IN: &str = "with your gh or glab sign-in";
    match push {
        Some(Action::Push {
            remote,
            publish: true,
        }) => {
            format!("Hive publishes {branch} to {remote} first, then opens its request {SIGN_IN}.")
        }
        Some(Action::Push { remote, .. }) => {
            format!("Hive pushes {branch} to {remote} first, then opens its request {SIGN_IN}.")
        }
        _ => format!(
            "Hive opens a request for {branch} {SIGN_IN}. The branch is already pushed; nothing else leaves this machine."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{request_base, request_hint};
    use hive_core::git::changes::Action;

    #[test]
    fn a_request_merges_into_the_review_base_only_on_the_pushed_remote() {
        let base = |base: &str| request_base(Some(base), "origin");
        assert_eq!(base("refs/remotes/origin/develop"), "develop");
        assert_eq!(base("refs/remotes/origin/release/2.0"), "release/2.0");
        // Another remote, a local branch and a remote whose name only starts the same way
        // do not name a branch of the repository the request is opened in.
        assert_eq!(base("refs/remotes/upstream/develop"), "");
        assert_eq!(base("refs/heads/develop"), "");
        assert_eq!(base("refs/remotes/origin-fork/develop"), "");
        assert_eq!(request_base(None, "origin"), "");
        assert_eq!(
            request_base(Some("refs/remotes/team/origin/main"), "team/origin"),
            "main"
        );
    }

    #[test]
    fn the_dialog_says_what_leaves_the_machine_first() {
        let push = |publish| Action::Push {
            remote: "origin".into(),
            publish,
        };
        assert!(
            request_hint("feature", Some(&push(true)))
                .starts_with("Hive publishes feature to origin first")
        );
        assert!(
            request_hint("feature", Some(&push(false)))
                .starts_with("Hive pushes feature to origin first")
        );
        let direct = request_hint("feature", None);
        assert!(direct.starts_with("Hive opens a request for feature"));
        assert!(direct.contains("already pushed"));
    }
}
