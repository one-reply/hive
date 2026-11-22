//! The branch a lane commits to: a free local branch, or a new one named in the same menu.
use super::*;
use crate::ui::hover::SmoothHoverBg as _;
use gpui_kit::component::{popover::Popover, spinner::Spinner};
use hive_core::git::branches::{Branch, is_valid_name};

const MENU_WIDTH: Pixels = px(340.);
const MENU_LIST_HEIGHT: Pixels = px(260.);

/// What the typed text can become besides a listed branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Create {
    Nothing,
    Valid,
    Invalid,
}

/// Local branches matching `query`, and whether `query` names a branch that can be created.
/// Remote-tracking refs are left out: the run commits to a local branch.
fn choices<'a>(branches: &'a [Branch], query: &str) -> (Vec<&'a Branch>, Create) {
    let query = query.trim();
    let needle = query.to_lowercase();
    let local: Vec<_> = branches
        .iter()
        .filter(|b| !b.remote && b.name.to_lowercase().contains(&needle))
        .collect();
    let create = if query.is_empty() || local.iter().any(|b| b.name == query) {
        Create::Nothing
    } else if is_valid_name(query) {
        Create::Valid
    } else {
        Create::Invalid
    };
    (local, create)
}

/// What the open menu lists. A read that failed is its own state: shown as one still in
/// progress it would leave the menu waiting for an answer that never comes.
#[derive(Debug, PartialEq, Eq)]
enum Listing {
    Reading,
    Failed(String),
    Branches(Vec<Branch>),
}

impl From<Preview<'_>> for Listing {
    fn from(read: Preview<'_>) -> Self {
        match read {
            Preview::Reading => Self::Reading,
            Preview::Failed(error) => Self::Failed(error.to_owned()),
            Preview::Ready(preview) => Self::Branches(preview.branches.clone()),
        }
    }
}

/// What Enter does with the typed text: take the exact free branch or create a new one.
fn enter_choice(branches: &[Branch], query: &str) -> Option<WorkBranch> {
    let query = query.trim();
    let (local, create) = choices(branches, query);
    match local.iter().find(|b| b.name == query) {
        Some(b) if b.worktree.is_none() => Some(WorkBranch::Existing(b.name.clone())),
        Some(_) => None,
        None => (create == Create::Valid).then(|| WorkBranch::New(query.to_owned())),
    }
}

impl LaunchForm {
    pub(super) fn branch_field(&self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.bindings[i];
        let view = cx.entity().downgrade();
        let label = match &b.branch {
            WorkBranch::Unchosen => "Choose or create…".to_owned(),
            WorkBranch::Existing(name) => name.clone(),
            WorkBranch::New(name) => format!("{name} · new"),
        };
        let trigger = dropdown(("pipeline-branch", i), Lucide::GitBranch, label)
            .w_full()
            .when(b.branch == WorkBranch::Unchosen, |trigger| {
                trigger.text_color(cx.theme().muted_foreground)
            });
        let toggle = view.clone();
        Popover::new(("pipeline-branch-menu", i))
            .trigger_style(StyleRefinement::default().w_full())
            .open(b.branch_open)
            .on_open_change(move |open, window, cx| {
                let _ = toggle.update(cx, |this, cx| this.set_branch_menu(i, *open, window, cx));
            })
            .track_focus(&b.branch_query.focus_handle(cx))
            .trigger(trigger)
            .content(move |_, window, cx| branch_menu(&view, i, window, cx))
            .into_any_element()
    }

    fn set_branch_menu(
        &mut self,
        i: usize,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(b) = self.bindings.get_mut(i) else {
            return;
        };
        b.branch_open = open;
        if open {
            b.branch_query
                .update(cx, |query, cx| query.set_value("", window, cx));
            // Branches made, removed or checked out outside Hive since the card was read
            // show up now; the list read earlier stays until the answer arrives.
            b.read(cx);
        }
        cx.notify();
    }

    /// Asks Git about repository `i` again, after a failed read or to pick up outside changes.
    pub(super) fn read_again(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(binding) = self.bindings.get(i) {
            binding.read(cx);
        }
        cx.notify();
    }

