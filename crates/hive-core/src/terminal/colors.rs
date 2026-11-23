//! The colors a program reads back with OSC 4, 10, 11 and 12: Codex and other CLIs ask for
//! the background to pick a light or dark look of their own.
use alacritty_terminal::vte::ansi::{NamedColor, Rgb};
use std::sync::{PoisonError, RwLock};

/// The colors Hive draws when a program has set none of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// Default text color.
    pub foreground: Rgb,
    /// Default background color.
    pub background: Rgb,
    /// Cursor color.
    pub cursor: Rgb,
    /// The 16 named ANSI colors, normal then bright.
    pub ansi: [Rgb; 16],
}

/// One theme serves the whole app, so one palette serves every terminal, also those of
/// pipeline steps that no window shows. `None` until the app applies its theme.
static PALETTE: RwLock<Option<Palette>> = RwLock::new(None);

/// Makes `palette` what every terminal reports from now on, also those already running.
pub fn set_palette(palette: Palette) {
    *PALETTE.write().unwrap_or_else(PoisonError::into_inner) = Some(palette);
}

/// The color a program asking for alacritty color `index` is told, `None` before the app has
/// a theme or for an index no query names.
// ponytail: a color the program set itself with OSC 4/10/11 is drawn but not reported, because
// the request arrives while the event loop holds the grid; answer from the UI thread if a
// program is found that reads back its own color.
pub(super) fn reported(index: usize) -> Option<Rgb> {
    PALETTE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .and_then(|palette| palette.color(index))
}

impl Palette {
    fn color(&self, index: usize) -> Option<Rgb> {
        match index {
            0..=15 => Some(self.ansi[index]),
            16..=255 => u8::try_from(index).ok().and_then(indexed),
            i if i == NamedColor::Foreground as usize => Some(self.foreground),
            i if i == NamedColor::Background as usize => Some(self.background),
            i if i == NamedColor::Cursor as usize => Some(self.cursor),
            _ => None,
        }
    }
}

/// xterm's colors 16–255, which no theme changes: a 6×6×6 cube, then 24 grays. `None` for the
/// 16 named colors, which come from the theme.
pub fn indexed(index: u8) -> Option<Rgb> {
    match index {
        0..=15 => None,
        16..=231 => {
            let cube = index - 16;
            let step = |level: u8| if level == 0 { 0 } else { 55 + 40 * level };
            Some(Rgb {
                r: step(cube / 36),
                g: step((cube / 6) % 6),
                b: step(cube % 6),
            })
        }
        _ => {
            let level = 8 + 10 * (index - 232);
            Some(Rgb {
                r: level,
                g: level,
                b: level,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    #[test]
    fn a_query_names_a_theme_color_or_one_of_xterms_fixed_256() {
        let palette = Palette {
            foreground: rgb(1, 2, 3),
            background: rgb(4, 5, 6),
            cursor: rgb(7, 8, 9),
            ansi: std::array::from_fn(|i| rgb(i as u8, 0, 0)),
        };
        assert_eq!(palette.color(256), Some(rgb(1, 2, 3)));
        assert_eq!(palette.color(257), Some(rgb(4, 5, 6)));
        assert_eq!(palette.color(258), Some(rgb(7, 8, 9)));
        assert_eq!(palette.color(9), Some(rgb(9, 0, 0)));
        assert_eq!(palette.color(16), Some(rgb(0, 0, 0)));
        assert_eq!(palette.color(196), Some(rgb(255, 0, 0)));
        assert_eq!(palette.color(231), Some(rgb(255, 255, 255)));
        assert_eq!(palette.color(232), Some(rgb(8, 8, 8)));
        assert_eq!(palette.color(255), Some(rgb(238, 238, 238)));
        // Dim and bright foregrounds have no query of their own.
        assert_eq!(palette.color(NamedColor::DimForeground as usize), None);
        assert_eq!(indexed(15), None);
    }
}
