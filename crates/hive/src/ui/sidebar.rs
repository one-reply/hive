//! Project tree (Orca-style): colored project headers → worktree cards. Sessions will hang
//! under worktrees.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::ui::text_field::TextField;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::motion::{Transition, transition};
use gpui_kit::component::badge::Badge;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hive_core::host::Host;
use hive_core::projects::{Project, Worktree};
use hive_core::state::WorktreeFilter;
use hive_core::worktree::{Removal, WorktreeError};

mod agents;
mod create;
mod pull_requests;
use agents::{AgentsSection, CardTarget};
mod reorder;
use reorder::DraggedProject;

#[cfg(test)]
mod header_tests;

use crate::app::AppState;
use crate::state::projects::Projects;
use crate::state::store::Store;
use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _, SmoothHoverBg as _};
use crate::ui::stepped_spinner::SteppedSpinner;
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
/// The badge's line box fills exactly the space inside its 1px border; the theme's taller
/// default line height overflows the badge and pushes the label below its center.
const BADGE_LINE_HEIGHT: Pixels = px(14.);
/// "primary" is all x-height with a descender, so a box centered on the title's caps and a
/// label centered in its line box both read high/low by about a pixel: the box drops by this
/// much to center on the title's lowercase, and the label rises by the same amount inside it.
const PRIMARY_BADGE_OPTICAL_SHIFT: Pixels = px(1.);
const STATUS_DOT: Pixels = px(8.);
const PROJECT_BADGE: Pixels = px(14.);
/// Rows under a project header have fixed heights, so the collapse animation knows the
/// height of the list without measuring it.
const CARD_HEIGHT: Pixels = px(34.);
const CARD_HEIGHT_WITH_META: Pixels = px(58.);
const CREATING_HEIGHT: Pixels = px(28.);
const ROW_GAP: Pixels = px(2.);
const COLLAPSE_DURATION: Duration = Duration::from_millis(200);
/// How long a folded or unfolded list keeps its animation mounted. The animation starts on the
/// first frame after the click, so this outlasts it; by then it rests at its final height, and
/// dropping it changes nothing on screen but stops a folded list from building hidden rows.
const COLLAPSE_SETTLED: Duration = Duration::from_millis(400);
const SELECTION_DURATION: Duration = Duration::from_millis(200);
const SELECTED_BACKGROUND_OPACITY: f32 = 0.09;
const SELECTED_BORDER_OPACITY: f32 = 0.14;
const HOVER_BACKGROUND_OPACITY: f32 = 0.05;

pub struct Sidebar {
    projects: Entity<Projects>,
    store: Entity<Store>,
    /// Startup visibility, then manual changes; background activity never unfolds projects.
    collapsed: HashSet<(Host, PathBuf)>,
    /// When projects were last folded or unfolded: only those animate, so the tree doesn't
    /// play its animations on startup.
    toggled: HashMap<(Host, PathBuf), Instant>,
    /// Cards whose agents strip is unfolded into one row per session.
    agents_open: HashSet<(Host, PathBuf)>,
    /// Turns the Working indicators of visible agents; idle while none is on screen.
    working: SteppedSpinner,
    _observers: [Subscription; 4],
}

