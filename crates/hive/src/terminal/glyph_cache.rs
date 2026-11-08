//! Per-cell glyph layouts kept across frames. The terminal still shapes every cell on its own
//! (one CoreText run per cell, so no ligature or kerning forms between cells), but a cell's
//! layout depends only on its character, font variant, family and size, so it is shaped once
//! and reused instead of rebuilding strings, fonts and runs for every cell on every frame.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::{
    DecorationRun, Font, FontStyle, FontWeight, Hsla, LineLayout, Pixels, SharedString,
    StrikethroughStyle, UnderlineStyle, px,
};
use hive_core::terminal::{Cell, Flags};

/// Distinct (character, variant) layouts kept before the cache starts over. A screen of ASCII
/// needs a few hundred; CJK or emoji-heavy output can need more.
// ponytail: clears at the limit; an LRU only if a CJK-heavy screen measurably thrashes it.
const MAX_LAYOUTS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Variant {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl From<Flags> for Variant {
    fn from(flags: Flags) -> Self {
        match (
            flags.intersects(Flags::BOLD),
            flags.intersects(Flags::ITALIC),
        ) {
            (false, false) => Self::Regular,
            (true, false) => Self::Bold,
            (false, true) => Self::Italic,
            (true, true) => Self::BoldItalic,
        }
    }
}

impl Variant {
    pub(super) fn font(self, base: &Font) -> Font {
        let mut font = base.clone();
        if matches!(self, Self::Bold | Self::BoldItalic) {
            font.weight = FontWeight::BOLD;
        }
        if matches!(self, Self::Italic | Self::BoldItalic) {
            font.style = FontStyle::Italic;
        }
        font
    }
}

/// What a cell draws: a lone character (cached) or one with zero-width marks after it.
#[derive(Debug, PartialEq)]
pub(super) enum CellText {
    Char(char),
    Cluster(String),
}

impl CellText {
    /// Length in UTF-8 bytes, as text runs count it.
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Char(c) => c.len_utf8(),
            Self::Cluster(text) => text.len(),
        }
    }
}

/// The text a cell paints, or `None` for a blank cell with nothing to draw.
pub(super) fn cell_text(cell: &Cell) -> Option<CellText> {
    let zerowidth = cell.zerowidth();
    let decorated = cell
        .flags
        .intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT);
    match zerowidth {
        None if cell.c == ' ' && !decorated => None,
        None => Some(CellText::Char(cell.c)),
        Some(marks) => {
            let mut text = String::with_capacity(4);
            text.push(cell.c);
            text.extend(marks);
            Some(CellText::Cluster(text))
        }
    }
}

/// Whether a cell draws lines besides its glyphs; those cells keep `LineLayout::paint`, which
/// draws them, while plain cells share one layer.
pub(super) fn has_lines(decoration: &DecorationRun) -> bool {
    decoration.underline.is_some() || decoration.strikethrough.is_some()
}

/// The colour and decorations of one cell's glyph, as `shape_line` derives them from its run.
pub(super) fn decoration(flags: Flags, fg: Hsla, len: usize) -> DecorationRun {
    DecorationRun {
        len: len as u32,
        color: fg,
        background_color: None,
        underline: flags
            .intersects(Flags::ALL_UNDERLINES)
            .then(|| UnderlineStyle {
                thickness: px(1.),
                color: Some(fg),
                wavy: flags.contains(Flags::UNDERCURL),
            }),
        strikethrough: flags
            .contains(Flags::STRIKEOUT)
            .then(|| StrikethroughStyle {
                thickness: px(1.),
                color: Some(fg),
            }),
    }
}

/// Shaped layouts by (character, variant) for one font family and size.
#[derive(Default)]
pub(super) struct GlyphCache {
    family: SharedString,
    font_size: Pixels,
    layouts: HashMap<(char, Variant), Arc<LineLayout>>,
}

impl GlyphCache {
    /// Starts a frame; a new family or size makes every kept layout stale.
    pub(super) fn begin_frame(&mut self, family: &SharedString, font_size: Pixels) {
        if self.family != *family || self.font_size != font_size {
            self.layouts.clear();
            self.family = family.clone();
            self.font_size = font_size;
        }
    }

    pub(super) fn get_or_shape(
        &mut self,
        c: char,
        variant: Variant,
        shape: impl FnOnce() -> Arc<LineLayout>,
    ) -> Arc<LineLayout> {
        if self.layouts.len() >= MAX_LAYOUTS && !self.layouts.contains_key(&(c, variant)) {
            self.layouts.clear();
        }
        self.layouts
            .entry((c, variant))
            .or_insert_with(shape)
            .clone()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.layouts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Rgba, StrikethroughStyle};

    fn cell(c: char, flags: Flags) -> Cell {
        Cell {
            c,
            flags,
            ..Cell::default()
        }
    }

    fn red() -> Hsla {
        Rgba {
            r: 1.,
            g: 0.,
            b: 0.,
            a: 1.,
        }
        .into()
    }

