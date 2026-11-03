//! GPUI keystrokes → the byte sequences a VT terminal expects (xterm conventions).

use gpui_kit::{Keystroke, Modifiers};
use hive_core::terminal::TermMode;

/// Bytes to write for `keystroke`, or `None` when the key is not the terminal's business
/// (e.g. `cmd-…` shortcuts the app handles).
pub fn to_bytes(keystroke: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let mods = keystroke.modifiers;
    if mods.platform {
        return None;
    }
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let key = keystroke.key.as_str();

    // Keys with dedicated sequences.
    let special: Option<Vec<u8>> = match key {
        "enter" => Some(if mods.alt {
            b"\x1b\r".to_vec()
        } else {
            b"\r".to_vec()
        }),
        "tab" => Some(if mods.shift {
            b"\x1b[Z".to_vec()
        } else {
            b"\t".to_vec()
        }),
        "escape" => Some(b"\x1b".to_vec()),
        "backspace" => Some(match (mods.control, mods.alt) {
            (true, _) => vec![0x08],
            (false, true) => vec![0x1b, 0x7f],
            _ => vec![0x7f],
        }),
        "space" if mods.control => Some(vec![0]),
        "up" => Some(cursor_key(b'A', app_cursor, mods)),
        "down" => Some(cursor_key(b'B', app_cursor, mods)),
        "right" => Some(cursor_key(b'C', app_cursor, mods)),
        "left" => Some(cursor_key(b'D', app_cursor, mods)),
        "home" => Some(cursor_key(b'H', app_cursor, mods)),
        "end" => Some(cursor_key(b'F', app_cursor, mods)),
        "insert" => Some(tilde_key(2, mods)),
        "delete" => Some(tilde_key(3, mods)),
        "pageup" => Some(tilde_key(5, mods)),
        "pagedown" => Some(tilde_key(6, mods)),
        "f1" => Some(ss3_or_csi(b'P', mods)),
        "f2" => Some(ss3_or_csi(b'Q', mods)),
        "f3" => Some(ss3_or_csi(b'R', mods)),
        "f4" => Some(ss3_or_csi(b'S', mods)),
        "f5" => Some(tilde_key(15, mods)),
        "f6" => Some(tilde_key(17, mods)),
        "f7" => Some(tilde_key(18, mods)),
        "f8" => Some(tilde_key(19, mods)),
        "f9" => Some(tilde_key(20, mods)),
        "f10" => Some(tilde_key(21, mods)),
        "f11" => Some(tilde_key(23, mods)),
        "f12" => Some(tilde_key(24, mods)),
        _ => None,
    };
    if special.is_some() {
        return special;
    }

    // ctrl-<x>: control characters.
    if mods.control {
        let mut chars = key.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return None;
        };
        let byte = match c.to_ascii_lowercase() {
            c @ 'a'..='z' => c as u8 - b'a' + 1,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '7' | '-' | '/' => 0x1f,
            '8' | '?' => 0x7f,
            '2' | '@' => 0,
            _ => return None,
        };
        return Some(if mods.alt {
            vec![0x1b, byte]
        } else {
            vec![byte]
        });
    }

    // Plain text (possibly with alt → ESC prefix, as xterm's `metaSendsEscape`).
    let text = keystroke
        .key_char
        .as_deref()
        .or(if key.chars().count() == 1 {
            Some(key)
        } else {
            None
        })?;
    let text = if key == "space" { " " } else { text };
    let mut bytes = Vec::with_capacity(text.len() + 1);
    if mods.alt && keystroke.key_char.is_none() {
        bytes.push(0x1b);
    }
    bytes.extend_from_slice(text.as_bytes());
    Some(bytes)
}

/// xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
fn modifier_param(mods: Modifiers) -> u8 {
    1 + (mods.shift as u8) + ((mods.alt as u8) << 1) + ((mods.control as u8) << 2)
}

fn cursor_key(final_byte: u8, app_cursor: bool, mods: Modifiers) -> Vec<u8> {
    let m = modifier_param(mods);
    if m > 1 {
        format!("\x1b[1;{m}{}", final_byte as char).into_bytes()
    } else if app_cursor {
        vec![0x1b, b'O', final_byte]
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

fn tilde_key(code: u8, mods: Modifiers) -> Vec<u8> {
    let m = modifier_param(mods);
    if m > 1 {
        format!("\x1b[{code};{m}~").into_bytes()
    } else {
        format!("\x1b[{code}~").into_bytes()
    }
}

fn ss3_or_csi(final_byte: u8, mods: Modifiers) -> Vec<u8> {
    let m = modifier_param(mods);
    if m > 1 {
        format!("\x1b[1;{m}{}", final_byte as char).into_bytes()
    } else {
        vec![0x1b, b'O', final_byte]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ks(key: &str, key_char: Option<&str>, f: impl FnOnce(&mut Modifiers)) -> Keystroke {
        let mut modifiers = Modifiers::default();
        f(&mut modifiers);
        Keystroke {
            modifiers,
            key: key.into(),
            key_char: key_char.map(Into::into),
        }
    }

    #[test]
    fn plain_and_control() {
        assert_eq!(
            to_bytes(&ks("a", Some("a"), |_| {}), TermMode::default()),
            Some(b"a".to_vec())
        );
        assert_eq!(
            to_bytes(&ks("c", None, |m| m.control = true), TermMode::default()),
            Some(vec![3])
        );
        assert_eq!(
            to_bytes(&ks("enter", None, |_| {}), TermMode::default()),
            Some(b"\r".to_vec())
        );
        assert_eq!(
            to_bytes(&ks("s", None, |m| m.platform = true), TermMode::default()),
            None
        );
    }

    #[test]
    fn arrows_follow_cursor_mode() {
        assert_eq!(
            to_bytes(&ks("up", None, |_| {}), TermMode::default()),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            to_bytes(&ks("up", None, |_| {}), TermMode::APP_CURSOR),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            to_bytes(&ks("left", None, |m| m.alt = true), TermMode::default()),
            Some(b"\x1b[1;3D".to_vec())
        );
    }
}
