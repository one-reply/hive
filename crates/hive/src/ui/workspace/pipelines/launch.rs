//! A task, its execution recipe and the repositories the planner may use.
use super::*;
use crate::state::pipelines::{Preview, PreviewKey};
use gpui_kit::component::{Disableable as _, button::ButtonVariants as _, input::InputEvent};
use hive_core::pipeline::{Lane, LanePreset, workspace::BasePreview};

mod actions;
mod again;
mod dictation;
mod draft;
mod format;
mod mode;
mod readiness;
mod repository;
mod route;
mod start;
mod task;
use readiness::{Blocker, LaneState, blocker, lane_state};
use repository::{Binding, dropdown, repositories};
use start::Shown;
pub(crate) use task::key_bindings as task_key_bindings;

const COMPOSER_WIDTH: Pixels = px(800.);

pub(super) struct LaunchForm {
    project: PathBuf,
    template: Template,
    parent: WeakEntity<PipelinePanel>,
    task: Entity<gpui_kit::component::input::EditorState>,
    bindings: Vec<Binding>,
    criteria: Entity<TextareaState>,
    dictation: Entity<crate::state::dictation::Dictation>,
    regression: bool,
    direct: bool,
    frozen: bool,
    keep_draft: bool,
    /// The form as it opened, before any change of the user's.
    pristine: hive_core::pipeline::draft::LaunchDraft,
    /// The pending write of the draft; dropping it cancels the write.
    draft_save: Option<Task<()>>,
    previous_recipe: Option<(Template, Template)>,
    /// Set from the user's Start until every repository has been read again.
    starting: Option<start::PendingStart>,
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
        let providers = app.ai_providers.clone();
        let dictating_with = app.store.read(cx).state.settings.dictation.provider();
        let task = task::editor(window, cx);
        let criteria = area("", "What should QA verify?", window, cx);
        let dictation = cx.new(|_| crate::state::dictation::Dictation::new());
        // The microphone explains a missing key, which only a check of the Keychain shows.
        if let Some(provider) = dictating_with {
            providers.update(cx, |providers, cx| providers.check(provider, cx));
        }
        let subscriptions = vec![
            cx.observe_self(|form: &mut Self, cx| form.schedule_draft_save(cx)),
            cx.observe(&dictation, |_, _, cx| cx.notify()),
            cx.observe(&providers, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &dictation,
                window,
                |form, _, text: &crate::state::dictation::Transcribed, window, cx| {
                    form.insert_dictated(&text.0, window, cx)
                },
            ),
            cx.observe(&criteria, |_, _, cx| cx.notify()),
            cx.observe(&projects, |_, _, cx| cx.notify()),
            cx.observe_in(&pipelines, window, |form, _, window, cx| {
                form.resume_start(window, cx);
                cx.notify();
            }),
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
            regression: false,
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
            dictation,
            regression: false,
            direct: false,
            frozen: false,
            keep_draft: true,
            pristine,
            draft_save: None,
            previous_recipe: None,
            starting: None,
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
            binding.read(cx);
        }
        self.error = None;
        cx.notify();
    }

    fn add_repository(&mut self, project: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.bindings.iter().position(|b| b.project.is_none()) {
            self.rebind(index, project, window, cx);
            return;
        }
        let preset = LanePreset {
            name: self.lane_name(self.bindings.len(), &project, cx),
            profile: self.template.development.profile.clone(),
            ..Default::default()
        };
        let binding = Binding::new(preset, Some(project), window, cx);
        binding.read(cx);
        self.bindings.push(binding);
        cx.notify();
    }

    /// Binds lane `index` to another repository, under the name `lane_name` gives it there.
    fn rebind(
        &mut self,
        index: usize,
        project: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.bindings[index].preset.name = self.lane_name(index, &project, cx);
        self.bindings[index].bind(Some(project), window, cx);
        cx.notify();
    }

    /// The name lane `index` has once it is bound to `project`.
    fn lane_name(&self, index: usize, project: &Path, cx: &App) -> String {
        let repository = repositories(cx)
            .into_iter()
            .find(|(path, _)| path == project)
            .map_or_else(|| path_name(project), |(_, name)| name);
        let others = self
            .bindings
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, binding)| binding.preset.name.as_str());
        let recipe_lane = self
            .template
            .lanes
            .get(index)
            .map(|lane| lane.name.as_str());
        lane_name_in(recipe_lane, &repository, others)
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
}

