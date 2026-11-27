//! Relate the source lists to the editor's wrapped rows through live conflict anchors.
use super::*;
use gpui_kit::component::input::RopeExt as _;

impl ConflictPanel {
    pub(in crate::ui::conflict_panel) fn install_source_scroll(&self, cx: &Context<Self>) {
        for (side, list) in self.lists.iter().enumerate() {
            let panel = cx.weak_entity();
            list.set_scroll_handler(move |_, window, cx| {
                // The list still holds its layout borrow inside the scroll handler.
                let panel = panel.clone();
                window.defer(cx, move |_, cx| {
                    let _ = panel.update(cx, |this, cx| this.source_scrolled(side, cx));
                });
            });
        }
    }

    fn source_scrolled(&self, side: usize, cx: &mut Context<Self>) {
        let top = self.lists[side].logical_scroll_top();
        let Some(row) = self.rows.get(top.item_ix) else {
            return;
        };
        let Some(result) = &self.result else { return };
        let Some(doc) = self.document(cx) else { return };
        let editor = result.state.read(cx);
        let Some(height) = editor.line_height() else {
            return;
        };
        let display = editor
            .display_map()
            .buffer_line_to_display_row(self.result_line(row, doc, cx));
        self.lists[1 - side].scroll_to(top);
        let offset = point(
            editor.scroll_offset().x,
            -(height * display as f32 + top.offset_in_item),
        );
        if (offset.y - editor.scroll_offset().y).abs() >= px(1.) {
            result
                .state
                .update(cx, |e, cx| e.set_scroll_offset(offset, cx));
        }
        cx.notify();
    }

    /// Source rows inside a longer side stop at the last result line of that conflict.
    fn result_line(&self, row: &DisplayRow, doc: &MergeDocument, cx: &App) -> usize {
        let (segment, index) = row.location();
        let Some(result) = &self.result else { return 0 };
        let text = result.state.read(cx).text();
        let (start, end) = match &doc.segments[segment] {
            MergeSegment::Conflict(region) => {
                let range = &result.blocks[region.index].range;
                (range.start, range.end)
            }
            MergeSegment::Common(_) => {
                let boundary = |part: Option<&MergeSegment>, after| match part {
                    Some(MergeSegment::Conflict(region)) => {
                        let r = &result.blocks[region.index].range;
                        if after { r.end } else { r.start }
                    }
                    _ if after => 0,
                    _ => text.len(),
                };
                (
                    boundary(
                        segment.checked_sub(1).and_then(|ix| doc.segments.get(ix)),
                        true,
                    ),
                    boundary(doc.segments.get(segment + 1), false),
                )
            }
        };
        let first = text.offset_to_point(start).row;
        let last = if end > start {
            text.offset_to_point(end - 1).row
        } else {
            first
        };
        (first + index).min(last)
    }

    pub(in crate::ui::conflict_panel) fn follow_editor(&mut self, cx: &mut Context<Self>) {
        let Some(result) = &mut self.result else {
            return;
        };
        let editor_state = result.state.clone();
        let editor = editor_state.read(cx);
        let caret = editor.cursor();
        if result.cursor != caret {
            result.cursor = caret;
            let active = super::super::draft::block_at(
                &result.blocks,
                caret,
                editor.text().len(),
                self.current,
            );
            if let Some(active) = active
                && self.current != Some(active)
            {
                self.current = Some(active);
                cx.notify();
            }
        }
        let editor = editor_state.read(cx);
        let y = editor.scroll_offset().y;
        if y == result.scroll_y {
            return;
        }
        result.scroll_y = y;
        let Some(height) = editor.line_height() else {
            return;
        };
        let display = ((-y / height).floor().max(0.) as usize)
            .min(editor.display_map().display_row_count().saturating_sub(1));
        let line = editor.display_map().display_row_to_buffer_line(display);
        let Some(doc) = self.document(cx) else { return };
        let end = self
            .rows
            .partition_point(|row| self.result_line(row, doc, cx) <= line);
        let Some(last) = end.checked_sub(1) else {
            return;
        };
        // Multiple side lines can correspond to a single (possibly wrapped) result line.
        let first = self.rows[..last].partition_point(|r| self.result_line(r, doc, cx) < line);
        let display_start = editor.display_map().buffer_line_to_display_row(line);
        let offset_in_item = (-y - height * display_start as f32).max(px(0.));
        for list in &self.lists {
            list.scroll_to(ListOffset {
                item_ix: first,
                offset_in_item,
            });
        }
        cx.notify();
    }

    pub(in crate::ui::conflict_panel) fn reveal_current(
        &self,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.current else { return };
        let Some(doc) = self.document(cx) else { return };
        let Some(result) = &self.result else { return };
        let segment = doc.segments.iter().enumerate().find_map(|(i, part)| {
            matches!(part, MergeSegment::Conflict(r) if r.index == ix).then_some(i)
        });
        if let Some(segment) = segment {
            let row = self.rows.partition_point(|r| r.location().0 < segment);
            for list in &self.lists {
                list.scroll_to(ListOffset {
                    item_ix: row,
                    offset_in_item: px(0.),
                });
            }
        }
        let offset = result.blocks[ix].range.start;
        result.state.update(cx, |e, cx| {
            e.set_selected_range(offset..offset, cx);
            if focus {
                e.focus(window, cx);
            }
        });
    }
}
