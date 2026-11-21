//! One repository of the run: where its lane commits, what must pass and how its worktree
//! is prepared.
use super::*;
use hive_core::git::branches::short_name;

mod branch;
mod card;
use hive_core::pipeline::draft::{DraftCheck as Check, DraftLane, WorkBranch};

pub(super) struct Binding {
    pub preset: LanePreset,
    pub project: Option<PathBuf>,
    checkout: Option<PathBuf>,
    /// The base ref being previewed: the chosen branch itself, or where a new one starts.
    reference: String,
    /// Where a new branch starts unless the user picks another base: their checkout's branch.
    default_reference: String,
    ref_input: Option<Entity<InputState>>,
    branch: WorkBranch,
    branch_query: Entity<InputState>,
    branch_open: bool,
    check: Check,
    custom_command: Entity<InputState>,
    /// Runs once in the new worktree; starts as the command this project used last time.
    setup: Entity<InputState>,
    /// The setup field is drawn only for a command or once the user asks for it: most
    /// worktrees need none.
    setup_open: bool,
    _subscriptions: Vec<Subscription>,
}

/// The branch a full ref names, local or remote-tracking; `None` for a tag, a commit or `HEAD`.
fn branch_of(reference: &str) -> Option<&str> {
    if let Some(local) = reference.strip_prefix("refs/heads/") {
        return Some(local);
    }
    let (_remote, branch) = reference.strip_prefix("refs/remotes/")?.split_once('/')?;
    Some(branch)
}

/// The setup command the project's last run used, to offer it again.
fn remembered_setup(project: Option<&Path>, cx: &App) -> String {
    let projects = &AppState::get(cx).store.read(cx).state.projects;
    project
        .and_then(|path| {
            projects
                .iter()
                .find(|p| p.host.is_local() && p.path == path)
        })
        .map(|p| p.setup.clone())
        .unwrap_or_default()
}

impl Binding {
    pub(super) fn new(
        preset: LanePreset,
        project: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<LaunchForm>,
    ) -> Self {
        let custom_command = input("", "Shell command", window, cx);
        let remembered = remembered_setup(project.as_deref(), cx);
        let setup = input(&remembered, "npm ci, bundle install…", window, cx);
        let branch_query = input("", "Search or name a new branch", window, cx);
        let subscriptions = vec![
            cx.subscribe(&setup, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&custom_command, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&branch_query, |this, query, event, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.submit_branch_query(&query, cx),
                _ => {}
            }),
        ];
        let (checkout, reference) = initial_ref(project.as_deref(), cx);
        Self {
            preset,
            project,
            checkout,
            default_reference: reference.clone(),
            reference,
            ref_input: None,
            branch: WorkBranch::Unchosen,
            branch_query,
            branch_open: false,
            check: Check::Suggested,
            custom_command,
            setup,
            setup_open: !remembered.trim().is_empty(),
            _subscriptions: subscriptions,
        }
    }
    pub(super) fn draft(&self, cx: &App) -> DraftLane {
        DraftLane {
            preset: self.preset.clone(),
            project: self.project.clone(),
            reference: self.reference.clone(),
            branch: self.branch.clone(),
            check: self.check.clone(),
            custom_command: self.custom_command.read(cx).value().to_string(),
            setup: self.setup.read(cx).value().to_string(),
        }
    }
    pub(super) fn from_draft(
        draft: DraftLane,
        window: &mut Window,
        cx: &mut Context<LaunchForm>,
    ) -> Self {
        let mut binding = Self::new(draft.preset, draft.project, window, cx);
        binding.reference = draft.reference;
        binding.branch = draft.branch;
        binding.check = draft.check;
        binding
            .custom_command
            .update(cx, |i, cx| i.set_value(draft.custom_command, window, cx));
        binding.set_setup(draft.setup, window, cx);
        binding.preview(cx);
        binding
    }
    pub(super) fn bind(
        &mut self,
        project: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<LaunchForm>,
    ) {
        (self.checkout, self.reference) = initial_ref(project.as_deref(), cx);
        self.default_reference.clone_from(&self.reference);
        let setup = remembered_setup(project.as_deref(), cx);
        self.set_setup(setup, window, cx);
        self.project = project;
        self.ref_input = None;
        self.branch = WorkBranch::Unchosen;
        self.check = Check::Suggested;
        self.preview(cx);
    }
    /// The branch the lane commits to, once the user has chosen or named one.
    pub(super) fn branch_name(&self) -> Option<&str> {
        self.branch.name()
    }
    pub(super) fn preview(&self, cx: &mut Context<LaunchForm>) {
        if let Some(project) = &self.project {
            AppState::get(cx).pipelines.clone().update(cx, |p, cx| {
                p.preview(
                    project.clone(),
                    self.checkout.as_ref().unwrap_or(project).clone(),
                    self.reference.clone(),
                    cx,
                )
            });
        }
    }
    pub(super) fn base_preview<'a>(&self, cx: &'a App) -> Option<&'a Result<BasePreview, String>> {
        let project = self.project.as_ref()?;
        AppState::get(cx).pipelines.read(cx).previews.get(&(
            project.clone(),
            self.checkout.as_ref().unwrap_or(project).clone(),
            self.reference.clone(),
        ))
    }
    /// `None` for an optional lane the user left without a repository.
    pub(super) fn state(&self, cx: &App) -> Option<LaneState> {
        self.project.as_ref()?;
        if self.ref_input.is_some() {
            return Some(LaneState::EditingRef);
        }
        if self.branch == WorkBranch::Unchosen {
            return Some(LaneState::NoBranch);
        }
        Some(match self.base_preview(cx) {
            None => LaneState::Reading,
            Some(Err(_)) => LaneState::Failed,
            Some(Ok(_))
                if matches!(self.check, Check::Custom)
                    && self.custom_command.read(cx).value().trim().is_empty() =>
            {
                LaneState::MissingCommand
            }
            Some(Ok(_)) => LaneState::Ready,
        })
    }
    pub(super) fn command(&self, preview: &BasePreview, cx: &App) -> String {
        match &self.check {
            Check::Suggested if !self.preset.command.trim().is_empty() => {
                self.preset.command.trim().to_owned()
            }
            Check::Suggested => preview.checks.first().cloned().unwrap_or_default(),
            Check::Command(s) => s.clone(),
            Check::Custom => self.custom_command.read(cx).value().trim().to_owned(),
        }
    }
    /// The branch a new work branch starts from, as the target of its pull request.
    pub(super) fn base_branch(&self) -> String {
        match self.branch {
            WorkBranch::New(_) => branch_of(&self.reference).unwrap_or_default().to_owned(),
            WorkBranch::Unchosen | WorkBranch::Existing(_) => String::new(),
        }
    }
    /// Binds the repository of an earlier run's `lane` with its check and setup commands.
    pub(super) fn restore(
        &mut self,
        lane: &Lane,
        window: &mut Window,
        cx: &mut Context<LaunchForm>,
    ) {
        self.bind(Some(lane.project.clone()), window, cx);
        self.check = Check::Command(lane.command.clone());
        self.set_setup(lane.setup.clone(), window, cx);
    }
    /// Replaces the setup command; its field stays in view exactly when there is one.
    fn set_setup(&mut self, command: String, window: &mut Window, cx: &mut Context<LaunchForm>) {
        self.setup_open = !command.trim().is_empty();
        self.setup
            .update(cx, |input, cx| input.set_value(command, window, cx));
    }
    /// The command to run once in the new worktree, empty for none.
    pub(super) fn setup_command(&self, cx: &App) -> String {
        self.setup.read(cx).value().trim().to_owned()
    }
}

