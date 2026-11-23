//! The project's runs as the library lists them, and what can be done with one from there.
use super::*;

/// What the run list shows of one run, read out of the store before rendering.
pub(super) struct RunRow {
    pub(super) id: u64,
    pub(super) task: String,
    pub(super) detail: String,
    pub(super) status: RunStatus,
    pub(super) finished: bool,
}

/// The second line of a run row: its recipe, the stage a live run is in, when it started and
/// how long a finished one took. Parts that are unknown are left out.
pub(super) fn run_detail(
    template: &str,
    phase: Option<&str>,
    age: Option<&str>,
    length: Option<&str>,
) -> String {
    [Some(template), phase, age, length]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("  ·  ")
}

pub(super) fn run_row(
    row: RunRow,
    window: &mut Window,
    cx: &mut Context<PipelinePanel>,
) -> AnyElement {
    // Keyed by the run, not by its place: the list draws only the rows in view.
    let hover = Hover::with_duration(("recent-run", row.id), WIDE_HOVER_DURATION, window, cx);
    let color = status_color(row.status, cx);
    let focus = cx.theme().ring;
    let id = row.id;
    let label = format!("{}, {}", row.task, row.status.label());
    let panel = cx.entity();
    let (finished, task) = (row.finished, row.task.clone());
    let menu = move |menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
        run_menu(menu, &panel, id, finished, &task)
    };
    let more = Button::new(("recent-run-menu", id))
        .cursor_pointer()
        .ghost()
        .xsmall()
        .icon(Lucide::Ellipsis)
        .accessibility_label(format!("Actions for {}", row.task))
        .tooltip("Run actions")
        .dropdown_menu(menu.clone());
    let row_element = h_flex()
        .id(("recent-run", id))
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
                .child(subtle(row.detail, cx).truncate()),
        )
        .child(status_pill(row.status.label(), color))
        .child(more)
        .child(
            Icon::new(Lucide::ChevronRight)
                .size_4()
                .text_color(cx.theme().muted_foreground),
        );
    on_activate(row_element, cx, move |p, w, cx| p.show_run(id, None, w, cx))
        .hover_bg(&hover, cx.theme().background, cx.theme().muted)
        .context_menu(menu)
        .into_any_element()
}

fn run_menu(
    menu: PopupMenu,
    panel: &Entity<PipelinePanel>,
    id: u64,
    finished: bool,
    task: &str,
) -> PopupMenu {
    let panel = panel.clone();
    let menu = menu.item(
        PopupMenuItem::new("Run again")
            .icon(Lucide::RotateCcw)
            .on_click(move |_, window, cx| {
                panel.update(cx, |p, cx| p.launch_again(id, window, cx));
            }),
    );
    // A live run is stopped from its own screen first; deleting it here would orphan its agents.
    if !finished {
        return menu;
    }
    let task = task.to_owned();
    menu.separator().item(
        PopupMenuItem::new("Delete run…")
            .icon(Lucide::Trash)
            .on_click(move |_, window, cx| confirm_delete_run(id, &task, window, cx)),
    )
}

fn confirm_delete_run(id: u64, task: &str, window: &mut Window, cx: &mut App) {
    let title = format!("Delete “{}”?", run::task_title(task));
    window.open_alert_dialog(cx, move |alert, _, _| {
        let alert = alert
            .confirm()
            .title(title.clone())
            .description(
                "This removes the run and its reports from Hive. Its worktrees, branches and changes stay on disk.",
            );
        crate::ui::destructive::confirmation(alert, "Delete", move |window, cx| {
            let pipelines = AppState::get(cx).pipelines.clone();
            if let Err(error) = pipelines.update(cx, |p, cx| p.delete_run(id, cx)) {
                window.push_notification(Notification::error(error), cx);
            }
            true
        })
    });
}

#[cfg(test)]
mod tests {
    // Not a glob import: the parent's `gpui_kit::*` brings in GPUI's `test` attribute.
    use super::run_detail;

    #[test]
    fn a_run_row_lists_only_what_is_known_about_the_run() {
        assert_eq!(
            run_detail("Quick fix", Some("QA"), Some("5m ago"), None),
            "Quick fix  ·  QA  ·  5m ago"
        );
        assert_eq!(
            run_detail("Quick fix", None, Some("2d ago"), Some("12m 4s")),
            "Quick fix  ·  2d ago  ·  12m 4s"
        );
        // A run saved before stage timings existed has neither a start nor a length.
        assert_eq!(run_detail("Quick fix", None, None, None), "Quick fix");
    }
}
