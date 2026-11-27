//! One native editor and one undo timeline for text edits and merge decisions.
use super::draft::{Block, Change};
use super::*;
use gpui_kit::component::input::{
    GutterDecoration, GutterDecorationCollection, InputEditsEvent, Rope,
};
use std::collections::VecDeque;

const HISTORY_LIMIT: usize = 256;
const HISTORY_BYTES: usize = 8 * 1024 * 1024;

struct Step {
    changes: Vec<Change>,
    bytes: usize,
}

pub(super) struct ResultEditor {
    pub state: Entity<EditorState>,
    pub original: Rope,
    pub blocks: Vec<Block>,
    text: Rope,
    pub cursor: usize,
    pub scroll_y: Pixels,
    marks: GutterDecorationCollection,
    undo: VecDeque<Step>,
    redo: Vec<Step>,
    /// Programmatic replacements already applied their anchor transition synchronously.
    pending_events: usize,
    _subscriptions: Vec<Subscription>,
}

impl ResultEditor {
    pub fn new(
        doc: &MergeDocument,
        language: String,
        window: &mut Window,
        cx: &mut Context<ConflictPanel>,
    ) -> Self {
        let (text, ranges) = doc.result_draft();
        let state = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .default_value(text)
                .line_number(true)
                .soft_wrap(true)
                .gutter_decorations_width(px(3.));
            editor.set_highlighter(language, cx);
            editor
        });
        let blocks: Vec<_> = ranges
            .into_iter()
            .map(|range| Block {
                range,
                status: Status::Pending,
            })
            .collect();
        let marks = state.update(cx, |e, cx| {
            e.create_gutter_decorations_collection(decorations(&blocks, cx), cx)
        });
        let text = state.read(cx).text().clone();
        let subscriptions = vec![
            cx.subscribe(&state, |this, editor, event: &InputEditsEvent, cx| {
                let Some(result) = this.result.as_mut().filter(|r| r.state == editor) else {
                    return;
                };
                result.edited(event, this.current, cx);
                cx.notify();
            }),
            cx.observe(&state, |this, _, cx| this.follow_editor(cx)),
        ];
        Self {
            state,
            original: text.clone(),
            text,
            blocks,
            marks,
            undo: VecDeque::new(),
            redo: Vec::new(),
            pending_events: 0,
            cursor: 0,
            scroll_y: px(0.),
            _subscriptions: subscriptions,
        }
    }

    fn edited(&mut self, event: &InputEditsEvent, active: Option<usize>, cx: &mut App) {
        if self.pending_events > 0 {
            self.pending_events -= 1;
            return;
        }
        let mut changes = Vec::new();
        for (edit, text) in event.edits() {
            let mut offset = edit.start_byte;
            let mut old = self.text.slice(offset..edit.old_end_byte).to_string();
            let mut new = text.slice(offset..edit.new_end_byte).to_string();
            // IME can report the whole document on commit. Strip its unchanged context
            // before deciding which conflicts the composition actually touched.
            if offset == 0
                && edit.old_end_byte == self.text.len()
                && edit.new_end_byte == text.len()
            {
                let prefix: usize = old
                    .chars()
                    .zip(new.chars())
                    .take_while(|(a, b)| a == b)
                    .map(|(c, _)| c.len_utf8())
                    .sum();
                offset += prefix;
                let suffix: usize = old[prefix..]
                    .chars()
                    .rev()
                    .zip(new[prefix..].chars().rev())
                    .take_while(|(a, b)| a == b)
                    .map(|(c, _)| c.len_utf8())
                    .sum();
                old = old[prefix..old.len() - suffix].to_owned();
                new = new[prefix..new.len() - suffix].to_owned();
            }
            if old != new {
                let change = Change::manual(
                    &self.blocks,
                    offset..offset + old.len(),
                    old,
                    new,
                    self.text.len(),
                    active,
                );
                change.apply(&mut self.blocks, true);
                changes.push(change);
            }
            self.text = text.clone();
        }
        self.record(changes, cx);
    }

    fn record(&mut self, changes: Vec<Change>, cx: &mut App) {
        if changes.is_empty() {
            return;
        }
        let bytes = changes.iter().map(Change::bytes).sum();
        self.undo.push_back(Step { bytes, changes });
        let mut total: usize = self.undo.iter().map(|step| step.bytes).sum();
        while self.undo.len() > 1 && (self.undo.len() > HISTORY_LIMIT || total > HISTORY_BYTES) {
            total -= self
                .undo
                .pop_front()
                .expect("at least two undo steps")
                .bytes;
        }
        self.redo.clear();
        self.refresh_marks(cx);
    }

    pub(super) fn refresh_marks(&self, cx: &mut App) {
        self.marks.set(decorations(&self.blocks, cx), cx);
    }

    fn write(&mut self, change: &Change, forward: bool, window: &mut Window, cx: &mut App) {
        let (from, to) = change.fragments(forward);
        if from.text != to.text {
            self.state.update(cx, |e, cx| {
                e.set_selected_range(change.offset..change.offset + from.text.len(), cx);
                e.replace(to.text.clone(), window, cx);
                self.text = e.text().clone();
            });
            self.pending_events += 1;
        }
        change.apply(&mut self.blocks, forward);
    }

    /// One editor transaction preserves common text and manually resolved blocks between bulk choices.
    fn choose(
        &mut self,
        document: &MergeDocument,
        indices: std::ops::RangeInclusive<usize>,
        choice: Option<ConflictResolution>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (first, last) = indices.into_inner();
        let blocks = &self.blocks[first..=last];
        let start = blocks[0].range.start;
        let mut end = start;
        let mut after = String::new();
        let mut updated = Vec::with_capacity(blocks.len());
        let editor = self.state.read(cx);
        let selection = editor.selected_range();
        let reversed = editor.is_selection_reversed();
        let mut mapped = selection.clone();
        for (region, block) in document.conflicts().skip(first).zip(blocks) {
            after.extend(self.text.slice(end..block.range.start).chunks());
            let block_start = start + after.len();
            let status = if first == last || block.status == Status::Pending {
                after.push_str(
                    &region
                        .resolution_text(choice.as_ref().unwrap_or(&ConflictResolution::Current)),
                );
                choice.clone().map_or(Status::Pending, Status::Accepted)
            } else {
                after.extend(self.text.slice(block.range.clone()).chunks());
                block.status.clone()
            };
            end = block.range.end;
            let block_end = start + after.len();
            // Apply the cumulative offset once per endpoint in original coordinates.
            for (pos, mapped) in [
                (selection.start, &mut mapped.start),
                (selection.end, &mut mapped.end),
            ] {
                if pos >= end {
                    *mapped = block_end + (pos - end);
                } else if pos >= block.range.start {
                    *mapped = (block_start + pos - block.range.start).min(block_end);
                }
            }
            updated.push(Block {
                range: block_start..block_end,
                status,
            });
        }
        let change = Change::span(
            first,
            start,
            self.text.slice(start..end).to_string(),
            blocks,
            after,
            updated,
        );
        self.write(&change, true, window, cx);
        self.state.update(cx, |e, cx| {
            e.set_selected_range_with_direction(mapped, reversed, cx)
        });
        self.record(vec![change], cx);
    }

    pub fn replay(&mut self, redo: bool, window: &mut Window, cx: &mut App) {
        if redo {
            if let Some(step) = self.redo.pop() {
                for change in &step.changes {
                    self.write(change, true, window, cx);
                }
                self.undo.push_back(step);
            }
        } else if let Some(step) = self.undo.pop_back() {
            for change in step.changes.iter().rev() {
                self.write(change, false, window, cx);
            }
            self.redo.push(step);
        }
        self.refresh_marks(cx);
    }
}

