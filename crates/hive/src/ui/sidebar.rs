//! Project tree (Orca-style): colored project folders → git worktrees as two-line rows.
//! Sessions will hang under worktrees.

use std::collections::HashSet;
use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_kit::assets::IconName as Lucide;
use hive_core::host::Host;

use crate::app::AppState;
use crate::state::projects::Projects;
use crate::state::store::Store;
use crate::ui::settings_sheet;

/// Folder colors, cycled by project position (Orca tints each project).
const FOLDER_COLORS: [u32; 8] = [0xf97316, 0xeab308, 0x737373, 0x22c55e, 0xec4899, 0x14b8a6, 0x8b5cf6, 0xef4444];

pub struct Sidebar {
    projects: Entity<Projects>,
    store: Entity<Store>,
    /// Projects the user folded; everything else is expanded.
    collapsed: HashSet<PathBuf>,
    _observer: Subscription,
}

impl Sidebar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (projects, store) = {
            let app = AppState::get(cx);
            (app.projects.clone(), app.store.clone())
        };
        let observer = cx.observe(&projects, |_, _, cx| cx.notify());
        Self { projects, store, collapsed: HashSet::new(), _observer: observer }
    }

    fn add_projects(&mut self, cx: &mut Context<Self>) {
        Store::prompt_add_projects(self.store.clone(), cx);
    }

    fn toggle_project(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(path) = self.projects.read(cx).items.get(i).map(|p| p.path.clone()) else { return };
        if !self.collapsed.remove(&path) {
            self.collapsed.insert(path);
        }
        self.projects.update(cx, |p, cx| p.select((i, None), cx));
    }
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (fg, muted_fg, hover, active) =
            (theme.sidebar_foreground, theme.muted_foreground, theme.list_hover, theme.sidebar_accent);
        let state = self.projects.read(cx);
        let selected = state.selected;

        let mut rows: Vec<AnyElement> = vec![];
        for (i, project) in state.items.iter().enumerate() {
            rows.push(
                h_flex()
                    .id(("project", i))
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(hover))
                    .when(selected == Some((i, None)), |s| s.bg(active))
                    .child(Icon::new(IconName::Folder).size_4().text_color(rgb(FOLDER_COLORS[i % FOLDER_COLORS.len()])))
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).truncate().child(project.name.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_project(i, cx)))
                    .into_any_element(),
            );
            if self.collapsed.contains(&project.path) {
                continue;
            }
            let host: SharedString = match &project.host {
                Host::Local => "Local Mac".into(),
                Host::Ssh { target } => target.clone().into(),
            };
            for (j, wt) in project.worktrees.iter().enumerate() {
                let dir = wt.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let detached = wt.branch == "detached";
                let (title, subtitle) = if detached { (dir, "detached".to_string()) } else { (wt.branch.clone(), dir) };
                let projects = self.projects.clone();
                rows.push(
                    v_flex()
                        .id(("worktree", i * 1000 + j))
                        .gap_1()
                        .ml_3()
                        .px_2()
                        .py_1p5()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(hover))
                        .when(selected == Some((i, Some(j))), |s| s.bg(active))
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().size_2().flex_none().rounded_full().bg(muted_fg.opacity(0.5)))
                                .child(div().text_sm().truncate().child(title))
                                .when(j == 0, |s| {
                                    s.child(
                                        div()
                                            .flex_none()
                                            .px_1p5()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(muted_fg.opacity(0.4))
                                            .text_xs()
                                            .text_color(muted_fg)
                                            .child("primary"),
                                    )
                                }),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .pl_4()
                                .text_xs()
                                .text_color(muted_fg)
                                .child(div().flex_none().px_1p5().rounded_sm().bg(fg.opacity(0.08)).child(host.clone()))
                                .child(div().truncate().child(subtitle)),
                        )
                        .on_click(move |_, _, cx| projects.update(cx, |p, cx| p.select((i, Some(j)), cx)))
                        .into_any_element(),
                );
            }
        }

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .text_color(fg)
            .child(
                h_flex()
                    .justify_between()
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(muted_fg).child("Projects"))
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
            .child(v_flex().id("project-tree").flex_1().min_h_0().overflow_y_scroll().px_2().gap_0p5().children(rows))
            .child(
                h_flex().px_3().pb_3().child(
                    Button::new("settings")
                        .cursor_pointer()
                        .ghost()
                        .small()
                        .icon(IconName::Settings)
                        .tooltip("Settings")
                        .on_click(|_, window, cx| settings_sheet::open(window, cx)),
                ),
            )
    }
}