fn initial_ref(project: Option<&Path>, cx: &App) -> (Option<PathBuf>, String) {
    let projects = AppState::get(cx).projects.read(cx);
    let Some(repo) = project.and_then(|p| projects.project(&Host::Local, p)) else {
        return (None, "HEAD".into());
    };
    let (path, reference) = checkout_ref(repo, projects.target().as_ref());
    (Some(path), reference)
}

fn checkout_ref(repo: &Project, selected: Option<&Target>) -> (PathBuf, String) {
    let path = selected
        .filter(|t| t.project == repo.path && t.host.is_local())
        .map_or(&repo.path, |t| &t.path);
    let branch = repo
        .worktrees
        .iter()
        .find(|w| w.path == *path)
        .and_then(Worktree::branch);
    (
        path.clone(),
        branch.map_or_else(|| "HEAD".into(), |b| format!("refs/heads/{b}")),
    )
}

pub(super) fn repositories(cx: &App) -> Vec<(PathBuf, String)> {
    let projects: Vec<_> = AppState::get(cx)
        .projects
        .read(cx)
        .items
        .iter()
        .filter(|p| p.host.is_local() && !p.worktrees.is_empty())
        .collect();
    projects
        .iter()
        .map(|p| {
            let name = if projects
                .iter()
                .any(|other| other.path != p.path && other.name == p.name)
            {
                format!("{} · {}", p.name, p.path.display())
            } else {
                p.name.clone()
            };
            (p.path.clone(), name)
        })
        .collect()
}

/// An outlined dropdown trigger, so every choice on the form reads as editable. Icon and
/// label travel as one child: the button spreads separate parts apart, which would float
/// the label in the middle of a wide field instead of next to its icon.
pub(super) fn dropdown(
    id: impl Into<ElementId>,
    icon: Lucide,
    label: impl Into<SharedString>,
) -> Button {
    let label = label.into();
    Button::new(id)
        .small()
        .outline()
        .cursor_pointer()
        .dropdown_caret(true)
        .accessibility_label(label.clone())
        .child(
            h_flex()
                .min_w_0()
                .gap_1p5()
                .child(Icon::new(icon).size_3p5().flex_none())
                .child(div().min_w_0().truncate().child(label)),
        )
}

#[cfg(test)]
mod tests {
    use super::{Host, Project, Target, Worktree, branch_of, checkout_ref};
    use hive_core::{projects::Head, state::ProjectRef};
    #[test]
    fn only_a_branch_ref_names_the_target_of_a_pull_request() {
        assert_eq!(branch_of("refs/heads/develop"), Some("develop"));
        assert_eq!(branch_of("refs/heads/feature/login"), Some("feature/login"));
        assert_eq!(
            branch_of("refs/remotes/origin/release/2"),
            Some("release/2")
        );
        for other in [
            "HEAD",
            "refs/tags/v1.0.27",
            "4f2c9d1",
            "refs/remotes/origin",
        ] {
            assert_eq!(branch_of(other), None, "{other}");
        }
    }
    #[test]
    fn a_detached_selection_keeps_its_own_head_in_the_preview_key() {
        let mut repo = Project::from(&ProjectRef::new("/project".into(), Host::Local));
        repo.worktrees = vec![
            Worktree {
                path: repo.path.clone(),
                head: Head::Branch("main".into()),
            },
            Worktree {
                path: "/detached".into(),
                head: Head::Detached,
            },
        ];
        let selected = Target::of(&repo, Some(&repo.worktrees[1]));
        assert_eq!(
            checkout_ref(&repo, Some(&selected)),
            (selected.path, "HEAD".into())
        );
        assert_eq!(
            checkout_ref(&repo, None),
            (repo.path, "refs/heads/main".into())
        );
    }
}
