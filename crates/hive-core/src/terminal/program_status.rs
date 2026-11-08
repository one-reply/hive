//! The Program Status Protocol (OSC 7501): a program tells its terminal that it is working,
//! done or waiting on the user, so Hive need not guess from output timing. Claude Code, Codex
//! and OpenCode emit it; Claude Code and OpenCode only after the terminal answers the support
//! query. Spec revision 0.4: <https://www.superlogical.com/rex/docs/build/program-status>.
//!
//! alacritty's parser drops unknown OSC sequences before its handler sees them, so the PTY
//! reader runs every chunk through a [`Scanner`] before the parser gets it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const INTRODUCER: &[u8] = b"\x1b]7501;";
/// Reports are a few dozen bytes; a longer one is noise or abuse, never something to buffer.
const MAX_PAYLOAD: usize = 4096;
/// Reports the app has not collected yet. The app drains every tick, so this only bounds a
/// program that floods reports while nobody looks.
const BACKLOG: usize = 64;

/// The answer to the support query: Hive understands every state and blocker kind.
pub(crate) const QUERY_REPLY: &[u8] =
    b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";

/// What a program reported about itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramState {
    /// At rest, waiting for the next instruction.
    Idle,
    /// Running.
    Working,
    /// Finished a piece of work the user has not looked at yet.
    Done,
    /// Cannot continue until the user acts; `None` when the program did not say why.
    Blocked(Option<Blocker>),
    /// Failed and stopped.
    Error,
    /// The program withdrew its status.
    Clear,
}

/// Why a program is blocked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocker {
    Permission,
    Question,
    Auth,
}

/// One complete OSC 7501 sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    Report(ProgramState),
    /// `OSC 7501 ; ?`: the program asks whether the terminal understands reports.
    Query,
}

/// Parses the payload between `OSC 7501 ;` and the terminator. Unknown keys and blocker kinds
/// are ignored so the protocol can grow; a report without a known state is dropped whole.
/// Only the root record counts: a report with an `id` describes a sub-task, such as one of
/// Claude Code's background agents, not the program in the pane.
pub(crate) fn parse(payload: &[u8]) -> Option<Signal> {
    let text = std::str::from_utf8(payload).ok()?;
    if text == "?" || text.starts_with("?:") {
        return Some(Signal::Query);
    }
    let (mut state, mut kind) = (None, None);
    for (key, value) in text.split(':').filter_map(|pair| pair.split_once('=')) {
        // A repeated key: the last value wins.
        match key {
            "state" => state = Some(value),
            "kind" => kind = Some(value),
            "id" => return None,
            _ => {}
        }
    }
    let state = match state? {
        "idle" => ProgramState::Idle,
        "working" => ProgramState::Working,
        "done" => ProgramState::Done,
        "error" => ProgramState::Error,
        "clear" => ProgramState::Clear,
        "blocked" => ProgramState::Blocked(match kind {
            Some("permission") => Some(Blocker::Permission),
            Some("question") => Some(Blocker::Question),
            Some("auth") => Some(Blocker::Auth),
            _ => None,
        }),
        _ => return None,
    };
    Some(Signal::Report(state))
}

/// Finds OSC 7501 sequences in PTY output without consuming it. Fed chunk by chunk, so a
/// sequence split across reads is still found.
#[derive(Debug, Default)]
pub(crate) struct Scanner {
    /// Bytes of [`INTRODUCER`] matched so far; its full length while inside a payload.
    matched: usize,
    payload: Vec<u8>,
    /// The previous payload byte was ESC: `\` ends the sequence, anything else aborts it.
    escape: bool,
}

impl Scanner {
    pub(crate) fn feed(&mut self, bytes: &[u8], mut on_signal: impl FnMut(Signal)) {
        for &byte in bytes {
            if self.matched < INTRODUCER.len() {
                self.match_introducer(byte);
            } else if let Some(signal) = self.payload_byte(byte) {
                on_signal(signal);
            }
        }
    }

    fn match_introducer(&mut self, byte: u8) {
        self.matched = if byte == INTRODUCER[self.matched] {
            self.matched + 1
        } else if byte == ESC {
            1
        } else {
            0
        };
    }

    fn payload_byte(&mut self, byte: u8) -> Option<Signal> {
        let terminated = match (self.escape, byte) {
            (false, BEL) | (true, b'\\') => true,
            (false, ESC) => {
                self.escape = true;
                return None;
            }
            (true, _) => {
                // The ESC started another sequence; it may be the next report.
                self.reset();
                self.match_introducer(ESC);
                self.match_introducer(byte);
                return None;
            }
            (false, byte) if byte < 0x20 || self.payload.len() == MAX_PAYLOAD => {
                self.reset();
                return None;
            }
            (false, _) => false,
        };
        if !terminated {
            self.payload.push(byte);
            return None;
        }
        let signal = parse(&self.payload);
        self.reset();
        signal
    }

    fn reset(&mut self) {
        self.matched = 0;
        self.payload.clear();
        self.escape = false;
    }
}

/// Reports from one terminal's program, queued by the PTY reader until the app collects them.
/// A queue rather than the latest state: a turn shorter than the app's tick must still be
/// seen as working.
#[derive(Clone, Debug, Default)]
pub struct ProgramStatus(Arc<Mutex<VecDeque<ProgramState>>>);