impl Sidebar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (projects, store, agents) = {
            let app = AppState::get(cx);
            (app.projects.clone(), app.store.clone(), app.agents.clone())
        };
        let pull_requests = AppState::get(cx).pull_requests.clone();
        let collapsed = {
            let state = &store.read(cx).state;
            state
                .projects
                .iter()
                .filter(|project| !project.expanded_on_start(&state.workspace))
                .map(|project| (project.host.clone(), project.path.clone()))
                .collect()
        };
        let observers = [
            cx.observe(&pull_requests, |_, _, cx| cx.notify()),
            cx.observe(&projects, |_, _, cx| cx.notify()),
            cx.observe(&agents, |_, _, cx| cx.notify()),
            cx.observe(&store, |this, _, cx| this.forget_removed_projects(cx)),
        ];
        Self {
            projects,
            store,
            collapsed,
            toggled: HashMap::new(),
            agents_open: HashSet::new(),
            working: SteppedSpinner::new(cx),
            _observers: observers,
        }
    }

    fn add_projects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::ui::add_project::open(window, cx);
    }

    fn forget_removed_projects(&mut self, cx: &mut Context<Self>) {
        let projects = &self.store.read(cx).state.projects;
        let registered = |(host, path): &(Host, PathBuf)| {
            projects.iter().any(|p| &p.host == host && &p.path == path)
        };
        let before = self.collapsed.len();
        self.collapsed.retain(registered);
        self.toggled.retain(|key, _| registered(key));
        if self.collapsed.len() != before {
            cx.notify();
        }
    }

    fn toggle_collapsed(&mut self, path: &(Host, PathBuf), cx: &mut Context<Self>) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.clone());
        }
        let expanded = !self.collapsed.contains(path);
        self.store.update(cx, |store, cx| {
            store.set_project_expanded(&path.0, &path.1, expanded, cx)
        });
        let now = Instant::now();
        self.toggled
            .retain(|_, toggled_at| collapse_animating(Some(*toggled_at), now));
        self.toggled.insert(path.clone(), now);
        cx.notify();
    }

    /// A project header and its worktree list, which slides open and closed.
    fn render_project(
        &self,
        ix: usize,
        project: &Project,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = !self
            .collapsed
            .contains(&(project.host.clone(), project.path.clone()));
        let animate = collapse_animating(
            self.toggled
                .get(&(project.host.clone(), project.path.clone()))
                .copied(),
            Instant::now(),
        );
        let header = self
            .render_header(ix, project, open, window, cx)
            .into_any_element();
        let group = self.project_drop_target(Some(project)).child(header);
        if !open && !animate {
            return group.into_any_element();
        }
        let rows = self.render_rows(project, window, cx);
        let height = rows
            .iter()
            .map(|(_, height)| *height + ROW_GAP)
            .sum::<Pixels>();
        let list = v_flex()
            .overflow_hidden()
            .children(rows.into_iter().map(|(row, _)| row));
        let list = if animate {
            list.with_animation(
                animation_id(
                    &format!("project-list:{:?}", project.host),
                    &project.path,
                    open,
                ),
                collapse_animation(),
                move |list, t| list.h(height * openness(open, t)),
            )
            .into_any_element()
        } else {
            list.into_any_element()
        };
        group.child(list).into_any_element()
    }

    /// Cards under a project header with their heights.
    fn render_rows(
        &self,
        project: &Project,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<(AnyElement, Pixels)> {
        let mut rows = vec![];
        if let Some(error) = self
            .projects
            .read(cx)
            .errors
            .get(&(project.host.clone(), project.path.clone()))
            .cloned()
        {
            let hover = Hover::new(
                SharedString::from(format!("scan-retry:{:?}:{:?}", project.host, project.path)),
                window,
                cx,
            );
            let projects = self.projects.clone();
            rows.push((
                Button::new(SharedString::from(format!(
                    "retry:{:?}:{:?}",
                    project.host, project.path
                )))
                .cursor_pointer()
                .small()
                .label("Scan failed · Retry")
                .tooltip(error)
                .on_click(move |_, _, cx| {
                    projects.update(cx, |projects, cx| projects.rescan(cx).detach())
                })
                .smooth_hover(ButtonLook::Ghost, &hover, cx)
                .into_any_element(),
                CREATING_HEIGHT,
            ));
        }
        let host = host_label(&project.host);
        if project.worktrees.is_empty() {
            let target = CardTarget {
                host: &project.host,
                project: &project.path,
                worktree: None,
                path: &project.path,
            };
            let agents = self.agents_section(target, window, cx);
            let projects = self.projects.read(cx);
            let folder = WorktreeCard {
                path: project.path.clone(),
                title: project.name.clone().into(),
                meta: Some(Meta::Path(project.path.display().to_string().into())),
                is_primary: false,
                is_active: projects.is_active(&project.host, &project.path),
                is_selected: projects.is_selected(&project.host, &project.path, None),
                agents,
            };
            rows.push(self.render_card(project, None, folder, host.as_ref(), window, cx));
        }
        for (_, worktree) in project.visible_worktrees() {
            let target = CardTarget {
                host: &project.host,
                project: &project.path,
                worktree: Some(&worktree.path),
                path: &worktree.path,
            };
            let agents = self.agents_section(target, window, cx);
            let card = WorktreeCard::new(project, worktree, self.projects.read(cx), agents);
            rows.push(self.render_card(project, Some(worktree), card, host.as_ref(), window, cx));
        }
        for _ in 0..self
            .projects
            .read(cx)
            .creating_count(&project.host, &project.path)
        {
            rows.push((render_creating(cx).into_any_element(), CREATING_HEIGHT));
        }
        rows
    }

    fn render_header(
        &self,
        ix: usize,
        project: &Project,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // An open project shows its selected card; a folded one carries the highlight itself.
        let holds_selection = self
            .projects
            .read(cx)
            .selection()
            .is_some_and(|(selected, _)| {
                selected.host == project.host && selected.path == project.path
            });
        let highlight = transition(
            format!("project-selection:{:?}:{:?}", project.host, project.path),
            if holds_selection && !open {
                1.0_f32
            } else {
                0.
            },
            Transition::new(SELECTION_DURATION).ease(ease_in_out),
            window,
            cx,
        );
        let fg = cx.theme().sidebar_foreground;
        let header_hover = Hover::new(("project", ix), window, cx);
        let new_worktree_hover = Hover::new(("new-worktree", ix), window, cx);
        let collapse_hover = Hover::new(("collapse-project", ix), window, cx);
        let menu_open = window.use_keyed_state(
            SharedString::from(format!(
                "project-menu-open:{:?}:{:?}",
                project.host, project.path
            )),
            cx,
            |_, _| false,
        );
        let is_menu_open = *menu_open.read(cx);
        let header_focus = window
            .use_keyed_state(
                SharedString::from(format!(
                    "project-focus:{:?}:{:?}",
                    project.host, project.path
                )),
                cx,
                |_, cx| cx.focus_handle(),
            )
            .read(cx)
            .clone();
        let keyboard_focus =
            window.last_input_was_keyboard() && header_focus.contains_focused(window, cx);
        let path = project.path.clone();
        let host = project.host.clone();
        let collapse_target = (host.clone(), path.clone());
        let collapse_label = format!(
            "{} {}",
            if open { "Collapse" } else { "Expand" },
            project.name
        );
        let unfolded = transition(
            format!("project-chevron:{:?}:{:?}", project.host, project.path),
            if open { 1.0_f32 } else { 0. },
            Transition::new(COLLAPSE_DURATION).ease(ease_in_out),
            window,
            cx,
        );
        let collapse = Button::new(("collapse-project", ix))
            .cursor_pointer()
            .small()
            .icon(
                Icon::new(Lucide::ChevronRight)
                    .rotate(radians(std::f32::consts::FRAC_PI_2 * unfolded)),
            )
            .accessibility_label(collapse_label.clone())
            .tooltip(collapse_label)
            .on_click(cx.listener(move |this, _, _, cx| {
                // Folding must not select the project behind the button.
                cx.stop_propagation();
                this.toggle_collapsed(&collapse_target, cx);
            }))
            .smooth_hover(ButtonLook::Ghost, &collapse_hover, cx);
        let actions = h_flex()
            .flex_none()
            .pl_1()
            .gap_0p5()
            .child(self.project_menu(ix, project, menu_open))
            .when(!project.worktrees.is_empty(), |s| {
                let (projects, project) = (self.projects.clone(), project.clone());
                s.child(
                    Button::new(("new-worktree", ix))
                        .cursor_pointer()
                        .xsmall()
                        .icon(IconName::Plus)
                        .tooltip(format!("New worktree in {}", project.name))
                        .on_click(move |_, window, cx| {
                            // Creating a worktree must not select the project behind the button.
                            cx.stop_propagation();
                            create::open(projects.clone(), &project, window, cx)
                        })
                        .smooth_hover(ButtonLook::Ghost, &new_worktree_hover, cx),
                )
            })
            .child(collapse)
            .opacity(if is_menu_open || keyboard_focus {
                1.
            } else {
                header_hover.progress()
            });
        h_flex()
            .id(("project", ix))
            .role(Role::Button)
            .aria_label(format!("Open project {}", project.name))
            .track_focus(&header_focus)
            .tab_index(0)
            .on_hover(header_hover.listener())
            .h(HEADER_HEIGHT)
            .gap_1p5()
            .pl(HEADER_INSET)
            // Mirrors the folder icon's inset from the card's left edge.
            .pr_1p5()
            .rounded_md()
            .bg(fg.opacity(SELECTED_BACKGROUND_OPACITY * highlight))
            .cursor_pointer()
            .on_drag(DraggedProject::new(project, ix), |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            })
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
            .child(actions)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.projects
                    .update(cx, |projects, cx| projects.select_project(&host, &path, cx));
            }))
    }

    /// The "…" menu of a project header.
    fn project_menu(
        &self,
        ix: usize,
        project: &Project,
        menu_open: Entity<bool>,
    ) -> impl IntoElement {
        let store = self.store.clone();
        let path = project.path.clone();
        let host = project.host.clone();
        let hidden = project.hidden_worktree_count();
        let filter_label: SharedString = match project.filter {
            WorktreeFilter::Hive => format!("Show hidden worktrees ({hidden})").into(),
            WorktreeFilter::All => "Hide non-Hive worktrees".into(),
        };
        let can_toggle = project.filter == WorktreeFilter::All || hidden > 0;
        let filter_icon = match project.filter {
            WorktreeFilter::Hive => Lucide::Eye,
            WorktreeFilter::All => Lucide::EyeOff,
        };
        Button::new(("project-menu", ix))
            .cursor_pointer()
            .ghost()
            .xsmall()
            .icon(Lucide::Ellipsis)
            .dropdown_menu(move |menu, _, _| {
                let (store, path, host) = (store.clone(), path.clone(), host.clone());
                menu.item(
                    PopupMenuItem::new(filter_label.clone())
                        .icon(filter_icon)
                        .disabled(!can_toggle)
                        .on_click({
                            let (store, path, host) = (store.clone(), path.clone(), host.clone());
                            move |_, _, cx| {
                                store.update(cx, |s, cx| s.toggle_worktree_filter(&host, &path, cx))
                            }
                        }),
                )
                .item(copy_item(
                    "Copy Path",
                    Lucide::Copy,
                    path.display().to_string(),
                ))
                .item(
                    PopupMenuItem::new("Project Settings")
                        .icon(Lucide::Settings)
                        .on_click({
                            let path = path.clone();
                            let host = host.clone();
                            move |_, _, cx| settings::open_project(&host, &path, cx)
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Remove Project")
                        .icon(Lucide::FolderMinus)
                        .on_click(move |_, window, cx| {
                            let (store, path, host) = (store.clone(), path.clone(), host.clone());
                            crate::ui::editor_panel::close::guard(
                                crate::ui::editor_panel::close::Scope::Project(
                                    host.clone(),
                                    path.clone(),
                                ),
                                window,
                                cx,
                                move |_, cx| {
                                    store.update(cx, |s, cx| s.remove_project(&host, &path, cx))
                                },
                            );
                        }),
                )
            })
            .on_open_change(move |open, _, cx| {
                menu_open.update(cx, |state, cx| {
                    *state = *open;
                    cx.notify();
                });
            })
    }

    fn render_card(
        &self,
        project: &Project,
        worktree: Option<&Worktree>,
        card: WorktreeCard,
        host: Option<&SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) -> (AnyElement, Pixels) {
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
        let meta_row = (host.is_some() || card.meta.is_some()).then(|| {
            h_flex()
                .gap_1p5()
                // Under the title, past the status dot and its gap.
                .pl(STATUS_DOT + px(8.))
                .text_size(META_TEXT)
                .text_color(muted_fg)
                .when_some(host, |row, host| {
                    row.child(host_badge(host.clone(), fg, muted_fg))
                })
                .children(card.meta.map(Meta::render))
        });
        let header_height = if meta_row.is_some() {
            CARD_HEIGHT_WITH_META
        } else {
            CARD_HEIGHT
        };
        let pr = worktree.map(|worktree| self.pr_card(project, worktree, window, cx));
        let height = header_height
            + card.agents.height
            + if pr.is_some() {
                pull_requests::HEIGHT
            } else {
                px(0.)
            };

        let projects = self.projects.clone();
        let project_path = project.path.clone();
        let project_host = project.host.clone();
        let worktree_path = worktree.map(|w| w.path.clone());
        let menu_target = worktree.map(|w| (project.clone(), w.clone()));
        // A stable channel retargets from the current opacity on rapid selection changes.
        let selection = transition(
            format!(
                "worktree-selection:{:?}:{:?}:{:?}",
                project.host, project.path, card.path
            ),
            if card.is_selected { 1.0_f32 } else { 0. },
            Transition::new(SELECTION_DURATION).ease(ease_in_out),
            window,
            cx,
        );
        let hover = HOVER_BACKGROUND_OPACITY
            + (SELECTED_BACKGROUND_OPACITY - HOVER_BACKGROUND_OPACITY) * selection;
        let card_id = SharedString::from(format!(
            "worktree:{:?}:{}",
            project.host,
            card.path.display()
        ));
        let card_hover = Hover::new(card_id.clone(), window, cx);
        let element = v_flex()
            .id(card_id)
            .role(Role::Button)
            .aria_label(format!("Open {}: {}", project.name, card.title))
            .h(height)
            .flex_none()
            .mt(ROW_GAP)
            .ml_1()
            .pl_2()
            .pr_1p5()
            .rounded_lg()
            .border_1()
            .hover_bg(
                &card_hover,
                fg.opacity(SELECTED_BACKGROUND_OPACITY * selection),
                fg.opacity(hover),
            )
            .border_color(fg.opacity(SELECTED_BORDER_OPACITY * selection))
            .cursor_pointer()
            .child(
                v_flex()
                    // The card's own border takes a pixel at the top and the bottom.
                    .h(header_height - px(2.))
                    .flex_none()
                    .justify_center()
                    .gap_1p5()
                    .child(title_row)
                    .children(meta_row),
            )
            .children(pr)
            .children(card.agents.element)
            .on_click(move |_, _, cx| {
                projects.update(cx, |p, cx| {
                    p.select(&project_host, &project_path, worktree_path.as_deref(), cx)
                })
            });
        let Some((project, worktree)) = menu_target else {
            return (element.into_any_element(), height);
        };
        let projects = self.projects.clone();
        (
            element
                .context_menu(move |menu, _, _| {
                    worktree_menu(menu, projects.clone(), &project, &worktree)
                })
                .into_any_element(),
            height,
        )
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.projects.read(cx).items.clone();
        let rows: Vec<AnyElement> = items
            .iter()
            .enumerate()
            .map(|(ix, project)| self.render_project(ix, project, window, cx))
            .collect();
        // Every Working indicator is built by now: folded projects and rows build none.
        self.working.keep_ticking(cx);
        let add_hover = Hover::new("add-project", window, cx);
        let attention_hover = Hover::new("needs-attention", window, cx);
        let attention = AppState::get(cx)
            .agents
            .read(cx)
            .all()
            .filter(|(_, s)| s.unread)
            .count();
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
                        h_flex()
                            .gap_1()
                            .child(
                                // The badge hides itself at zero, leaving a quiet bell.
                                Badge::new().count(attention).child(
                                    Button::new("needs-attention")
                                        .cursor_pointer()
                                        .small()
                                        .icon(Lucide::Bell)
                                        .tooltip(format!("Needs attention · {attention}"))
                                        .on_click(|_, window, cx| {
                                            crate::ui::workspace::switcher::open(true, window, cx)
                                        })
                                        .smooth_hover(ButtonLook::Ghost, &attention_hover, cx),
                                ),
                            )
                            .child(
                                Button::new("add-project")
                                    .cursor_pointer()
                                    .small()
                                    .icon(Lucide::FolderPlus)
                                    .tooltip("Add project")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_projects(window, cx)
                                    }))
                                    .smooth_hover(ButtonLook::Ghost, &add_hover, cx),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .id("project-tree")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .child(
                        // An explicit min height drops the flex item's `min-height: auto`, so
                        // without `flex_shrink_0` the list shrinks to the viewport and never
                        // overflows it, leaving nothing to scroll.
                        v_flex()
                            .flex_shrink_0()
                            .min_h_full()
                            .children(rows)
                            .child(self.project_drop_target(None).flex_1().min_h(HEADER_HEIGHT)),
                    ),
            )
    }
}

