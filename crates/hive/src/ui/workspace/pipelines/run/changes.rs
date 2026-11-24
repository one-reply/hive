//! Changes are browsed by repository; only the selected file gets a diff display.
use super::*;
use crate::ui::hover::SmoothHoverBg as _;
use gpui_kit::component::diff::{Diff, DiffMode};
use hive_core::{git::status::Change, pipeline::workspace::ReviewFile};
use std::ops::Range;

const FILE_HEIGHT: f32 = 32.;

impl RunView {
    pub(super) fn changes(
        &mut self,
        run: &Run,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pipelines = AppState::get(cx).pipelines.clone();
        if let Some((lane, path)) = self.selected_file.clone() {
            return self.patch(run, lane, path, w, cx);
        }
        let mut page = v_flex().gap_1();
        let mut actions = h_flex().gap_2().child(header_button(
            "refresh-diff",
            "Refresh",
            Lucide::RefreshCw,
            ButtonLook::Outline,
            w,
            cx,
            |view, cx| {
                view.refresh_changes(cx);
                cx.notify();
            },
        ));
        if !run.feedback_trees().is_empty()
            && !run.review_trees().is_empty()
            && matches!(run.phase(), Phase::Acceptance | Phase::Accepted)
        {
            for (since, label, icon) in [
                (true, "Since your feedback", Lucide::MessageSquareReply),
                (false, "All changes", Lucide::Files),
            ] {
                actions = actions.child(header_button(
                    label,
                    label,
                    icon,
                    if since == self.since_feedback {
                        ButtonLook::Primary
                    } else {
                        ButtonLook::Outline
                    },
                    w,
                    cx,
                    move |v, cx| {
                        v.since_feedback = since;
                        v.refresh_changes(cx);
                        cx.notify();
                    },
                ));
            }
        }
        page = page.child(
            h_flex()
                .justify_between()
                .gap_2()
                .child(heading("Changed files"))
                .child(actions),
        );
        if run.phase() == Phase::Acceptance && !run.comments().is_empty() {
            page = page.child(self.comment_actions(run, w, cx));
        }
        match pipelines.read(cx).review(self.id) {
            None => page = page.child(subtle("Reading worktree changes…", cx)),
            Some(Err(e)) => {
                page = page.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(e.clone()),
                )
            }
            Some(Ok(repos)) => {
                for repo in repos {
                    let name = run
                        .lanes()
                        .get(repo.lane)
                        .map(|lane| path_name(&lane.project))
                        .unwrap_or_else(|| repo.name.clone());
                    let count = repo.files.len();
                    let totals = repo.files.iter().try_fold((0_usize, 0_usize), |(a, d), f| {
                        Some((a.saturating_add(f.added?), d.saturating_add(f.removed?)))
                    });
                    let mut header = h_flex()
                        .gap_2()
                        .mt_2()
                        .py_2()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(
                            Icon::new(Lucide::Folder)
                                .size_4()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(heading(name))
                        .child(subtle(
                            format!("{count} {}", if count == 1 { "file" } else { "files" }),
                            cx,
                        ));
                    if let Some((added, removed)) = totals
                        && count > 0
                    {
                        header = header
                            .child(div().flex_1())
                            .child(line_stats(added, removed, cx));
                    }
                    page = page.child(header);
                    if repo.files.is_empty() {
                        page = page.child(subtle("No changed files", cx).py_2().pl_5());
                        continue;
                    }
                    let lane = repo.lane;
                    let list_height = (count.min(10) as f32) * FILE_HEIGHT;
                    // A large rename can change thousands of paths. Build only visible rows.
                    page = page.child(
                        uniform_list(
                            ("changed-files", lane),
                            count,
                            cx.processor(move |this, range, w, cx| {
                                this.file_rows(lane, range, w, cx)
                            }),
                        )
                        .h(px(list_height))
                        .w_full(),
                    );
                }
            }
        }
        scroll("pipeline-changes", page)
    }
    fn file_rows(
        &self,
        lane: usize,
        range: Range<usize>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let pipelines = AppState::get(cx).pipelines.clone();
        let repo = pipelines
            .read(cx)
            .review(self.id)
            .and_then(|r| r.as_ref().ok())
            .and_then(|repos| repos.iter().find(|r| r.lane == lane));
        let Some(repo) = repo else { return vec![] };
        let visible = range
            .filter_map(|i| repo.files.get(i).cloned().map(|file| (i, file)))
            .collect::<Vec<_>>();
        visible
            .into_iter()
            .map(|(i, file)| Self::file_row(lane, i, file, w, cx))
            .collect()
    }

