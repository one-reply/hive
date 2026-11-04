//! Settings window (Zed-style): sections of pages on the left, the selected page on the right.
//! Controls write straight to the `Store`; the theme and keymap observers apply each change to
//! every window at once, so the effect is visible while the window is open.

mod appearance;
mod project;
mod projects;
mod shortcuts;
mod terminal;

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::Root;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, TitleBar, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::AppState;
use crate::ui::project_badge;
use project::ProjectPage;
use shortcuts::ShortcutsPage;

actions!(hive, [OpenSettings]);

const WINDOW_WIDTH: Pixels = px(880.);
const WINDOW_HEIGHT: Pixels = px(600.);
const WINDOW_MIN_WIDTH: Pixels = px(640.);
const WINDOW_MIN_HEIGHT: Pixels = px(420.);
const NAV_WIDTH: Pixels = px(220.);
const NAV_PROJECT_INDENT: Pixels = px(20.);
const NAV_BADGE: Pixels = px(14.);
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
    open_at(None, cx);
}

/// Opens the settings window on the page of the project at `path`.
pub fn open_project(path: &Path, cx: &mut App) {
    open_at(Some(path.to_path_buf()), cx);
}

fn open_at(project: Option<PathBuf>, cx: &mut App) {
    if let Some(root) = settings_window(cx) {
        let shown = root.update(cx, |root, window, cx| {
            window.activate_window();
            let Some(project) = project else { return };
            if let Ok(settings) = root.view().clone().downcast::<SettingsWindow>() {
                settings.update(cx, |settings, cx| {
                    settings.show_project(project, window, cx)
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
    Projects,
    Terminal,
}

impl Page {
    /// The navigation: section titles and their pages, in order.
    const SECTIONS: [(&'static str, &'static [Page]); 2] = [
        ("Interface", &[Page::Appearance, Page::Shortcuts]),
        ("Workspace", &[Page::Projects, Page::Terminal]),
    ];

    fn label(self) -> &'static str {
        match self {
            Page::Appearance => "Appearance",
            Page::Shortcuts => "Shortcuts",
            Page::Projects => "Projects",
            Page::Terminal => "Terminal",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Page::Appearance => "Theme and text size of every Hive window.",
            Page::Shortcuts => {
                "Keyboard shortcuts of the main window. Click Edit and press new keys."
            }
            Page::Projects => "Folders listed in the sidebar.",
            Page::Terminal => "How terminal tabs look.",
        }
    }

    fn icon(self) -> Lucide {
        match self {
            Page::Appearance => Lucide::Palette,
            Page::Shortcuts => Lucide::Keyboard,
            Page::Projects => Lucide::Folder,
            Page::Terminal => Lucide::SquareTerminal,
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
    shortcuts: Entity<ShortcutsPage>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsWindow {
    fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = AppState::get(cx);
        let (store, projects) = (state.store.clone(), state.projects.clone());
        Self {
            view: View::Page(Page::Appearance),
            shortcuts: cx.new(ShortcutsPage::new),
            _subscriptions: vec![
                cx.observe(&store, |this, _, cx| {
                    this.leave_removed_project(cx);
                    cx.notify();
                }),
                cx.observe(&projects, |_, _, cx| cx.notify()),
            ],
        }
    }

    fn show_page(&mut self, page: Page, cx: &mut Context<Self>) {
        self.view = View::Page(page);
        cx.notify();
    }

    fn show_project(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.shown_project(cx).is_some_and(|shown| shown == path) {
            return;
        }
        self.view = View::Project(cx.new(|cx| ProjectPage::new(path, window, cx)));
        cx.notify();
    }

    fn shown_project(&self, cx: &App) -> Option<PathBuf> {
        match &self.view {
            View::Project(page) => Some(page.read(cx).path().to_path_buf()),
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
            .any(|project| project.path == path)
        {
            self.view = View::Page(Page::Projects);
        }
    }

    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let sections: Vec<Div> = Page::SECTIONS
            .iter()
            .map(|(title, pages)| {
                v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .px_3()
                            .pb_1()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(muted)
                            .child(title.to_uppercase()),
                    )
                    .children(pages.iter().flat_map(|page| {
                        let mut items = vec![self.render_nav_item(*page, cx)];
                        if *page == Page::Projects {
                            items.extend(self.render_nav_projects(cx));
                        }
                        items
                    }))
            })
            .collect();
        let theme = cx.theme();
        v_flex()
            .w(NAV_WIDTH)
            .h_full()
            .flex_none()
            .px_2()
            .py_3()
            .gap_5()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .children(sections)
    }

    fn render_nav_item(&self, page: Page, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (fg, muted) = (theme.sidebar_foreground, theme.muted_foreground);
        let selected = matches!(self.view, View::Page(shown) if shown == page);
        h_flex()
            .id(page.label())
            .gap_2()
            .px_3()
            .py_1p5()
            .rounded_lg()
            .cursor_pointer()
            .text_sm()
            .text_color(if selected { fg } else { muted })
            .hover(|style| style.bg(fg.opacity(0.05)))
            .when(selected, |el| el.bg(fg.opacity(0.09)))
            .child(Icon::new(page.icon()).size_4())
            .child(page.label())
            .on_click(cx.listener(move |this, _, _, cx| this.show_page(page, cx)))
    }

    /// One nav item per project, indented under "Projects".
    fn render_nav_projects(&self, cx: &mut Context<Self>) -> Vec<Stateful<Div>> {
        let shown = self.shown_project(cx);
        let theme = cx.theme();
        let (fg, muted) = (theme.sidebar_foreground, theme.muted_foreground);
        let projects = AppState::get(cx).projects.read(cx);
        let items: Vec<_> = projects
            .items
            .iter()
            .enumerate()
            .map(|(ix, project)| {
                let selected = shown.as_deref() == Some(project.path.as_path());
                let badge = project_badge::render(
                    &project.identity,
                    &project.name,
                    project_badge::color(&project.identity, ix),
                    NAV_BADGE,
                );
                (
                    ix,
                    project.path.clone(),
                    project.name.clone(),
                    badge,
                    selected,
                )
            })
            .collect();
        items
            .into_iter()
            .map(|(ix, path, name, badge, selected)| {
                h_flex()
                    .id(("nav-project", ix))
                    .gap_2()
                    .pl(NAV_PROJECT_INDENT)
                    .pr_3()
                    .py_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(if selected { fg } else { muted })
                    .hover(|style| style.bg(fg.opacity(0.05)))
                    .when(selected, |el| el.bg(fg.opacity(0.09)))
                    .child(badge)
                    .child(div().min_w_0().truncate().child(name))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.show_project(path.clone(), window, cx)
                    }))
            })
            .collect()
    }

    fn render_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let (title, description, body): (SharedString, SharedString, AnyElement) = match &self.view
        {
            View::Page(page) => {
                let body = match page {
                    Page::Appearance => appearance::render(cx).into_any_element(),
                    Page::Shortcuts => self.shortcuts.clone().into_any_element(),
                    Page::Projects => projects::render(cx).into_any_element(),
                    Page::Terminal => terminal::render(cx).into_any_element(),
                };
                (page.label().into(), page.description().into(), body)
            }
            View::Project(project) => {
                let path = project.read(cx).path();
                let name = AppState::get(cx)
                    .projects
                    .read(cx)
                    .project(path)
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child(self.render_nav(cx))
                    .child(self.render_page(cx)),
            )
    }
}

/// One setting: its title block on the left, its control on the right.
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
) -> impl IntoElement {
    let step = |delta: f32, ix: usize, icon: Lucide, disabled: bool| {
        let set = set.clone();
        Button::new((id, ix))
            .cursor_pointer()
            .ghost()
            .small()
            .icon(icon)
            .disabled(disabled)
            .on_click(move |_, _, cx| set(value + delta, cx))
    };
    h_flex()
        .gap_1()
        .items_center()
        .child(step(
            -FONT_SIZE_STEP,
            0,
            Lucide::Minus,
            value <= *range.start(),
        ))
        .child(
            div()
                .w(STEPPER_VALUE_WIDTH)
                .text_center()
                .text_sm()
                .child(format!("{value:.0}")),
        )
        .child(step(FONT_SIZE_STEP, 1, Lucide::Plus, value >= *range.end()))
        .when(value != default, |el| {
            let set = set.clone();
            el.child(
                Button::new((id, 2usize))
                    .cursor_pointer()
                    .ghost()
                    .small()
                    .icon(Lucide::RotateCcw)
                    .tooltip("Reset to default")
                    .on_click(move |_, _, cx| set(default, cx)),
            )
        })
}