/// A recipe's lane keeps the name its stages refer to, whatever repository fills it. A lane
/// the user added is named after its repository, so it takes the new one's name when it is
/// rebound, numbered when another lane has that name: the planner prompt, the timeline and
/// the pull request text all tell lanes apart by name.
fn lane_name_in<'a>(
    recipe_lane: Option<&str>,
    repository: &str,
    others: impl Iterator<Item = &'a str>,
) -> String {
    match recipe_lane {
        Some(name) => name.to_owned(),
        None => hive_core::pipeline::unique_name(repository, others),
    }
}

fn required_heading(title: &'static str, cx: &App) -> Div {
    h_flex()
        .gap_0p5()
        .child(heading(title))
        .child(required_mark(cx))
}

/// A section title with one line on why it matters.
fn section(title: impl IntoElement, hint: &'static str, cx: &App) -> Div {
    v_flex().gap_0p5().child(title).child(subtle(hint, cx))
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

impl LaunchForm {
    /// The recipe and the stages it runs, so the user knows what the run does before
    /// describing the task.
    fn header(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let templates = AppState::get(cx)
            .store
            .read(cx)
            .state
            .pipelines
            .templates
            .clone();
        let recipe = options_menu(
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
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("New run"),
                    )
                    .child(h_flex().gap_2().child(subtle("Workflow", cx)).child(recipe)),
            )
            .child(self.route(cx))
            .children(self.recipe_version(window, cx))
    }

    fn repositories(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let projects = repositories(cx);
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
        let mut section = v_flex().gap_2().child(section(
            required_heading("Repositories", cx),
            "Agents commit to the chosen branch in a separate worktree, so your checkout stays as it is.",
            cx,
        ));
        for i in 0..self.bindings.len() {
            section = section.child(self.repository_card(i, &projects, window, cx));
        }
        // The menu button keeps its own width instead of stretching across the column.
        section.child(h_flex().child(add))
    }
}

impl Render for LaunchForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let task = self.task_field(window, cx);
        let body = v_flex()
            .w_full()
            .max_w(COMPOSER_WIDTH)
            .gap_6()
            .child(self.header(window, cx))
            .child(task)
            .child(self.repositories(window, cx))
            .children(self.options(window, cx));
        v_flex()
            .size_full()
            .min_h_0()
            .capture_action(cx.listener(Self::start_on_secondary_enter))
            .child(scroll(
                "pipeline-launch",
                h_flex()
                    .items_start()
                    .justify_center()
                    .pt_2()
                    .pb_8()
                    .child(body),
            ))
            .child(self.action_bar(window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::{Path, PathBuf, Template, default_projects, lane_name_in};

    #[test]
    fn an_added_lane_takes_the_name_of_the_repository_it_is_bound_to() {
        // Added for `api`, then switched to `web` in its card.
        assert_eq!(lane_name_in(None, "web", ["Web UI"].into_iter()), "web");
        // A second lane of a repository, and a name the recipe already uses.
        assert_eq!(
            lane_name_in(None, "web", ["web", "Web 2"].into_iter()),
            "web 3"
        );
        assert_eq!(lane_name_in(None, "API", ["api"].into_iter()), "API 2");
    }

    #[test]
    fn a_recipe_lane_keeps_its_name_whatever_repository_fills_it() {
        assert_eq!(lane_name_in(Some("API"), "web", ["Web"].into_iter()), "API");
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
