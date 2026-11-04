//! Changed-file groups and commit review lists.
use super::*;

impl GitPanel {
    fn file_row(
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
        let code = match change {
            Some(Change::Added | Change::Untracked) => "A",
            Some(Change::Deleted) => "D",
            Some(Change::Renamed) => "R",
            Some(Change::Copied) => "C",
            Some(Change::TypeChanged) => "T",
            Some(Change::Conflicted) => "!",
            _ => "M",
        };
        let selected = file.clone();
        let open_source = source.clone();
        let busy = self.git.read(cx).is_busy();
        let mut row = h_flex()
            .gap_1()
            .items_center()
            .min_w_0()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(code),
            )
            .child(
                Button::new(("file-open", index))
                    .cursor_pointer()
                    .ghost()
                    .small()
                    .flex_1()
                    .min_w_0()
                    .label(file.path.clone())
                    .tooltip(
                        file.old_path
                            .as_ref()
                            .map(|old| format!("{old} → {}", file.path))
                            .unwrap_or_else(|| file.path.clone()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.git
                            .update(cx, |g, cx| g.open_diff(&selected, open_source.clone(), cx))
                    })),
            );
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
            row = row.child(
                Button::new(("file-stage", index))
                    .cursor_pointer()
                    .ghost()
                    .small()
                    .label(if staged { "−" } else { "+" })
                    .tooltip(if staged { "Unstage file" } else { "Stage file" })
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.git.update(cx, |g, cx| g.perform(action.clone(), cx))
                    })),
            );
        }
        if source == DiffSource::WorkingTree && !conflicted && !file.submodule {
            let path = file.path.clone();
            row = row.child(Button::new(("file-discard", index)).cursor_pointer().ghost().small().label("↶").tooltip("Discard unstaged changes").disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| git_controls::confirm(this.git.clone(), Action::Discard(path.clone()), "Discard changes", format!("Restore {path} from its staged version? Unstaged edits will be lost."), window, cx))));
        }
        row.into_any_element()
    }

    pub(super) fn changes(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = self.git.read(cx);
        let Some(status) = &state.status else {
            return div()
                .p_3()
                .child(if state.loading {
                    "Loading changes…"
                } else {
                    "Select a Git worktree."
                })
                .into_any_element();
        };
        let files = status.files.clone();
        let busy = state.is_busy();
        let filter = self.filter.read(cx).value().to_lowercase();
        let mut groups = Vec::new();
        for (group, label, source) in [
            (0, "Conflicts", DiffSource::WorkingTree),
            (1, "Staged", DiffSource::Staged),
            (2, "Changes", DiffSource::WorkingTree),
            (3, "Untracked", DiffSource::Untracked),
        ] {
            let group_files: Vec<_> = files
                .iter()
                .filter(|f| match group {
                    0 => f.staged == Some(Change::Conflicted),
                    1 => f.staged.is_some() && f.staged != Some(Change::Conflicted),
                    2 => {
                        f.unstaged.is_some()
                            && !matches!(f.unstaged, Some(Change::Untracked | Change::Conflicted))
                    }
                    _ => f.unstaged == Some(Change::Untracked),
                })
                .collect();
            if group_files.is_empty() {
                continue;
            }
            let mut header = h_flex().justify_between().items_center().mt_2().child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{label} · {}", group_files.len())),
            );
            if group != 0 {
                let paths = group_files
                    .iter()
                    .flat_map(|f| {
                        let mut paths = vec![f.path.clone()];
                        if group == 1
                            && let Some(old) = &f.old_path
                        {
                            paths.push(old.clone());
                        }
                        paths
                    })
                    .collect();
                let action = if group == 1 {
                    Action::Unstage(paths)
                } else {
                    Action::Stage(paths)
                };
                header = header.child(
                    Button::new(("stage-group", group))
                        .cursor_pointer()
                        .ghost()
                        .small()
                        .label(if group == 1 {
                            "Unstage all"
                        } else {
                            "Stage all"
                        })
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.git.update(cx, |g, cx| g.perform(action.clone(), cx))
                        })),
                );
            }
            let rows = group_files
                .into_iter()
                .filter(|f| f.path.to_lowercase().contains(&filter))
                .enumerate()
                .map(|(i, f)| self.file_row(f, source.clone(), group * (files.len() + 1) + i, cx))
                .collect::<Vec<_>>();
            groups.push(v_flex().child(header).children(rows).into_any_element());
        }
        if files.is_empty() {
            groups.push(
                v_flex()
                    .p_3()
                    .gap_2()
                    .child("Working tree clean")
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Review commits or compare your branch with a base."),
                    )
                    .into_any_element(),
            );
        }
        v_flex().children(groups).into_any_element()
    }

    pub(super) fn review(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = self.git.read(cx);
        let section = state.section;
        let loading = state.review_loading;
        let commits = state.commits.clone();
        let files = state.files.clone();
        let selected = state.selected_commit.clone();
        let base = state.base.clone();
        let more = state.history_more;
        let no_base = base.is_none();
        let review_failed = state.error.is_some();
        let filter = self.filter.read(cx).value().to_lowercase();
        let mut content = v_flex().gap_1();
        if section == Section::Compare {
            content = content
                .child(
                    Button::new("compare-base")
                        .cursor_pointer()
                        .small()
                        .label(format!(
                            "Base: {}",
                            base.as_deref().unwrap_or("Choose branch…")
                        ))
                        .on_click(cx.listener(|this, _, window, cx| {
                            git_controls::branches(this.git.clone(), true, window, cx)
                        })),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Committed changes since the common ancestor"),
                );
        } else if selected.is_some() {
            content = content.child(
                Button::new("history-back")
                    .cursor_pointer()
                    .ghost()
                    .small()
                    .label("← Commits · diff against first parent")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.git
                            .update(cx, |g, cx| g.set_section(Section::History, cx))
                    })),
            );
        } else {
            for (i, commit) in commits.into_iter().enumerate() {
                let oid = commit.oid.clone();
                content = content.child(
                    v_flex()
                        .gap_1()
                        .py_1()
                        .child(
                            Button::new(("commit", i))
                                .cursor_pointer()
                                .ghost()
                                .small()
                                .label(commit.subject)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.git
                                        .update(cx, |g, cx| g.select_commit(oid.clone(), cx))
                                })),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{} · {} · {}",
                                    &commit.oid[..commit.oid.len().min(7)],
                                    commit.author,
                                    commit.date
                                )),
                        ),
                );
            }
            if more {
                content = content.child(
                    Button::new("history-more")
                        .cursor_pointer()
                        .small()
                        .label("Load 50 more")
                        .disabled(loading)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.git.update(cx, |g, cx| g.load_review(true, cx))
                        })),
                );
            }
        }
        let source = selected
            .map(DiffSource::Commit)
            .or_else(|| base.map(DiffSource::Compare));
        if let Some(source) = source {
            content = content.children(
                files
                    .iter()
                    .filter(|f| f.path.to_lowercase().contains(&filter))
                    .enumerate()
                    .map(|(i, f)| self.file_row(f, source.clone(), i, cx))
                    .collect::<Vec<_>>(),
            );
        }
        if loading {
            content = content.child(div().p_2().child("Loading…"));
        } else if files.is_empty() && section == Section::Compare && !review_failed {
            content = content.child(div().p_2().text_sm().child(if no_base {
                "Choose a base branch to compare with HEAD."
            } else {
                "No committed differences from this base."
            }));
        }
        content.into_any_element()
    }
}
