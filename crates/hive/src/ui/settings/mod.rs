//! Settings window (Zed-style): sections of pages on the left, the selected page on the right.
//! Settings dispatch to `Store` or `AiProviders`; views do not perform IO.

mod ai;
mod appearance;
mod files;
mod git;
mod project;
mod shortcuts;
mod terminal;
mod updates;

use hive_core::host::Host;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::Root;
use gpui_kit::component::button::Button;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, TitleBar, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::AppState;
use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _, SmoothHoverBg as _};
use crate::ui::project_badge;
use ai::AiPage;
use appearance::AppearancePage;
use project::ProjectPage;
use shortcuts::ShortcutsPage;

actions!(hive, [OpenSettings]);

const WINDOW_WIDTH: Pixels = px(880.);
const WINDOW_HEIGHT: Pixels = px(600.);
const WINDOW_MIN_WIDTH: Pixels = px(640.);
const WINDOW_MIN_HEIGHT: Pixels = px(420.);
const NAV_WIDTH: Pixels = px(220.);
const NAV_BADGE: Pixels = px(14.);
const NAV_HOVER_OPACITY: f32 = 0.05;
const NAV_SELECTED_OPACITY: f32 = 0.09;
const PAGE_MAX_WIDTH: Pixels = px(680.);
const STEPPER_VALUE_WIDTH: Pixels = px(32.);
/// Font sizes change by whole pixels.
const FONT_SIZE_STEP: f32 = 1.;

/// `cmd-,` (bound in `ui::keymap`) opens the settings from any window.
pub fn init(cx: &mut App) {
    cx.on_action(|_: &OpenSettings, cx| open(cx));
}

/// Brings the settings window to the front, opening it if it isn't open yet.
pub fn open(cx: &mut App) {
    open_at(None, None, cx);
}

/// Opens the settings window on the page of the project at `path`.
pub fn open_project(host: &Host, path: &Path, cx: &mut App) {
    open_at(Some((host.clone(), path.to_path_buf())), None, cx);
}

pub(crate) fn open_git(cx: &mut App) {
    open_at(None, Some(Page::Git), cx);
}

pub(crate) fn open_files(cx: &mut App) {
    open_at(None, Some(Page::Files), cx);
}

pub(crate) fn open_updates(cx: &mut App) {
    open_at(None, Some(Page::Updates), cx);
}

fn open_at(project: Option<(Host, PathBuf)>, page: Option<Page>, cx: &mut App) {
    if let Some(root) = settings_window(cx) {
        let shown = root.update(cx, |root, window, cx| {
            window.activate_window();
            if let Ok(settings) = root.view().clone().downcast::<SettingsWindow>() {
                settings.update(cx, |settings, cx| {
                    if let Some(project) = project {
                        settings.show_project(project, window, cx);
                    }
                    if let Some(page) = page {
                        settings.show_page(page, window, cx);
                    }
                });
            }
        });
        if let Err(err) = shown {
            tracing::warn!(%err, "failed to activate the settings window");
        }
        return;
    }
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Settings".into()),
            ..TitleBar::title_bar_options()
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(WINDOW_WIDTH, WINDOW_HEIGHT),
            cx,
        ))),
        window_min_size: Some(size(WINDOW_MIN_WIDTH, WINDOW_MIN_HEIGHT)),
        ..TitleBar::window_options()
    };
    if let Err(err) = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| {
            let mut settings = SettingsWindow::new(window, cx);
            if let Some(project) = project {
                settings.show_project(project, window, cx);
            }
            if let Some(page) = page {
                settings.show_page(page, window, cx);
            }
            settings
        })
    }) {
        tracing::error!(%err, "failed to open the settings window");
    }
}

/// The open settings window, found among the app's windows by its root view.
fn settings_window(cx: &App) -> Option<WindowHandle<Root>> {
    cx.windows()
        .into_iter()
        .filter_map(|window| window.downcast::<Root>())
        .find(|root| {
            root.read(cx)
                .is_ok_and(|root| root.view().clone().downcast::<SettingsWindow>().is_ok())
        })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Appearance,
    Shortcuts,
    Terminal,
    Ai,
    Git,
    Files,
    Updates,
}

