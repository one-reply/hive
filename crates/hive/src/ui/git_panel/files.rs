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

    pub(super) fn changes(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
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
                        self.file_row(
                            f,
                            source.clone(),
                            group_ix * (files.len() + 1) + i,
                            window,
                            cx,
                        )
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
