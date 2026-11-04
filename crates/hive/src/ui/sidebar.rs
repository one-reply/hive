//! Project tree (Orca-style): colored project headers → worktree cards. Sessions will hang
//! under worktrees.

use std::cell::Cell;
use std::collections::HashSet;
use std::f32::consts::FRAC_PI_2;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hive_core::host::Host;
use hive_core::projects::{Project, Worktree};
use hive_core::state::WorktreeFilter;
use hive_core::worktree::{Removal, WorktreeError};

use crate::app::AppState;
use crate::state::projects::Projects;
use crate::state::store::Store;
use crate::ui::{project_badge, settings};

/// Status dot of a worktree with an open terminal (Tailwind emerald-500, as in Orca).
const ACTIVE_COLOR: u32 = 0x10b981;
/// Detached-HEAD badge (Tailwind amber-500, git's "modified" decoration).
const DETACHED_COLOR: u32 = 0xf59e0b;
const HEADER_HEIGHT: Pixels = px(28.);
const HEADER_INSET: Pixels = px(10.);
const TITLE_TEXT: Pixels = px(13.);
const META_TEXT: Pixels = px(11.);
const BADGE_TEXT: Pixels = px(10.);
const BADGE_HEIGHT: Pixels = px(16.);
const STATUS_DOT: Pixels = px(8.);
const PROJECT_BADGE: Pixels = px(14.);
const CREATE_DIALOG_WIDTH: Pixels = px(420.);
/// Rows under a project header have fixed heights, so the collapse animation knows the
/// height of the list without measuring it.
const CARD_HEIGHT: Pixels = px(58.);
const CREATING_HEIGHT: Pixels = px(28.);
const ROW_GAP: Pixels = px(2.);
const COLLAPSE_DURATION: Duration = Duration::from_millis(200);
/// Chevron rotation of a folded project: pointing right instead of down.
const CHEVRON_FOLDED: f32 = -FRAC_PI_2;

pub struct Sidebar {
    projects: Entity<Projects>,
    store: Entity<Store>,
    /// Projects the user folded; everything else is expanded.
    collapsed: HashSet<PathBuf>,
    /// Projects folded or unfolded since launch: they animate, so the tree doesn't play
    /// its animations on startup.
    toggled: HashSet<PathBuf>,
    _observer: Subscription,
}