impl Page {
    /// The navigation: section titles and their pages, in order.
    const SECTIONS: [(&'static str, &'static [Page]); 2] = [
        (
            "Application",
            &[Page::Appearance, Page::Shortcuts, Page::Updates],
        ),
        (
            "Workspace",
            &[Page::Terminal, Page::Ai, Page::Git, Page::Files],
        ),
    ];

    fn label(self) -> &'static str {
        match self {
            Page::Appearance => "Appearance",
            Page::Shortcuts => "Shortcuts",
            Page::Terminal => "Terminal",
            Page::Ai => "AI",
            Page::Git => "Git",
            Page::Files => "Files",
            Page::Updates => "Updates",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Page::Appearance => "Theme, fonts and scrollbars of every Hive window.",
            Page::Shortcuts => {
                "Keyboard shortcuts of the main window. Click Edit and press new keys."
            }
            Page::Terminal => "Terminal appearance and the default CLI agent.",
            Page::Git => "Commit generation for every project and worktree.",
            Page::Files => "File appearance and navigation for every local worktree.",
            Page::Updates => "Your version of Hive and software updates.",
            Page::Ai => {
                "Connect LLM providers. Keys are verified online and stored in macOS Keychain."
            }
        }
    }

    fn icon(self) -> Lucide {
        match self {
            Page::Appearance => Lucide::Palette,
            Page::Shortcuts => Lucide::Keyboard,
            Page::Terminal => Lucide::SquareTerminal,
            Page::Ai => Lucide::Sparkles,
            Page::Git => Lucide::GitBranch,
            Page::Files => Lucide::Folder,
            Page::Updates => Lucide::Download,
        }
    }
}

/// What the right side shows: a fixed page or one project's page.
enum View {
    Page(Page),
    Project(Entity<ProjectPage>),
}

