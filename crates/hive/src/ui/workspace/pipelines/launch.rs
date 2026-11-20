//! A task, its execution recipe and the repositories the planner may use.
use super::*;
use gpui_kit::component::{Disableable as _, button::ButtonVariants as _, input::InputEvent};
use hive_core::pipeline::{Lane, LanePreset, workspace::BasePreview};

mod again;
mod draft;
mod mode;
mod readiness;
mod repository;
mod route;
use readiness::{Blocker, LaneState, blocker};
use repository::{Binding, dropdown, repositories};

const COMPOSER_WIDTH: Pixels = px(800.);
/// Tall enough for a few sentences of intent and acceptance criteria without scrolling.
const TASK_ROWS: usize = 6;

pub(super) struct LaunchForm {
    project: PathBuf,
    template: Template,
    parent: WeakEntity<PipelinePanel>,
    task: Entity<TextareaState>,
    bindings: Vec<Binding>,
    criteria: Entity<TextareaState>,
    direct: bool,
    frozen: bool,
    keep_draft: bool,
    /// The form as it opened, before any change of the user's.
    pristine: hive_core::pipeline::draft::LaunchDraft,
    /// The pending write of the draft; dropping it cancels the write.
    draft_save: Option<Task<()>>,
    previous_recipe: Option<(Template, Template)>,
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
        let task = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(TASK_ROWS)
                .placeholder("What should the agents build or fix? Include what “done” looks like.")
        });
        let criteria = area("", "What should QA verify?", window, cx);
        let subscriptions = vec![
            cx.observe_self(|form: &mut Self, cx| form.schedule_draft_save(cx)),
            cx.observe(&criteria, |_, _, cx| cx.notify()),
            cx.observe(&projects, |_, _, cx| cx.notify()),
            cx.observe(&pipelines, |_, _, cx| cx.notify()),
            cx.subscribe(&task, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        let pristine = hive_core::pipeline::draft::LaunchDraft {
            owner: project.clone(),
            template: template.clone(),
            original: None,
            frozen: false,
            task: String::new(),
            direct: false,
            criteria: String::new(),
            lanes: Vec::new(),
        };
        let mut this = Self {
            project,
            template: template.clone(),
            parent,
            task,
            bindings: Vec::new(),
            criteria,
            direct: false,
            frozen: false,
            keep_draft: true,
            pristine,
            draft_save: None,
            previous_recipe: None,
            error: None,
            _subscriptions: subscriptions,
        };
        this.apply_template(template, window, cx);
        // The recipe's default repositories are part of how the form opens.
        this.pristine = this.snapshot(cx);
        this.task.focus_handle(cx).focus(window, cx);
        this
    }

    fn apply_template(&mut self, template: Template, window: &mut Window, cx: &mut Context<Self>) {
        self.direct = false;
        let projects = repositories(cx);
        let mut old = std::mem::take(&mut self.bindings);
        let previous = std::mem::replace(&mut self.template, template);
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
        // Repositories the user added survive a recipe change; only the old recipe's lanes go.
        old.retain(|b| !previous.lanes.iter().any(|lane| lane.name == b.preset.name));
        self.bindings.append(&mut old);
        for binding in &self.bindings {
            binding.preview(cx);
        }
        self.error = None;
        cx.notify();
    }

    fn add_repository(&mut self, project: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(binding) = self.bindings.iter_mut().find(|b| b.project.is_none()) {
            binding.bind(Some(project), window, cx);
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
                ..Default::default()
            };
            let binding = Binding::new(preset, Some(project), window, cx);
            binding.preview(cx);
            self.bindings.push(binding);
        }
        cx.notify();
    }

    /// A recipe's lane only empties, so the user can bind it again; an added one goes away.
    fn remove_repository(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.template.lanes.len() {
            self.bindings[index].bind(None, window, cx);
        } else {
            self.bindings.remove(index);
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
                branch: b.branch_name().unwrap_or_default().to_owned(),
                base_branch: b.base_branch(),
                command: b.command(preview, cx),
                setup: b.setup_command(cx),
                profile: b.preset.profile.clone(),
                edits: b.preset.edits,
                ..Default::default()
            });
        }
        let task = self.task.read(cx).value().trim().to_owned();
        let result = AppState::get(cx).pipelines.clone().update(cx, |p, cx| {
            let options = crate::state::pipelines::LaunchOptions {
                frozen: self.frozen,
                direct_criteria: (self.direct && self.can_start_direct())
                    .then(|| self.criteria.read(cx).value().to_string()),
            };
            p.start(
                self.project.clone(),
                task,
                self.template.clone(),
                lanes,
                options,
                cx,
            )
        });
        match result {
            Ok(id) => {
                self.abandon();
                let _ = self.parent.update(cx, |p, cx| {
                    p.draft = None;
                    p.show_run(id, window, cx)
                });
            }
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }
}

/// Marks a field that must be filled before planning can start.
fn required_mark(cx: &App) -> Div {
    div().text_xs().text_color(cx.theme().danger).child("*")
}

