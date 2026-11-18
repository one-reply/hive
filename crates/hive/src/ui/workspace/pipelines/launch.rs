//! A task, its execution recipe and the repositories the planner may use.
use super::*;
use gpui_kit::component::{Disableable as _, input::InputEvent};
use hive_core::pipeline::{Lane, LanePreset, workspace::BasePreview};

mod repository;
use repository::{Binding, Check, repositories, select_menu};

const COMPOSER_WIDTH: Pixels = px(850.);

pub(super) struct LaunchForm {
    project: PathBuf,
    template: Template,
    parent: WeakEntity<PipelinePanel>,
    task: Entity<TextareaState>,
    bindings: Vec<Binding>,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}
impl LaunchForm {
    pub(super) fn new(
        project: PathBuf,
        template: Template,
        parent: WeakEntity<PipelinePanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let app = AppState::get(cx);
        let projects = app.projects.clone();
        let pipelines = app.pipelines.clone();
        let task = area(
            "",
            "What would you like the agents to build or fix?",
            window,
            cx,
        );
        let subscriptions = vec![
            cx.observe(&projects, |_, _, cx| cx.notify()),
            cx.observe(&pipelines, |_, _, cx| cx.notify()),
            cx.subscribe(&task, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        let mut this = Self {
            project,
            template: template.clone(),
            parent,
            task,
            bindings: Vec::new(),
            error: None,
            _subscriptions: subscriptions,
        };
        this.apply_template(template, window, cx);
        this.task.focus_handle(cx).focus(window, cx);
        this
    }

    fn apply_template(&mut self, template: Template, window: &mut Window, cx: &mut Context<Self>) {
        let projects = repositories(cx);
        let mut old = std::mem::take(&mut self.bindings);
        self.template = template;
        let defaults = default_projects(&self.template.lanes, &self.project, &projects);
        for (preset, project) in self.template.lanes.iter().zip(defaults) {
            if let Some(index) = old.iter().position(|b| b.preset.name == preset.name) {
                let mut binding = old.remove(index);
                binding.preset = preset.clone();
                self.bindings.push(binding);
            } else {
                self.bindings
                    .push(Binding::new(preset.clone(), project, window, cx));
            }
        }
        for binding in &self.bindings {
            binding.preview(cx);
        }
        self.error = None;
        cx.notify();
    }

    fn add_repository(&mut self, project: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(binding) = self.bindings.iter_mut().find(|b| b.project.is_none()) {
            binding.bind(Some(project), cx);
        } else {
            let project_name = repositories(cx)
                .into_iter()
                .find(|(p, _)| p == &project)
                .map(|(_, name)| name)
                .unwrap_or_else(|| path_name(&project));
            let mut name = project_name.clone();
            let mut index = 2;
            while self.bindings.iter().any(|b| b.preset.name == name) {
                name = format!("{project_name} {index}");
                index += 1;
            }
            let preset = LanePreset {
                name,
                profile: self.template.development.profile.clone(),
                command: String::new(),
            };
            let binding = Binding::new(preset, Some(project), window, cx);
            binding.preview(cx);
            self.bindings.push(binding);
        }
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut lanes = Vec::new();
        for b in &self.bindings {
            let Some(project) = &b.project else { continue };
            let Some(Ok(preview)) = b.base_preview(cx) else {
                return;
            };
            lanes.push(Lane {
                name: b.preset.name.clone(),
                project: project.clone(),
                base: preview.commit.clone(),
                command: b.command(preview, cx),
                profile: b.preset.profile.clone(),
                ..Default::default()
            });
        }
        let task = self.task.read(cx).value().trim().to_owned();
        let result = AppState::get(cx).pipelines.clone().update(cx, |p, cx| {
            p.start(self.project.clone(), task, self.template.clone(), lanes, cx)
        });
        match result {
            Ok(id) => {
                let _ = self.parent.update(cx, |p, cx| p.show_run(id, window, cx));
            }
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }

    fn route(&self, cx: &App) -> Div {
        let mut route = h_flex().flex_wrap().gap_2();
        let steps = std::iter::once((Lucide::Map, self.template.planner.name.as_str()))
            .chain(std::iter::once((
                Lucide::GitBranch,
                self.template.development.name.as_str(),
            )))
            .chain(
                self.template
                    .reviews
                    .iter()
                    .map(|s| (Lucide::ShieldCheck, s.name.as_str())),
            )
            .chain(std::iter::once((Lucide::Check, "Acceptance")));
        for (i, (icon, name)) in steps.enumerate() {
            if i > 0 {
                route = route.child(
                    Icon::new(Lucide::ChevronRight)
                        .size_3()
                        .text_color(cx.theme().muted_foreground),
                );
            }
            route = route.child(
                h_flex()
                    .gap_1()
                    .child(
                        Icon::new(icon)
                            .size_3()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(subtle(name.to_owned(), cx)),
            );
        }
        route
    }
}

fn default_projects(
    lanes: &[LanePreset],
    owner: &Path,
    projects: &[(PathBuf, String)],
) -> Vec<Option<PathBuf>> {
    let mut result: Vec<_> = lanes
        .iter()
        .map(|lane| {
            let mut named = projects
                .iter()
                .filter(|(_, name)| name.eq_ignore_ascii_case(&lane.name));
            let found = named.next()?;
            named.next().is_none().then(|| found.0.clone())
        })
        .collect();
    if !result.iter().flatten().any(|p| p == owner)
        && let Some(slot) = result.iter_mut().find(|p| p.is_none())
    {
        *slot = Some(owner.to_owned());
    }
    result
}

impl Render for LaunchForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let projects = repositories(cx);
        let templates = AppState::get(cx)
            .store
            .read(cx)
            .state
            .pipelines
            .templates
            .clone();
        let bound = self.bindings.iter().filter(|b| b.project.is_some()).count();
        let ready = bound > 0
            && !self.task.read(cx).value().trim().is_empty()
            && self
                .bindings
                .iter()
                .filter(|b| b.project.is_some())
                .all(|b| {
                    b.ref_input.is_none()
                        && b.base_preview(cx).is_some_and(|p| p.is_ok())
                        && (!matches!(b.check, Check::Custom)
                            || !b.custom_command.read(cx).value().trim().is_empty())
                });
        let template = select_menu(
            "launch-recipe",
            Lucide::Layers,
            self.template.name.clone(),
            templates
                .into_iter()
                .map(|t| (t.name.clone(), t == self.template, t))
                .collect(),
            cx,
            |this, t, w, cx| this.apply_template(t, w, cx),
        );
        let mut repos = v_flex().w_full().gap_2();
        for i in 0..self.bindings.len() {
            repos = repos.child(self.repository_row(i, &projects, window, cx));
        }
        let add = select_menu(
            "add-pipeline-repo",
            Lucide::Plus,
            "Add repository",
            projects
                .iter()
                .map(|(p, n)| (n.clone(), false, p.clone()))
                .collect(),
            cx,
            |this, p, w, cx| this.add_repository(p, w, cx),
        );
        let hover = Hover::new("start-planning", window, cx).enabled(ready);
        let submit = Button::new("start-planning")
            .cursor_pointer()
            .label("Start planning")
            .icon(Lucide::Play)
            .small()
            .disabled(!ready)
            .on_click(cx.listener(|this, _, w, cx| this.submit(w, cx)))
            .smooth_hover(ButtonLook::Primary, &hover, cx);
        let body = v_flex()
            .w_full()
            .max_w(COMPOSER_WIDTH)
            .gap_5()
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_3xl()
                            .font_weight(FontWeight::MEDIUM)
                            .child("New run"),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .flex_wrap()
                            .child(template)
                            .child(self.route(cx)),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(heading("Task"))
                    .child(TextField::textarea(
                        Textarea::new(&self.task),
                        &self.task,
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(heading("Repositories"))
                            .child(add),
                    )
                    .child(repos),
            )
            .when_some(self.error.clone(), |el, e| {
                el.child(div().text_sm().text_color(cx.theme().danger).child(e))
            })
            .child(h_flex().gap_3().flex_wrap().child(submit).child(subtle(
                "You’ll review the plan before development starts.",
                cx,
            )));
        scroll(
            "pipeline-launch",
            h_flex().items_start().justify_center().pt_4().child(body),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Path, PathBuf, Template, default_projects};

    #[test]
    fn matching_lanes_keep_their_repository_when_the_owner_matches_a_later_lane() {
        let lanes = Template::presets().pop().unwrap().lanes;
        let projects = [(PathBuf::from("/web"), "Web".into())];
        assert_eq!(
            default_projects(&lanes, Path::new("/web"), &projects),
            [None, Some("/web".into())]
        );
        assert_eq!(
            default_projects(&lanes, Path::new("/api"), &projects),
            [Some("/api".into()), Some("/web".into())]
        );
        assert_eq!(
            default_projects(&Template::default().lanes, Path::new("/home"), &[]),
            [Some("/home".into())]
        );
    }
}
