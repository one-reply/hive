//! The library previews executable routes and the selected project's actual runs.
use super::*;
use crate::ui::hover::SmoothHoverBg as _;
use gpui_kit::component::{
    button::{ButtonVariant, ButtonVariants as _},
    menu::{ContextMenuExt as _, PopupMenu},
};

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
            .filter(|r| r.owner() == self.project)
            .map(|r| {
                (
                    r.id(),
                    r.task().to_owned(),
                    r.template().name.clone(),
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
            let launch = t.clone();
            actions = actions.child(button_with_size(
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
            let id = SharedString::from(format!("workflow-template-{}", t.name));
            let hover = Hover::new(id.clone(), window, cx);
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
            let panel = cx.entity();
            let template = t.clone();
            let menu = move |menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
                template_menu(menu, &panel, &template)
            };
            let more = Button::new(("workflow-menu", i))
                .cursor_pointer()
                .ghost()
                .xsmall()
                .icon(Lucide::Ellipsis)
                .accessibility_label(format!("Actions for {}", t.name))
                .tooltip("Workflow actions")
                .dropdown_menu(menu.clone());
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
                .child(
                    h_flex()
                        .justify_between()
                        .child(subtle(summary, cx))
                        .child(more),
                );
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
                    .hover_bg(&hover, cx.theme().sidebar, cx.theme().accent)
                    .context_menu(menu),
            );
        }
        if templates.is_empty() {
            cards = cards.child(subtle("Create a template to start a run.", cx));
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

fn template_menu(menu: PopupMenu, panel: &Entity<PipelinePanel>, template: &Template) -> PopupMenu {
    let duplicate_panel = panel.clone();
    let source = template.clone();
    let panel = panel.clone();
    let edit = template.clone();
    let delete = template.clone();
    menu.item(
        PopupMenuItem::new("Edit workflow")
            .icon(Lucide::Pencil)
            .on_click(move |_, window, cx| {
                panel.update(cx, |p, cx| p.edit(Some(edit.clone()), window, cx));
            }),
    )
    .item(
        PopupMenuItem::new("Duplicate template")
            .icon(Lucide::Copy)
            .on_click(move |_, window, cx| {
                let pipelines = AppState::get(cx).pipelines.clone();
                match pipelines.update(cx, |p, cx| p.duplicate_template(&source, cx)) {
                    Ok(copy) => duplicate_panel.update(cx, |p, cx| {
                        p.selected_template = Some(copy.name.clone());
                        p.edit(Some(copy), window, cx);
                    }),
                    Err(error) => window.push_notification(Notification::error(error), cx),
                }
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("Delete template…")
            .icon(Lucide::Trash)
            .on_click(move |_, window, cx| confirm_delete(&delete, window, cx)),
    )
}

fn confirm_delete(template: &Template, window: &mut Window, cx: &mut App) {
    let template = template.clone();
    window.open_alert_dialog(cx, move |alert, _, _| {
        let template = template.clone();
        alert
            .confirm()
            .title(format!("Delete “{}”?", template.name))
            .description(
                "This removes the template from all projects. Existing runs keep their workflow.",
            )
            .ok_text("Delete")
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, window, cx| {
                let pipelines = AppState::get(cx).pipelines.clone();
                if let Err(error) = pipelines.update(cx, |p, cx| p.delete_template(&template, cx)) {
                    window.push_notification(Notification::error(error), cx);
                }
                true
            })
    });
}
