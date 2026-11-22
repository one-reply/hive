//! GPUI keystrokes → the byte sequences a VT terminal expects (xterm conventions).

use gpui_kit::{Keystroke, Modifiers};
use hive_core::terminal::TermMode;

/// Bytes to write for `keystroke`, or `None` when the key is not sent from here: `cmd-…`
/// shortcuts belong to the app, and text belongs to the system's text input.
///
/// Text is every printable key, alone, with Shift, or with an Option that composes. Left
/// alone, it reaches the input method — which is what turns a dead key and a letter into
/// one accented character, and kana into kanji — and comes back to the terminal's input
/// handler as finished text. Writing the key's own character here would send the bare accent.
///
/// `option_as_meta` makes Option a modifier instead: the key itself goes out after ESC, as
/// xterm's `metaSendsEscape` does, and nothing is composed.
pub fn to_bytes(keystroke: &Keystroke, mode: TermMode, option_as_meta: bool) -> Option<Vec<u8>> {
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

    if option_as_meta && mods.alt {
        return meta_key(key, mods.shift).map(|key| {
            let mut bytes = vec![0x1b];
            bytes.extend_from_slice(key.as_bytes());
            bytes
        });
    }
    None
}

/// What Meta sends after ESC for `key`: the key as the layout has it without Option, in
/// upper case with Shift. Named keys other than Space have their own sequences above.
fn meta_key(key: &str, shift: bool) -> Option<String> {
    if key == "space" {
        return Some(" ".to_owned());
    }
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return None;
    };
    Some(if shift {
        c.to_uppercase().collect()
    } else {
        c.to_string()
    })
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

    /// The bytes for a key with Option composing characters, as macOS does by default.
    fn bytes(keystroke: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
        to_bytes(keystroke, mode, false)
    }

    #[test]
    fn control_keys_and_named_keys_are_sent_and_app_shortcuts_are_not() {
        assert_eq!(
            bytes(&ks("c", None, |m| m.control = true), TermMode::default()),
            Some(vec![3])
        );
        assert_eq!(
            bytes(&ks("enter", Some("\n"), |_| {}), TermMode::default()),
            Some(b"\r".to_vec())
        );
        assert_eq!(
            bytes(&ks("s", None, |m| m.platform = true), TermMode::default()),
            None
        );
    }

    #[test]
    fn a_printable_key_without_modifiers_is_left_to_the_text_input() {
        // As macOS reports them: `key_char` is the character the key types.
        for (key, key_char) in [
            ("a", "a"),
            ("space", " "),
            ("1", "1"),
            ("/", "/"),
            // A Ukrainian layout: the key is named by its Latin position.
            ("a", "ф"),
            // A dead key of a German layout: its own character is the bare accent.
            ("´", "´"),
        ] {
            for option_as_meta in [false, true] {
                assert_eq!(
                    to_bytes(
                        &ks(key, Some(key_char), |_| {}),
                        TermMode::default(),
                        option_as_meta
                    ),
                    None,
                    "{key:?} typing {key_char:?} must reach the input method"
                );
            }
        }
        // Shift picks another character; it is still text.
        assert_eq!(
            bytes(&ks("a", Some("A"), |m| m.shift = true), TermMode::default()),
            None
        );
        assert_eq!(
            bytes(&ks("!", Some("!"), |_| {}), TermMode::default()),
            None
        );
    }

    #[test]
    fn option_composes_characters_unless_it_is_meta() {
        // Option+E on a US layout is the dead key for an acute accent; Option+3 on a British
        // one types "#"; Option+B is the Meta-b a shell uses to move back a word.
        let dead_key = ks("e", Some("´"), |m| m.alt = true);
        let hash = ks("3", Some("#"), |m| m.alt = true);
        let word_back = ks("b", Some("∫"), |m| m.alt = true);
        for key in [&dead_key, &hash, &word_back] {
            assert_eq!(
                to_bytes(key, TermMode::default(), false),
                None,
                "with Option composing, {} is text",
                key.key
            );
        }
        assert_eq!(
            to_bytes(&dead_key, TermMode::default(), true),
            Some(b"\x1be".to_vec())
        );
        assert_eq!(
            to_bytes(&hash, TermMode::default(), true),
            Some(b"\x1b3".to_vec())
        );
        assert_eq!(
            to_bytes(&word_back, TermMode::default(), true),
            Some(b"\x1bb".to_vec())
        );
    }

    #[test]
    fn meta_sends_the_key_itself_with_shift_and_for_space() {
        let shifted = ks("f", Some("Ï"), |m| {
            m.alt = true;
            m.shift = true;
        });
        assert_eq!(
            to_bytes(&shifted, TermMode::default(), true),
            Some(b"\x1bF".to_vec())
        );
        // macOS folds Shift into a key that is not a letter.
        let punctuation = ks("<", Some("¯"), |m| m.alt = true);
        assert_eq!(
            to_bytes(&punctuation, TermMode::default(), true),
            Some(b"\x1b<".to_vec())
        );
        let space = ks("space", Some(" "), |m| m.alt = true);
        assert_eq!(
            to_bytes(&space, TermMode::default(), true),
            Some(b"\x1b ".to_vec())
        );
        assert_eq!(to_bytes(&space, TermMode::default(), false), None);
    }

    #[test]
    fn option_with_a_named_key_keeps_its_sequence_whatever_the_setting() {
        for option_as_meta in [false, true] {
            let send = |key: &str| {
                to_bytes(
                    &ks(key, None, |m| m.alt = true),
                    TermMode::default(),
                    option_as_meta,
                )
            };
            assert_eq!(send("enter"), Some(b"\x1b\r".to_vec()));
            assert_eq!(send("backspace"), Some(vec![0x1b, 0x7f]));
            assert_eq!(send("left"), Some(b"\x1b[1;3D".to_vec()));
            assert_eq!(
                to_bytes(
                    &ks("c", None, |m| {
                        m.alt = true;
                        m.control = true;
                    }),
                    TermMode::default(),
                    option_as_meta,
                ),
                Some(vec![0x1b, 3])
            );
        }
    }

    #[test]
    fn tui_shortcuts_preserve_their_terminal_sequences() {
        for (key, expected) in [
            ("tab", b"\t".as_slice()),
            ("shift-tab", b"\x1b[Z".as_slice()),
            ("ctrl-p", b"\x10".as_slice()),
            ("ctrl-x", b"\x18".as_slice()),
            ("ctrl-t", b"\x14".as_slice()),
        ] {
            let key = Keystroke::parse(key).expect("valid shortcut");
            assert_eq!(bytes(&key, TermMode::ALT_SCREEN).as_deref(), Some(expected));
        }
    }

    #[test]
    fn arrows_follow_cursor_mode() {
        assert_eq!(
            bytes(&ks("up", None, |_| {}), TermMode::default()),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            bytes(&ks("up", None, |_| {}), TermMode::APP_CURSOR),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            bytes(&ks("left", None, |m| m.alt = true), TermMode::default()),
            Some(b"\x1b[1;3D".to_vec())
        );
    }
}