fn decorations(blocks: &[Block], cx: &App) -> Vec<GutterDecoration> {
    blocks
        .iter()
        .map(|b| {
            let theme = cx.theme();
            let color = match b.status {
                Status::Pending => theme.warning,
                Status::Accepted(ConflictResolution::Current) => theme.success,
                Status::Accepted(ConflictResolution::Incoming) => theme.info,
                _ => theme.primary,
            };
            GutterDecoration::new(b.range.clone()).with_color(color)
        })
        .collect()
}

impl ConflictPanel {
    pub(super) fn accepted(&self, ix: usize) -> Option<&ConflictResolution> {
        match &self.result.as_ref()?.blocks.get(ix)?.status {
            Status::Accepted(choice) => Some(choice),
            _ => None,
        }
    }

    pub(super) fn resolve_conflict(
        &mut self,
        ix: usize,
        choice: Option<ConflictResolution>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.conflict.read(cx).is_saving() {
            return;
        }
        let Some(Content::Text { document, .. }) = self.conflict.read(cx).content().cloned() else {
            return;
        };
        let Some(result) = &mut self.result else {
            return;
        };
        if ix >= result.blocks.len() {
            return;
        }
        result.choose(&document, ix..=ix, choice, window, cx);
        self.current = Some(ix);
        result.state.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    pub(super) fn accept_all(
        &mut self,
        resolution: ConflictResolution,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.conflict.read(cx).is_saving() {
            return;
        }
        let Some(Content::Text { document, .. }) = self.conflict.read(cx).content().cloned() else {
            return;
        };
        let Some(result) = &mut self.result else {
            return;
        };
        let Some(first) = result
            .blocks
            .iter()
            .position(|b| b.status == Status::Pending)
        else {
            return;
        };
        let last = result
            .blocks
            .iter()
            .rposition(|b| b.status == Status::Pending)
            .unwrap_or(first);
        result.choose(&document, first..=last, Some(resolution), window, cx);
        result.state.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }
}
