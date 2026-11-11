use super::{HEADER_HEIGHT, HEADER_INSET, PROJECT_BADGE, Sidebar, TITLE_TEXT, project_badge};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::*;
use hive_core::projects::Project;
use hive_core::state::ProjectRef;

#[derive(Clone)]
pub(super) struct DraggedProject {
    project: ProjectRef,
    name: SharedString,
    color: Hsla,
}

impl DraggedProject {
    pub(super) fn new(project: &Project, ix: usize) -> Self {
        Self {
            project: project_ref(project),
            name: project.name.clone().into(),
            color: project_badge::color(&project.identity, ix),
        }
    }
}

impl Render for DraggedProject {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .h(HEADER_HEIGHT)
            .px(HEADER_INSET)
            .gap_1p5()
            .rounded_md()
            .shadow_md()
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .text_size(TITLE_TEXT)
            .font_weight(FontWeight::SEMIBOLD)
            .child(project_badge::render(
                &self.project.identity,
                &self.name,
                self.color,
                PROJECT_BADGE,
            ))
            .child(self.name.clone())
    }
}

impl Sidebar {
    pub(super) fn project_drop_target(&self, before: Option<&Project>) -> Stateful<Div> {
        let id: SharedString = before.map_or_else(
            || "project-drop-end".into(),
            |project| format!("project-drop:{:?}:{}", project.host, project.path.display()).into(),
        );
        let before = before.map(project_ref);
        let store = self.store.clone();
        v_flex()
            .id(id)
            .flex_shrink_0()
            .border_t_2()
            .border_color(transparent_black())
            .drag_over::<DraggedProject>(|style, _, _, cx| {
                style.border_color(cx.theme().drag_border)
            })
            .on_drop(move |drag: &DraggedProject, _, cx| {
                store.update(cx, |store, cx| {
                    store.move_project(&drag.project, before.as_ref(), cx);
                });
            })
    }
}

fn project_ref(project: &Project) -> ProjectRef {
    ProjectRef {
        path: project.path.clone(),
        host: project.host.clone(),
        worktrees: project.filter,
        identity: project.identity.clone(),
        expansion: Default::default(),
    }
}