impl Sidebar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (projects, store) = {
            let app = AppState::get(cx);
            (app.projects.clone(), app.store.clone())
        };
        let observer = cx.observe(&projects, |_, _, cx| cx.notify());
        Self {
            projects,
            store,
            collapsed: HashSet::new(),
            toggled: HashSet::new(),
            _observer: observer,
        }
    }

    fn add_projects(&mut self, cx: &mut Context<Self>) {
        Store::prompt_add_projects(self.store.clone(), cx);
    }

    fn toggle_collapsed(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_path_buf());
        }
        self.toggled.insert(path.to_path_buf());
        cx.notify();
    }

    /// A project header and its worktree list, which slides open and closed.
    fn render_project(&self, ix: usize, project: &Project, cx: &mut Context<Self>) -> AnyElement {
        let open = !self.collapsed.contains(&project.path);
        let animate = self.toggled.contains(&project.path);
        let header = self
            .render_header(ix, project, open, animate, cx)
            .into_any_element();
        if !open && !animate {
            return v_flex().child(header).into_any_element();
        }
        let rows = self.render_rows(project, cx);
        let height = rows
            .iter()
            .map(|(_, height)| *height + ROW_GAP)
            .sum::<Pixels>();
        let list = v_flex()
            .overflow_hidden()
            .children(rows.into_iter().map(|(row, _)| row));
        let list = if animate {
            list.with_animation(
                animation_id("project-list", &project.path, open),
                collapse_animation(),
                move |list, t| list.h(height * openness(open, t)),
            )
            .into_any_element()
        } else {
            list.into_any_element()
        };
        v_flex().child(header).child(list).into_any_element()
    }

    /// Cards under a project header with their heights.
    fn render_rows(&self, project: &Project, cx: &mut Context<Self>) -> Vec<(AnyElement, Pixels)> {
        let mut rows = vec![];
        let projects = self.projects.read(cx);
        let host = host_label(&project.host);
        if project.worktrees.is_empty() {
            let folder = WorktreeCard {
                path: project.path.clone(),
                title: project.name.clone().into(),
                meta: Meta::Path(project.path.display().to_string().into()),
                is_primary: false,
                is_active: projects.is_active(&project.path),
                is_selected: projects.is_selected(&project.path, None),
            };
            rows.push((
                self.render_card(project, None, folder, &host, cx),
                CARD_HEIGHT,
            ));
        }
        for (_, worktree) in project.visible_worktrees() {
            let card = WorktreeCard::new(project, worktree, projects);
            rows.push((
                self.render_card(project, Some(worktree), card, &host, cx),
                CARD_HEIGHT,
            ));
        }
        for _ in 0..projects.creating_count(&project.path) {
            rows.push((render_creating(cx).into_any_element(), CREATING_HEIGHT));
        }
        rows
    }

    fn render_header(
        &self,
        ix: usize,
        project: &Project,
        open: bool,
        animate: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let group: SharedString = format!("project-header-{ix}").into();
        let path = project.path.clone();
        let chevron = Icon::new(IconName::ChevronDown)
            .size_3p5()
            .text_color(cx.theme().muted_foreground);
        let chevron = if animate {
            chevron
                .with_animation(
                    animation_id("project-chevron", &project.path, open),
                    collapse_animation(),
                    move |icon, t| icon.rotate(radians(CHEVRON_FOLDED * (1. - openness(open, t)))),
                )
                .into_any_element()
        } else {
            let angle = if open { 0. } else { CHEVRON_FOLDED };
            chevron.rotate(radians(angle)).into_any_element()
        };
        // Collapsed to zero width until hovered, so the chevron sits at the row's edge (Orca).
        // The gap before the buttons lives inside, so it collapses with them.
        let buttons = h_flex()
            .flex_none()
            .pl_1()
            .gap_0p5()
            .child(self.project_menu(ix, project))
            .when(!project.worktrees.is_empty(), |s| {
                let (projects, project) = (self.projects.clone(), project.clone());
                s.child(
                    Button::new(("new-worktree", ix))
                        .cursor_pointer()
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .tooltip(format!("New worktree in {}", project.name))
                        .on_click(move |_, window, cx| {
                            open_create_dialog(projects.clone(), &project, window, cx)
                        }),
                )
            });
        let actions = h_flex()
            .w_0()
            .overflow_hidden()
            .opacity(0.)
            .group_hover(group.clone(), |s| s.w_auto().opacity(1.))
            .child(buttons);
        h_flex()
            .id(("project", ix))
            .group(group)
            .h(HEADER_HEIGHT)
            .gap_1p5()
            .pl(HEADER_INSET)
            // Mirrors the folder icon's inset from the card's left edge.
            .pr_1p5()
            .rounded_md()
            .cursor_pointer()
            .child(project_badge::render(
                &project.identity,
                &project.name,
                project_badge::color(&project.identity, ix),
                PROJECT_BADGE,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(TITLE_TEXT)
                    .font_weight(FontWeight::SEMIBOLD)
                    .truncate()
                    .child(project.name.clone()),
            )
            .child(h_flex().flex_none().child(chevron).child(actions))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_collapsed(&path, cx)))
    }

    /// The "…" menu of a project header.
    fn project_menu(&self, ix: usize, project: &Project) -> impl IntoElement {
        let store = self.store.clone();
        let path = project.path.clone();
        let hidden = project.hidden_worktree_count();
        let filter_label: SharedString = match project.filter {
            WorktreeFilter::Hive => format!("Show hidden worktrees ({hidden})").into(),
            WorktreeFilter::All => "Hide non-Hive worktrees".into(),
        };
        let can_toggle = project.filter == WorktreeFilter::All || hidden > 0;
        Button::new(("project-menu", ix))
            .cursor_pointer()
            .ghost()
            .xsmall()
            .icon(Lucide::Ellipsis)
            .dropdown_menu(move |menu, _, _| {
                let (store, path) = (store.clone(), path.clone());
                menu.item(
                    PopupMenuItem::new(filter_label.clone())
                        .disabled(!can_toggle)
                        .on_click({
                            let (store, path) = (store.clone(), path.clone());
                            move |_, _, cx| {
                                store.update(cx, |s, cx| s.toggle_worktree_filter(&path, cx))
                            }
                        }),
                )
                .item(copy_item("Copy Path", path.display().to_string()))
                .item(PopupMenuItem::new("Project Settings").on_click({
                    let path = path.clone();
                    move |_, _, cx| settings::open_project(&path, cx)
                }))
                .separator()
                .item(
                    PopupMenuItem::new("Remove Project").on_click(move |_, _, cx| {
                        store.update(cx, |s, cx| s.remove_project(&path, cx))
                    }),
                )
            })
    }

    fn render_card(
        &self,
        project: &Project,
        worktree: Option<&Worktree>,
        card: WorktreeCard,
        host: &SharedString,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let (fg, muted_fg) = (theme.sidebar_foreground, theme.muted_foreground);
        let dot = if card.is_active {
            rgb(ACTIVE_COLOR).into()
        } else {
            muted_fg.opacity(0.4)
        };
        let title_row = h_flex()
            .gap_2()
            .child(div().size(STATUS_DOT).flex_none().rounded_full().bg(dot))
            .child(
                div()
                    .min_w_0()
                    .text_size(TITLE_TEXT)
                    .truncate()
                    .child(card.title.clone()),
            )
            .when(card.is_primary, |s| s.child(primary_badge(fg)));
        let meta_row = h_flex()
            .gap_1p5()
            // Under the title, past the status dot and its gap.
            .pl(STATUS_DOT + px(8.))
            .text_size(META_TEXT)
            .text_color(muted_fg)
            .child(host_badge(host.clone(), fg, muted_fg))
            .child(card.meta.render());

        let projects = self.projects.clone();
        let project_path = project.path.clone();
        let worktree_path = worktree.map(|w| w.path.clone());
        let menu_target = worktree.map(|w| (project.clone(), w.clone()));
        let element = v_flex()
            .id(SharedString::from(format!(
                "worktree:{}",
                card.path.display()
            )))
            .h(CARD_HEIGHT)
            .flex_none()
            .justify_center()
            .gap_1p5()
            .mt(ROW_GAP)
            .ml_1()
            .pl_2()
            .pr_1p5()
            .rounded_lg()
            .border_1()
            .border_color(transparent_black())
            .cursor_pointer()
            .hover(|s| s.bg(fg.opacity(0.05)))
            .when(card.is_selected, |s| {
                s.bg(fg.opacity(0.09)).border_color(fg.opacity(0.14))
            })
            .child(title_row)
            .child(meta_row)
            .on_click(move |_, _, cx| {
                projects.update(cx, |p, cx| {
                    p.select(&project_path, worktree_path.as_deref(), cx)
                })
            });
        let Some((project, worktree)) = menu_target else {
            return element.into_any_element();
        };
        let projects = self.projects.clone();
        element
            .context_menu(move |menu, _, _| {
                worktree_menu(menu, projects.clone(), &project, &worktree)
            })
            .into_any_element()
    }
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.projects.read(cx).items.clone();
        let rows: Vec<AnyElement> = items
            .iter()
            .enumerate()
            .map(|(ix, project)| self.render_project(ix, project, cx))
            .collect();
        let theme = cx.theme();
        let (fg, muted_fg) = (theme.sidebar_foreground, theme.muted_foreground);

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .text_color(fg)
            .child(
                h_flex()
                    .justify_between()
                    .pl_4()
                    // Matches the title bar's inset so the add button lines up under the toggle.
                    .pr_2()
                    .pt_3()
                    .pb_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted_fg)
                            .child("Projects"),
                    )
                    .child(
                        Button::new("add-project")
                            .cursor_pointer()
                            .ghost()
                            .small()
                            .icon(Lucide::FolderPlus)
                            .tooltip("Add project")
                            .on_click(cx.listener(|this, _, _, cx| this.add_projects(cx))),
                    ),
            )
            .child(
                v_flex()
                    .id("project-tree")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .gap_0p5()
                    .children(rows),
            )
            .child(
                h_flex().px_3().pb_3().child(
                    Button::new("settings")
                        .cursor_pointer()
                        .ghost()
                        .small()
                        .icon(IconName::Settings)
                        .tooltip("Settings")
                        .on_click(|_, _, cx| settings::open(cx)),
                ),
            )
    }
}