    fn choose_branch(&mut self, i: usize, branch: WorkBranch, cx: &mut Context<Self>) {
        let Some(b) = self.bindings.get_mut(i) else {
            return;
        };
        // An existing branch is its own base; a new one starts where the user's checkout is.
        b.reference = match &branch {
            WorkBranch::Existing(name) => format!("refs/heads/{name}"),
            _ => b.default_reference.clone(),
        };
        b.ref_input = None;
        b.branch = branch;
        b.branch_open = false;
        b.read(cx);
        cx.notify();
    }

    pub(super) fn submit_branch_query(
        &mut self,
        query: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) {
        let Some(i) = self.bindings.iter().position(|b| &b.branch_query == query) else {
            return;
        };
        let b = &self.bindings[i];
        let text = b.branch_query.read(cx).value().to_string();
        let Some(preview) = b.ready(cx) else {
            return;
        };
        if let Some(choice) = enter_choice(&preview.branches, &text) {
            self.choose_branch(i, choice, cx);
        }
    }
}

/// The open menu reads the form each frame, so typing filters the list immediately.
fn branch_menu(
    view: &WeakEntity<LaunchForm>,
    i: usize,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some(form) = view.upgrade() else {
        return div().into_any_element();
    };
    let Some((query, listing, selected, base)) = form.read(cx).bindings.get(i).map(|b| {
        (
            b.branch_query.clone(),
            Listing::from(b.base(cx)),
            b.branch.clone(),
            short_name(&b.default_reference).to_owned(),
        )
    }) else {
        return div().into_any_element();
    };
    let text = query.read(cx).value().trim().to_owned();
    let mut list = v_flex()
        .id(("pipeline-branch-list", i))
        .max_h(MENU_LIST_HEIGHT)
        .overflow_y_scroll()
        .gap_0p5();
    let branches = match listing {
        Listing::Branches(branches) => branches,
        Listing::Reading => {
            let reading = h_flex()
                .gap_2()
                .px_2()
                .py_1()
                .child(Spinner::new().icon(Lucide::LoaderCircle).xsmall())
                .child(subtle("Reading branches…", cx));
            return menu_frame(&query, list.child(reading), cx);
        }
        Listing::Failed(error) => {
            let failure = v_flex()
                .gap_2()
                .px_2()
                .py_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(format!("Branches can’t be read: {error}")),
                )
                .child(h_flex().child(retry_button(view, i, window, cx)));
            return menu_frame(&query, list.child(failure), cx);
        }
    };
    let (local, create) = choices(&branches, &text);
    match create {
        Create::Valid => {
            let label = format!("Create “{text}”");
            let detail = format!("New branch from {base}");
            let choice = WorkBranch::New(text.clone());
            list = list.child(menu_row(
                view,
                i,
                ("create", Lucide::Plus, label, Some(detail)),
                Some(choice),
                false,
                window,
                cx,
            ));
        }
        Create::Invalid => {
            list = list.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(format!("“{text}” is not a valid branch name.")),
            );
        }
        Create::Nothing => {}
    }
    if local.is_empty() && create == Create::Nothing {
        list = list.child(subtle("No local branches. Type a name to create one.", cx));
    }
    for branch in local {
        let detail = branch
            .worktree
            .as_deref()
            .map(|path| format!("Open in {}", path_name(path)));
        let choice = branch
            .worktree
            .is_none()
            .then(|| WorkBranch::Existing(branch.name.clone()));
        let current = selected.name() == Some(branch.name.as_str());
        list = list.child(menu_row(
            view,
            i,
            ("branch", Lucide::GitBranch, branch.name.clone(), detail),
            choice,
            current,
            window,
            cx,
        ));
    }
    menu_frame(&query, list, cx)
}

fn menu_frame(query: &Entity<InputState>, list: impl IntoElement, cx: &App) -> AnyElement {
    v_flex()
        .w(MENU_WIDTH)
        .gap_2()
        .child(TextField::input(Input::new(query), query, cx))
        .child(list)
        .child(subtle(
            "A branch open in another folder can’t be used: Git checks out a branch in one place.",
            cx,
        ))
        .into_any_element()
}