impl ProgramStatus {
    pub(crate) fn push(&self, state: ProgramState) {
        let mut queue = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if queue.len() == BACKLOG {
            queue.pop_front();
        }
        queue.push_back(state);
    }

    /// Reports since the last call, oldest first.
    pub fn drain(&self) -> Vec<ProgramState> {
        let mut queue = self.0.lock().unwrap_or_else(|e| e.into_inner());
        queue.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<Signal> {
        let mut scanner = Scanner::default();
        let mut signals = Vec::new();
        for chunk in chunks {
            scanner.feed(chunk, |signal| signals.push(signal));
        }
        signals
    }

    fn report(payload: &str) -> Option<ProgramState> {
        match parse(payload.as_bytes()) {
            Some(Signal::Report(state)) => Some(state),
            _ => None,
        }
    }

    #[test]
    fn parses_every_state_and_blocker_kind() {
        assert_eq!(report("state=idle"), Some(ProgramState::Idle));
        assert_eq!(
            report("state=working:app=codex"),
            Some(ProgramState::Working)
        );
        assert_eq!(report("state=done"), Some(ProgramState::Done));
        assert_eq!(report("state=error"), Some(ProgramState::Error));
        assert_eq!(report("state=clear"), Some(ProgramState::Clear));
        assert_eq!(
            report("state=blocked:kind=permission"),
            Some(ProgramState::Blocked(Some(Blocker::Permission)))
        );
        assert_eq!(
            report("kind=question:state=blocked"),
            Some(ProgramState::Blocked(Some(Blocker::Question)))
        );
        assert_eq!(
            report("state=blocked:kind=auth"),
            Some(ProgramState::Blocked(Some(Blocker::Auth)))
        );
    }

    #[test]
    fn unknown_keys_and_kinds_are_ignored_but_unknown_states_drop_the_report() {
        assert_eq!(
            report("state=blocked:kind=dance:progress=40:x"),
            Some(ProgramState::Blocked(None))
        );
        assert_eq!(report("state=working:state=done"), Some(ProgramState::Done));
        assert_eq!(report("state=sleeping"), None);
        assert_eq!(report("app=codex"), None);
        assert_eq!(report(""), None);
        assert_eq!(parse(b"state=\xff"), None);
    }

    #[test]
    fn sub_task_records_do_not_describe_the_program() {
        assert_eq!(report("state=working:app=claude-code:id=task-1"), None);
        assert_eq!(report("state=clear:id=task-1"), None);
        assert_eq!(
            report("state=idle:app=claude-code"),
            Some(ProgramState::Idle)
        );
    }

    #[test]
    fn the_support_query_is_recognized() {
        assert_eq!(parse(b"?"), Some(Signal::Query));
        assert_eq!(scan(&[b"\x1b]7501;?\x1b\\"]), vec![Signal::Query]);
    }

    #[test]
    fn reports_end_with_bel_or_string_terminator() {
        assert_eq!(
            scan(&[b"text\x1b]7501;state=working\x07more\x1b]7501;state=idle\x1b\\"]),
            vec![
                Signal::Report(ProgramState::Working),
                Signal::Report(ProgramState::Idle)
            ]
        );
    }

    #[test]
    fn a_report_split_across_reads_is_found() {
        let bytes = b"\x1b]7501;state=blocked:kind=permission\x1b\\";
        let chunks: Vec<&[u8]> = bytes.chunks(1).collect();
        assert_eq!(
            scan(&chunks),
            vec![Signal::Report(ProgramState::Blocked(Some(
                Blocker::Permission
            )))]
        );
    }

    #[test]
    fn other_sequences_and_plain_text_yield_nothing() {
        assert!(scan(&[b"\x1b]2;title\x07\x1b]133;A\x07\x1b]750;state=idle\x07"]).is_empty());
        assert!(scan(&[b"7501;state=idle\x07 ]7501;state=idle\x07"]).is_empty());
    }

    #[test]
    fn an_interrupted_or_oversized_payload_is_dropped_and_scanning_resumes() {
        let mut oversized = b"\x1b]7501;state=idle:".to_vec();
        oversized.extend(std::iter::repeat_n(b'a', MAX_PAYLOAD));
        oversized.extend(b"\x07\x1b]7501;state=done\x07");
        assert_eq!(
            scan(&[&oversized]),
            vec![Signal::Report(ProgramState::Done)]
        );
        assert_eq!(
            scan(&[b"\x1b]7501;state=idle\n\x1b]7501;state=working\x1bx\x1b]7501;state=done\x07"]),
            vec![Signal::Report(ProgramState::Done)]
        );
        assert_eq!(
            scan(&[b"\x1b]7501;state=idle\x1b\x1b]7501;state=working\x07"]),
            vec![Signal::Report(ProgramState::Working)]
        );
    }

    #[test]
    fn collected_reports_come_out_once_in_order_and_the_backlog_is_bounded() {
        let status = ProgramStatus::default();
        status.push(ProgramState::Working);
        status.push(ProgramState::Idle);
        assert_eq!(
            status.drain(),
            vec![ProgramState::Working, ProgramState::Idle]
        );
        assert!(status.drain().is_empty());
        for _ in 0..BACKLOG {
            status.push(ProgramState::Working);
        }
        status.push(ProgramState::Done);
        let drained = status.drain();
        assert_eq!(drained.len(), BACKLOG);
        assert_eq!(drained.last(), Some(&ProgramState::Done));
    }
}