/// What a worktree card shows, resolved from the project, the worktree and app state.
struct WorktreeCard {
    path: PathBuf,
    title: SharedString,
    meta: Meta,
    is_primary: bool,
    is_active: bool,
    is_selected: bool,
}

/// The text after the host badge.
enum Meta {
    Branch(SharedString),
    Detached,
    /// A folder that is not a git repository.
    Path(SharedString),
}

impl WorktreeCard {
    fn new(project: &Project, worktree: &Worktree, projects: &Projects) -> Self {
        let (title, meta) = match worktree.branch() {
            Some(branch) => (
                SharedString::from(branch.to_string()),
                Meta::Branch(branch.to_string().into()),
            ),
            None => (worktree.name().into(), Meta::Detached),
        };
        Self {
            path: worktree.path.clone(),
            title,
            meta,
            is_primary: project.is_primary(worktree),
            is_active: projects.is_active(&worktree.path),
            is_selected: projects.is_selected(&project.path, Some(&worktree.path)),
        }
    }
}

impl Meta {
    fn render(self) -> AnyElement {
        match self {
            Meta::Branch(branch) => div().min_w_0().truncate().child(branch).into_any_element(),
            Meta::Path(path) => div()
                .min_w_0()
                .truncate()
                .font_family("monospace")
                .child(path)
                .into_any_element(),
            Meta::Detached => {
                let amber = rgb(DETACHED_COLOR);
                h_flex()
                    .flex_none()
                    .gap_1()
                    .h(BADGE_HEIGHT)
                    .px_1p5()
                    .rounded_sm()
                    .border_1()
                    .border_color(amber.opacity(0.3))
                    .bg(amber.opacity(0.08))
                    .text_size(BADGE_TEXT)
                    .text_color(amber)
                    .child(Icon::new(Lucide::GitCommitHorizontal).size_3())
                    .child("detached")
                    .into_any_element()
            }
        }
    }
}

