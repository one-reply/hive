//! A task, its execution recipe and the repositories the planner may use.
use super::*;
use gpui_kit::component::{Disableable as _, button::ButtonVariants as _, input::InputEvent};
use hive_core::pipeline::{Lane, LanePreset, workspace::BasePreview};

mod repository;
use repository::{Binding, dropdown, repositories};

const COMPOSER_WIDTH: Pixels = px(800.);
/// Tall enough for a few sentences of intent and acceptance criteria without scrolling.
const TASK_ROWS: usize = 6;

/// Where one repository of the run stands before planning can start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LaneState {
    Ready,
    NoBranch,
    Reading,
    Failed,
    EditingRef,
    MissingCommand,
}

/// The first thing that keeps **Start planning** disabled, named so the user knows what to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Blocker {
    Task,
    Repository,
    Branch,
    Failed,
    EditingRef,
    MissingCommand,
    Reading,
}

impl Blocker {
    fn message(self) -> &'static str {
        match self {
            Self::Task => "Describe the task to start planning.",
            Self::Repository => "Add at least one repository.",
            Self::Branch => "Choose or create a branch for each repository.",
            Self::Failed => "A repository can’t be read. Pick another branch or repository.",
            Self::EditingRef => "Press Use ref to apply the branch, tag or commit.",
            Self::MissingCommand => "Enter the custom check command.",
            Self::Reading => "Reading repositories…",
        }
    }
}

/// Things the user must fix come before a read that only needs time.
fn blocker(has_task: bool, lanes: &[LaneState]) -> Option<Blocker> {
    if !has_task {
        return Some(Blocker::Task);
    }
    if lanes.is_empty() {
        return Some(Blocker::Repository);
    }
    [
        (LaneState::NoBranch, Blocker::Branch),
        (LaneState::Failed, Blocker::Failed),
        (LaneState::EditingRef, Blocker::EditingRef),
        (LaneState::MissingCommand, Blocker::MissingCommand),
        (LaneState::Reading, Blocker::Reading),
    ]
    .into_iter()
    .find(|(state, _)| lanes.contains(state))
    .map(|(_, blocker)| blocker)
}

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
        let task = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(TASK_ROWS)
                .placeholder("What should the agents build or fix? Include what “done” looks like.")
        });
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

    /// A recipe's lane only empties, so the user can bind it again; an added one goes away.
    fn remove_repository(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.template.lanes.len() {
            self.bindings[index].bind(None, cx);
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

    /// The stages this recipe runs, in order, so the user sees where their approval comes.
    fn route(&self, cx: &App) -> Div {
        let mut route = h_flex().flex_wrap().items_center().gap_1p5();
        let steps = std::iter::once((Lucide::Map, self.template.planner.name.as_str()))
            .chain(std::iter::once((Lucide::UserRoundCheck, "Your approval")))
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
            route = route.child(stage_chip(icon, name.to_owned(), cx));
        }
        route
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

fn stage_chip(icon: Lucide, name: String, cx: &App) -> Div {
    h_flex()
        .flex_none()
        .gap_1p5()
        .px_2()
        .py_0p5()
        .rounded_full()
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background)
        .text_xs()
        .child(
            Icon::new(icon)
                .size_3()
                .text_color(cx.theme().muted_foreground),
        )
        .child(name)
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
        let blocked = blocker(has_task, &lanes);
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
            |this, t, w, cx| this.apply_template(t, w, cx),
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
            .label("Start planning")
            .icon(Lucide::Play)
            .disabled(blocked.is_some())
            .on_click(cx.listener(|this, _, w, cx| this.submit(w, cx)))
            .smooth_hover(ButtonLook::Primary, &hover, cx);
        let footer_hint = blocked.map_or(
            "Agents only plan first. You approve the plan before any code changes.",
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
                    .child("Describe the task and choose the repositories. A planner drafts the work for your approval."),
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
            .child(self.route(cx));
        let task = v_flex()
            .gap_2()
            .child(section(
                required_heading("Task", cx),
                "Plain language is fine. The planner asks if something is unclear.",
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
            .child(subtle(footer_hint, cx));
        let body = v_flex()
            .w_full()
            .max_w(COMPOSER_WIDTH)
            .gap_6()
            .child(header)
            .child(workflow)
            .child(task)
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
    use super::{Blocker, LaneState, Path, PathBuf, Template, blocker, default_projects};

    #[test]
    fn a_missing_task_is_named_before_anything_else() {
        assert_eq!(blocker(false, &[]), Some(Blocker::Task));
        assert_eq!(blocker(false, &[LaneState::Failed]), Some(Blocker::Task));
    }

    #[test]
    fn a_run_needs_at_least_one_repository() {
        assert_eq!(blocker(true, &[]), Some(Blocker::Repository));
    }

    #[test]
    fn a_fix_the_user_must_make_outranks_a_read_in_progress() {
        let lanes = [LaneState::Reading, LaneState::MissingCommand];
        assert_eq!(blocker(true, &lanes), Some(Blocker::MissingCommand));
        let lanes = [LaneState::EditingRef, LaneState::Failed];
        assert_eq!(blocker(true, &lanes), Some(Blocker::Failed));
        assert_eq!(
            blocker(true, &[LaneState::Ready, LaneState::Reading]),
            Some(Blocker::Reading)
        );
    }

    #[test]
    fn a_lane_without_a_branch_blocks_planning_before_slower_reads() {
        let lanes = [LaneState::Reading, LaneState::NoBranch];
        assert_eq!(blocker(true, &lanes), Some(Blocker::Branch));
        assert_eq!(blocker(false, &lanes), Some(Blocker::Task));
    }

    #[test]
    fn ready_lanes_and_a_task_unblock_planning() {
        assert_eq!(blocker(true, &[LaneState::Ready, LaneState::Ready]), None);
    }

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
