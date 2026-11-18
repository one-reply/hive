//! The library previews executable routes and the selected project's actual runs.
use super::*;
use crate::ui::hover::SmoothHoverBg as _;

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
impl PipelinePanel {
    pub(super) fn library(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let lib = &AppState::get(cx).store.read(cx).state.pipelines;
        let templates = lib.templates.clone();
        let runs = lib
            .runs
            .iter()
            .rev()
            .filter(|r| r.owner == self.project)
            .map(|r| {
                (
                    r.id,
                    r.task.clone(),
                    r.template.name.clone(),
                    phase_label(r),
                )
            })
            .collect::<Vec<_>>();
        let selected = templates
            .iter()
            .position(|t| Some(&t.name) == self.selected_template.as_ref())
            .unwrap_or(0);
        let mut actions = h_flex().gap_2().flex_wrap().child(button_with_size(
            "new-template",
            "New template",
            ButtonLook::Ghost,
            ButtonSize::Medium,
            window,
            cx,
            |p, w, cx| p.edit(None, w, cx),
        ));
        if let Some(t) = templates.get(selected) {
            let edit = t.clone();
            let launch = t.clone();
            actions = actions
                .child(button_with_size(
                    "edit-template",
                    "Edit workflow",
                    ButtonLook::Outline,
                    ButtonSize::Medium,
                    window,
                    cx,
                    move |p, w, cx| p.edit(Some(edit.clone()), w, cx),
                ))
                .child(button_with_size(
                    "launch-template",
                    "Start run",
                    ButtonLook::Primary,
                    ButtonSize::Medium,
                    window,
                    cx,
                    move |p, w, cx| p.launch(launch.clone(), w, cx),
                ));
        }
        let mut cards = h_flex().items_stretch().gap_3().flex_wrap();
        for (i, t) in templates.iter().enumerate() {
            let id = ("workflow-template", i);
            let hover = Hover::new(id, window, cx);
            let selected = selected == i;
            let name = t.name.clone();
            let pick = move |this: &mut Self, cx: &mut Context<Self>| {
                this.selected_template = Some(name.clone());
                cx.notify()
            };
            let click = pick.clone();
            let summary = format!(
                "{}   ·   {}",
                count(t.lanes.len(), "lane", "lanes"),
                count(t.reviews.len(), "review", "reviews")
            );
            let accent = cx.theme().blue;
            let bottom = v_flex()
                .px_3()
                .py_2p5()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .flex_1()
                                .truncate()
                                .child(t.name.clone()),
                        )
                        .child(
                            Icon::new(if selected {
                                Lucide::CircleCheck
                            } else {
                                Lucide::Circle
                            })
                            .size_4()
                            .text_color(if selected {
                                accent
                            } else {
                                cx.theme().border
                            }),
                        ),
                )
                .child(subtle(summary, cx));
            cards = cards.child(
                v_flex()
                    .id(id)
                    .role(Role::Button)
                    .aria_label(t.name.clone())
                    .aria_toggled(if selected {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .tab_index(0)
                    .min_w(rems(11.))
                    .max_w(rems(20.))
                    .flex_1()
                    .border_1()
                    .border_color(if selected { accent } else { cx.theme().border })
                    .rounded_lg()
                    .overflow_hidden()
                    .cursor_pointer()
                    .focus_visible(move |s| s.border_color(accent))
                    .child(map::miniature(t, cx))
                    .child(bottom)
                    .on_click(cx.listener(move |this, _, _, cx| click(this, cx)))
                    .on_key_down(cx.listener(move |this, e: &KeyDownEvent, _, cx| {
                        if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                            cx.stop_propagation();
                            pick(this, cx)
                        }
                    }))
                    .hover_bg(&hover, cx.theme().sidebar, cx.theme().accent),
            );
        }
        let mut history = v_flex().when(!runs.is_empty(), |e| {
            e.border_t_1().border_color(cx.theme().border)
        });
        for (i, (id, task, name, status)) in runs.iter().enumerate() {
            let id = *id;
            history = history.child(
                h_flex()
                    .gap_3()
                    .py_2p5()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Icon::new(Lucide::GitPullRequest)
                            .size_4()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().text_sm().child(task.clone()))
                            .child(subtle(name.clone(), cx)),
                    )
                    .child(subtle(status.clone(), cx))
                    .child(button(
                        ("open-run", i),
                        "Open",
                        ButtonLook::Ghost,
                        window,
                        cx,
                        move |p, w, cx| p.show_run(id, w, cx),
                    )),
            );
        }
        if runs.is_empty() {
            history = history.child(
                h_flex()
                    .gap_2()
                    .py_3()
                    .child(
                        Icon::new(Lucide::Clock)
                            .size_4()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(subtle("No runs in this project yet.", cx)),
            );
        }
        scroll(
            "pipeline-library",
            v_flex()
                .p_2()
                .gap_5()
                .child(
                    h_flex()
                        .gap_3()
                        .flex_wrap()
                        .justify_between()
                        .child(
                            div()
                                .text_xl()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Workflows"),
                        )
                        .child(actions),
                )
                .child(cards)
                .child(
                    v_flex()
                        .gap_2()
                        .pt_3()
                        .child(heading("Recent runs"))
                        .child(history),
                ),
        )
    }
}
