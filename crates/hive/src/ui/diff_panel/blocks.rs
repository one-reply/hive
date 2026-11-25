//! Every action captures the request and patch of the block that rendered it.
use super::*;
use crate::state::{documents::OpenFile, git_diff::Block};
use gpui_kit::component::diff::DiffFile;
use gpui_kit::prelude::FluentBuilder as _;
use std::collections::HashMap;

impl DiffPanel {
    pub(super) fn blocks(&self, pan: ScrollHandle, cx: &Context<Self>) -> Diff {
        let review = self.review.read(cx);
        let busy = self.git.read(cx).is_busy_at(&review.request.target);
        let mut annotations = Vec::new();
        let mut actions = HashMap::new();
        for key in &review.order {
            let block = &review.blocks[key];
            let request = &block.entry.request;
            let Some(patch) = block.patch.as_ref().filter(|patch| {
                !request.whole_file_only
                    && patch.can_stage_hunks()
                    && matches!(request.source, DiffSource::WorkingTree | DiffSource::Staged)
            }) else {
                continue;
            };
            for (index, hunk) in patch.hunks().iter().enumerate() {
                let (side, line) = if hunk.new_line == 0 {
                    (DiffSide::Original, hunk.old_line)
                } else {
                    (DiffSide::Modified, hunk.new_line)
                };
                let id = ElementId::NamedInteger(key.clone(), index as u64);
                annotations.push(DiffAnnotation::line(
                    id.clone(),
                    DiffLinePosition::new(key.clone(), side, line.max(1)),
                ));
                actions.insert(id, (request.clone(), patch.clone(), index));
            }
        }
        let model = self.review.clone();
        let headers = model.clone();
        let store = self.store.clone();
        let git = self.git.clone();
        let panel = cx.entity().downgrade();

        Diff::new(&self.display)
            .render_header(move |file, window, cx| {
                let Some(block) = headers.read(cx).blocks.get(file.path()) else {
                    return div().into_any_element();
                };
                let header = header(file, block.clone(), &store, window, cx);
                // The widget reserves a disclosure button and horizontal padding outside the slot.
                in_viewport(header, &pan, window.rem_size() * 2.75).into_any_element()
            })
            .on_line_click(move |position, event, _, cx| {
                if event.click_count() < 2 || position.side() != DiffSide::Modified {
                    return;
                }
                if let Some(block) = model.read(cx).blocks.get(position.path()) {
                    let request = block.entry.request.clone();
                    if matches!(
                        request.source,
                        DiffSource::WorkingTree | DiffSource::Untracked
                    ) {
                        open(
                            &request,
                            Some(Position::new(
                                u32::try_from(position.line().saturating_sub(1))
                                    .unwrap_or(u32::MAX),
                                0,
                            )),
                            cx,
                        );
                    }
                }
            })
            .on_add_annotation(move |range, window, cx| {
                let _ = panel.update(cx, |this, cx| this.add_note(range.clone(), window, cx));
            })
            .annotations(annotations)
            .render_annotation(move |annotation, window, cx| {
                let Some((request, patch, index)) = actions.get(annotation.id()).cloned() else {
                    return div().into_any_element();
                };
                let git = git.clone();
                let hover = Hover::new(annotation.id().clone(), window, cx).enabled(!busy);
                h_flex()
                    .justify_end()
                    .child(
                        Button::new(annotation.id().clone())
                            .cursor_pointer()
                            .small()
                            .label(if request.source == DiffSource::Staged {
                                "Unstage hunk"
                            } else {
                                "Stage hunk"
                            })
                            .disabled(busy)
                            .on_click(move |_, _, cx| {
                                git.update(cx, |git, cx| {
                                    git.apply_hunk(request.clone(), patch.clone(), index, cx)
                                });
                            })
                            .smooth_hover(ButtonLook::Outline, &hover, cx),
                    )
                    .into_any_element()
            })
    }
}

/// Keep controls in the visible pane while the split code columns pan underneath.
fn in_viewport(row: Div, pan: &ScrollHandle, inset: Pixels) -> Div {
    let width = pan.bounds().size.width;
    row.when(width > inset, |row| {
        row.w(width - inset).ml(-pan.offset().x)
    })
}

fn header(
    file: &DiffFile,
    block: Block,
    store: &Entity<Store>,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let request = block.entry.request;
    let source = match request.source {
        DiffSource::Staged => Some("Staged"),
        DiffSource::WorkingTree => Some("Unstaged"),
        DiffSource::Untracked => Some("Untracked"),
        _ => None,
    };
    let revision = block.revision.clone();
    let checked = revision
        .as_ref()
        .is_some_and(|r| store.read(cx).state.reviewed.contains(r));
    let review_id = ElementId::NamedInteger(file.path().clone(), u64::MAX);
    let open_id = ElementId::NamedInteger(file.path().clone(), u64::MAX - 1);
    let review_hover = Hover::new(review_id.clone(), window, cx).enabled(revision.is_some());
    let open_hover = Hover::new(open_id.clone(), window, cx);
    let store = store.clone();
    let stats = block.entry.stats.map(|s| (s.added, s.removed)).or_else(|| {
        block
            .patch
            .as_ref()
            .filter(|_| !file.is_binary())
            .map(|_| (file.additions(), file.deletions()))
    });
    h_flex()
        .w_full()
        .min_w_0()
        .gap_2()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .child(request.path.clone()),
        )
        .children(source.map(|s| {
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(s)
        }))
        .child(crate::ui::git_panel::status_badge(block.entry.change, cx))
        .when_some(stats, |row, (added, removed)| {
            row.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().green)
                    .child(format!("+{added}")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().red)
                    .child(format!("−{removed}")),
            )
        })
        .child(
            Button::new(review_id)
                .small()
                .cursor_pointer()
                .label(if checked {
                    "✓ Reviewed"
                } else {
                    "Mark reviewed"
                })
                .disabled(revision.is_none())
                .on_click(move |_, _, cx| {
                    if let Some(revision) = &revision {
                        store.update(cx, |s, cx| s.set_reviewed(revision.clone(), !checked, cx));
                    }
                })
                .smooth_hover(ButtonLook::Ghost, &review_hover, cx),
        )
        .child(
            Button::new(open_id)
                .small()
                .cursor_pointer()
                .label("Open file")
                .on_click(move |_, _, cx| open(&request, None, cx))
                .smooth_hover(ButtonLook::Ghost, &open_hover, cx),
        )
}

fn open(request: &DiffRequest, position: Option<Position>, cx: &mut App) {
    AppState::get(cx)
        .documents
        .clone()
        .update(cx, |documents, cx| {
            documents.request(
                OpenFile {
                    target: request.target.clone(),
                    path: request.path.clone().into(),
                    position,
                    preview: false,
                    placement: Default::default(),
                },
                cx,
            )
        });
}
