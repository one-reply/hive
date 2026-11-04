//! Changed-file groups and the file rows shared with commit review.
use super::*;

/// Groups of the Changes section, in display order.
const GROUPS: [(&str, DiffSource); 4] = [
    ("Conflicts", DiffSource::WorkingTree),
    ("Staged", DiffSource::Staged),
    ("Changes", DiffSource::WorkingTree),
    ("Untracked", DiffSource::Untracked),
];

fn in_group(file: &FileChange, group: &str) -> bool {
    match group {
        "Conflicts" => file.staged == Some(Change::Conflicted),
        "Staged" => file.staged.is_some_and(|c| c != Change::Conflicted),
        "Changes" => file
            .unstaged
            .is_some_and(|c| !matches!(c, Change::Untracked | Change::Conflicted)),
        _ => file.unstaged == Some(Change::Untracked),
    }
}

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
        .child(div().flex_none().child(name))
}

impl GitPanel {
    pub(super) fn file_row(
        &self,
        file: &FileChange,
        source: DiffSource,
        index: usize,
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
        let conflicted = change == Some(Change::Conflicted);
        let busy = self.git.read(cx).is_busy();
        let fg = cx.theme().foreground;
        let group: SharedString = format!("file-row-{index}").into();
        let tooltip: SharedString = file
            .old_path
            .as_ref()
            .map(|old| format!("{old} → {}", file.path))
            .unwrap_or_else(|| file.path.clone())
            .into();

        let mut actions = h_flex().flex_none().gap_0p5();
        if mutable && !conflicted {
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
                    .ghost()
                    .xsmall()
                    .icon(if staged { Lucide::Minus } else { Lucide::Plus })
                    .tooltip(if staged { "Unstage file" } else { "Stage file" })
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // The row behind would otherwise open the diff too.
                        cx.stop_propagation();
                        this.git.update(cx, |g, cx| g.perform(action.clone(), cx))
                    })),
            );
        }
        if source == DiffSource::WorkingTree && !conflicted && !file.submodule {
            let path = file.path.clone();
            actions = actions.child(
                Button::new(("file-discard", index))
                    .cursor_pointer()
                    .ghost()
                    .xsmall()
                    .icon(Lucide::Undo2)
                    .tooltip("Discard unstaged changes")
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
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
                    })),
            );
        }
        // Hidden until the row is hovered, as the project header buttons in the sidebar.
        let actions = h_flex()
            .w_0()
            .overflow_hidden()
            .opacity(0.)
            .group_hover(group.clone(), |s| s.w_auto().opacity(1.))
            .child(actions);

        let (selected, open_source) = (file.clone(), source);
        h_flex()
            .id(("file-open", index))
            .group(group)
            .h(ROW_HEIGHT)
            .flex_none()
            .gap_2()
            .items_center()
            .pl_2()
            .pr_1()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(fg.opacity(0.05)))
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            .child(status_badge(change, cx))
            .child(path_label(&file.path, cx))
            .child(actions)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.git
                    .update(cx, |g, cx| g.open_diff(&selected, open_source.clone(), cx))
            }))
            .into_any_element()
    }

    fn group_header(
        &self,
        label: &'static str,
        count: usize,
        open: bool,
        bulk: Option<(&'static str, Action)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let busy = self.git.read(cx).is_busy();
        let group: SharedString = format!("group-{label}").into();
        let chevron = if open {
            Lucide::ChevronDown
        } else {
            Lucide::ChevronRight
        };
        h_flex()
            .id(SharedString::from(format!("group-{label}")))
            .group(group.clone())
            .h(GROUP_HEIGHT)
            .flex_none()
            .mt_1()
            .pl_1()
            .pr_1()
            .gap_1()
            .items_center()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(cx.theme().foreground.opacity(0.04)))
            .child(Icon::new(chevron).size_3().text_color(muted))
            .child(
                div()
                    .text_size(META_TEXT)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child(label),
            )
            .child(count_tag(count))
            .child(div().flex_1())
            .when_some(bulk, |row, (text, action)| {
                row.child(
                    div()
                        .opacity(0.)
                        .group_hover(group, |s| s.opacity(1.))
                        .child(
                            Button::new(SharedString::from(format!("stage-group-{label}")))
                                .cursor_pointer()
                                .ghost()
                                .xsmall()
                                .label(text)
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    // The header behind would otherwise fold the group.
                                    cx.stop_propagation();
                                    this.git.update(cx, |g, cx| g.perform(action.clone(), cx))
                                })),
                        ),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_group(label, cx)))
            .into_any_element()
    }

    pub(super) fn changes(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = self.git.read(cx);
        let Some(status) = &state.status else {
            return if state.loading {
                loading_row("Loading changes…", cx).into_any_element()
            } else {
                div().into_any_element()
            };
        };
        if status.files.is_empty() {
            return empty_state(
                Lucide::CircleCheck,
                "Working tree clean",
                "Review commits or compare your branch with a base.",
                cx,
            )
            .into_any_element();
        }
        let files = status.files.clone();
        let filter = self.filter.read(cx).value().to_lowercase();
        let mut groups = Vec::new();
        for (group_ix, (label, source)) in GROUPS.iter().enumerate() {
            let group_files: Vec<_> = files.iter().filter(|f| in_group(f, label)).collect();
            if group_files.is_empty() {
                continue;
            }
            let bulk = (*label != "Conflicts").then(|| {
                let paths = group_files
                    .iter()
                    .flat_map(|f| {
                        let mut paths = vec![f.path.clone()];
                        if *label == "Staged"
                            && let Some(old) = &f.old_path
                        {
                            paths.push(old.clone());
                        }
                        paths
                    })
                    .collect();
                if *label == "Staged" {
                    ("Unstage all", Action::Unstage(paths))
                } else {
                    ("Stage all", Action::Stage(paths))
                }
            });
            let open = !self.collapsed.contains(label);
            let header = self.group_header(label, group_files.len(), open, bulk, cx);
            let rows = if open {
                group_files
                    .into_iter()
                    .filter(|f| f.path.to_lowercase().contains(&filter))
                    .enumerate()
                    .map(|(i, f)| {
                        self.file_row(f, source.clone(), group_ix * (files.len() + 1) + i, cx)
                    })
                    .collect()
            } else {
                vec![]
            };
            groups.push(v_flex().child(header).children(rows).into_any_element());
        }
        v_flex().children(groups).into_any_element()
    }
}