/// What a worktree card shows, resolved from the project, the worktree and app state.
struct WorktreeCard {
    path: PathBuf,
    title: SharedString,
    meta: Option<Meta>,
    is_primary: bool,
    is_active: bool,
    is_selected: bool,
    agents: AgentsSection,
}

/// Details that are not already shown in the title.
enum Meta {
    Detached,
    /// A folder that is not a git repository.
    Path(SharedString),
}

impl WorktreeCard {
    fn new(
        project: &Project,
        worktree: &Worktree,
        projects: &Projects,
        agents: AgentsSection,
    ) -> Self {
        let (title, meta) = match worktree.branch() {
            Some(branch) => (SharedString::from(branch.to_string()), None),
            None => (worktree.name().into(), Some(Meta::Detached)),
        };
        Self {
            path: worktree.path.clone(),
            title,
            meta,
            is_primary: project.is_primary(worktree),
            is_active: projects.is_active(&project.host, &worktree.path),
            is_selected: projects.is_selected(&project.host, &project.path, Some(&worktree.path)),
            agents,
        }
    }
}

impl Meta {
    fn render(self) -> AnyElement {
        match self {
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
                    .line_height(BADGE_LINE_HEIGHT)
                    .text_color(amber)
                    .child(Icon::new(Lucide::GitCommitHorizontal).size_3())
                    .child("detached")
                    .into_any_element()
            }
        }
    }
}

