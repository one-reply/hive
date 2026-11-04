//! One project's page (Orca's "Identity"): display name, color and badge shown in the sidebar.

use std::path::{Path, PathBuf};

use crate::ui::text_field::TextField;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::Button;
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hive_core::identity::{Badge, ProjectIcon, ProjectIdentity};

use crate::app::AppState;
use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _, SmoothHoverBg as _};
use crate::ui::project_badge::{self, PALETTE};

const PREVIEW_TILE: Pixels = px(40.);
const PREVIEW_BADGE: Pixels = px(20.);
const SWATCH: Pixels = px(28.);
const ICON_TILE: Pixels = px(32.);
const ICON_SIZE: Pixels = px(18.);
const ICONS_PER_ROW: usize = 10;
const AVATAR_TILE: Pixels = px(32.);
const TILE_HOVER_OPACITY: f32 = 0.06;
const TILE_CHOSEN_OPACITY: f32 = 0.12;

/// The badge picker's tabs, in order.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BadgeTab {
    Avatar,
    Icon,
    Emoji,
}

impl BadgeTab {
    const ALL: [BadgeTab; 3] = [BadgeTab::Avatar, BadgeTab::Icon, BadgeTab::Emoji];

    fn label(self) -> &'static str {
        match self {
            BadgeTab::Avatar => "Avatar",
            BadgeTab::Icon => "Icon",
            BadgeTab::Emoji => "Emoji",
        }
    }
}

impl From<&Badge> for BadgeTab {
    fn from(badge: &Badge) -> Self {
        match badge {
            Badge::Avatar => BadgeTab::Avatar,
            Badge::Icon(_) => BadgeTab::Icon,
            Badge::Emoji(_) => BadgeTab::Emoji,
        }
    }
}

/// What the page shows of its project, read fresh from the store on every render.
struct Snapshot {
    identity: ProjectIdentity,
    /// Position in the project list: picks the default color.
    ix: usize,
    name: String,
    is_git: bool,
}

pub(crate) struct ProjectPage {
    path: PathBuf,
    tab: BadgeTab,
    name_input: Entity<InputState>,
    emoji_input: Entity<InputState>,
    custom_color: Entity<ColorPickerState>,
    _subscriptions: Vec<Subscription>,
}

