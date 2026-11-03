//! Project tree: project folder → git worktrees. Sessions will hang under worktrees.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::sidebar::{
    Sidebar as SidebarElement, SidebarCollapsible, SidebarFooter, SidebarGroup, SidebarHeader, SidebarMenu,
    SidebarMenuItem,
};
use gpui_kit::component::{IconName, Sizable as _, h_flex};
use gpui_kit::*;

use crate::app::AppState;
use crate::state::projects::Projects;
use crate::state::store::Store;
use crate::ui::settings_sheet;

pub struct Sidebar {
    projects: Entity<Projects>,
    store: Entity<Store>,
    _observer: Subscription,
}

impl Sidebar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (projects, store) = {
            let app = AppState::get(cx);
            (app.projects.clone(), app.store.clone())
        };
        let observer = cx.observe(&projects, |_, _, cx| cx.notify());
        Self { projects, store, _observer: observer }
    }

    fn add_projects(&mut self, cx: &mut Context<Self>) {
        Store::prompt_add_projects(self.store.clone(), cx);
    }
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let projects = self.projects.clone();
        let state = projects.read(cx);
        let selected = state.selected;

        let items: Vec<SidebarMenuItem> = state
            .items
            .iter()
            .enumerate()
            .map(|(i, project)| {
                let children = project.worktrees.iter().enumerate().map(|(j, wt)| {
                    let label = if j == 0 { format!("{}  ·  primary", wt.branch) } else { wt.branch.clone() };
                    let projects = projects.clone();
                    SidebarMenuItem::new(label)
                        .active(selected == Some((i, Some(j))))
                        .on_click(move |_, _, cx| projects.update(cx, |p, cx| p.select((i, Some(j)), cx)))
                });
                let projects = projects.clone();
                SidebarMenuItem::new(project.name.clone())
                    .icon(IconName::Folder)
                    .default_open(true)
                    .click_to_open(true)
                    .active(selected == Some((i, None)))
                    .on_click(move |_, _, cx| projects.update(cx, |p, cx| p.select((i, None), cx)))
                    .children(children)
            })
            .collect();

        SidebarElement::new("sidebar")
            .collapsible(SidebarCollapsible::None)
            .w(px(260.))
            .header(
                SidebarHeader::new().child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .items_center()
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Projects"))
                        .child(
                            Button::new("add-project")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Plus)
                                .tooltip("Add project folder")
                                .on_click(cx.listener(|this, _, _, cx| this.add_projects(cx))),
                        ),
                ),
            )
            .child(SidebarGroup::new("").child(SidebarMenu::new().children(items)))
            .footer(
                SidebarFooter::new().child(
                    Button::new("settings")
                        .ghost()
                        .small()
                        .icon(IconName::Settings)
                        .tooltip("Settings")
                        .on_click(|_, window, cx| settings_sheet::open(window, cx)),
                ),
            )
    }
}