fn host_label(host: &Host) -> Option<SharedString> {
    match host {
        Host::Local => None,
        Host::Ssh { target } => Some(target.clone().into()),
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
        .relative()
        .top(PRIMARY_BADGE_OPTICAL_SHIFT)
        .border_color(fg.opacity(0.2))
        .bg(fg.opacity(0.06))
        .text_size(BADGE_TEXT)
        .line_height(BADGE_LINE_HEIGHT)
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg.opacity(0.7))
        .child(
            div()
                .relative()
                .top(-PRIMARY_BADGE_OPTICAL_SHIFT)
                .child("primary"),
        )
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
        .line_height(BADGE_LINE_HEIGHT)
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

/// Whether a project list toggled at `toggled_at` still needs its fold animation.
fn collapse_animating(toggled_at: Option<Instant>, now: Instant) -> bool {
    toggled_at.is_some_and(|at| now.saturating_duration_since(at) < COLLAPSE_SETTLED)
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
        .child(Spinner::new().icon(IconName::LoaderCircle).xsmall())
        .child("Creating worktree…")
}

fn copy_item(label: &'static str, icon: Lucide, text: String) -> PopupMenuItem {
    PopupMenuItem::new(label)
        .icon(icon)
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
    let (project_path, worktree) = (
        (project.host.clone(), project.path.clone()),
        worktree.clone(),
    );
    let history = history_item(projects.clone(), &project_path, &worktree.path);
    menu.item(copy_item(
        "Copy Path",
        Lucide::Copy,
        worktree.path.display().to_string(),
    ))
    .when_some(worktree.branch(), |menu, branch| {
        menu.item(copy_item(
            "Copy Branch Name",
            Lucide::GitBranch,
            branch.to_string(),
        ))
    })
    .separator()
    .item(history)
    .separator()
    .item(
        PopupMenuItem::new(delete_label)
            .icon(Lucide::Trash)
            .disabled(is_primary)
            .on_click(move |_, window, cx| {
                let (projects, project_path, worktree) =
                    (projects.clone(), project_path.clone(), worktree.clone());
                crate::ui::editor_panel::close::guard(
                    crate::ui::editor_panel::close::Scope::Worktree(
                        project_path.0.clone(),
                        worktree.path.clone(),
                    ),
                    window,
                    cx,
                    move |window, cx| {
                        confirm_delete(projects, &project_path, &worktree, window, cx)
                    },
                );
            }),
    )
}

/// Selects the worktree first: the history dialog and the tab it opens act on the selection.
fn history_item(
    projects: Entity<Projects>,
    project: &(Host, PathBuf),
    worktree: &Path,
) -> PopupMenuItem {
    let local = project.0.is_local();
    let label = if local {
        "Conversation History…"
    } else {
        "Conversation History — local projects only"
    };
    let (project, worktree) = (project.clone(), worktree.to_path_buf());
    PopupMenuItem::new(label)
        .icon(Lucide::MessagesSquare)
        .disabled(!local)
        .on_click(move |_, window, cx| {
            projects.update(cx, |projects, cx| {
                projects.select(&project.0, &project.1, Some(&worktree), cx)
            });
            crate::ui::workspace::history::open_selected(window, cx);
        })
}

fn confirm_delete(
    projects: Entity<Projects>,
    project: &(Host, PathBuf),
    worktree: &Worktree,
    window: &mut Window,
    cx: &mut App,
) {
    let description: SharedString = format!(
        "Remove {} from git and delete its folder. Its branch is deleted only if fully merged.",
        worktree.name()
    )
    .into();
    let (project, worktree) = (project.clone(), worktree.clone());
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
    project: &(Host, PathBuf),
    worktree: &Worktree,
    removal: Removal,
    window: &mut Window,
    cx: &mut App,
) {
    let task = projects.update(cx, |p, cx| {
        p.delete_worktree(&project.0, &project.1, &worktree.path, removal, cx)
    });
    let (project, worktree) = (project.clone(), worktree.clone());
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
    project: &(Host, PathBuf),
    worktree: &Worktree,
    window: &mut Window,
    cx: &mut App,
) {
    let description: SharedString = format!(
        "{} has modified or untracked files. Force delete discards them.",
        worktree.name()
    )
    .into();
    let (project, worktree) = (project.clone(), worktree.clone());
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

fn notify_error(title: &'static str, err: &WorktreeError, window: &mut Window, cx: &mut App) {
    tracing::warn!(%err, "{title}");
    window.push_notification(Notification::error(err.to_string()).title(title), cx);
}

#[cfg(test)]
mod tests {
    use super::{COLLAPSE_DURATION, COLLAPSE_SETTLED, collapse_animating};
    use std::time::{Duration, Instant};

    #[test]
    fn collapse_animates_only_until_it_settles() {
        let toggled_at = Instant::now();
        let after = |ms| toggled_at + Duration::from_millis(ms);
        assert!(!collapse_animating(None, toggled_at));
        assert!(collapse_animating(Some(toggled_at), toggled_at));
        assert!(collapse_animating(Some(toggled_at), after(399)));
        assert!(!collapse_animating(Some(toggled_at), after(400)));
        assert!(
            COLLAPSE_SETTLED > COLLAPSE_DURATION,
            "the animation must finish before it is dropped"
        );
    }

    #[test]
    fn collapse_toggled_in_the_future_still_animates() {
        let now = Instant::now();
        assert!(collapse_animating(
            Some(now + Duration::from_millis(5)),
            now
        ));
    }
}
