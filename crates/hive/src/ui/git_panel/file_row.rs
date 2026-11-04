//! File rows with a stable, animated trailing action slot.
use super::*;
use gpui_kit::base::motion::{Transition, transition};
use std::time::Duration;

const ACTION_TRANSITION_DURATION: Duration = Duration::from_millis(160);
const ACTION_WIDTH: Pixels = px(28.);
const ACTION_BACKGROUND_OPACITY: f32 = 0.08;

/// Letter and color of a change, as Git's own short status.
fn status_badge(change: Option<Change>, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (code, color) = match change {
        Some(Change::Added | Change::Untracked) => ("A", theme.green),
        Some(Change::Deleted) => ("D", theme.red),
        Some(Change::Renamed) => ("R", theme.blue),
        Some(Change::Copied) => ("C", theme.blue),
        Some(Change::TypeChanged) => ("T", theme.muted_foreground),
        Some(Change::Conflicted) => ("!", theme.danger),
        _ => ("M", theme.yellow),
    };
    div()
        .size(BADGE_SIZE)
        .flex_none()
        .rounded_sm()
        .bg(color.opacity(0.15))
        .text_color(color)
        .text_size(META_TEXT)
        .font_weight(FontWeight::SEMIBOLD)
        .font_family(theme.mono_font_family.clone())
        .flex()
        .items_center()
        .justify_center()
        .child(code)
}

/// Directory in muted text, file name in full color.
fn path_label(path: &str, cx: &App) -> impl IntoElement {
    let (dir, name) = match path.rsplit_once('/') {
        Some((dir, name)) => (Some(format!("{dir}/")), name.to_string()),
        None => (None, path.to_string()),
    };
    h_flex()
        .min_w_0()
        .flex_1()
        .overflow_hidden()
        .text_size(META_TEXT + px(1.))
        .when_some(dir, |row, dir| {
            row.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(cx.theme().muted_foreground)
                    .child(dir),
            )
        })
        .child(div().flex_none().max_w_full().truncate().child(name))
}

impl GitPanel {
    pub(super) fn file_row(
        &self,
        file: &FileChange,
        source: DiffSource,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let staged = source == DiffSource::Staged;
        let mutable = matches!(
            source,
            DiffSource::WorkingTree | DiffSource::Staged | DiffSource::Untracked
        );
        let change = if staged || !mutable {
            file.staged
        } else {
            file.unstaged
        };
        let stats = if staged || !mutable {
            file.staged_stats
        } else {
            file.unstaged_stats
        };
        let conflicted = change == Some(Change::Conflicted);
        let busy = self.git.read(cx).is_busy();
        let fg = cx.theme().foreground;
        let can_act = mutable && !conflicted;
        let can_discard = source == DiffSource::WorkingTree && !conflicted && !file.submodule;
        let row_id: SharedString = format!(
            "file-row:{:?}:{source:?}:{}",
            self.git.read(cx).target,
            file.path
        )
        .into();
        let row_key = ElementId::from(row_id.clone());
        let row_hover = Hover::new(row_key.clone(), window, cx);
        let discard_hover = Hover::new((row_key.clone(), "discard"), window, cx).enabled(!busy);
        let stage_hover = Hover::new((row_key.clone(), "stage"), window, cx).enabled(!busy);
        let focus = window
            .use_keyed_state((row_key, "focus"), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let show_actions =
            can_act && (row_hover.is_hovered(cx) || focus.contains_focused(window, cx));
        let progress = transition(
            (row_id.clone(), "actions"),
            if show_actions { 1.0_f32 } else { 0. },
            Transition::new(ACTION_TRANSITION_DURATION).ease(ease_in_out),
            window,
            cx,
        );
        let tooltip: SharedString = file
            .old_path
            .as_ref()
            .map(|old| format!("{old} → {}", file.path))
            .unwrap_or_else(|| file.path.clone())
            .into();

        let mut actions = h_flex().justify_end().items_center();
        if can_discard {
            let path = file.path.clone();
            actions = actions.child(
                Button::new(("file-discard", index))
                    .cursor_pointer()
                    .xsmall()
                    .w(ACTION_WIDTH)
                    .h(ROW_HEIGHT)
                    .rounded_none()
                    .icon(Lucide::Undo2)
                    .tooltip("Discard unstaged changes")
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !show_actions {
                            return;
                        }
                        cx.stop_propagation();
                        git_controls::confirm(
                            this.git.clone(),
                            Action::Discard(path.clone()),
                            git_controls::Confirmation {
                                title: "Discard Changes",
                                description: format!(
                                    "{path} will be restored from its staged version. Unstaged edits are lost and cannot be recovered."
                                ),
                                ok_text: "Discard",
                                danger: true,
                            },
                            window,
                            cx,
                        )
                    }))
                    .smooth_hover(ButtonLook::Ghost, &discard_hover, cx),
            );
        }
        if can_act {
            let mut paths = vec![file.path.clone()];
            if staged && let Some(old) = &file.old_path {
                paths.push(old.clone());
            }
            let action = if staged {
                Action::Unstage(paths)
            } else {
                Action::Stage(paths)
            };
            actions = actions.child(
                Button::new(("file-stage", index))
                    .cursor_pointer()
                    .xsmall()
                    .w(ACTION_WIDTH)
                    .h(ROW_HEIGHT)
                    .rounded_none()
                    .icon(if staged { Lucide::Minus } else { Lucide::Plus })
                    .tooltip(if staged { "Unstage file" } else { "Stage file" })
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !show_actions {
                            return;
                        }
                        // The row behind would otherwise open the diff too.
                        cx.stop_propagation();
                        this.git.update(cx, |g, cx| g.perform(action.clone(), cx))
                    }))
                    .smooth_hover(ButtonLook::Ghost, &stage_hover, cx),
            );
        }
        let (selected, open_source) = (file.clone(), source);
        h_flex()
            .id(row_id)
            .h(ROW_HEIGHT)
            .flex_none()
            .gap_2()
            .items_center()
            .pl_2()
            .pr_1()
            .rounded_md()
            .cursor_pointer()
            .hover_bg(&row_hover, fg.opacity(0.), fg.opacity(0.05))
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            .child(path_label(&file.path, cx))
            .child(
                h_flex()
                    .relative()
                    .h_full()
                    .flex_none()
                    .track_focus(&focus)
                    .tab_index(0)
                    .tab_stop(false)
                    .when(can_act, |slot| {
                        slot.min_w(ACTION_WIDTH * if can_discard { 2. } else { 1. })
                    })
                    .child(
                        h_flex()
                            .h_full()
                            .flex_1()
                            .justify_end()
                            .gap_2()
                            .opacity(1. - progress)
                            .when_some(stats, |row, stats| {
                                row.child(
                                    h_flex()
                                        .flex_none()
                                        .gap_1()
                                        .text_size(META_TEXT)
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .child(
                                            div()
                                                .text_color(cx.theme().green)
                                                .child(format!("+{}", stats.added)),
                                        )
                                        .child(
                                            div()
                                                .text_color(cx.theme().red)
                                                .child(format!("−{}", stats.removed)),
                                        ),
                                )
                            })
                            .child(status_badge(change, cx)),
                    )
                    .when(can_act, |slot| {
                        slot.child(
                            actions
                                .absolute()
                                .inset_0()
                                .rounded_md()
                                .bg(fg.opacity(ACTION_BACKGROUND_OPACITY))
                                .opacity(progress),
                        )
                    }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.git
                    .update(cx, |g, cx| g.open_diff(&selected, open_source.clone(), cx))
            }))
            .into_any_element()
    }
}