fn host_label(host: &Host) -> SharedString {
    match host {
        Host::Local => "Local Mac".into(),
        Host::Ssh { target } => target.clone().into(),
    }
}

fn primary_badge(fg: Hsla) -> impl IntoElement {
    div()
        .flex_none()
        .h(BADGE_HEIGHT)
        .px_1p5()
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(fg.opacity(0.2))
        .bg(fg.opacity(0.06))
        .text_size(BADGE_TEXT)
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg.opacity(0.7))
        .child("primary")
}

fn host_badge(label: SharedString, fg: Hsla, muted_fg: Hsla) -> impl IntoElement {
    div()
        .flex_none()
        .h(BADGE_HEIGHT)
        .px_1p5()
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(fg.opacity(0.15))
        .bg(fg.opacity(0.06))
        .text_size(BADGE_TEXT)
        .text_color(muted_fg)
        .child(label)
}

/// Restarts whenever `open` flips, because the id changes with it.
fn animation_id(kind: &str, project: &Path, open: bool) -> ElementId {
    SharedString::from(format!("{kind}:{}:{open}", project.display())).into()
}

fn collapse_animation() -> Animation {
    Animation::new(COLLAPSE_DURATION).with_easing(ease_in_out)
}

/// 1.0 = fully open; `t` is the eased animation progress.
fn openness(open: bool, t: f32) -> f32 {
    if open { t } else { 1. - t }
}

/// Placeholder card while `git worktree add` runs.
fn render_creating(cx: &App) -> impl IntoElement {
    h_flex()
        .h(CREATING_HEIGHT)
        .flex_none()
        .mt(ROW_GAP)
        .ml_1()
        .pl_2()
        .gap_2()
        .text_size(META_TEXT)
        .text_color(cx.theme().muted_foreground)
        .child(Icon::new(IconName::LoaderCircle).size_3())
        .child("Creating worktree…")
}

fn copy_item(label: &'static str, text: String) -> PopupMenuItem {
    PopupMenuItem::new(label)
        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())))
}

/// Right-click menu of a worktree card.
fn worktree_menu(
    menu: PopupMenu,
    projects: Entity<Projects>,
    project: &Project,
    worktree: &Worktree,
) -> PopupMenu {
    let is_primary = project.is_primary(worktree);
    let delete_label = if is_primary {
        "Primary worktree — can't be deleted"
    } else {
        "Delete Worktree"
    };
    let (project_path, worktree) = (project.path.clone(), worktree.clone());
    menu.item(copy_item("Copy Path", worktree.path.display().to_string()))
        .when_some(worktree.branch(), |menu, branch| {
            menu.item(copy_item("Copy Branch Name", branch.to_string()))
        })
        .separator()
        .item(
            PopupMenuItem::new(delete_label)
                .disabled(is_primary)
                .on_click(move |_, window, cx| {
                    confirm_delete(projects.clone(), &project_path, &worktree, window, cx)
                }),
        )
}

