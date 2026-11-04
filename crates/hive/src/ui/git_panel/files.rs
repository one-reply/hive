//! Changed-file groups and their bulk actions.
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

impl GitPanel {
    fn group_header(
        &self,
        label: &'static str,
        count: usize,
        open: bool,
        bulk: Option<(&'static str, Action)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let busy = self.git.read(cx).is_busy();
        let header_id: SharedString = format!("group-{label}").into();
        let stage_id: SharedString = format!("stage-group-{label}").into();
        let header_hover = Hover::new(header_id.clone(), window, cx);
        let stage_hover = Hover::new(stage_id.clone(), window, cx).enabled(!busy);
        let chevron = if open {
            Lucide::ChevronDown
        } else {
            Lucide::ChevronRight
        };
        h_flex()
            .id(header_id)
            .h(GROUP_HEIGHT)
            .flex_none()
            .mt_1()
            .pl_1()
            .pr_1()
            .gap_1()
            .items_center()
            .rounded_md()
            .cursor_pointer()
            .hover_bg(
                &header_hover,
                cx.theme().foreground.opacity(0.),
                cx.theme().foreground.opacity(0.04),
            )
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
                    div().opacity(header_hover.progress()).child(
                        Button::new(stage_id)
                            .cursor_pointer()
                            .xsmall()
                            .label(text)
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                // The header behind would otherwise fold the group.
                                cx.stop_propagation();
                                this.git.update(cx, |g, cx| g.perform(action.clone(), cx))
                            }))
                            .smooth_hover(ButtonLook::Ghost, &stage_hover, cx),
                    ),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_group(label, cx)))
            .into_any_element()
    }

    pub(super) fn changes(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
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
        let mut rows = Vec::new();
        let groups: Vec<Vec<usize>> = GROUPS
            .iter()
            .enumerate()
            .map(|(group, (label, _))| {
                let indices: Vec<_> = files
                    .iter()
                    .enumerate()
                    .filter(|(_, file)| in_group(file, label))
                    .map(|(index, _)| index)
                    .collect();
                if !indices.is_empty() {
                    rows.push((group, None));
                    if !self.collapsed.contains(label) {
                        rows.extend(
                            indices
                                .iter()
                                .copied()
                                .filter(|index| files[*index].path.to_lowercase().contains(&filter))
                                .map(|index| (group, Some(index))),
                        );
                    }
                }
                indices
            })
            .collect();
        if self.change_rows != rows {
            self.changes_list.reset(rows.len());
            self.change_rows.clone_from(&rows);
        }
        let view = cx.entity().downgrade();
        list(self.changes_list.clone(), move |index, window, cx| {
            view.update(cx, |this, cx| {
                let (group, file) = rows[index];
                let (label, source) = &GROUPS[group];
                if let Some(file) = file {
                    return this.file_row(&files[file], source.clone(), index, window, cx);
                }
                let bulk = (*label != "Conflicts").then(|| {
                    let paths = groups[group]
                        .iter()
                        .flat_map(|index| {
                            let file = &files[*index];
                            std::iter::once(file.path.clone()).chain(
                                (*label == "Staged")
                                    .then_some(file.old_path.clone())
                                    .flatten(),
                            )
                        })
                        .collect();
                    if *label == "Staged" {
                        ("Unstage all", Action::Unstage(paths))
                    } else {
                        ("Stage all", Action::Stage(paths))
                    }
                });
                this.group_header(
                    label,
                    groups[group].len(),
                    !this.collapsed.contains(label),
                    bulk,
                    window,
                    cx,
                )
            })
            .unwrap_or_else(|_| div().into_any_element())
        })
        .size_full()
        .into_any_element()
    }
}
