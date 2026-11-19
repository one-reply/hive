//! The library previews executable routes and the selected project's actual runs.
use super::*;
use crate::state::pipelines::RunStatus;
use crate::ui::hover::{SmoothHoverBg as _, WIDE_HOVER_DURATION};
use gpui_kit::component::{
    button::{ButtonVariant, ButtonVariants as _},
    menu::{ContextMenuExt as _, PopupMenu},
};

/// Strength of the selection accent a hovered, unselected template card's border eases to.
const HOVER_BORDER_OPACITY: f32 = 0.6;

impl PipelinePanel {
    pub(super) fn library(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let state = AppState::get(cx);
        let pipelines = state.pipelines.read(cx);
        let lib = &state.store.read(cx).state.pipelines;
        let templates = lib.templates.clone();
        let runs = lib
            .runs
            .iter()
            .rev()
            .filter(|r| r.owner() == self.project)
            .map(|r| RunRow {
                id: r.id(),
                task: r.task().to_owned(),
                template: r.template().name.clone(),
                phase: phase_label(r),
                status: pipelines.status(r, cx),
            })
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
        for (i, row) in runs.into_iter().enumerate() {
            history = history.child(run_row(i, row, window, cx));
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

/// What the run list shows of one run, read out of the store before rendering.
struct RunRow {
    id: u64,
    task: String,
    template: String,
    phase: String,
    status: RunStatus,
}

fn run_row(
    ix: usize,
    row: RunRow,
    window: &mut Window,
    cx: &mut Context<PipelinePanel>,
) -> AnyElement {
    let hover = Hover::with_duration(("recent-run", ix), WIDE_HOVER_DURATION, window, cx);
    let color = status_color(row.status, cx);
    let focus = cx.theme().ring;
    let id = row.id;
    // A finished run's phase repeats its status, so only a live one names the stage it is in.
    let detail = match row.status {
        RunStatus::Completed | RunStatus::Cancelled => row.template,
        _ => format!("{}  ·  {}", row.template, row.phase),
    };
    let label = format!("{}, {}", row.task, row.status.label());
    let row_element = h_flex()
        .id(("recent-run", ix))
        .role(Role::Button)
        .aria_label(label)
        .tab_index(0)
        .gap_3()
        .px_3()
        .py_2p5()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().border)
        .cursor_pointer()
        .focus_visible(move |s| s.border_color(focus))
        .child(
            h_flex()
                .size_8()
                .flex_none()
                .justify_center()
                .rounded_md()
                .bg(color.opacity(STATUS_TINT_OPACITY))
                .child(
                    Icon::new(status_icon(row.status))
                        .size_4()
                        .text_color(color),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(row.task),
                )
                .child(subtle(detail, cx).truncate()),
        )
        .child(status_pill(row.status.label(), color))
        .child(
            Icon::new(Lucide::ChevronRight)
                .size_4()
                .text_color(cx.theme().muted_foreground),
        );
    on_activate(row_element, cx, move |p, w, cx| p.show_run(id, w, cx))
        .hover_bg(&hover, cx.theme().background, cx.theme().muted)
        .into_any_element()
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