fn open_create_dialog(
    projects: Entity<Projects>,
    project: &Project,
    window: &mut Window,
    cx: &mut App,
) {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("Leave empty to generate one"));
    let title: SharedString = format!("New worktree in {}", project.name).into();
    let hint: SharedString = format!(
        "A branch of the same name is created from the default branch in {}.",
        project.worktrees_dir().display()
    )
    .into();
    let project_path = project.path.clone();
    // Focus once per opening; the builder runs again whenever the dialog layer rebuilds.
    let initial_focus = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if initial_focus.replace(false) {
            input.focus_handle(cx).focus(window, cx);
        }
        let muted_fg = cx.theme().muted_foreground;
        let (projects, project_path, input_state) =
            (projects.clone(), project_path.clone(), input.clone());
        dialog
            .title(title.clone())
            .w(CREATE_DIALOG_WIDTH)
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child("Name"))
                    .child(Input::new(&input))
                    .child(div().text_xs().text_color(muted_fg).child(hint.clone())),
            )
            .button_props(
                DialogButtonProps::default()
                    .ok_text("Create")
                    .show_cancel(true)
                    .on_ok(move |_, window, cx| {
                        let name = input_state.read(cx).value().to_string();
                        let task =
                            projects.update(cx, |p, cx| p.create_worktree(&project_path, name, cx));
                        report_failure("Couldn't create worktree", task, window, cx);
                        true
                    }),
            )
    });
}

fn confirm_delete(
    projects: Entity<Projects>,
    project: &Path,
    worktree: &Worktree,
    window: &mut Window,
    cx: &mut App,
) {
    let description: SharedString = format!(
        "Remove {} from git and delete its folder. Its branch is deleted only if fully merged.",
        worktree.name()
    )
    .into();
    let (project, worktree) = (project.to_path_buf(), worktree.clone());
    window.open_alert_dialog(cx, move |alert, _, _| {
        let (projects, project, worktree) = (projects.clone(), project.clone(), worktree.clone());
        alert
            .confirm()
            .title("Delete Worktree")
            .description(description.clone())
            .ok_text("Delete")
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, window, cx| {
                delete(
                    projects.clone(),
                    &project,
                    &worktree,
                    Removal::Safe,
                    window,
                    cx,
                );
                true
            })
    });
}

/// Runs the removal; a worktree with changes asks again before forcing it.
fn delete(
    projects: Entity<Projects>,
    project: &Path,
    worktree: &Worktree,
    removal: Removal,
    window: &mut Window,
    cx: &mut App,
) {
    let task = projects.update(cx, |p, cx| {
        p.delete_worktree(project, &worktree.path, removal, cx)
    });
    let (project, worktree) = (project.to_path_buf(), worktree.clone());
    window
        .spawn(cx, async move |cx| {
            let Err(err) = task.await else { return };
            cx.update(|window, cx| match err {
                WorktreeError::HasChanges { .. } => {
                    confirm_force_delete(projects, &project, &worktree, window, cx)
                }
                err => notify_error("Couldn't delete worktree", &err, window, cx),
            })
            .ok();
        })
        .detach();
}

fn confirm_force_delete(
    projects: Entity<Projects>,
    project: &Path,
    worktree: &Worktree,
    window: &mut Window,
    cx: &mut App,
) {
    let description: SharedString = format!(
        "{} has modified or untracked files. Force delete discards them.",
        worktree.name()
    )
    .into();
    let (project, worktree) = (project.to_path_buf(), worktree.clone());
    window.open_alert_dialog(cx, move |alert, _, _| {
        let (projects, project, worktree) = (projects.clone(), project.clone(), worktree.clone());
        alert
            .confirm()
            .title("Worktree Has Changes")
            .description(description.clone())
            .ok_text("Force Delete")
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, window, cx| {
                delete(
                    projects.clone(),
                    &project,
                    &worktree,
                    Removal::Force,
                    window,
                    cx,
                );
                true
            })
    });
}

fn report_failure(
    title: &'static str,
    task: Task<Result<(), WorktreeError>>,
    window: &mut Window,
    cx: &mut App,
) {
    window
        .spawn(cx, async move |cx| {
            if let Err(err) = task.await {
                cx.update(|window, cx| notify_error(title, &err, window, cx))
                    .ok();
            }
        })
        .detach();
}

fn notify_error(title: &'static str, err: &WorktreeError, window: &mut Window, cx: &mut App) {
    tracing::warn!(%err, "{title}");
    window.push_notification(Notification::error(err.to_string()).title(title), cx);
}
