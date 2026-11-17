use super::GitPanel;
use crate::state::{git::Git, projects::Projects, store::Store};
use gpui_kit as gpui;
use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use hive_core::{host::Host, projects::Target, state::StateError};

fn worktree(path: &str) -> Target {
    Target {
        host: Host::Local,
        project: "/unused/project".into(),
        path: path.into(),
    }
}

/// A window with source control that shows `shown` and a panel opened on it.
fn open<'a>(
    shown: Option<&Target>,
    cx: &'a mut TestAppContext,
) -> (Entity<Git>, Entity<GitPanel>, &'a mut VisualTestContext) {
    cx.update(gpui_kit::init);
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
    let git = cx.new(|cx| Git::new(projects, cx));
    // Opening the window lets the empty project list settle before a worktree is shown.
    let cx = cx.add_empty_window();
    git.update(cx, |git, _| git.target = shown.cloned());
    let panel = cx.update(|window, cx| {
        let git = git.clone();
        cx.new(|cx| GitPanel::over(git, store, window, cx))
    });
    (git, panel, cx)
}

/// Makes `target` the worktree `git` shows, as following the project selection does.
fn show(git: &Entity<Git>, target: &Target, cx: &mut VisualTestContext) {
    git.update(cx, |git, cx| {
        git.target = Some(target.clone());
        cx.notify();
    });
}

fn set_filter(panel: &Entity<GitPanel>, text: &'static str, cx: &mut VisualTestContext) {
    panel.update_in(cx, |panel, window, cx| {
        panel
            .filter
            .update(cx, |filter, cx| filter.set_value(text, window, cx));
    });
}

fn filter(panel: &Entity<GitPanel>, cx: &mut VisualTestContext) -> String {
    panel.read_with(cx, |panel, cx| panel.filter.read(cx).value().to_string())
}

fn folded(panel: &Entity<GitPanel>, cx: &mut VisualTestContext) -> Vec<&'static str> {
    panel.read_with(cx, |panel, _| panel.collapsed.iter().copied().collect())
}

#[gpui::test]
fn the_filter_and_folded_groups_stay_with_their_worktree(cx: &mut TestAppContext) {
    let (first, second) = (worktree("/unused/project"), worktree("/unused/worktree"));
    let (git, panel, cx) = open(None, cx);

    show(&git, &first, cx);
    set_filter(&panel, "lib", cx);
    panel.update(cx, |panel, cx| panel.toggle_group("Staged", cx));

    show(&git, &second, cx);
    assert_eq!(filter(&panel, cx), "");
    assert!(folded(&panel, cx).is_empty());
    panel.update(cx, |panel, cx| panel.toggle_group("Changes", cx));

    show(&git, &first, cx);
    assert_eq!(filter(&panel, cx), "lib");
    assert_eq!(folded(&panel, cx), ["Staged"]);

    show(&git, &second, cx);
    assert_eq!(filter(&panel, cx), "");
    assert_eq!(folded(&panel, cx), ["Changes"]);
}

#[gpui::test]
fn an_update_of_the_shown_worktree_keeps_the_filter(cx: &mut TestAppContext) {
    let shown = worktree("/unused/project");
    // The panel opens on a worktree that is already shown, as after a restart.
    let (git, panel, cx) = open(Some(&shown), cx);
    set_filter(&panel, "lib", cx);
    panel.update(cx, |panel, cx| panel.toggle_group("Staged", cx));

    git.update(cx, |_, cx| cx.notify());
    assert_eq!(git.read_with(cx, |git, _| git.target.clone()), Some(shown));
    assert_eq!(filter(&panel, cx), "lib");
    assert_eq!(folded(&panel, cx), ["Staged"]);
}