fn required_heading(title: &'static str, cx: &App) -> Div {
    h_flex()
        .gap_0p5()
        .child(heading(title))
        .child(required_mark(cx))
}

/// A section title with one line on why it matters, and an optional action on the right.
fn section(
    title: impl IntoElement,
    hint: &'static str,
    action: Option<AnyElement>,
    cx: &App,
) -> Div {
    h_flex()
        .w_full()
        .items_end()
        .justify_between()
        .gap_3()
        .child(
            v_flex()
                .min_w_0()
                .gap_0p5()
                .child(title)
                .child(subtle(hint, cx)),
        )
        .children(action)
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
        let lanes: Vec<_> = self.bindings.iter().filter_map(|b| b.state(cx)).collect();
        let has_task = !self.task.read(cx).value().trim().is_empty();
        let direct = self.direct && self.can_start_direct();
        let blocked = blocker(has_task, &lanes).or_else(|| {
            (direct && self.criteria.read(cx).value().trim().is_empty())
                .then_some(Blocker::Criteria)
        });
        let template = options_menu(
            dropdown(
                "launch-recipe",
                Lucide::Workflow,
                self.template.name.clone(),
            ),
            templates
                .into_iter()
                .map(|t| (t.name.clone(), t.name == self.template.name, t))
                .collect(),
            cx,
            |this, t, w, cx| {
                this.frozen = false;
                this.previous_recipe = None;
                this.apply_template(t, w, cx)
            },
        );
        let mut repos = v_flex().w_full().gap_2();
        for i in 0..self.bindings.len() {
            repos = repos.child(self.repository_card(i, &projects, window, cx));
        }
        let add = options_menu(
            Button::new("add-pipeline-repo")
                .small()
                .ghost()
                .cursor_pointer()
                .icon(Lucide::Plus)
                .label("Add repository"),
            projects
                .iter()
                .map(|(p, n)| (n.clone(), false, p.clone()))
                .collect(),
            cx,
            |this, p, w, cx| this.add_repository(p, w, cx),
        );
        let hover = Hover::new("start-planning", window, cx).enabled(blocked.is_none());
        let submit = Button::new("start-planning")
            .cursor_pointer()
            .with_size(ButtonSize::Medium)
            .label(if direct {
                "Start development"
            } else {
                "Start planning"
            })
            .icon(Lucide::Play)
            .disabled(blocked.is_some())
            .on_click(cx.listener(|this, _, w, cx| this.submit(w, cx)))
            .smooth_hover(ButtonLook::Primary, &hover, cx);
        let footer_hint = blocked.map_or(
            if direct {
                "QA and your final acceptance follow development."
            } else {
                "You approve the plan before development."
            },
            Blocker::message,
        );
        let header = v_flex()
            .gap_1()
            .child(
                div()
                    .text_3xl()
                    .font_weight(FontWeight::MEDIUM)
                    .child("New run"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Describe the task and choose where to run it."),
            );
        let workflow = card(cx)
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .child(heading("Workflow"))
                    .child(template),
            )
            .child(self.route(cx))
            .children(self.recipe_version(window, cx))
            .children(self.quick_start_toggle(direct, window, cx));
        let task = v_flex()
            .gap_2()
            .child(section(
                required_heading("Task", cx),
                if direct {
                    "Describe the change for the developer."
                } else {
                    "Plain language is fine. The planner asks if something is unclear."
                },
                None,
                cx,
            ))
            .child(TextField::textarea(
                Textarea::new(&self.task),
                &self.task,
                cx,
            ));
        let repositories = v_flex()
            .gap_2()
            .child(section(
                required_heading("Repositories", cx),
                "Agents commit to the chosen branch in a separate worktree, so your checkout stays as it is.",
                Some(add),
                cx,
            ))
            .child(repos);
        let footer = h_flex()
            .w_full()
            .gap_3()
            .flex_wrap()
            .pt_4()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(submit)
            .child(button(
                "discard-launch-draft",
                "Discard draft",
                ButtonLook::Ghost,
                window,
                cx,
                |v, w, cx| {
                    v.abandon();
                    let _ = v.parent.update(cx, |p, cx| {
                        p.discard_draft(cx);
                        p.home(w, cx);
                    });
                },
            ))
            .child(subtle(footer_hint, cx));
        let body = v_flex()
            .w_full()
            .max_w(COMPOSER_WIDTH)
            .gap_6()
            .child(header)
            .child(workflow)
            .child(task)
            .when(direct, |el| {
                el.child(paragraph("Acceptance criteria", &self.criteria, cx))
            })
            .child(repositories)
            .when_some(self.error.clone(), |el, e| {
                el.child(div().text_sm().text_color(cx.theme().danger).child(e))
            })
            .child(footer);
        scroll(
            "pipeline-launch",
            h_flex()
                .items_start()
                .justify_center()
                .pt_6()
                .pb_8()
                .child(body),
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