fn retry_button(
    view: &WeakEntity<LaunchForm>,
    i: usize,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let id = ElementId::from(("pipeline-branch-retry", i));
    let hover = Hover::new(id.clone(), window, cx);
    let view = view.clone();
    Button::new(id)
        .cursor_pointer()
        .small()
        .icon(Lucide::RotateCcw)
        .label("Try again")
        .on_click(move |_, _, cx| {
            let _ = view.update(cx, |this, cx| this.read_again(i, cx));
        })
        .smooth_hover(ButtonLook::Outline, &hover, cx)
}

/// One menu entry; `choice` is `None` for a branch Git cannot check out again.
fn menu_row(
    view: &WeakEntity<LaunchForm>,
    i: usize,
    (kind, icon, label, detail): (&'static str, Lucide, String, Option<String>),
    choice: Option<WorkBranch>,
    current: bool,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = SharedString::from(format!("pipeline-branch-{i}-{kind}-{label}"));
    let muted = cx.theme().muted_foreground;
    let row = h_flex()
        .id(id.clone())
        .w_full()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_md()
        .text_sm()
        .child(Icon::new(icon).size_3p5().flex_none().text_color(muted))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().truncate().child(label))
                .when_some(detail, |el, detail| {
                    el.child(div().text_xs().text_color(muted).truncate().child(detail))
                }),
        )
        .when(current, |row| {
            row.child(Icon::new(Lucide::Check).size_3p5().flex_none())
        });
    let Some(choice) = choice else {
        return row.text_color(muted).into_any_element();
    };
    let hover = Hover::new(id, window, cx);
    let view = view.clone();
    row.cursor_pointer()
        .on_click(move |_, _, cx| {
            let _ = view.update(cx, |this, cx| this.choose_branch(i, choice.clone(), cx));
        })
        .hover_bg(&hover, cx.theme().transparent, cx.theme().muted)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{Branch, Create, Listing, Preview, WorkBranch, choices, enter_choice};

    fn local(name: &str, worktree: Option<&str>) -> Branch {
        Branch {
            reference: format!("refs/heads/{name}"),
            name: name.into(),
            remote: false,
            worktree: worktree.map(Into::into),
        }
    }

    fn branches() -> Vec<Branch> {
        vec![
            local("develop", Some("/repo")),
            local("feature/login", None),
            Branch {
                reference: "refs/remotes/origin/main".into(),
                name: "origin/main".into(),
                remote: true,
                worktree: None,
            },
        ]
    }

    #[test]
    fn a_failed_read_is_listed_as_its_error_and_not_as_a_read_in_progress() {
        assert_eq!(Listing::from(Preview::Reading), Listing::Reading);
        assert_eq!(
            Listing::from(Preview::Failed("fatal: not a git repository")),
            Listing::Failed("fatal: not a git repository".into())
        );
        let preview = hive_core::pipeline::workspace::BasePreview {
            worktrees_dir: "/repo-worktrees".into(),
            commit: "4f2c9d1".into(),
            dirty: false,
            branches: branches(),
            checks: Vec::new(),
        };
        assert_eq!(
            Listing::from(Preview::Ready(&preview)),
            Listing::Branches(branches())
        );
    }

    #[test]
    fn the_menu_lists_local_branches_and_offers_to_create_an_unknown_name() {
        let branches = branches();
        let (all, create) = choices(&branches, "");
        assert_eq!(all.len(), 2, "remote refs are not offered");
        assert_eq!(create, Create::Nothing);
        let (matching, create) = choices(&branches, "LOG");
        assert_eq!(matching[0].name, "feature/login");
        assert_eq!(create, Create::Valid);
        assert_eq!(choices(&branches, "develop").1, Create::Nothing);
        assert_eq!(choices(&branches, "bad name").1, Create::Invalid);
    }

    #[test]
    fn enter_takes_a_free_branch_creates_a_new_one_and_skips_an_occupied_one() {
        let branches = branches();
        assert_eq!(
            enter_choice(&branches, " feature/login "),
            Some(WorkBranch::Existing("feature/login".into()))
        );
        assert_eq!(
            enter_choice(&branches, "fix-timeout"),
            Some(WorkBranch::New("fix-timeout".into()))
        );
        assert_eq!(enter_choice(&branches, "develop"), None);
        assert_eq!(enter_choice(&branches, "bad name"), None);
        assert_eq!(enter_choice(&branches, ""), None);
    }
}
