//! Review drafts stay local; copying never submits input to a CLI.
use super::*;
use crate::ui::text_field::TextField;
use gpui_kit::component::diff::DiffLineRange;
use gpui_kit::component::{
    IndexPath, WindowExt as _,
    dialog::DialogButtonProps,
    input::{Textarea, TextareaState},
    searchable_list::SearchableVec,
    select::{Select, SelectState},
};
use hive_core::git::review_notes::{self, Line, Note};
use std::cell::Cell;

const WIDTH: Pixels = px(640.);
const ROWS: usize = 7;

impl DiffPanel {
    pub(super) fn add_note(&self, range: DiffLineRange, window: &mut Window, cx: &mut App) {
        let request = self.review.read(cx).request.clone();
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(ROWS)
                .placeholder("What should the agent change?")
        });
        let git = self.git.clone();
        let initial_focus = Cell::new(true);
        window.open_alert_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                let input = input.clone();
                window.defer(cx, move |window, cx| {
                    input.focus_handle(cx).focus(window, cx)
                });
            }
            let (input, git, request, range) =
                (input.clone(), git.clone(), request.clone(), range.clone());
            dialog
                .title(format!(
                    "{} · lines {}–{}",
                    request.path,
                    range.start(),
                    range.end()
                ))
                .w(WIDTH)
                .child(TextField::textarea(Textarea::new(&input), &input, cx))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Add to review")
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            let text = input.read(cx).value().to_string();
                            if text.trim().is_empty() {
                                return false;
                            }
                            let line = |side, number| {
                                if side == DiffSide::Original {
                                    Line::Original(number)
                                } else {
                                    Line::Modified(number)
                                }
                            };
                            git.update(cx, |git, cx| {
                                git.add_note(
                                    request.target.clone(),
                                    Note {
                                        path: request.path.clone(),
                                        source: request.source.clone(),
                                        start: line(range.side(), range.start()),
                                        end: line(range.end_side(), range.end()),
                                        text,
                                    },
                                    cx,
                                )
                            });
                            true
                        }),
                )
        });
    }

    pub(super) fn review_prompt(&self, window: &mut Window, cx: &mut App) {
        let target = self.review.read(cx).request.target.clone();
        let notes = self.git.read(cx).notes(&target);
        let prompt = review_notes::prompt(notes);
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(ROWS * 2)
                .default_value(prompt)
        });
        let agents = AppState::get(cx).agents.clone();
        let sessions = agents
            .read(cx)
            .sessions(&target.host, &target.path)
            .to_vec();
        let labels: Vec<String> = sessions
            .iter()
            .map(|session| {
                format!(
                    "{} · {} ({})",
                    session.kind.name(),
                    session.title.as_deref().unwrap_or("Agent"),
                    session.session
                )
            })
            .collect();
        let select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(labels.clone()),
                (!labels.is_empty()).then_some(IndexPath::default()),
                window,
                cx,
            )
        });
        let git = self.git.clone();
        window.open_alert_dialog(cx, move |dialog, window, cx| {
            let (input, select, agents, sessions, labels) = (
                input.clone(),
                select.clone(),
                agents.clone(),
                sessions.clone(),
                labels.clone(),
            );
            let (git, target) = (git.clone(), target.clone());
            let hover = Hover::new("clear-review", window, cx);
            dialog
                .title("Review feedback")
                .w(WIDTH)
                .child(TextField::textarea(Textarea::new(&input), &input, cx))
                .child(
                    Select::new(&select)
                        .placeholder("Copy only — no running agent")
                        .w_full(),
                )
                .child(
                    Button::new("clear-review")
                        .cursor_pointer()
                        .small()
                        .label("Clear review draft")
                        .on_click(move |_, window, cx| {
                            git.update(cx, |git, cx| git.clear_notes(&target, cx));
                            window.close_dialog(cx);
                        })
                        .smooth_hover(ButtonLook::Ghost, &hover, cx),
                )
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Copy & go to agent")
                        .show_cancel(true)
                        .on_ok(move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                input.read(cx).value().to_string(),
                            ));
                            let session = select
                                .read(cx)
                                .selected_value()
                                .and_then(|label| labels.iter().position(|l| l == label))
                                .and_then(|index| sessions.get(index))
                                .map(|s| s.session);
                            let agents = agents.clone();
                            if let Some(session) = session {
                                window.defer(cx, move |_, cx| {
                                    agents.update(cx, |agents, cx| agents.reveal(session, cx))
                                });
                            }
                            true
                        }),
                )
        });
    }
}
