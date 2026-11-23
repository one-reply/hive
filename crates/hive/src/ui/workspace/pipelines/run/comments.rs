//! Line comments refer to the immutable comparison the user actually inspected.
use super::*;
use gpui_kit::component::diff::{DiffLineRange, DiffSide};
use hive_core::git::{
    diff::DiffSource,
    review_notes::{Line, Note},
};

impl RunView {
    pub(super) fn comment(
        &mut self,
        repo: usize,
        path: &str,
        range: &DiffLineRange,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(run) = &self.run else {
            return;
        };
        let Some(tree) = run.review_trees().get(repo).cloned() else {
            return;
        };
        let Some(binding) = run.lanes().get(repo) else {
            return;
        };
        let base = if self.since_feedback {
            run.feedback_trees().get(repo).unwrap_or(&binding.base)
        } else {
            &binding.base
        };
        let choices: Vec<_> = run
            .lanes()
            .iter()
            .enumerate()
            .filter(|(_, l)| l.worktree == binding.worktree && l.is_assigned())
            .map(|(i, l)| (i, l.name.clone()))
            .collect();
        if choices.is_empty() {
            return;
        }
        let select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(choices.iter().map(|(_, n)| n.clone()).collect::<Vec<_>>()),
                (choices.len() == 1).then(IndexPath::default),
                w,
                cx,
            )
        });
        let line = |side, n| {
            if side == DiffSide::Original {
                Line::Original(n)
            } else {
                Line::Modified(n)
            }
        };
        let note = Note {
            path: path.into(),
            source: DiffSource::Compare(base.clone()),
            start: line(range.side(), range.start()),
            end: line(range.end_side(), range.end()),
            text: String::new(),
        };
        let input = area("", "What should change?", w, cx);
        let view = cx.weak_entity();
        w.open_alert_dialog(cx, move |dialog, _, cx| {
            let (view, tree, choices, select, input, note) = (
                view.clone(),
                tree.clone(),
                choices.clone(),
                select.clone(),
                input.clone(),
                note.clone(),
            );
            dialog
                .title(format!("{} · {}–{}", note.path, note.start, note.end))
                .when(choices.len() > 1, |d| {
                    d.child(heading("Send to lane"))
                        .child(Select::new(&select).w_full())
                })
                .child(paragraph("Comment", &input, cx))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Add comment")
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            let Some((lane, _)) = choices
                                .iter()
                                .find(|(_, name)| Some(name) == select.read(cx).selected_value())
                            else {
                                return false;
                            };
                            let mut note = note.clone();
                            note.text = input.read(cx).value().trim().to_owned();
                            if note.text.is_empty() {
                                return false;
                            }
                            let _ = view.update(cx, |v, cx| {
                                v.error = AppState::get(cx)
                                    .pipelines
                                    .clone()
                                    .update(cx, |p, cx| {
                                        p.edit_comments(v.id, cx, |r| {
                                            r.add_comment(*lane, tree.clone(), note.clone())
                                        })
                                    })
                                    .err();
                                cx.notify();
                            });
                            true
                        }),
                )
        });
    }

    pub(super) fn comment_actions(&self, run: &Run, w: &mut Window, cx: &mut Context<Self>) -> Div {
        let mut body = v_flex().gap_1();
        for (i, (lane, tree, note)) in run.comments().iter().enumerate() {
            let selected = (*lane, tree.clone(), note.clone());
            let stale = run.review_trees().get(*lane) != Some(tree);
            body = body.child(
                h_flex()
                    .gap_2()
                    .child(subtle(
                        format!(
                            "{}{} · {}–{} · {}",
                            if stale { "Outdated position · " } else { "" },
                            note.path,
                            note.start,
                            note.end,
                            note.text
                        ),
                        cx,
                    ))
                    .child(button(
                        ("remove-comment", i),
                        "Remove",
                        ButtonLook::Ghost,
                        w,
                        cx,
                        move |v, _, cx| {
                            v.error = AppState::get(cx)
                                .pipelines
                                .clone()
                                .update(cx, |p, cx| {
                                    p.edit_comments(v.id, cx, |r| {
                                        r.remove_comment(&selected);
                                        Ok(())
                                    })
                                })
                                .err();
                            cx.notify();
                        },
                    )),
            );
        }
        for (ix, lane) in run.lanes().iter().enumerate() {
            let Some(tree) = run.review_trees().get(ix) else {
                continue;
            };
            let batch: Vec<_> = run
                .comments()
                .iter()
                .filter(|(i, t, _)| *i == ix && t == tree)
                .map(|(_, _, n)| n.clone())
                .collect();
            if batch.is_empty() {
                continue;
            }
            let tree = tree.clone();
            body = body.child(button(
                ("send-comments", ix),
                format!(
                    "Send {} to {}",
                    count(batch.len(), "comment", "comments"),
                    lane.name
                ),
                ButtonLook::Primary,
                w,
                cx,
                move |v, _, cx| {
                    let work = AppState::get(cx).pipelines.clone().update(cx, |p, cx| {
                        p.send_comments(v.id, ix, tree.clone(), batch.clone(), cx)
                    });
                    cx.spawn(async move |view, cx| {
                        let result = work.await;
                        let _ = view.update(cx, |v, cx| {
                            v.error = result.err();
                            cx.notify();
                        });
                    })
                    .detach();
                },
            ));
        }
        body
    }
}
