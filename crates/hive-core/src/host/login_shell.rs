//! Text for an SSH account's login shell. sshd hands the remote command line to whatever
//! shell the account has (sh, bash, zsh, fish, csh, tcsh), and those disagree on quoting:
//! fish treats `\'` and `\\` inside single quotes as escapes, csh expands `!` inside them and
//! ends a command at a newline even inside quotes, running the rest as the next command. So
//! Hive writes every remote line as `exec sh -c <POSIX script>` in a quoting all of them read
//! the same way, and the POSIX script does the rest.

use std::{
    ffi::{OsStr, OsString},
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::Path,
};

/// Text that sh, bash, zsh, fish and csh all read as `exec sh -c <posix>`.
pub(super) fn exec_sh(posix: &OsStr) -> OsString {
    let mut line = OsString::from("exec sh -c ");
    if !posix.as_bytes().contains(&b'\n') {
        line.push(quote_for_login_shell(posix));
        return line;
    }
    // A newline cannot be quoted for csh and sh alike; `printf %b` rebuilds it inside sh.
    // Every backslash is doubled, so `\n` is the only escape `%b` finds.
    let mut escaped = Vec::with_capacity(posix.len() + 16);
    for &byte in posix.as_bytes() {
        match byte {
            b'\\' => escaped.extend_from_slice(b"\\\\"),
            b'\n' => escaped.extend_from_slice(b"\\n"),
            _ => escaped.push(byte),
        }
    }
    let mut decoder = OsString::from("eval \"$(printf %b ");
    decoder.push(quote_for_login_shell(OsStr::from_bytes(&escaped)));
    decoder.push(")\"");
    line.push(quote_for_login_shell(&decoder));
    line
}

/// POSIX text that runs the POSIX `action` with `sh` once the account's shell has read its
/// login profile, so tools resolve as in the user's terminal. csh and tcsh accept `-l` only
/// as their sole option (`-lc` is "Unknown option"), so for them `-c` reads `.cshrc` or
/// `.tcshrc` but not `.login`.
pub(super) fn in_login_environment(action: &OsStr) -> OsString {
    let mut posix = OsString::from(
        "login=-lc && case ${SHELL##*/} in *csh) login=-c ;; esac && exec \"${SHELL:-/bin/sh}\" \"$login\" ",
    );
    posix.push(quote_for_login_shell(&exec_sh(action)));
    posix
}

/// POSIX single-quoting of raw path bytes, for a script only `sh` reads.
pub(super) fn quote_path(path: &Path) -> OsString {
    let mut bytes = vec![b'\''];
    for &byte in path.as_os_str().as_bytes() {
        if byte == b'\'' {
            bytes.extend_from_slice(b"'\\''");
        } else {
            bytes.push(byte);
        }
    }
    bytes.push(b'\'');
    OsString::from_vec(bytes)
}

/// Single-quotes `value` so that sh, fish and csh read back the same bytes. A backslash, a
/// quote and `!` go outside the quotes, escaped, where all three read them as sh does.
/// The value must not contain a newline (see [`exec_sh`]).
pub(super) fn quote_for_login_shell(value: &OsStr) -> OsString {
    let mut bytes = vec![b'\''];
    for &byte in value.as_bytes() {
        match byte {
            b'\'' => bytes.extend_from_slice(b"'\\''"),
            b'\\' => bytes.extend_from_slice(b"'\\\\'"),
            b'!' => bytes.extend_from_slice(b"'\\!'"),
            _ => bytes.push(byte),
        }
    }
    bytes.push(b'\'');
    OsString::from_vec(bytes)
}

/// POSIX single-quote escaping.
pub(crate) fn quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
pub(super) mod tests;
