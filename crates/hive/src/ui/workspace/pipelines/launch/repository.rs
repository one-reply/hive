use super::*;
use gpui_kit::component::spinner::Spinner;
use hive_core::git::branches::short_name;

/// Repository, base and check share one row until the drawer is too narrow for three.
const FIELD_MIN_WIDTH: Pixels = px(170.);

#[derive(Clone, PartialEq)]
enum Check {
    Suggested,
    Command(String),
    Custom,
}

pub(super) struct Binding {
    pub preset: LanePreset,
    pub project: Option<PathBuf>,
    checkout: Option<PathBuf>,
    reference: String,
    ref_input: Option<Entity<InputState>>,
    check: Check,
    custom_command: Entity<InputState>,
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
        let (checkout, reference) = initial_ref(project.as_deref(), cx);
        Self {
            preset,
            project,
            checkout,
            reference,
            ref_input: None,
            check: Check::Suggested,
            custom_command,
            _command_changes: command_changes,
        }
    }
    pub(super) fn bind(&mut self, project: Option<PathBuf>, cx: &mut Context<LaunchForm>) {
        (self.checkout, self.reference) = initial_ref(project.as_deref(), cx);
        self.project = project;
        self.ref_input = None;
        self.check = Check::Suggested;
        self.preview(cx);
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

/// A caption above a control; fields share a row and wrap below this width.
fn labeled(label: &'static str, control: impl IntoElement, cx: &App) -> Div {
    v_flex()
        .flex_1()
        .min_w(FIELD_MIN_WIDTH)
        .gap_1()
        .child(subtle(label, cx))
        .child(control)
}

/// What the repository is doing right now, under its fields.
enum Notice {
    Reading,
    Dirty,
    Failed(String),
}

fn notice(notice: Notice, cx: &App) -> Div {
    let row = h_flex().gap_2().items_center().text_xs();
    let (color, text) = match notice {
        Notice::Reading => {
            return row
                .text_color(cx.theme().muted_foreground)
                .child(Spinner::new().icon(Lucide::LoaderCircle).xsmall())
                .child("Reading repository…");
        }
        Notice::Dirty => (
            cx.theme().warning,
            "This checkout has local changes. The run uses committed files only.".to_owned(),
        ),
        Notice::Failed(error) => (cx.theme().danger, error),
    };
    row.child(Icon::new(Lucide::CircleAlert).size_3p5().text_color(color))
        .child(div().min_w_0().flex_1().text_color(color).child(text))
}

impl LaunchForm {
    pub(super) fn repository_card(
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
        let repo = options_menu(
            dropdown(
                ("bound-repo", i),
                Lucide::Folder,
                selected.unwrap_or("Choose repository").to_owned(),
            )
            .w_full(),
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
        let mut group = card(cx).w_full().gap_3();
        if i < self.template.lanes.len() && self.template.lanes.len() > 1 {
            group = group.child(
                h_flex()
                    .gap_1p5()
                    .child(
                        Icon::new(Lucide::GitFork)
                            .size_3p5()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(heading(b.preset.name.clone())),
            );
        }
        if b.project.is_none() {
            return group
                .child(labeled("Repository", repo, cx))
                .child(subtle("Optional lane. Leave it empty to skip it.", cx));
        }
        let preview = b.base_preview(cx);
        let data = preview.and_then(|p| p.as_ref().ok()).cloned();
        let message = match preview {
            Some(Err(e)) => Some(Notice::Failed(e.clone())),
            Some(Ok(p)) if p.dirty => Some(Notice::Dirty),
            None => Some(Notice::Reading),
            _ => None,
        };
        let fields = h_flex()
            .w_full()
            .flex_wrap()
            .items_start()
            .gap_3()
            .child(labeled("Repository", repo, cx))
            .child(labeled(
                "Start from",
                self.ref_menu(i, data.as_ref(), cx),
                cx,
            ))
            .child(labeled(
                "Check before acceptance",
                self.check_menu(i, data.as_ref(), cx),
                cx,
            ));
        group = group.child(fields);
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
                "Command that must pass before acceptance",
                &b.custom_command,
                cx,
            ));
        }
        group.when_some(message, |el, m| el.child(notice(m, cx)))
    }

    fn ref_menu(&self, i: usize, data: Option<&BasePreview>, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.bindings[i];
        let mut branches: Vec<_> = data
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
        options_menu(
            dropdown(
                ("pipeline-ref", i),
                Lucide::GitBranch,
                short_name(&b.reference).to_owned(),
            )
            .w_full(),
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
        )
    }

    /// Until the commit is read there are no manifest checks to offer, so the slot stays
    /// in place but disabled instead of making the row jump when it loads.
    fn check_menu(
        &self,
        i: usize,
        data: Option<&BasePreview>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let b = &self.bindings[i];
        let Some(preview) = data else {
            return dropdown(("pipeline-check", i), Lucide::ListChecks, "Agent review")
                .w_full()
                .disabled(true)
                .into_any_element();
        };
        let command = b.command(preview, cx);
        let label = if matches!(b.check, Check::Custom) {
            "Custom command"
        } else if command.is_empty() {
            "Agent review"
        } else {
            &command
        };
        let mut commands = preview.checks.clone();
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
        options_menu(
            dropdown(("pipeline-check", i), Lucide::ListChecks, label.to_owned()).w_full(),
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
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Host, Project, Target, Worktree, checkout_ref};
    use hive_core::{projects::Head, state::ProjectRef};
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