    #[test]
    fn flags_map_to_font_variant() {
        assert_eq!(Variant::from(Flags::empty()), Variant::Regular);
        assert_eq!(Variant::from(Flags::BOLD), Variant::Bold);
        assert_eq!(Variant::from(Flags::ITALIC), Variant::Italic);
        assert_eq!(Variant::from(Flags::BOLD_ITALIC), Variant::BoldItalic);
        assert_eq!(Variant::from(Flags::DIM | Flags::INVERSE), Variant::Regular);

        let base = gpui_kit::font("Menlo");
        let bold_italic = Variant::BoldItalic.font(&base);
        assert_eq!(bold_italic.weight, FontWeight::BOLD);
        assert_eq!(bold_italic.style, FontStyle::Italic);
        assert_eq!(Variant::Regular.font(&base), base);
    }

    #[test]
    fn blank_cell_without_decoration_is_skipped() {
        assert_eq!(cell_text(&cell(' ', Flags::empty())), None);
        assert_eq!(cell_text(&cell(' ', Flags::BOLD)), None);
        assert_eq!(
            cell_text(&cell(' ', Flags::UNDERLINE)),
            Some(CellText::Char(' '))
        );
        assert_eq!(
            cell_text(&cell(' ', Flags::STRIKEOUT)),
            Some(CellText::Char(' '))
        );
        assert_eq!(
            cell_text(&cell('x', Flags::empty())),
            Some(CellText::Char('x'))
        );
    }

    #[test]
    fn zero_width_marks_make_a_cluster() {
        let mut accented = cell('e', Flags::empty());
        accented.push_zerowidth('\u{301}');
        let text = cell_text(&accented).expect("cluster");
        assert_eq!(text, CellText::Cluster("e\u{301}".into()));
        assert_eq!(text.len(), 3);

        let mut blank = cell(' ', Flags::empty());
        blank.push_zerowidth('\u{200d}');
        assert_eq!(
            cell_text(&blank),
            Some(CellText::Cluster(" \u{200d}".into()))
        );
        assert_eq!(CellText::Char('日').len(), 3);
    }

    #[test]
    fn decoration_run_mirrors_shape_line() {
        let fg = red();
        let plain = decoration(Flags::empty(), fg, 1);
        assert_eq!(plain.len, 1);
        assert_eq!(plain.color, fg);
        assert_eq!(plain.background_color, None);
        assert_eq!(plain.underline, None);
        assert_eq!(plain.strikethrough, None);

        let curly = decoration(Flags::UNDERCURL | Flags::STRIKEOUT, fg, 3);
        assert_eq!(curly.len, 3);
        assert_eq!(
            curly.underline,
            Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(fg),
                wavy: true,
            })
        );
        assert_eq!(
            curly.strikethrough,
            Some(StrikethroughStyle {
                thickness: px(1.),
                color: Some(fg),
            })
        );
        let straight = decoration(Flags::DOUBLE_UNDERLINE, fg, 1);
        assert_eq!(straight.underline.map(|u| u.wavy), Some(false));
    }

    #[test]
    fn only_underlined_or_struck_cells_have_lines() {
        let fg = red();
        assert!(!has_lines(&decoration(Flags::empty(), fg, 1)));
        assert!(!has_lines(&decoration(
            Flags::BOLD | Flags::ITALIC | Flags::INVERSE,
            fg,
            1
        )));
        assert!(has_lines(&decoration(Flags::UNDERLINE, fg, 1)));
        assert!(has_lines(&decoration(Flags::UNDERCURL, fg, 1)));
        assert!(has_lines(&decoration(Flags::STRIKEOUT, fg, 1)));
    }

    #[test]
    fn same_char_and_variant_shapes_once() {
        let mut cache = GlyphCache::default();
        cache.begin_frame(&"Menlo".into(), px(13.));
        let shaped = std::cell::Cell::new(0);
        let shape = || {
            shaped.set(shaped.get() + 1);
            Arc::new(LineLayout::default())
        };
        let first = cache.get_or_shape('a', Variant::Regular, shape);
        let again = cache.get_or_shape('a', Variant::Regular, shape);
        assert!(Arc::ptr_eq(&first, &again));
        cache.get_or_shape('a', Variant::Bold, shape);
        assert_eq!(shaped.get(), 2);

        cache.begin_frame(&"Menlo".into(), px(13.));
        cache.get_or_shape('a', Variant::Regular, shape);
        assert_eq!(shaped.get(), 2, "the next frame reuses the layout");
    }

    #[test]
    fn font_change_clears_cache() {
        let mut cache = GlyphCache::default();
        let shaped = std::cell::Cell::new(0);
        let shape = || {
            shaped.set(shaped.get() + 1);
            Arc::new(LineLayout::default())
        };
        cache.begin_frame(&"Menlo".into(), px(13.));
        cache.get_or_shape('a', Variant::Regular, shape);
        cache.begin_frame(&"Menlo".into(), px(14.));
        cache.get_or_shape('a', Variant::Regular, shape);
        cache.begin_frame(&"Monaco".into(), px(14.));
        cache.get_or_shape('a', Variant::Regular, shape);
        assert_eq!(shaped.get(), 3);
    }

    #[test]
    fn cache_stays_bounded() {
        let mut cache = GlyphCache::default();
        cache.begin_frame(&"Menlo".into(), px(13.));
        for c in (0x4e00..0x4e00 + MAX_LAYOUTS as u32 * 2).filter_map(char::from_u32) {
            cache.get_or_shape(c, Variant::Regular, || Arc::new(LineLayout::default()));
            assert!(cache.len() <= MAX_LAYOUTS);
        }
    }
}
