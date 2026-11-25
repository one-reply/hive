//! Changed-file groups and their bulk actions.
use super::*;
use std::rc::Rc;

use crate::state::git_diff::catalog::{GROUPS, in_group};

impl GitPanel {
    fn discard_group_button(
        &self,
        label: &str,
        files: &[&FileChange],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (kind, icon, tooltip, ok_text) = match label {
            "Changes" => (
                Discard::Restore,
                Lucide::Undo2,
                "Discard all unstaged changes",
                "Discard",
            ),
            "Untracked" => (
                Discard::Delete,
                Lucide::Trash,
                "Delete all untracked files",
                "Delete",
            ),
            _ => return None,
        };
        let paths: Vec<_> = files
            .iter()
            .filter(|file| Discard::for_file(file) == Some(kind))
            .map(|file| file.path.clone())
            .collect();
        if paths.is_empty() {
            return None;
        }
        let target = self.git.read(cx).target.clone()?;
        let count = paths.len();
        let noun = if count == 1 { "file" } else { "files" };
        let mut description = match kind {
            Discard::Restore => format!(
                "Restore {count} {noun} from the index? Staged changes are preserved. Unstaged edits will be lost."
            ),
            Discard::Delete => {
                format!("Permanently delete {count} untracked {noun}? Git cannot recover them.")
            }
        };
        if !self.filter.read(cx).value().is_empty() {
            description.push_str(" This includes files hidden by the filter.");
        }
        let busy = self.git.read(cx).is_busy();
        let id: SharedString = format!("discard-group-{label}").into();
        let hover = Hover::new(id.clone(), window, cx).enabled(!busy);
        let button = Button::new(id)
            .cursor_pointer()
            .xsmall()
            .icon(icon)
            .tooltip(tooltip)
            .disabled(busy)
            .on_click(cx.listener(move |this, _, window, cx| {
                // The header behind would otherwise fold the group.
                cx.stop_propagation();
                git_controls::confirm_at(
                    this.git.clone(),
                    target.clone(),
                    Action::Discard {
                        kind,
                        paths: paths.clone(),
                    },
                    git_controls::Confirmation {
                        title: tooltip,
                        description: description.clone(),
                        ok_text,
                        danger: true,
                    },
                    window,
                    cx,
                );
            }))
            .smooth_hover(ButtonLook::Ghost, &hover, cx);
        Some(button.into_any_element())
    }

    fn group_header(
        &self,
        label: &'static str,
        files: Vec<&FileChange>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let busy = self.git.read(cx).is_busy();
        let header_id: SharedString = format!("group-{label}").into();
        let stage_id: SharedString = format!("stage-group-{label}").into();
        let header_hover = Hover::new(header_id.clone(), window, cx);
        let stage_hover = Hover::new(stage_id.clone(), window, cx).enabled(!busy);
        let chevron = if !self.collapsed.contains(label) {
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
            .child(count_tag(files.len()))
            .child(div().flex_1())
            .when_some(
                self.discard_group_button(label, &files, window, cx),
                |row, button| row.child(div().opacity(header_hover.progress()).child(button)),
            )
            .when(label != "Conflicts", |row| {
                let staged = label == "Staged";
                let paths = files
                    .iter()
                    .flat_map(|file| {
                        std::iter::once(file.path.clone())
                            .chain(file.old_path.iter().filter(|_| staged).cloned())
                    })
                    .collect();
                let (text, action) = if staged {
                    ("Unstage all", Action::Unstage(paths))
                } else {
                    ("Stage all", Action::Stage(paths))
                };
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
        let unreadable = unreadable_notice(status.unreadable_paths).map(|text| {
            div()
                .px_3()
                .py_1p5()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(text)
        });
        if status.files.is_empty() {
            if let Some(notice) = unreadable {
                return notice.into_any_element();
            }
            return empty_state(
                Lucide::CircleCheck,
                "Working tree clean",
                "Review commits or compare your branch with a base.",
                cx,
            )
            .into_any_element();
        }
        let files: Rc<[FileChange]> = status.files.clone().into();
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
            crate::ui::list_scroll::replace_rows(
                &self.changes_list,
                &row_keys(&self.change_rows, &self.change_files),
                &row_keys(&rows, &files),
            );
            self.change_rows.clone_from(&rows);
        }
        // The rows name files by position, so they are only ever read with this snapshot.
        self.change_files = files.clone();
        let view = cx.entity().downgrade();
        let changes = list(self.changes_list.clone(), move |index, window, cx| {
            view.update(cx, |this, cx| {
                let (group, file) = rows[index];
                let (label, source) = &GROUPS[group];
                if let Some(file) = file {
                    return this.file_row(&files[file], source.clone(), index, window, cx);
                }
                this.group_header(
                    label,
                    groups[group].iter().map(|&index| &files[index]).collect(),
                    window,
                    cx,
                )
            })
            .unwrap_or_else(|_| div().into_any_element())
        });
        match unreadable {
            None => changes.size_full().into_any_element(),
            Some(notice) => v_flex()
                .size_full()
                .child(notice)
                .child(changes.flex_1().min_h_0())
                .into_any_element(),
        }
    }
}

/// What each row shows, whatever position its file has in the status: a group's header, or
/// one path in a group. This is what tells a row that moved from one that was replaced.
fn row_keys<'a>(
    rows: &[(usize, Option<usize>)],
    files: &'a [FileChange],
) -> Vec<(usize, Option<&'a str>)> {
    rows.iter()
        .map(|&(group, file)| {
            let path = file.and_then(|index| files.get(index));
            (group, path.map(|file| file.path.as_str()))
        })
        .collect()
}

/// What the list says about changed files whose names Git returned in another encoding.
fn unreadable_notice(count: usize) -> Option<String> {
    match count {
        0 => None,
        1 => Some("1 changed file has a name that is not UTF-8. It is not listed here; use the terminal for it.".into()),
        count => Some(format!(
            "{count} changed files have names that are not UTF-8. They are not listed here; use the terminal for them."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::unreadable_notice;

    #[test]
    fn the_list_says_how_many_changed_files_it_cannot_name() {
        assert_eq!(unreadable_notice(0), None);
        assert!(
            unreadable_notice(1)
                .unwrap()
                .starts_with("1 changed file has a name")
        );
        assert!(
            unreadable_notice(3)
                .unwrap()
                .starts_with("3 changed files have names")
        );
    }
}