pub struct SettingsWindow {
    view: View,
    appearance: Entity<AppearancePage>,
    shortcuts: Entity<ShortcutsPage>,
    ai: Entity<AiPage>,
    git: Entity<git::GitPage>,
    updates: Entity<updates::UpdatesPage>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = AppState::get(cx);
        let (store, projects) = (state.store.clone(), state.projects.clone());
        Self {
            view: View::Page(Page::Appearance),
            appearance: cx.new(|cx| AppearancePage::new(window, cx)),
            shortcuts: cx.new(ShortcutsPage::new),
            ai: cx.new(|cx| AiPage::new(window, cx)),
            git: cx.new(|cx| git::GitPage::new(window, cx)),
            updates: cx.new(updates::UpdatesPage::new),
            _subscriptions: vec![
                cx.observe(&store, |this, _, cx| {
                    this.leave_removed_project(cx);
                    cx.notify();
                }),
                cx.observe(&projects, |_, _, cx| cx.notify()),
            ],
        }
    }

    fn show_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.view, View::Page(shown) if shown == page) {
            return;
        }
        self.git.update(cx, |git, cx| {
            git.save(cx);
            if page == Page::Git {
                git.refresh(cx);
            }
        });
        self.ai.update(cx, |ai, cx| {
            ai.clear(window, cx);
            if page == Page::Ai {
                ai.refresh(cx);
            }
        });
        self.view = View::Page(page);
        cx.notify();
    }

    fn show_project(&mut self, path: (Host, PathBuf), window: &mut Window, cx: &mut Context<Self>) {
        if self.shown_project(cx).is_some_and(|shown| shown == path) {
            return;
        }
        self.git.update(cx, |git, cx| git.save(cx));
        self.ai.update(cx, |ai, cx| ai.clear(window, cx));
        self.view = View::Project(cx.new(|cx| ProjectPage::new(path.0, path.1, window, cx)));
        cx.notify();
    }

    fn shown_project(&self, cx: &App) -> Option<(Host, PathBuf)> {
        match &self.view {
            View::Project(page) => Some((
                page.read(cx).host.clone(),
                page.read(cx).path().to_path_buf(),
            )),
            View::Page(_) => None,
        }
    }

    /// A project removed (here or from the sidebar) takes its page with it.
    fn leave_removed_project(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.shown_project(cx) else {
            return;
        };
        let store = AppState::get(cx).store.read(cx);
        if !store
            .state
            .projects
            .iter()
            .any(|project| project.host == path.0 && project.path == path.1)
        {
            self.view = View::Page(Page::Appearance);
        }
    }

    fn render_nav(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut sections: Vec<Div> = Page::SECTIONS
            .iter()
            .map(|(title, pages)| {
                nav_section(title, cx).children(
                    pages
                        .iter()
                        .map(|page| self.render_nav_item(*page, window, cx)),
                )
            })
            .collect();
        let projects = self.render_nav_projects(window, cx);
        if !projects.is_empty() {
            sections.push(nav_section("Projects", cx).children(projects));
        }
        let theme = cx.theme();
        v_flex()
            .id("settings-navigation")
            .tab_group()
            .w(NAV_WIDTH)
            .h_full()
            .overflow_y_scroll()
            .flex_none()
            .px_2()
            .py_3()
            .gap_5()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .children(sections)
    }

    fn render_nav_item(
        &self,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let hover = Hover::new(page.label(), window, cx);
        let theme = cx.theme();
        let (fg, muted) = (theme.sidebar_foreground, theme.muted_foreground);
        let selected = matches!(self.view, View::Page(shown) if shown == page);
        h_flex()
            .id(page.label())
            .role(Role::Button)
            .aria_label(page.label())
            .tab_index(0)
            .gap_2()
            .px_3()
            .py_1p5()
            .rounded_lg()
            .cursor_pointer()
            .text_sm()
            .text_color(if selected { fg } else { muted })
            .hover_bg(
                &hover,
                nav_idle_bg(selected, fg),
                fg.opacity(NAV_HOVER_OPACITY),
            )
            .focus_visible(|style| style.bg(fg.opacity(NAV_SELECTED_OPACITY)))
            .child(Icon::new(page.icon()).size_4())
            .child(page.label())
            .on_click(cx.listener(move |this, _, window, cx| this.show_page(page, window, cx)))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    this.show_page(page, window, cx);
                }
            }))
    }

    /// One nav item per project, the Projects section of the navigation.
    fn render_nav_projects(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let shown = self.shown_project(cx);
        let theme = cx.theme();
        let (fg, muted) = (theme.sidebar_foreground, theme.muted_foreground);
        let projects = AppState::get(cx).projects.read(cx);
        let items: Vec<_> = projects
            .items
            .iter()
            .enumerate()
            .map(|(ix, project)| {
                let selected = shown
                    .as_ref()
                    .is_some_and(|(host, path)| *host == project.host && *path == project.path);
                let badge = project_badge::render(
                    &project.identity,
                    &project.name,
                    project_badge::color(&project.identity, ix),
                    NAV_BADGE,
                );
                (
                    ix,
                    (project.host.clone(), project.path.clone()),
                    project.name.clone(),
                    badge,
                    selected,
                )
            })
            .collect();
        items
            .into_iter()
            .map(|(ix, path, name, badge, selected)| {
                let hover = Hover::new(("nav-project", ix), window, cx);
                h_flex()
                    .id(("nav-project", ix))
                    .gap_2()
                    .px_3()
                    .py_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(if selected { fg } else { muted })
                    .hover_bg(
                        &hover,
                        nav_idle_bg(selected, fg),
                        fg.opacity(NAV_HOVER_OPACITY),
                    )
                    .child(badge)
                    .child(div().min_w_0().truncate().child(name))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.show_project(path.clone(), window, cx)
                    }))
            })
            .collect()
    }

    fn render_page(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let (title, description, body): (SharedString, SharedString, AnyElement) = match &self.view
        {
            View::Page(page) => {
                let body = match page {
                    Page::Appearance => self.appearance.clone().into_any_element(),
                    Page::Shortcuts => self.shortcuts.clone().into_any_element(),
                    Page::Terminal => terminal::render(window, cx).into_any_element(),
                    Page::Ai => self.ai.clone().into_any_element(),
                    Page::Git => self.git.clone().into_any_element(),
                    Page::Files => files::render(window, cx).into_any_element(),
                    Page::Updates => self.updates.clone().into_any_element(),
                };
                (page.label().into(), page.description().into(), body)
            }
            View::Project(project) => {
                let path = project.read(cx).path();
                let name = AppState::get(cx)
                    .projects
                    .read(cx)
                    .project(&project.read(cx).host, path)
                    .map(|project| project.name.clone())
                    .unwrap_or_default();
                let path = path.display().to_string();
                (name.into(), path.into(), project.clone().into_any_element())
            }
        };
        div()
            .id("settings-page")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .w_full()
                    .max_w(PAGE_MAX_WIDTH)
                    .px_8()
                    .py_6()
                    .gap_4()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(div().text_sm().text_color(muted).child(description)),
                    )
                    .child(body),
            )
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let nav = self.render_nav(window, cx).into_any_element();
        let theme = cx.theme();
        let (bg, fg) = (theme.background, theme.foreground);
        v_flex()
            .size_full()
            .bg(bg)
            .text_color(fg)
            .child(
                TitleBar::new()
                    .bg(bg)
                    .child(div().text_sm().child("Settings")),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(nav)
                    .child(self.render_page(window, cx)),
            )
    }
}