impl ProjectPage {
    pub(crate) fn new(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let identity = identity(&path, cx).unwrap_or_default();
        let emoji = match identity.badge() {
            Badge::Emoji(emoji) => emoji.as_str(),
            _ => "",
        };
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(folder_name(&path))
                .default_value(identity.name().unwrap_or_default().to_owned())
        });
        let emoji_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Type or paste an emoji")
                .default_value(emoji.to_owned())
        });
        let custom_color = cx.new(|cx| {
            let picker = ColorPickerState::new(window, cx);
            match identity.color().filter(|rgb| !PALETTE.contains(rgb)) {
                Some(rgb) => picker.default_value(gpui_kit::rgb(rgb)),
                None => picker,
            }
        });
        let store = AppState::get(cx).store.clone();
        let projects = AppState::get(cx).projects.clone();
        let subscriptions = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.observe(&projects, |_, _, cx| cx.notify()),
            cx.subscribe(&name_input, |this, input, event, cx| {
                if is_commit(event) {
                    let name = input.read(cx).value();
                    this.update_identity(cx, |identity| identity.set_name(&name));
                }
            }),
            cx.subscribe(&emoji_input, |this, input, event, cx| {
                if is_commit(event) {
                    let emoji = input.read(cx).value();
                    this.update_identity(cx, |identity| identity.set_emoji(&emoji));
                }
            }),
            cx.subscribe(&custom_color, |this, _, event, cx| {
                let ColorPickerEvent::Change(Some(color)) = event else {
                    return;
                };
                let rgb = project_badge::to_rgb_hex(*color);
                this.update_identity(cx, |identity| identity.set_color(rgb));
            }),
        ];
        Self {
            path,
            tab: BadgeTab::from(identity.badge()),
            name_input,
            emoji_input,
            custom_color,
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn update_identity(&self, cx: &mut App, edit: impl FnOnce(&mut ProjectIdentity)) {
        let store = AppState::get(cx).store.clone();
        store.update(cx, |store, cx| store.update_identity(&self.path, edit, cx));
    }

    fn snapshot(&self, cx: &App) -> Option<Snapshot> {
        let state = AppState::get(cx);
        let (ix, project) = state
            .store
            .read(cx)
            .state
            .projects
            .iter()
            .enumerate()
            .find(|(_, project)| project.path == self.path)?;
        let scanned = state.projects.read(cx).project(&self.path);
        Some(Snapshot {
            identity: project.identity.clone(),
            ix,
            name: project
                .identity
                .name()
                .map_or_else(|| folder_name(&self.path), str::to_owned),
            is_git: scanned.is_some_and(|project| !project.worktrees.is_empty()),
        })
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update_identity(cx, |identity| *identity = ProjectIdentity::default());
        self.name_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.emoji_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.custom_color
            .update(cx, |picker, cx| picker.clear_value(window, cx));
        self.tab = BadgeTab::Icon;
        cx.notify();
    }

    fn render_header(
        &self,
        snapshot: &Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let remove_hover = Hover::new("remove-project", window, cx);
        let muted = cx.theme().muted_foreground;
        let path = self.path.clone();
        h_flex()
            .justify_between()
            .items_start()
            .child(
                v_flex()
                    .gap_1()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Identity"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child("Project-specific display details for the sidebar and tabs."),
                    )
                    .child(div().text_sm().text_color(muted).child(format!(
                        "Type: {}",
                        if snapshot.is_git { "Git" } else { "Folder" }
                    ))),
            )
            .child(
                Button::new("remove-project")
                    .cursor_pointer()
                    .small()
                    .icon(Lucide::Trash)
                    .tooltip("Remove from Hive")
                    .on_click(move |_, _, cx| {
                        let store = AppState::get(cx).store.clone();
                        store.update(cx, |store, cx| store.remove_project(&path, cx));
                    })
                    .smooth_hover(ButtonLook::Outline, &remove_hover, cx),
            )
    }

    fn render_preview(
        &self,
        snapshot: &Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let reset_hover = Hover::new("reset-identity", window, cx);
        let theme = cx.theme();
        let color = project_badge::color(&snapshot.identity, snapshot.ix);
        let caption = match snapshot.identity.badge() {
            Badge::Icon(ProjectIcon::Folder) => "Folder icon with repo color",
            Badge::Icon(_) => "Icon with repo color",
            Badge::Avatar => "Initial on repo color",
            Badge::Emoji(_) => "Emoji",
        };
        let tile = div()
            .size(PREVIEW_TILE)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .child(project_badge::render(
                &snapshot.identity,
                &snapshot.name,
                color,
                PREVIEW_BADGE,
            ));
        h_flex()
            .justify_between()
            .items_center()
            .child(
                h_flex().gap_3().child(tile).child(
                    v_flex()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child("Repo Icon"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(caption),
                        ),
                ),
            )
            .when(!snapshot.identity.is_default(), |row| {
                row.child(
                    Button::new("reset-identity")
                        .cursor_pointer()
                        .small()
                        .icon(Lucide::RotateCcw)
                        .label("Reset")
                        .on_click(cx.listener(|this, _, window, cx| this.reset(window, cx)))
                        .smooth_hover(ButtonLook::Outline, &reset_hover, cx),
                )
            })
    }

    fn render_colors(&self, snapshot: &Snapshot, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = snapshot.identity.color();
        let ring = cx.theme().foreground;
        let swatches = PALETTE.iter().enumerate().map(|(ix, &rgb)| {
            div()
                .id(("project-color", ix))
                .size(SWATCH)
                .rounded_md()
                .cursor_pointer()
                .bg(gpui_kit::rgb(rgb))
                .when(selected == Some(rgb), |swatch| {
                    swatch.border_2().border_color(ring)
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.update_identity(cx, |identity| identity.set_color(rgb));
                    this.custom_color
                        .update(cx, |picker, cx| picker.clear_value(window, cx));
                }))
        });
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Color"),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .children(swatches)
                    .child(ColorPicker::new(&self.custom_color).small().label("Custom")),
            )
    }

    fn render_badge_picker(
        &self,
        snapshot: &Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = BadgeTab::ALL
            .iter()
            .position(|tab| *tab == self.tab)
            .unwrap_or_default();
        let tabs = TabBar::new("badge-tabs")
            .underline()
            .selected_index(selected)
            .children(BadgeTab::ALL.map(|tab| Tab::new().label(tab.label())))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.tab = BadgeTab::ALL[*ix];
                cx.notify();
            }));
        let body = match self.tab {
            BadgeTab::Avatar => self.render_avatar(snapshot, window, cx).into_any_element(),
            BadgeTab::Icon => self.render_icons(snapshot, window, cx).into_any_element(),
            BadgeTab::Emoji => {
                TextField::input(Input::new(&self.emoji_input), &self.emoji_input, cx)
                    .into_any_element()
            }
        };
        v_flex().gap_3().child(tabs).child(body)
    }

    fn render_avatar(
        &self,
        snapshot: &Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let color = project_badge::color(&snapshot.identity, snapshot.ix);
        let chosen = *snapshot.identity.badge() == Badge::Avatar;
        tile("avatar", chosen, window, cx)
            .child(project_badge::avatar(
                &snapshot.name,
                color,
                AVATAR_TILE * 0.7,
            ))
            .on_click(cx.listener(|this, _, _, cx| {
                this.update_identity(cx, |identity| identity.set_badge(Badge::Avatar));
            }))
    }

    fn render_icons(
        &self,
        snapshot: &Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows = ProjectIcon::ALL
            .chunks(ICONS_PER_ROW)
            .enumerate()
            .map(|(row, icons)| {
                h_flex()
                    .gap_2()
                    .children(icons.iter().enumerate().map(|(col, &icon)| {
                        let chosen = *snapshot.identity.badge() == Badge::Icon(icon);
                        tile(
                            ("project-icon", row * ICONS_PER_ROW + col),
                            chosen,
                            window,
                            cx,
                        )
                        .child(Icon::new(project_badge::lucide(icon)).size(ICON_SIZE))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_identity(cx, |identity| {
                                identity.set_badge(Badge::Icon(icon))
                            });
                        }))
                    }))
            });
        v_flex().gap_2().children(rows)
    }
}

