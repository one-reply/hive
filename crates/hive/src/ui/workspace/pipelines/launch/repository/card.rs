//! A repository drawn as a card: the repository itself on top, then its branch, base and
//! check, then the optional setup command.
use super::*;
use gpui_kit::component::spinner::Spinner;
use readiness::{BranchProblem, branch_problem};

/// Branch, base and check share one row until the drawer is too narrow for them.
const FIELD_MIN_WIDTH: Pixels = px(170.);

/// A caption above a control; fields share a row and wrap below this width.
fn labeled(caption: impl IntoElement, control: impl IntoElement) -> Div {
    v_flex()
        .flex_1()
        .min_w(FIELD_MIN_WIDTH)
        .gap_1()
        .child(caption)
        .child(control)
}

/// A caption whose field must be filled before planning can start.
fn required(label: &'static str, cx: &App) -> Div {
    h_flex()
        .gap_0p5()
        .child(subtle(label, cx))
        .child(required_mark(cx))
}

/// What the repository is doing right now, under its fields.
enum Notice {
    Reading,
    Dirty,
    Branch(BranchProblem),
    Failed(String),
}

impl LaunchForm {
    fn notice(&self, i: usize, notice: Notice, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let row = h_flex().gap_2().items_center().text_xs();
        let failed = matches!(notice, Notice::Failed(_));
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
            Notice::Branch(problem) => (cx.theme().danger, problem.message().to_owned()),
            Notice::Failed(error) => (cx.theme().danger, error),
        };
        let row = row
            .child(Icon::new(Lucide::CircleAlert).size_3p5().text_color(color))
            .child(div().min_w_0().flex_1().text_color(color).child(text));
        if !failed {
            return row;
        }
        row.child(button(
            ("read-repo-again", i),
            "Try again",
            ButtonLook::Outline,
            window,
            cx,
            move |this, _, cx| this.read_again(i, cx),
        ))
    }

    pub(in crate::ui::workspace::pipelines::launch) fn repository_card(
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
        let choices = projects
            .iter()
            .map(|(p, n)| (n.clone(), Some(p) == b.project.as_ref(), p.clone()))
            .collect();
        let repo = options_menu(
            dropdown(
                ("bound-repo", i),
                Lucide::Folder,
                selected.unwrap_or("Choose repository").to_owned(),
            ),
            choices,
            cx,
            move |this, p, window, cx| this.rebind(i, p, window, cx),
        );
        // A recipe with several lanes names the one this repository fills.
        let lane = (i < self.template.lanes.len() && self.template.lanes.len() > 1).then(|| {
            h_flex()
                .flex_none()
                .gap_1p5()
                .child(
                    Icon::new(Lucide::GitFork)
                        .size_3p5()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(heading(b.preset.name.clone()))
        });
        let head = h_flex()
            .w_full()
            .gap_3()
            .children(lane)
            .child(div().min_w_0().child(repo));
        let group = card(cx).w_full().gap_3();
        if b.project.is_none() {
            return group
                .child(head)
                .child(subtle("Optional lane. Leave it empty to skip it.", cx));
        }
        let data = b.ready(cx).cloned();
        let message = match b.base(cx) {
            Preview::Failed(failure) => Some(Notice::Failed(failure.detail.clone())),
            Preview::Reading => Some(Notice::Reading),
            Preview::Ready(preview) => branch_problem(&b.branch, &preview.branches)
                .map(Notice::Branch)
                .or_else(|| b.shown(preview, cx).local_changes.then_some(Notice::Dirty)),
        };
        let remove = icon_action(
            ("remove-repo", i),
            "Remove from run",
            Lucide::X,
            window,
            cx,
            move |this, window, cx| this.remove_repository(i, window, cx),
        );
        let mut fields = h_flex()
            .w_full()
            .flex_wrap()
            .items_start()
            .gap_3()
            .child(labeled(required("Branch", cx), self.branch_field(i, cx)));
        if matches!(b.branch, WorkBranch::New(_)) {
            fields = fields.child(labeled(
                subtle("From", cx),
                self.ref_menu(i, data.as_ref(), cx),
            ));
        }
        let fields = fields.child(labeled(
            subtle("Check before acceptance", cx),
            self.check_menu(i, data.as_ref(), cx),
        ));
        let mut group = group
            .child(head.child(div().flex_1()).child(remove))
            .child(fields);
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
                                b.read(cx);
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
        group
            .child(self.setup_field(i, window, cx))
            .children(message.map(|message| self.notice(i, message, window, cx)))
    }

    /// The setup command, or the button that brings its field in.
    fn setup_field(&self, i: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.bindings[i];
        if b.setup_open {
            return field(
                "Setup command. Runs once in the new worktree, after files listed in \
                 .worktreeinclude are copied",
                &b.setup,
                cx,
            )
            .into_any_element();
        }
        let id = ElementId::from(("add-setup-command", i));
        let hover = Hover::new(id.clone(), window, cx);
        let add = Button::new(id)
            .cursor_pointer()
            .small()
            .icon(Lucide::Plus)
            .label("Setup command")
            .tooltip("Run a command once in the new worktree, such as npm ci")
            .on_click(cx.listener(move |this, _, window, cx| {
                let b = &mut this.bindings[i];
                b.setup_open = true;
                b.setup.focus_handle(cx).focus(window, cx);
                cx.notify();
            }))
            .smooth_hover(ButtonLook::Ghost, &hover, cx);
        // The button keeps its own width instead of stretching across the card.
        h_flex().child(add).into_any_element()
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
                    b.read(cx);
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
