//! The library previews executable routes and the selected project's actual runs.
use super::*;
use crate::state::pipelines::RunStatus;
use crate::ui::hover::{SmoothHoverBg as _, WIDE_HOVER_DURATION};
use gpui_kit::component::{
    button::{ButtonVariant, ButtonVariants as _},
    menu::{ContextMenuExt as _, PopupMenu},
};

mod runs;
use runs::{RunRow, run_detail, run_row};

/// Strength of the selection accent a hovered, unselected template card's border eases to.
const HOVER_BORDER_OPACITY: f32 = 0.6;

impl PipelinePanel {
    pub(super) fn library(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let state = AppState::get(cx);
        let pipelines = state.pipelines.read(cx);
        let lib = &state.store.read(cx).state.pipelines;
        let templates = lib.templates.clone();
        let now = hive_core::pipeline::now_ms();
        let mut runs = lib
            .runs
            .iter()
            .rev()
            .filter(|r| r.owner() == self.project)
            .map(|r| {
                let status = pipelines.status(r, cx);
                let finished = matches!(status, RunStatus::Completed | RunStatus::Cancelled);
                // A finished run's phase repeats its status, and a live one's length is still
                // growing, so each row shows only the one that says something.
                let detail = run_detail(
                    &r.template().name,
                    (!finished).then(|| phase_label(r)).as_deref(),
                    r.age(now).as_deref(),
                    finished.then(|| run::length(r, now)).flatten().as_deref(),
                );
                RunRow {
                    id: r.id(),
                    task: r.task().to_owned(),
                    detail,
                    status,
                    finished,
                }
            })
            .collect::<Vec<_>>();
        let draft_template = self.draft_template(cx);
        let outside_attention = pipelines
            .attention(cx)
            .into_iter()
            .filter(|(_, owner, _)| owner != &self.project)
            .collect::<Vec<_>>();
        let selected = templates
            .iter()
            .position(|t| Some(&t.name) == self.selected_template.as_ref())
            .unwrap_or(0);
        let mut actions = h_flex().gap_2().flex_wrap().child(button_with_size(
            "new-template",
            "New template",
            ButtonLook::Outline,
            ButtonSize::Medium,
            window,
            cx,
            |p, w, cx| p.edit(None, w, cx),
        ));
        let has_draft = draft_template.is_some();
        if let Some(launch) = draft_template.or_else(|| templates.get(selected).cloned()) {
            actions = actions.child(button_with_size(
                "launch-template",
                if has_draft {
                    "Resume draft"
                } else {
                    "Start run"
                },
                ButtonLook::Primary,
                ButtonSize::Medium,
                window,
                cx,
                move |p, w, cx| p.launch(launch.clone(), w, cx),
            ));
        }
        let history_is_empty = runs.is_empty();
        let mut cards = h_flex().items_stretch().gap_3().flex_wrap();
        for (i, t) in templates.iter().enumerate() {
            let id = SharedString::from(format!("workflow-template-{}", t.name));
            let hover = Hover::new(id.clone(), window, cx);
            let selected = selected == i;
            let name = t.name.clone();
            let pick = move |this: &mut Self, _: &mut Window, cx: &mut Context<Self>| {
                this.selected_template = Some(name.clone());
                cx.notify()
            };
            let summary = format!(
                "{}   ·   {}   ·   2 decisions",
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
            let card = v_flex()
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
                .bg(cx.theme().sidebar)
                .border_color(if selected {
                    accent
                } else {
                    hover.mix(cx.theme().border, accent.opacity(HOVER_BORDER_OPACITY))
                })
                .on_hover(hover.listener())
                .rounded_lg()
                .overflow_hidden()
                .cursor_pointer()
                .focus_visible(move |s| s.border_color(accent))
                .child(map::miniature(t, cx))
                .child(bottom);
            cards = cards.child(on_activate(card, cx, pick).context_menu(menu));
        }
        if templates.is_empty() {
            cards = cards.child(subtle("Create a template to start a run.", cx));
        }
        let mut history = v_flex().gap_1();
        // The sort is stable, so each section keeps the newest run first.
        runs.sort_by_key(|r| history_section(r.status).0);
        let mut previous = None;
        for (i, row) in runs.into_iter().enumerate() {
            let (_, section) = history_section(row.status);
            if previous != Some(section) {
                history = history.child(heading(section));
                previous = Some(section);
            }
            history = history.child(run_row(i, row, window, cx));
        }
        if !outside_attention.is_empty() {
            history = history.child(heading("Needs you in other projects"));
            for (id, owner, task) in outside_attention {
                history = history.child(button(
                    ("other-project-run", id as usize),
                    format!("{} · {}", path_name(&owner), task),
                    ButtonLook::Ghost,
                    window,
                    cx,
                    move |_, _, cx| {
                        AppState::get(cx)
                            .pipelines
                            .clone()
                            .update(cx, |p, cx| p.reveal(id, cx))
                    },
                ));
            }
        }
        if history_is_empty {
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
                .child(v_flex().gap_2().pt_3().child(history)),
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

/// Where a run is listed in the history, in display order: what waits for the user comes
/// before what is working, and finished runs come last.
fn history_section(status: RunStatus) -> (u8, &'static str) {
    match status {
        RunStatus::NeedsYou | RunStatus::Failed => (0, "Needs you"),
        RunStatus::Running => (1, "Running"),
        RunStatus::Completed | RunStatus::Cancelled => (2, "Recent"),
    }
}

#[cfg(test)]
mod tests {
    use super::{RunStatus, history_section};

    #[test]
    fn runs_that_wait_for_the_user_are_listed_before_working_and_finished_ones() {
        use RunStatus::*;
        let mut runs = [Completed, Running, Failed, Cancelled, NeedsYou];
        runs.sort_by_key(|status| history_section(*status).0);
        assert_eq!(runs, [Failed, NeedsYou, Running, Completed, Cancelled]);
        assert_eq!(history_section(Failed).1, history_section(NeedsYou).1);
        assert_eq!(history_section(Completed).1, history_section(Cancelled).1);
    }
}
