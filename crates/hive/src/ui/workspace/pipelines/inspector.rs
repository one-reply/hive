use super::editor::{Item, TemplateEditor};
use super::*;
use crate::ui::hover::SmoothHoverBg as _;
use hive_core::agent::{
    AgentKind,
    launch::{AGENTS, pipeline_efforts},
};
use hive_core::pipeline::Profile;

mod agents;
mod findings;
mod models;
use models::ModelPicker;
pub(super) struct Inspector {
    picker: ModelPicker,
    pub(super) name: Entity<InputState>,
    instruction: Entity<TextareaState>,
    pub(super) model: Entity<InputState>,
    limit: u8,
    rename: bool,
    edit_prompt: bool,
    custom_model: bool,
    pub(super) agent: AgentKind,
    effort: String,
    route: Option<String>,
}
impl Inspector {
    pub(super) fn new(
        item: Item,
        t: &Template,
        w: &mut Window,
        cx: &mut Context<TemplateEditor>,
    ) -> Self {
        fn stage(step: &hive_core::pipeline::Step) -> (&str, &str, &Profile, u8, Option<String>) {
            (
                &step.name,
                &step.instruction,
                &step.profile,
                step.limit,
                step.return_lane.clone(),
            )
        }
        let (name, body, profile, limit, route) = match item {
            Item::Planner => stage(&t.planner),
            Item::Development => stage(&t.development),
            Item::Review(i) => stage(&t.reviews[i]),
            Item::Lane(i) => {
                let l = &t.lanes[i];
                (l.name.as_str(), l.command.as_str(), &l.profile, 3, None)
            }
        };
        ensure_models(profile.agent, cx);
        Self {
            picker: ModelPicker::new(profile.agent, &profile.model, t, w, cx),
            name: input(name, "Name", w, cx),
            instruction: area(body, "", w, cx),
            model: input(&profile.model, "CLI default or exact model ID", w, cx),
            limit,
            rename: false,
            edit_prompt: false,
            custom_model: false,
            agent: profile.agent,
            effort: profile.effort.clone(),
            route,
        }
    }
    pub(super) fn apply(&self, item: Item, t: &mut Template, cx: &App) -> Result<(), String> {
        let name = self.name.read(cx).value().trim().to_owned();
        if name.is_empty() {
            return Err("Enter a name for this step or lane.".into());
        }
        let body = self.instruction.read(cx).value().to_string();
        let profile = Profile {
            agent: self.agent,
            model: self.model.read(cx).value().trim().to_owned(),
            effort: self.effort.clone(),
        };
        hive_core::agent::launch::validate_pipeline_profile(&profile).map_err(|e| e.to_string())?;
        if item == Item::Development {
            t.set_development_profile(profile.clone());
        }
        let step = match item {
            Item::Planner => &mut t.planner,
            Item::Development => &mut t.development,
            Item::Review(i) => t
                .reviews
                .get_mut(i)
                .ok_or("This review no longer exists.")?,
            Item::Lane(i) => {
                t.rename_lane(i, name)?;
                let lane = &mut t.lanes[i];
                lane.command = body;
                lane.profile = profile;
                return Ok(());
            }
        };
        if matches!(item, Item::Review(_)) {
            step.limit = self.limit;
            step.return_lane = self.route.clone();
        }
        step.name = name;
        step.instruction = body;
        step.profile = profile;
        Ok(())
    }
}
fn ensure_models(agent: AgentKind, cx: &mut App) {
    let pipelines = AppState::get(cx).pipelines.clone();
    if !pipelines.read(cx).models.options.contains_key(&agent) {
        pipelines.update(cx, |p, cx| p.load_models(agent, cx));
    }
}
impl TemplateEditor {
    pub(super) fn sync_models(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let id = self.inspector.model.read(cx).value();
        self.inspector
            .picker
            .sync(self.inspector.agent, id.as_ref(), &self.template, w, cx);
    }
    pub(super) fn inspector_fields(&self, w: &mut Window, cx: &mut Context<Self>) -> Div {
        let mut fields = v_flex()
            .gap_4()
            .child(self.stage_identity(w, cx))
            .child(self.agent_settings(w, cx))
            .child(self.instructions(w, cx));
        if matches!(self.selected, Item::Review(_)) {
            fields = fields.child(self.review_routing(cx));
        }
        fields
    }
    fn stage_identity(&self, w: &mut Window, cx: &mut Context<Self>) -> Div {
        let form = &self.inspector;
        let kind = self.role(self.selected);
        let color = kind.color(cx);
        let lane = matches!(self.selected, Item::Lane(_));
        let title = form.name.read(cx).value();
        let mut identity = v_flex().gap_2().child(
            h_flex()
                .gap_2()
                .child(
                    h_flex()
                        .size_7()
                        .flex_none()
                        .justify_center()
                        .rounded_md()
                        .bg(color.opacity(ROLE_TINT_OPACITY))
                        .child(Icon::new(kind.icon()).size_4().text_color(color)),
                )
                .child(
                    subtle(
                        if lane {
                            "Development lane"
                        } else {
                            kind.label()
                        },
                        cx,
                    )
                    .flex_1(),
                )
                .children(self.ordering(cx)),
        );
        if form.rename {
            identity = identity.child(
                h_flex()
                    .gap_1()
                    .child(div().flex_1().min_w_0().child(TextField::input(
                        Input::new(&form.name),
                        &form.name,
                        cx,
                    )))
                    .child(icon_action(
                        "stage-name-done",
                        "Finish editing name",
                        Lucide::Check,
                        w,
                        cx,
                        |s: &mut Self, _, cx| {
                            s.inspector.rename = false;
                            cx.notify()
                        },
                    )),
            );
        } else {
            identity = identity.child(
                h_flex()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(icon_action(
                        "edit-stage-name",
                        "Rename stage",
                        Lucide::Pencil,
                        w,
                        cx,
                        |s: &mut Self, w, cx| {
                            s.inspector.rename = true;
                            s.inspector.name.read(cx).focus_handle(cx).focus(w, cx);
                            cx.notify()
                        },
                    )),
            );
        }
        identity
    }
    fn instructions(&self, w: &mut Window, cx: &mut Context<Self>) -> Div {
        let form = &self.inspector;
        let prompt = form.instruction.read(cx).value();
        let lane = matches!(self.selected, Item::Lane(_));
        let mut prompt_box = section(cx).child(
            section_title(
                if lane {
                    Lucide::SquareTerminal
                } else {
                    Lucide::MessageSquare
                },
                if lane {
                    "Required QA command"
                } else {
                    "Instructions"
                },
                cx,
            )
            .child(icon_action(
                "edit-stage-prompt",
                if form.edit_prompt {
                    "Finish editing"
                } else {
                    "Edit"
                },
                if form.edit_prompt {
                    Lucide::Check
                } else {
                    Lucide::Pencil
                },
                w,
                cx,
                |s: &mut Self, w, cx| {
                    s.inspector.edit_prompt = !s.inspector.edit_prompt;
                    if s.inspector.edit_prompt {
                        s.inspector
                            .instruction
                            .read(cx)
                            .focus_handle(cx)
                            .focus(w, cx);
                    }
                    cx.notify()
                },
            )),
        );
        if form.edit_prompt {
            prompt_box = prompt_box.child(TextField::textarea(
                Textarea::new(&form.instruction),
                &form.instruction,
                cx,
            ));
        } else {
            let empty = prompt.is_empty();
            let hover = Hover::new("stage-prompt-preview", w, cx);
            // The text itself opens the editor: the pencil is easy to miss in a narrow panel.
            let preview = div()
                .id("stage-prompt-preview")
                .role(Role::Button)
                .aria_label(if lane {
                    "Edit required QA command"
                } else {
                    "Edit instructions"
                })
                .tab_index(0)
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().border)
                .cursor_pointer()
                .text_sm()
                .when(empty, |el| el.italic())
                .text_color(if empty {
                    cx.theme().muted_foreground
                } else {
                    cx.theme().foreground
                })
                .child(match (empty, lane) {
                    (false, _) => prompt,
                    (true, true) => "No command required".into(),
                    (true, false) => "No instructions".into(),
                });
            prompt_box = prompt_box.child(
                on_activate(preview, cx, |s: &mut Self, w, cx| {
                    s.inspector.edit_prompt = true;
                    s.inspector
                        .instruction
                        .read(cx)
                        .focus_handle(cx)
                        .focus(w, cx);
                    cx.notify()
                })
                .hover_bg(&hover, cx.theme().transparent, cx.theme().muted),
            );
        }
        prompt_box
    }
}

/// The role icon's tile is washed out so the icon on top keeps its contrast.
const ROLE_TINT_OPACITY: f32 = 0.12;

/// One block of the inspector, divided from the block above it.
fn section(cx: &App) -> Div {
    v_flex()
        .gap_3()
        .pt_4()
        .border_t_1()
        .border_color(cx.theme().border)
}

/// A section's icon and name. Trailing actions added by the caller sit at the far end.
fn section_title(icon: Lucide, title: &'static str, cx: &App) -> Div {
    h_flex()
        .gap_2()
        .child(
            Icon::new(icon)
                .size_3p5()
                .text_color(cx.theme().muted_foreground),
        )
        .child(heading(title).flex_1())
}

/// A label above its control: the panel is too narrow for both on one line.
fn setting(label: &'static str, value: AnyElement, cx: &App) -> Div {
    v_flex()
        .gap_1p5()
        .child(subtle(label, cx))
        .child(div().w_full().min_w_0().child(value))
}
