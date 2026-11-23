use super::editor::{Item, TemplateEditor};
use super::*;
use crate::ui::hover::SmoothHoverBg as _;
use hive_core::agent::{
    AgentKind,
    launch::{AGENTS, PipelineRole, pipeline_efforts, pipeline_modes},
};
use hive_core::pipeline::{PermissionMode, Profile};

mod agents;
mod findings;
mod instructions;
mod models;
mod modes;
use instructions::PromptPicker;
use models::ModelPicker;
pub(super) use modes::mode_label;
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
    /// Library prompt the stage follows; `instruction` then shows that prompt's text.
    prompt: Option<u64>,
    /// Absent for a lane, which has a QA command instead of instructions.
    prompt_picker: Option<PromptPicker>,
    /// `None` follows the role's own mode, as a stage saved before modes existed does.
    mode: Option<PermissionMode>,
}
impl Inspector {
    pub(super) fn new(
        item: Item,
        t: &Template,
        w: &mut Window,
        cx: &mut Context<TemplateEditor>,
    ) -> Self {
        type Fields<'a> = (
            &'a str,
            &'a str,
            &'a Profile,
            u8,
            Option<String>,
            Option<u64>,
        );
        fn stage(step: &hive_core::pipeline::Step) -> Fields<'_> {
            (
                &step.name,
                &step.instruction,
                &step.profile,
                step.limit,
                step.return_lane.clone(),
                step.prompt,
            )
        }
        let (name, body, profile, limit, route, prompt) = match item {
            Item::Planner => stage(&t.planner),
            Item::Development => stage(&t.development),
            Item::Review(i) => stage(&t.reviews[i]),
            Item::Lane(i) => {
                let l = &t.lanes[i];
                (
                    l.name.as_str(),
                    l.command.as_str(),
                    &l.profile,
                    3,
                    None,
                    None,
                )
            }
        };
        let is_stage = !matches!(item, Item::Lane(_));
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
            prompt,
            prompt_picker: is_stage.then(|| PromptPicker::new(prompt, w, cx)),
            mode: profile.mode,
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
            mode: self.mode,
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
        step.prompt = self.prompt;
        step.profile = profile;
        Ok(())
    }
}
/// The mode a stage launches in when its recipe chose none: only developers may edit.
fn role_mode(item: Item) -> PermissionMode {
    let role = match item {
        Item::Planner => PipelineRole::Plan,
        Item::Review(_) => PipelineRole::Review,
        Item::Development | Item::Lane(_) => PipelineRole::Develop,
    };
    role.default_mode()
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
        let lane = matches!(self.selected, Item::Lane(_));
        let title = form.name.read(cx).value();
        let mut identity = v_flex().gap_2().child(
            h_flex()
                .gap_2()
                .child(kind.tile(cx))
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
        let editing = form.rename && form.name.read(cx).focus_handle(cx).is_focused(w);
        let name = if form.rename {
            // Edited in place, in the title's own type.
            Input::new(&form.name)
                .appearance(false)
                .p_0()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .into_any_element()
        } else {
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title)
                .into_any_element()
        };
        let action = if form.rename {
            icon_action(
                "stage-name-done",
                "Finish editing name",
                Lucide::Check,
                w,
                cx,
                |s: &mut Self, _, cx| {
                    s.inspector.rename = false;
                    cx.notify()
                },
            )
        } else {
            icon_action(
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
            )
        };
        // The underline also divides the name from the Agent block below it.
        identity = identity.child(
            h_flex()
                .gap_1()
                .pb_1()
                .border_b_1()
                .border_color(if editing {
                    cx.theme().ring
                } else {
                    cx.theme().border
                })
                .child(div().flex_1().min_w_0().child(name))
                .child(action),
        );
        identity
    }
}

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