    fn file_row(
        lane: usize,
        index: usize,
        file: ReviewFile,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let (label, color) = change_label(file.change, cx);
        let id = ElementId::Name(format!("change-{lane}-{index}").into());
        let hover = Hover::new(id.clone(), w, cx);
        let mut row = h_flex()
            .id(id)
            .h(px(FILE_HEIGHT))
            .w_full()
            .rounded_sm()
            .px_2()
            .gap_2()
            .cursor_pointer()
            .hover_bg(&hover, cx.theme().background, cx.theme().muted)
            .child(
                Icon::new(Lucide::FileCode)
                    .size_4()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .truncate()
                    .child(file.path.clone()),
            )
            .child(div().text_xs().text_color(color).child(label));
        if let Some((added, removed)) = file.added.zip(file.removed) {
            row = row.child(line_stats(added, removed, cx));
        }
        row.child(
            Icon::new(Lucide::ChevronRight)
                .size_3()
                .text_color(cx.theme().muted_foreground),
        )
        .map(|row| {
            actionable(
                row,
                format!("View {} · {label}", file.path),
                None,
                cx,
                move |view, cx| {
                    view.display = None;
                    view.selected_file = Some((lane, file.path.clone()));
                    AppState::get(cx).pipelines.clone().update(cx, |p, cx| {
                        p.load_patch(view.id, lane, file.path.clone(), cx)
                    });
                    cx.notify();
                },
            )
        })
    }

    fn patch(
        &mut self,
        run: &Run,
        lane: usize,
        path: String,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pipelines = AppState::get(cx).pipelines.clone();
        let home = button(
            "all-changes",
            "All files",
            ButtonLook::Ghost,
            w,
            cx,
            |this, _, cx| {
                this.selected_file = None;
                this.display = None;
                cx.notify();
            },
        );
        let project = run
            .lanes()
            .get(lane)
            .map(|l| path_name(&l.project))
            .unwrap_or_default();
        let header = h_flex()
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(home)
            .child(Icon::new(Lucide::ChevronRight).size_3())
            .child(subtle(project, cx))
            .child(Icon::new(Lucide::ChevronRight).size_3())
            .child(div().text_sm().truncate().child(path.clone()));
        let result = pipelines
            .read(cx)
            .patch(self.id)
            .filter(|(i, p, _)| *i == lane && *p == path);
        let body = match result {
            Some((_, _, Ok(files))) if files.iter().any(|f| f.is_binary()) => {
                subtle("This file contains binary changes.", cx)
                    .p_4()
                    .into_any_element()
            }
            Some((_, _, Ok(files))) if files.is_empty() => {
                subtle("No text changes to display.", cx)
                    .p_4()
                    .into_any_element()
            }
            Some((_, _, Ok(files))) => {
                if self.display.is_none() {
                    let files = files.clone();
                    self.display =
                        Some(cx.new(|cx| DiffState::new(files, cx).with_mode(DiffMode::Unified)));
                }
                let display = self.display.as_ref().expect("display was just initialized");
                let view = cx.weak_entity();
                Diff::new(display)
                    .when(
                        run.phase() == Phase::Acceptance && run.review_trees().get(lane).is_some(),
                        |d| {
                            d.on_add_annotation(move |range, w, cx| {
                                let _ =
                                    view.update(cx, |v, cx| v.comment(lane, &path, range, w, cx));
                            })
                        },
                    )
                    .flex_1()
                    .min_h_0()
                    .into_any_element()
            }
            Some((_, _, Err(error))) => div()
                .p_4()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(error.clone())
                .into_any_element(),
            None => subtle("Reading file changes…", cx).p_4().into_any_element(),
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .child(header)
            .when(
                run.phase() == Phase::Acceptance && !run.comments().is_empty(),
                |panel| panel.child(self.comment_actions(run, w, cx)),
            )
            .child(body)
            .into_any_element()
    }
}
/// A labeled icon button of the "Changed files" header.
fn header_button(
    id: &'static str,
    label: &'static str,
    icon: Lucide,
    look: ButtonLook,
    w: &mut Window,
    cx: &mut Context<RunView>,
    click: impl Fn(&mut RunView, &mut Context<RunView>) + 'static,
) -> Stateful<Div> {
    let hover = Hover::new(id, w, cx);
    Button::new(id)
        .cursor_pointer()
        .small()
        .icon(icon)
        .label(label)
        .on_click(cx.listener(move |view, _, _, cx| click(view, cx)))
        .smooth_hover(look, &hover, cx)
}
fn line_stats(added: usize, removed: usize, cx: &App) -> Div {
    h_flex()
        .gap_1p5()
        .text_xs()
        .child(
            div()
                .text_color(cx.theme().success)
                .child(format!("+{added}")),
        )
        .child(
            div()
                .text_color(cx.theme().danger)
                .child(format!("−{removed}")),
        )
}
fn change_label(change: Change, cx: &App) -> (&'static str, Hsla) {
    match change {
        Change::Added | Change::Untracked => ("Added", cx.theme().success),
        Change::Deleted => ("Deleted", cx.theme().danger),
        Change::Renamed => ("Renamed", cx.theme().info),
        Change::Copied => ("Copied", cx.theme().info),
        Change::Conflicted => ("Conflicted", cx.theme().warning),
        Change::Modified => ("Modified", cx.theme().warning),
        Change::TypeChanged => ("Type changed", cx.theme().warning),
    }
}
