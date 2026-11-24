//! Reviewer findings that did not block the run, each a row that opens to its full text.
use super::*;
use crate::ui::hover::SmoothHoverBg as _;
use crate::ui::stage_role::RoleKind;

impl RunView {
    /// Reviewer findings that did not block the run, listed where the user decides to accept.
    /// Each row opens to its full text, so long notes do not bury the criteria.
    pub(super) fn review_notes(
        &self,
        run: &Run,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let notes: Vec<_> = run
            .notes()
            .iter()
            .enumerate()
            .filter_map(|(i, (review, note))| {
                let step = run.template().reviews.get(*review)?;
                let lane = run.lanes().get(note.lane)?;
                let role = RoleKind::review(run.template(), *review);
                Some((i, role, step, lane, note))
            })
            .collect();
        if notes.is_empty() {
            return None;
        }
        let named = names_lanes(run);
        let title = section_title(Lucide::MessageSquareText, "Reviewer notes", cx).child(subtle(
            format!(
                "{} did not block acceptance",
                count(notes.len(), "note", "notes")
            ),
            cx,
        ));
        let mut rows = v_flex()
            .p_1()
            .border_1()
            .border_color(cx.theme().border)
            .rounded_md()
            .bg(cx.theme().sidebar);
        for (i, role, step, lane, note) in notes {
            let open = self.expanded_notes.contains(&i);
            let author = if named {
                format!("{} · {}", step.name, lane.name)
            } else {
                step.name.clone()
            };
            let preview = note.description.lines().next().unwrap_or_default();
            let hover = Hover::new(("review-note-hover", i), w, cx);
            let row = h_flex()
                .id(("review-note-row", i))
                .gap_2()
                .px_2()
                .py_1p5()
                .rounded_sm()
                .cursor_pointer()
                .hover_bg(&hover, cx.theme().sidebar, cx.theme().muted)
                .child(
                    Icon::new(role.icon())
                        .flex_none()
                        .size_4()
                        .text_color(role.color(cx)),
                )
                .child(
                    div()
                        .flex_none()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(author.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .truncate()
                        .text_color(cx.theme().muted_foreground)
                        .when(!open, |text| text.child(preview.to_owned())),
                )
                .child(
                    Icon::new(if open {
                        Lucide::ChevronDown
                    } else {
                        Lucide::ChevronRight
                    })
                    .flex_none()
                    .size_3(),
                );
            rows = rows.child(actionable(
                row,
                format!("{author}: {preview}"),
                Some(open),
                cx,
                move |v, cx| {
                    toggle(&mut v.expanded_notes, i);
                    cx.notify();
                },
            ));
            if open {
                rows = rows.child(
                    div()
                        .ml_4()
                        .mb_1()
                        .pl_3()
                        .pr_2()
                        .py_1()
                        .border_l_1()
                        .border_color(cx.theme().border)
                        .text_sm()
                        .child(SelectableText::new(
                            ("review-note", i),
                            note.description.clone(),
                        )),
                );
            }
        }
        Some(v_flex().gap_1p5().child(title).child(rows))
    }
}
