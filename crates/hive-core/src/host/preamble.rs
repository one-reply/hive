//! A tool's stdout behind what an account's startup files print first. A login shell reads
//! `.zprofile`, `.bash_profile` or `config.fish` before running anything, and a banner there
//! would otherwise land in a formatted buffer or break a language server's first frame.
//! [`Host::login_command`](super::Host::login_command) prints [`MARKER`] right before the
//! tool starts; readers drop everything up to it.

use std::io::{self, BufRead};

/// Printed by the script itself, after every startup file has finished.
pub const MARKER: &str = "__HIVE_TOOL_OUTPUT__";
/// Startup noise tolerated before [`MARKER`]: a banner, a warning, never megabytes.
pub const LIMIT: usize = 8192;
/// What an output budget must add on top of the tool's own output.
pub const OVERHEAD: usize = LIMIT + MARKER.len();

/// Consume `reader` up to and including [`MARKER`]. `Ok(false)` means the output ended
/// before it, for example because the shell could not start the tool. More than [`LIMIT`]
/// bytes of noise is [`io::ErrorKind::InvalidData`].
pub fn skip(reader: &mut impl BufRead) -> io::Result<bool> {
    let marker = MARKER.as_bytes();
    let mut seen = Vec::with_capacity(marker.len());
    let mut noise = 0;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(false);
        }
        let mut used = 0;
        let mut found = false;
        for &byte in buffer {
            used += 1;
            if seen.len() == marker.len() {
                seen.remove(0);
                noise += 1;
            }
            seen.push(byte);
            if noise > LIMIT {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("startup files printed more than {LIMIT} bytes before the tool"),
                ));
            }
            if seen == marker {
                found = true;
                break;
            }
        }
        reader.consume(used);
        if found {
            return Ok(true);
        }
    }
}

/// The tool's own output in a complete `stdout`, or why it cannot be told apart.
pub fn strip(stdout: &[u8]) -> io::Result<&[u8]> {
    let mut rest = stdout;
    if skip(&mut rest)? {
        Ok(rest)
    } else {
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the shell exited before starting the tool",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_noise_before_the_marker_is_dropped() {
        let mut output = b"Welcome to build-01\nLast login: Tue\n".to_vec();
        output.extend_from_slice(MARKER.as_bytes());
        output.extend_from_slice(b"formatted\n");
        assert_eq!(strip(&output).unwrap(), b"formatted\n");
        let mut bare = MARKER.as_bytes().to_vec();
        bare.extend_from_slice(b"x");
        assert_eq!(strip(&bare).unwrap(), b"x");
        assert_eq!(strip(MARKER.as_bytes()).unwrap(), b"");
    }

    #[test]
    fn only_the_first_marker_counts() {
        let mut output = MARKER.as_bytes().to_vec();
        output.extend_from_slice(MARKER.as_bytes());
        assert_eq!(strip(&output).unwrap(), MARKER.as_bytes());
    }

    #[test]
    fn a_marker_split_across_reads_is_found() {
        let mut output = b"banner".to_vec();
        output.extend_from_slice(MARKER.as_bytes());
        output.extend_from_slice(b"Content-Length: 2");
        let mut reader = io::BufReader::with_capacity(3, output.as_slice());
        assert!(skip(&mut reader).unwrap());
        let mut rest = String::new();
        io::Read::read_to_string(&mut reader, &mut rest).unwrap();
        assert_eq!(rest, "Content-Length: 2");
    }

    #[test]
    fn missing_or_late_markers_fail_boundedly() {
        assert!(!skip(&mut &b""[..]).unwrap());
        assert_eq!(
            strip(b"sh: 1: cd: can't cd to /gone\n").unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(
            strip(&MARKER.as_bytes()[1..]).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        let mut exact = vec![b'x'; LIMIT];
        exact.extend_from_slice(MARKER.as_bytes());
        assert_eq!(strip(&exact).unwrap(), b"");
        let mut late = vec![b'x'; LIMIT + 1];
        late.extend_from_slice(MARKER.as_bytes());
        assert_eq!(strip(&late).unwrap_err().kind(), io::ErrorKind::InvalidData);
        // An endless banner is rejected after the limit, not read to the end.
        let mut endless = io::BufReader::new(io::repeat(b'x'));
        assert!(skip(&mut endless).is_err());
    }
}
