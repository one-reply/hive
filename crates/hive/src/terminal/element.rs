//! Paints the terminal grid: cell backgrounds, glyphs (one layout per non-blank cell, kept
//! across frames by [`super::glyph_cache`]), decorations and the cursor. Also measures how many
//! cells fit in the bounds and resizes the PTY to match.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use hive_core::terminal::{Cell, Color, CursorShape, Flags, NamedColor, Point as GridPoint, Rgb};

use std::sync::Arc;

use super::glyph_cache::{self, CellText, Variant};
use super::mouse::GridGeometry;
use super::view::TerminalView;

/// Floor for the theme's mono size so a misconfigured theme cannot make the grid unreadable.
const MIN_FONT_SIZE: Pixels = px(11.);
/// Zed's "standard" terminal line height: tight like a native terminal, still readable.
const LINE_HEIGHT_RATIO: f32 = 1.3;
const GRID_PADDING: Pixels = px(4.);

pub struct TerminalElement {
    view: Entity<TerminalView>,
    focused: bool,
}

impl TerminalElement {
    pub fn new(view: Entity<TerminalView>, focused: bool) -> Self {
        Self { view, focused }
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

pub struct Prepaint {
    background: Hsla,
    rects: Vec<(Bounds<Pixels>, Hsla)>,
    glyphs: Vec<(Point<Pixels>, Arc<LineLayout>, DecorationRun)>,
    cursor: Option<(Bounds<Pixels>, Hsla, CursorShape)>,
    line_height: Pixels,
}

/// Font metrics and the theme-derived palette for one frame.
struct Palette {
    fg: Hsla,
    bg: Hsla,
    cursor: Hsla,
    selection: Hsla,
    ansi: [Hsla; 16],
}

impl Palette {
    fn from_theme(cx: &App) -> Self {
        let palette = crate::ui::theme::terminal_palette(cx);
        Self {
            fg: palette.foreground,
            bg: palette.background,
            cursor: palette.cursor,
            selection: palette.selection,
            ansi: palette.ansi,
        }
    }

    fn resolve(&self, color: Color, colors: &hive_core::terminal::RenderableContent<'_>) -> Hsla {
        match color {
            Color::Spec(rgb) => to_hsla(rgb),
            Color::Named(named) => {
                if let Some(rgb) = colors.colors[named] {
                    return to_hsla(rgb);
                }
                match named {
                    NamedColor::Foreground | NamedColor::BrightForeground => self.fg,
                    NamedColor::DimForeground => self.fg.opacity(0.66),
                    NamedColor::Background => self.bg,
                    NamedColor::Cursor => self.cursor,
                    n if (n as usize) < 16 => self.ansi[n as usize],
                    n if (n as usize) >= NamedColor::DimBlack as usize
                        && (n as usize) <= NamedColor::DimWhite as usize =>
                    {
                        self.ansi[n as usize - NamedColor::DimBlack as usize].opacity(0.66)
                    }
                    _ => self.fg,
                }
            }
            Color::Indexed(i) => {
                if let Some(rgb) = colors.colors[i as usize] {
                    return to_hsla(rgb);
                }
                match i {
                    0..=15 => self.ansi[i as usize],
                    16..=231 => {
                        let i = i - 16;
                        let step = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
                        to_hsla(Rgb {
                            r: step(i / 36),
                            g: step((i / 6) % 6),
                            b: step(i % 6),
                        })
                    }
                    _ => {
                        let v = 8 + 10 * (i - 232);
                        to_hsla(Rgb { r: v, g: v, b: v })
                    }
                }
            }
        }
    }
}

fn to_hsla(rgb: Rgb) -> Hsla {
    Rgba {
        r: rgb.r as f32 / 255.,
        g: rgb.g as f32 / 255.,
        b: rgb.b as f32 / 255.,
        a: 1.,
    }
    .into()
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let theme = cx.theme();
        let font_size = theme.mono_font_size.max(MIN_FONT_SIZE);
        let base_font = Font {
            family: theme.mono_font_family.clone(),
            features: FontFeatures::default(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        };
        let palette = Palette::from_theme(cx);

        let text_system = window.text_system().clone();
        let font_id = text_system.resolve_font(&base_font);
        let cell_width = text_system
            .advance(font_id, font_size, 'm')
            .map(|s| s.width)
            .unwrap_or(font_size * 0.6);
        let line_height = (font_size * LINE_HEIGHT_RATIO).round();

        let padding = GRID_PADDING;
        let inner = Bounds {
            origin: bounds.origin + point(padding, padding),
            size: size(
                bounds.size.width - padding * 2.,
                bounds.size.height - padding * 2.,
            ),
        };
        let cols = (inner.size.width / cell_width).floor().max(2.) as u16;
        let rows = (inner.size.height / line_height).floor().max(1.) as u16;
        self.view.update(cx, |view, cx| {
            view.set_grid(
                GridGeometry {
                    bounds: inner,
                    cell: size(cell_width, line_height),
                    cols,
                    rows,
                },
                cx,
            )
        });

        let mut rects = Vec::new();
        let mut cells = Vec::new();
        let mut cursor = None;

        let view = self.view.read(cx);
        let session = view.session().read(cx);
        let terminal = session
            .terminal()
            .expect("terminal element renders only ready sessions");
        let term = terminal.term().lock();
        let mut content = term.renderable_content();
        let display_offset = content.display_offset;
        let display_iter = std::mem::replace(&mut content.display_iter, term.grid().display_iter());
        let cursor_point =
            hive_core::terminal::point_to_viewport(display_offset, content.cursor.point);
        let cursor_shape = if self.focused {
            content.cursor.shape
        } else {
            CursorShape::HollowBlock
        };
        let cursor_visible = content.cursor.shape != CursorShape::Hidden;

        for indexed in display_iter {
            let cell: &Cell = indexed.cell;
            let flags = cell.flags;
            if flags.contains(Flags::WIDE_CHAR_SPACER)
                || flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let row = (indexed.point.line.0 + display_offset as i32) as usize;
            let col = indexed.point.column.0;
            if row >= rows as usize || col >= cols as usize {
                continue;
            }
            let origin = inner.origin + point(cell_width * col as f32, line_height * row as f32);
            let width = if flags.contains(Flags::WIDE_CHAR) {
                cell_width * 2.
            } else {
                cell_width
            };
            let cell_bounds = Bounds {
                origin,
                size: size(width, line_height),
            };

            let (mut fg, mut bg) = (
                palette.resolve(cell.fg, &content),
                palette.resolve(cell.bg, &content),
            );
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if flags.intersects(Flags::DIM) {
                fg = fg.opacity(0.66);
            }
            if content.selection.is_some_and(|selection| {
                selection.contains_cell(&indexed, content.cursor.point, cursor_shape)
            }) {
                bg = bg.blend(palette.selection);
            }
            if flags.contains(Flags::HIDDEN) {
                fg = bg;
            }
            let at_cursor = cursor_visible
                && cursor_point == Some(GridPoint::new(row, hive_core::terminal::Column(col)));
            if at_cursor {
                if matches!(cursor_shape, CursorShape::Block) {
                    fg = bg;
                }
                cursor = Some((cell_bounds, palette.cursor, cursor_shape));
            }

            if bg != palette.bg {
                rects.push((cell_bounds, bg));
            }

            let Some(text) = glyph_cache::cell_text(cell) else {
                continue;
            };
            let decoration = glyph_cache::decoration(flags, fg, text.len());
            cells.push((origin, text, Variant::from(flags), decoration));
        }
        // Shaping happens after the grid is released, so the PTY reader never waits on it.
        drop(term);

        let shape = |text: &str, variant: Variant, decoration: &DecorationRun| {
            let run = TextRun {
                len: text.len(),
                font: variant.font(&base_font),
                color: decoration.color,
                background_color: None,
                underline: decoration.underline,
                strikethrough: decoration.strikethrough,
            };
            text_system.layout_line(text, font_size, &[run], None)
        };
        let glyphs = self.view.update(cx, |view, _| {
            let cache = view.glyph_cache();
            cache.begin_frame(&base_font.family, font_size);
            cells
                .into_iter()
                .map(|(origin, text, variant, decoration)| {
                    let layout = match &text {
                        CellText::Char(c) => cache.get_or_shape(*c, variant, || {
                            shape(c.encode_utf8(&mut [0; 4]), variant, &decoration)
                        }),
                        CellText::Cluster(text) => shape(text, variant, &decoration),
                    };
                    (origin, layout, decoration)
                })
                .collect()
        });

        Prepaint {
            background: palette.bg,
            rects,
            glyphs,
            cursor,
            line_height,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            // One layer per kind instead of one scene entry per quad and per cell: GPUI files
            // every layer and every layer-less primitive in its bounds tree, which cost more
            // than the rest of the terminal's paint on a busy screen.
            window.paint_layer(bounds, |window| {
                window.paint_quad(fill(bounds, prepaint.background));
                for (rect, color) in prepaint.rects.drain(..) {
                    window.paint_quad(fill(rect, color));
                }
                if let Some((rect, color, shape)) = prepaint.cursor.take() {
                    paint_cursor(rect, color, shape, window);
                }
            });
            let line_height = prepaint.line_height;
            let (plain, decorated): (Vec<_>, Vec<_>) = prepaint
                .glyphs
                .drain(..)
                .partition(|(_, _, decoration)| !glyph_cache::has_lines(decoration));
            window.paint_layer(bounds, |window| {
                for (origin, layout, decoration) in &plain {
                    paint_glyphs(*origin, layout, line_height, decoration.color, window);
                }
            });
            for (origin, layout, decoration) in decorated {
                let _ = layout.paint(
                    origin,
                    line_height,
                    TextAlign::Left,
                    None,
                    std::slice::from_ref(&decoration),
                    window,
                    cx,
                );
            }
        });
    }
}

fn paint_cursor(rect: Bounds<Pixels>, color: Hsla, shape: CursorShape, window: &mut Window) {
    match shape {
        CursorShape::Block => window.paint_quad(fill(rect, color)),
        CursorShape::HollowBlock => window.paint_quad(outline(rect, color, BorderStyle::Solid)),
        CursorShape::Underline => {
            let b = Bounds {
                origin: point(rect.origin.x, rect.bottom() - px(2.)),
                size: size(rect.size.width, px(2.)),
            };
            window.paint_quad(fill(b, color));
        }
        CursorShape::Beam => {
            let b = Bounds {
                origin: rect.origin,
                size: size(px(2.), rect.size.height),
            };
            window.paint_quad(fill(b, color));
        }
        CursorShape::Hidden => {}
    }
}

/// Paints an undecorated cell's glyphs where `LineLayout::paint` would, with the same
/// arithmetic so positions match to the bit, minus the layer that call opens per cell.
fn paint_glyphs(
    origin: Point<Pixels>,
    layout: &LineLayout,
    line_height: Pixels,
    color: Hsla,
    window: &mut Window,
) {
    let padding_top = (line_height - layout.ascent - layout.descent) / 2.;
    let baseline_offset = point(px(0.), padding_top + layout.ascent);
    let mut glyph_origin = origin;
    let mut previous = Point::default();
    for run in &layout.runs {
        for glyph in &run.glyphs {
            glyph_origin.x += glyph.position.x - previous.x;
            previous = glyph.position;
            let at = glyph_origin + baseline_offset + point(px(0.), glyph.position.y);
            let painted = if glyph.is_emoji {
                window.paint_emoji(at, run.font_id, glyph.id, layout.font_size)
            } else {
                window.paint_glyph(at, run.font_id, glyph.id, layout.font_size, color)
            };
            // Like `LineLayout::paint`, a glyph the atlas rejects ends the cell.
            if painted.is_err() {
                return;
            }
        }
    }
}
