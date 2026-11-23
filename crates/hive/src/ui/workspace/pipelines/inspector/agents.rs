use super::*;
impl TemplateEditor {
    pub(super) fn agent_settings(&self, w: &mut Window, cx: &mut Context<Self>) -> Div {
        let form = &self.inspector;
        let color = self.role(self.selected).color(cx);
        let mut provider = v_flex().gap_1p5();
        for pair in AGENTS.chunks(2) {
            let mut row = h_flex().gap_1p5();
            for &agent in pair {
                let selected = form.agent == agent;
                let id = SharedString::from(format!("provider-{}", agent.name()));
                let hover = Hover::new(id.clone(), w, cx);
                let pick = move |s: &mut Self, w: &mut Window, cx: &mut Context<Self>| {
                    if s.inspector.agent != agent {
                        s.inspector.agent = agent;
                        s.inspector.effort.clear();
                        // A mode the new CLI lacks falls back to the role's own.
                        if s.inspector
                            .mode
                            .is_some_and(|mode| !pipeline_modes(agent).contains(&mode))
                        {
                            s.inspector.mode = None;
                        }
                        s.inspector.custom_model = false;
                        s.inspector.model.update(cx, |m, cx| m.set_value("", w, cx));
                        ensure_models(agent, cx);
                        s.inspector.picker = ModelPicker::new(agent, "", &s.template, w, cx);
                        cx.notify();
                    }
                };
                use crate::ui::hover::SmoothHoverBg as _;
                let card = h_flex()
                    .id(id)
                    .role(Role::Button)
                    .tab_index(0)
                    .aria_label(agent.name())
                    .aria_toggled(if selected {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .flex_1()
                    .min_w_0()
                    .gap_1p5()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(if selected { color } else { cx.theme().border })
                    .cursor_pointer()
                    .focus_visible(move |s| s.border_color(color))
                    .child(crate::ui::agent_logo::icon(agent, cx).size_3p5())
                    .child(div().text_xs().truncate().child(agent.name()));
                row = row.child(on_activate(card, cx, pick).hover_bg(
                    &hover,
                    if selected {
                        color.alpha(0.12)
                    } else {
                        cx.theme().sidebar
                    },
                    cx.theme().accent,
                ));
            }
            provider = provider.child(row);
        }
        let agent = form.agent;
        let loaded = AppState::get(cx)
            .pipelines
            .read(cx)
            .models
            .options
            .get(&agent);
        let load_error = loaded.and_then(|r| r.as_ref().err()).cloned();
        let mut config = section(cx)
            .child(section_title(Lucide::Bot, "Agent", cx))
            .child(provider)
            .child(setting("Model", form.picker.render(), cx));
        if form.custom_model {
            config = config.child(TextField::input(Input::new(&form.model), &form.model, cx));
        }
        if let Some(error) = load_error {
            let retry_hover = Hover::new("reload-provider-models", w, cx);
            config = config.child(
                h_flex()
                    .gap_1()
                    .child(subtle("Model list unavailable", cx).flex_1())
                    .child(
                        Button::new("reload-provider-models")
                            .cursor_pointer()
                            .small()
                            .label("Retry")
                            .tooltip(error)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                AppState::get(cx)
                                    .pipelines
                                    .clone()
                                    .update(cx, |p, cx| p.load_models(agent, cx))
                            }))
                            .smooth_hover(ButtonLook::Outline, &retry_hover, cx),
                    ),
            );
        }
        let efforts = pipeline_efforts(agent)
            .iter()
            .map(|e| {
                (
                    e.to_string(),
                    if e.is_empty() {
                        "CLI default".into()
                    } else {
                        e.to_string()
                    },
                )
            })
            .collect();
        config = config.child(setting(
            "Reasoning",
            choice(
                "stage-effort",
                &form.effort,
                efforts,
                cx,
                |s: &mut Self, e, _, _| s.inspector.effort = e,
            ),
            cx,
        ));
        config.child(self.mode_setting(cx))
    }
}