impl Render for ProjectPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The window swaps this page out once the project is removed; until then, nothing.
        let Some(snapshot) = self.snapshot(cx) else {
            return div().into_any_element();
        };
        v_flex()
            .gap_5()
            .child(self.render_header(&snapshot, window, cx))
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Display Name"),
                    )
                    .child(TextField::input(
                        Input::new(&self.name_input),
                        &self.name_input,
                        cx,
                    )),
            )
            .child(self.render_preview(&snapshot, window, cx))
            .child(self.render_colors(&snapshot, cx))
            .child(self.render_badge_picker(&snapshot, window, cx))
            .into_any_element()
    }
}

/// A square picker cell, highlighted when `chosen`.
fn tile(
    id: impl Into<ElementId>,
    chosen: bool,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let id: ElementId = id.into();
    let hover = Hover::new(id.clone(), window, cx);
    let fg = cx.theme().foreground;
    let idle = fg.opacity(if chosen { TILE_CHOSEN_OPACITY } else { 0. });
    div()
        .id(id)
        .size(ICON_TILE)
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .cursor_pointer()
        .hover_bg(&hover, idle, fg.opacity(TILE_HOVER_OPACITY))
}

/// Text fields save on Enter or when they lose focus, not per keystroke: every save rescans
/// all projects with git.
fn is_commit(event: &InputEvent) -> bool {
    matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur)
}

fn identity(path: &Path, cx: &App) -> Option<ProjectIdentity> {
    let store = AppState::get(cx).store.read(cx);
    let project = store.state.projects.iter().find(|p| p.path == path)?;
    Some(project.identity.clone())
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
