use super::*;
use gpui_kit::component::button::ButtonVariants as _;
use hive_core::git::branches::short_name;

#[derive(Clone, PartialEq)]
pub(super) enum Check {
    Suggested,
    Command(String),
    Custom,
}

pub(super) struct Binding {
    pub preset: LanePreset,
    pub project: Option<PathBuf>,
    reference: String,
    pub ref_input: Option<Entity<InputState>>,
    pub check: Check,
    pub custom_command: Entity<InputState>,
    _command_changes: Subscription,
}

impl Binding {
    pub(super) fn new(
        preset: LanePreset,
        project: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<LaunchForm>,
    ) -> Self {
        let custom_command = input("", "Shell command", window, cx);
        let command_changes = cx.subscribe(&custom_command, |_, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let reference = initial_ref(project.as_deref(), cx);
        Self {
            preset,
            project,
            reference,
            ref_input: None,
            check: Check::Suggested,
            custom_command,
            _command_changes: command_changes,
        }
    }
    pub(super) fn bind(&mut self, project: Option<PathBuf>, cx: &mut Context<LaunchForm>) {
        self.reference = initial_ref(project.as_deref(), cx);
        self.project = project;
        self.ref_input = None;
        self.check = Check::Suggested;
        self.preview(cx);
    }
    pub(super) fn preview(&self, cx: &mut Context<LaunchForm>) {
        if let Some(project) = &self.project {
            AppState::get(cx).pipelines.clone().update(cx, |p, cx| {
                p.preview(project.clone(), self.reference.clone(), cx)
            });
        }
    }
    pub(super) fn base_preview<'a>(&self, cx: &'a App) -> Option<&'a Result<BasePreview, String>> {
        let project = self.project.as_ref()?;
        AppState::get(cx)
            .pipelines
            .read(cx)
            .previews
            .get(&(project.clone(), self.reference.clone()))
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
}

fn initial_ref(project: Option<&Path>, cx: &App) -> String {
    let projects = AppState::get(cx).projects.read(cx);
    let branch = project.and_then(|p| {
        let repo = projects.project(&Host::Local, p)?;
        let selected = projects
            .target()
            .filter(|t| t.project == p && t.host.is_local());
        let path = selected.as_ref().map_or(p, |t| t.path.as_path());
        repo.worktrees
            .iter()
            .find(|w| w.path == path)
            .and_then(|w| w.branch())
    });
    branch.map_or_else(|| "HEAD".into(), |b| format!("refs/heads/{b}"))
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

pub(super) fn select_menu<T: Clone + 'static>(
    id: impl Into<ElementId>,
    icon: Lucide,
    label: impl Into<SharedString>,
    options: Vec<(String, bool, T)>,
    cx: &mut Context<LaunchForm>,
    select: impl Fn(&mut LaunchForm, T, &mut Window, &mut Context<LaunchForm>) + 'static,
) -> AnyElement {
    options_menu(
        Button::new(id)
            .small()
            .ghost()
            .cursor_pointer()
            .icon(icon)
            .label(label),
        options,
        cx,
        select,
    )
}