/// A titled group of navigation items.
fn nav_section(title: &str, cx: &App) -> Div {
    v_flex().flex_none().gap_0p5().child(
        div()
            .px_3()
            .pb_1()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(cx.theme().muted_foreground)
            .child(title.to_uppercase()),
    )
}

/// One setting: its title block on the left, its control on the right.
/// A selected nav item keeps its surface; the others are clear until hovered.
fn nav_idle_bg(selected: bool, fg: Hsla) -> Hsla {
    fg.opacity(if selected { NAV_SELECTED_OPACITY } else { 0. })
}

fn setting_row(title: impl IntoElement, control: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .justify_between()
        .items_center()
        .gap_6()
        .py_3()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(title)
        .child(div().flex_none().child(control))
}

/// A setting's name with a muted line explaining it.
fn setting_title(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    cx: &App,
) -> Div {
    v_flex()
        .gap_0p5()
        .min_w_0()
        .child(div().text_sm().child(title.into()))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(description.into()),
        )
}

/// `− 16 +` with a reset button while the size differs from `default`.
fn font_size_stepper(
    id: &'static str,
    value: f32,
    range: RangeInclusive<f32>,
    default: f32,
    set: impl Fn(f32, &mut App) + Clone + 'static,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let [decrease_hover, increase_hover, reset_hover] =
        [0usize, 1, 2].map(|ix| Hover::new((id, ix), window, cx));
    let step = |delta: f32, ix: usize, icon: Lucide, disabled: bool, hover: &Hover| {
        let set = set.clone();
        Button::new((id, ix))
            .cursor_pointer()
            .small()
            .icon(icon)
            .tooltip(if delta < 0. {
                "Decrease font size"
            } else {
                "Increase font size"
            })
            .disabled(disabled)
            .on_click(move |_, _, cx| set(value + delta, cx))
            .smooth_hover(ButtonLook::Ghost, &hover.clone().enabled(!disabled), cx)
    };
    let (at_min, at_max) = (value <= *range.start(), value >= *range.end());
    h_flex()
        .gap_1()
        .items_center()
        .child(step(
            -FONT_SIZE_STEP,
            0,
            Lucide::Minus,
            at_min,
            &decrease_hover,
        ))
        .child(
            div()
                .w(STEPPER_VALUE_WIDTH)
                .text_center()
                .text_sm()
                .child(format!("{value:.0}")),
        )
        .child(step(
            FONT_SIZE_STEP,
            1,
            Lucide::Plus,
            at_max,
            &increase_hover,
        ))
        .when(value != default, |el| {
            let set = set.clone();
            el.child(
                Button::new((id, 2usize))
                    .cursor_pointer()
                    .small()
                    .icon(Lucide::RotateCcw)
                    .tooltip("Reset to default")
                    .on_click(move |_, _, cx| set(default, cx))
                    .smooth_hover(ButtonLook::Ghost, &reset_hover, cx),
            )
        })
}
