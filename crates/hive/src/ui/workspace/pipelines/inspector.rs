use super::editor::{Item, TemplateEditor};
use super::*;
use hive_core::agent::{
    AgentKind,
    launch::{AGENTS, pipeline_efforts},
};
use hive_core::pipeline::Profile;

mod agents;
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
            update_inherited_profiles(t, &profile);
        }
        let step = match item {
            Item::Planner => &mut t.planner,
            Item::Development => &mut t.development,
            Item::Review(i) => &mut t.reviews[i],
            Item::Lane(i) => {
                if t.lanes
                    .iter()
                    .enumerate()
                    .any(|(ix, l)| ix != i && l.name == name)
                {
                    return Err("Lane names must be unique.".into());
                }
                let lane = &mut t.lanes[i];
                for step in &mut t.reviews {
                    if step.return_lane.as_ref() == Some(&lane.name) {
                        step.return_lane = Some(name.clone())
                    }
                }
                lane.name = name;
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
fn update_inherited_profiles(t: &mut Template, profile: &Profile) {
    for lane in &mut t.lanes {
        if lane.profile == t.development.profile {
            lane.profile = profile.clone();
        }
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
                .child(Icon::new(kind.icon()).size_4().text_color(color))
                .child(subtle(
                    if lane {
                        "Development lane"
                    } else {
                        kind.label()
                    },
                    cx,
                )),
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
        let mut prompt_box = v_flex()
            .gap_2()
            .pt_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .child(div().flex_1().child(heading(if lane {
                        "Required QA command"
                    } else {
                        "Instructions"
                    })))
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
            prompt_box = prompt_box.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(if prompt.is_empty() {
                        if lane {
                            "No command required".into()
                        } else {
                            "No instructions".into()
                        }
                    } else {
                        prompt
                    }),
            );
        }
        prompt_box
    }
    fn review_routing(&self, cx: &mut Context<Self>) -> Div {
        let form = &self.inspector;
        let mut routes = vec![(None, "Affected lanes".into())];
        routes.extend(
            self.template
                .lanes
                .iter()
                .map(|l| (Some(l.name.clone()), l.name.clone())),
        );
        let mut limits = vec![1, 2, 3, 4, 5];
        if !limits.contains(&form.limit) {
            limits.push(form.limit);
        }
        let options = limits
            .into_iter()
            .map(|n| {
                (
                    n,
                    format!("{n} {}", if n == 1 { "round" } else { "rounds" }),
                )
            })
            .collect();
        v_flex()
            .gap_2()
            .pt_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Icon::new(Lucide::CornerUpLeft)
                            .size_3p5()
                            .text_color(cx.theme().warning),
                    )
                    .child(heading("Findings")),
            )
            .child(setting(
                "Return to",
                choice(
                    "stage-route",
                    &form.route,
                    routes,
                    cx,
                    |s: &mut Self, r, _, _| {
                        if let Item::Review(i) = s.selected {
                            s.template.reviews[i].return_lane = r.clone();
                        }
                        s.inspector.route = r
                    },
                ),
                cx,
            ))
            .child(setting(
                "Ask after",
                choice(
                    "stage-limit",
                    &form.limit,
                    options,
                    cx,
                    |s: &mut Self, n, _, _| s.inspector.limit = n,
                ),
                cx,
            ))
    }
}

fn setting(label: &'static str, value: AnyElement, cx: &App) -> Div {
    h_flex()
        .gap_2()
        .justify_between()
        .child(subtle(label, cx).flex_none())
        .child(div().min_w_0().overflow_hidden().child(value))
}

#[cfg(test)]
mod tests {
    use super::{Template, update_inherited_profiles};
    #[test]
    fn changing_development_profile_preserves_independent_lane_model() {
        let mut t = Template::default();
        let mut custom = t.lanes[0].clone();
        custom.name = "API".into();
        custom.profile.model = "custom".into();
        t.lanes.push(custom.clone());
        let mut profile = t.development.profile.clone();
        profile.model = "new-default".into();
        update_inherited_profiles(&mut t, &profile);
        assert_eq!(t.lanes[0].profile, profile);
        assert_eq!(t.lanes[1], custom);
    }
}