impl LaunchForm {
    pub(super) fn repository_row(
        &self,
        i: usize,
        projects: &[(PathBuf, String)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let b = &self.bindings[i];
        let selected = projects
            .iter()
            .find(|(p, _)| Some(p) == b.project.as_ref())
            .map(|(_, n)| n.as_str());
        let mut choices: Vec<_> = projects
            .iter()
            .map(|(p, n)| (n.clone(), Some(p) == b.project.as_ref(), Some(p.clone())))
            .collect();
        if b.project.is_some() {
            choices.push(("Remove from run".into(), false, None));
        }
        let repo = select_menu(
            ("bound-repo", i),
            Lucide::Folder,
            selected.unwrap_or("Choose repository").to_owned(),
            choices,
            cx,
            move |this, p, _, cx| {
                if p.is_none() && i >= this.template.lanes.len() {
                    this.bindings.remove(i);
                } else {
                    this.bindings[i].bind(p, cx);
                }
            },
        );
        let mut row = h_flex().flex_wrap().gap_2().child(repo);
        if i < self.template.lanes.len()
            && self.template.lanes.len() > 1
            && selected != Some(b.preset.name.as_str())
        {
            row = row.child(subtle(b.preset.name.clone(), cx));
        }
        let mut group = v_flex()
            .w_full()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .py_2();
        if b.project.is_none() {
            return group.child(row.child(subtle("Optional lane", cx)));
        }
        let preview = b.base_preview(cx);
        let data = preview.and_then(|p| p.as_ref().ok()).cloned();
        let message = match preview {
            Some(Err(e)) => Some(e.clone()),
            Some(Ok(p)) if p.dirty => {
                Some("This checkout has local changes. The run will use committed files.".into())
            }
            None => Some("Reading repository…".into()),
            _ => None,
        };
        let mut branches: Vec<_> = data
            .as_ref()
            .into_iter()
            .flat_map(|p| &p.branches)
            .map(|branch| {
                (
                    branch.name.clone(),
                    branch.reference == b.reference,
                    Some(branch.reference.clone()),
                )
            })
            .collect();
        if !branches
            .iter()
            .any(|(_, _, v)| v.as_deref() == Some(&b.reference))
        {
            branches.insert(
                0,
                (
                    short_name(&b.reference).to_owned(),
                    true,
                    Some(b.reference.clone()),
                ),
            );
        }
        branches.push(("Other branch, tag or commit…".into(), false, None));
        row = row.child(select_menu(
            ("pipeline-ref", i),
            Lucide::GitBranch,
            short_name(&b.reference).to_owned(),
            branches,
            cx,
            move |this, value, w, cx| {
                let b = &mut this.bindings[i];
                if let Some(value) = value {
                    b.reference = value;
                    b.ref_input = None;
                    b.preview(cx);
                } else {
                    let editor = input(&b.reference, "Branch, tag or commit", w, cx);
                    editor.focus_handle(cx).focus(w, cx);
                    b.ref_input = Some(editor);
                }
            },
        ));
        if let Some(preview) = data {
            let command = b.command(&preview, cx);
            let label = if matches!(b.check, Check::Custom) {
                "Custom check"
            } else if command.is_empty() {
                "Agent review"
            } else {
                &command
            };
            let mut commands = preview.checks;
            if !b.preset.command.trim().is_empty() && !commands.contains(&b.preset.command) {
                commands.insert(0, b.preset.command.clone());
            }
            let mut checks: Vec<_> = commands
                .into_iter()
                .map(|s| (s.clone(), s == command, Check::Command(s)))
                .collect();
            checks.push((
                "Agent review only".into(),
                command.is_empty() && !matches!(b.check, Check::Custom),
                Check::Command(String::new()),
            ));
            checks.push((
                "Custom command…".into(),
                matches!(b.check, Check::Custom),
                Check::Custom,
            ));
            row = row.child(select_menu(
                ("pipeline-check", i),
                Lucide::ListChecks,
                label.to_owned(),
                checks,
                cx,
                move |this, check, w, cx| {
                    if check == Check::Custom {
                        this.bindings[i]
                            .custom_command
                            .focus_handle(cx)
                            .focus(w, cx);
                    }
                    this.bindings[i].check = check;
                },
            ));
        }
        group = group.child(row);
        if let Some(ref_input) = &b.ref_input {
            group = group.child(
                h_flex()
                    .gap_2()
                    .child(div().min_w_0().flex_1().child(TextField::input(
                        Input::new(ref_input),
                        ref_input,
                        cx,
                    )))
                    .child(button(
                        ("apply-ref", i),
                        "Use ref",
                        ButtonLook::Outline,
                        window,
                        cx,
                        move |this, _, cx| {
                            let b = &mut this.bindings[i];
                            if let Some(input) = b.ref_input.take() {
                                let value = input.read(cx).value().trim().to_owned();
                                if !value.is_empty() {
                                    b.reference = value;
                                }
                                b.preview(cx);
                                cx.notify();
                            }
                        },
                    )),
            );
        }
        if matches!(b.check, Check::Custom) {
            group = group.child(field(
                "Command to pass before acceptance",
                &b.custom_command,
                cx,
            ));
        }
        if let Some(message) = message {
            group = group.child(subtle(message, cx));
        }
        group
    }
}
