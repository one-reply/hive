//! Paints the terminal grid: cell backgrounds, glyphs (one shaped line per non-blank cell,
//! which GPUI caches), decorations and the cursor. Also measures how many cells fit in the
//! bounds and resizes the PTY to match.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use hive_core::terminal::{Cell, Color, CursorShape, Flags, NamedColor, Point as GridPoint, Rgb};

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
    glyphs: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<(Bounds<Pixels>, Hsla, CursorShape)>,
    line_height: Pixels,
}

/// Font metrics and the theme-derived palette for one frame.
struct Palette {
    fg: Hsla,
    bg: Hsla,
    cursor: Hsla,
    ansi: [Hsla; 16],
}

impl Palette {
    fn from_theme(cx: &App) -> Self {
        let theme = cx.theme();
        let hex = |v: u32| -> Hsla { rgb(v).into() };
        let ansi = if theme.is_dark() {
            [
                0x3f4451, 0xe05561, 0x8cc265, 0xd18f52, 0x4aa5f0, 0xc162de, 0x42b3c2,
                0xe6e6e6, //
                0x4f5666, 0xff616e, 0xa5e075, 0xf0a45d, 0x4dc4ff, 0xde73ff, 0x4cd1e0, 0xffffff,
            ]
        } else {
            [
                0x000000, 0xc91b00, 0x00a600, 0x999900, 0x0225c7, 0xca30c7, 0x00a6b2,
                0xbfbfbf, //
                0x686868, 0xe50000, 0x00d900, 0xe5e500, 0x0000ff, 0xe500e5, 0x00e5e5, 0xffffff,
            ]
        };
        Self {
            fg: theme.foreground,
            bg: theme.background,
            cursor: theme.foreground,
            ansi: ansi.map(hex),
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
        self.view.update(cx, |view, cx| view.resize(cols, rows, cx));

        let mut rects = Vec::new();
        let mut glyphs = Vec::new();
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

            let c = cell.c;
            if c == ' '
                && cell.zerowidth().is_none()
                && !flags.intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT)
            {
                continue;
            }
            let mut text = String::with_capacity(4);
            text.push(c);
            if let Some(zw) = cell.zerowidth() {
                text.extend(zw);
            }
            let mut font = base_font.clone();
            if flags.intersects(Flags::BOLD) {
                font.weight = FontWeight::BOLD;
            }
            if flags.intersects(Flags::ITALIC) {
                font.style = FontStyle::Italic;
            }
            let underline = flags
                .intersects(Flags::ALL_UNDERLINES)
                .then(|| UnderlineStyle {
                    thickness: px(1.),
                    color: Some(fg),
                    wavy: flags.contains(Flags::UNDERCURL),
                });
            let strikethrough = flags
                .contains(Flags::STRIKEOUT)
                .then(|| StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(fg),
                });
            let run = TextRun {
                len: text.len(),
                font,
                color: fg,
                background_color: None,
                underline,
                strikethrough,
            };
            let line = text_system.shape_line(SharedString::from(text), font_size, &[run], None);
            glyphs.push((origin, line));
        }
        drop(term);

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
            window.paint_quad(fill(bounds, prepaint.background));
            for (rect, color) in prepaint.rects.drain(..) {
                window.paint_quad(fill(rect, color));
            }
            if let Some((rect, color, shape)) = prepaint.cursor.take() {
                match shape {
                    CursorShape::Block => window.paint_quad(fill(rect, color)),
                    CursorShape::HollowBlock => {
                        window.paint_quad(outline(rect, color, BorderStyle::Solid))
                    }
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
            let line_height = prepaint.line_height;
            for (origin, line) in prepaint.glyphs.drain(..) {
                let _ = line.paint(origin, line_height, TextAlign::Left, None, window, cx);
            }
        });
    }
}
