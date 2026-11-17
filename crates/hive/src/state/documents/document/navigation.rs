use super::*;
use gpui_kit::component::input::Rope;

pub(crate) const STICKY_SCROLL_ROWS: usize = 2;
pub(crate) const STICKY_BORDER_WIDTH: Pixels = px(1.);

pub(crate) fn sticky_row_limit(viewport_height: Pixels, line_height: Pixels) -> usize {
    if line_height <= px(0.) {
        return 0;
    }
    (((viewport_height - line_height - STICKY_BORDER_WIDTH) / line_height)
        .floor()
        .max(0.) as usize)
        .min(STICKY_SCROLL_ROWS)
}

impl Document {
    pub fn goto(&mut self, position: Position, window: &mut Window, cx: &mut Context<Self>) {
        if self.content.is_raster() {
            return;
        }
        self.set_svg_mode(SvgMode::Code, window, cx);
        if self.base.is_none() {
            self.pending_position = Some(position);
            return;
        }
        let (cursor, text) = self.editor.update(cx, |editor, cx| {
            editor.unfold_at(position, cx);
            editor.set_cursor_position(position, window, cx);
            (editor.cursor(), editor.text().clone())
        });
        if !self.store.read(cx).state.settings.files.sticky_scroll {
            return;
        }
        let document = cx.weak_entity();
        // Native next-frame callbacks run before draw. Let one frame apply the editor's
        // pending scroll/fold layout, then use its bounds without replacing horizontal reveal.
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |_, cx| {
                let _ = document.update(cx, |document, cx| {
                    document.reveal_navigation(cursor, &text, cx);
                });
            });
        });
    }

    fn reveal_navigation(&mut self, cursor: usize, text: &Rope, cx: &mut Context<Self>) {
        if !self.store.read(cx).state.settings.files.sticky_scroll {
            return;
        }
        self.editor.update(cx, |editor, cx| {
            if editor.cursor() != cursor || editor.text() != text {
                return;
            }
            let Some(caret) = editor.range_to_bounds(&(cursor..cursor)) else {
                return;
            };
            let Some(height) = editor.line_height() else {
                return;
            };
            let offset = editor.scroll_offset();
            let revealed = reveal_below_sticky(offset, editor.input_bounds(), caret, height);
            if revealed != offset {
                editor.set_scroll_offset(revealed, cx);
            }
        });
    }
}

fn reveal_below_sticky(
    offset: Point<Pixels>,
    viewport: Bounds<Pixels>,
    caret: Bounds<Pixels>,
    line_height: Pixels,
) -> Point<Pixels> {
    let rows = sticky_row_limit(viewport.size.height, line_height);
    let inset = if rows == 0 {
        px(0.)
    } else {
        line_height * rows + STICKY_BORDER_WIDTH
    };
    let hidden = (viewport.top() + inset - caret.top()).max(px(0.));
    point(offset.x, (offset.y + hidden).min(px(0.)))
}

#[cfg(test)]
mod tests;
