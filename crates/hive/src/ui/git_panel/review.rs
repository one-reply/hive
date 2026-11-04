//! Compare and History sections: committed changes against a base or a selected commit.
use super::*;
use hive_core::git::history::relative_date;

const SHORT_OID: usize = 7;

fn short(oid: &str) -> &str {
    &oid[..oid.len().min(SHORT_OID)]
}

impl GitPanel {
    pub(super) fn compare(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let state = self.git.read(cx);
        let base = state.base.clone();
        let loading = state.review_loading;
        let failed = state.error.is_some();
        let files = state.files.clone();
        let muted = cx.theme().muted_foreground;
        let mut content = v_flex().gap_1().child(
            h_flex()
                .mt_1()
                .px_1()
                .gap_2()
                .items_center()
                .child(
                    Button::new("compare-base")
                        .cursor_pointer()
                        .outline()
                        .xsmall()
                        .icon(Lucide::GitCompareArrows)
                        .label(base.clone().unwrap_or_else(|| "Choose base…".into()))
                        .dropdown_caret(true)
                        .tooltip("Compare against another branch")
                        .on_click(cx.listener(|this, _, window, cx| {
                            git_controls::branches(this.git.clone(), true, window, cx)
                        })),
                )
                .child(
                    div()
                        .text_size(META_TEXT)
                        .text_color(muted)
                        .child("… HEAD, committed only"),
                ),
        );
        if let Some(base) = base.clone() {
            content =
                content.children(self.review_files(&files, DiffSource::Compare(base), window, cx));
        }
        if loading {
            content = content.child(loading_row("Comparing…", cx));
        } else if files.is_empty() && !failed {
            content = content.child(if base.is_none() {
                empty_state(
                    Lucide::GitCompareArrows,
                    "No base branch",
                    "Choose a branch to compare HEAD with.",
                    cx,
                )
            } else {
                empty_state(
                    Lucide::CircleCheck,
                    "Nothing to compare",
                    "No committed differences from this base.",
                    cx,
                )
            });
        }
        content.into_any_element()
    }

    pub(super) fn history(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let state = self.git.read(cx);
        let loading = state.review_loading;
        let files = state.files.clone();
        let muted = cx.theme().muted_foreground;
        let mut content = v_flex().gap_0p5();
        if let Some(oid) = state.selected_commit.clone() {
            let commit = state.commits.iter().find(|c| c.oid == oid).cloned();
            content = content
                .child(
                    Button::new("history-back")
                        .cursor_pointer()
                        .ghost()
                        .xsmall()
                        .mt_1()
                        .icon(Lucide::ArrowLeft)
                        .label("All commits")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.git
                                .update(cx, |g, cx| g.set_section(Section::History, cx))
                        })),
                )
                .child(
                    v_flex()
                        .mx_1()
                        .mb_1()
                        .p_2()
                        .gap_0p5()
                        .rounded_md()
                        .bg(cx.theme().foreground.opacity(0.04))
                        .child(
                            div().text_size(TITLE_TEXT).child(
                                commit
                                    .as_ref()
                                    .map(|c| c.subject.clone())
                                    .unwrap_or_default(),
                            ),
                        )
                        .child(
                            div()
                                .text_size(META_TEXT)
                                .text_color(muted)
                                .child(format!("{} · against first parent", short(&oid))),
                        ),
                )
                .children(self.review_files(&files, DiffSource::Commit(oid), window, cx));
            if loading {
                content = content.child(loading_row("Loading files…", cx));
            }
            return content.into_any_element();
        }
        let commits = state.commits.clone();
        let more = state.history_more;
        content = content.children(commits.into_iter().enumerate().map(|(i, commit)| {
            let oid = commit.oid.clone();
            v_flex()
                .id(("commit", i))
                .px_2()
                .py_1()
                .gap_0p5()
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(cx.theme().foreground.opacity(0.05)))
                .child(
                    div()
                        .text_size(META_TEXT + px(1.))
                        .truncate()
                        .child(commit.subject.clone()),
                )
                .child(
                    h_flex()
                        .gap_1p5()
                        .text_size(META_TEXT)
                        .text_color(muted)
                        .child(
                            div()
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(short(&commit.oid).to_string()),
                        )
                        .child(div().min_w_0().truncate().child(commit.author.clone()))
                        .child(div().flex_none().child(relative_date(&commit.date))),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.git
                        .update(cx, |g, cx| g.select_commit(oid.clone(), cx))
                }))
        }));
        if loading {
            content = content.child(loading_row("Loading commits…", cx));
        } else if more {
            content = content.child(
                h_flex().justify_center().py_2().child(
                    Button::new("history-more")
                        .cursor_pointer()
                        .outline()
                        .xsmall()
                        .label("Load 50 more")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.git.update(cx, |g, cx| g.load_review(true, cx))
                        })),
                ),
            );
        }
        content.into_any_element()
    }

    fn review_files(
        &self,
        files: &[FileChange],
        source: DiffSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let filter = self.filter.read(cx).value().to_lowercase();
        files
            .iter()
            .filter(|f| f.path.to_lowercase().contains(&filter))
            .enumerate()
            .map(|(i, f)| self.file_row(f, source.clone(), i, window, cx))
            .collect()
    }
}
