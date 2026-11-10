//! Coding agents running interactively in a terminal: which agent holds a PTY's foreground job.
//!
//! The terminal's shell knows nothing about agents, and the agents print no marker we could
//! rely on, so detection reads the process table: the shell's controlling terminal reports
//! its foreground process group (`tpgid`), and the command lines in that group name the agent.
//! One process table snapshot serves every open terminal.

pub mod api;
pub mod conversation;
pub mod draft;
pub mod hooks;
pub mod launch;
mod process_table;

pub use process_table::{AgentError, ProcessTable};

use serde::{Deserialize, Serialize};
use std::time::Instant;

use crate::terminal::Activity;
use crate::terminal::program_status::{Blocker, ProgramState};

/// An agent CLI Hive recognizes in a terminal.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    OpenCode,
    GeminiCli,
    CursorAgent,
    CopilotCli,
    QwenCode,
    Amp,
    Aider,
    Droid,
    Antigravity,
}

impl AgentKind {
    /// Every recognized agent, in display order.
    pub const ALL: [Self; 11] = [
        Self::ClaudeCode,
        Self::Codex,
        Self::OpenCode,
        Self::GeminiCli,
        Self::CursorAgent,
        Self::CopilotCli,
        Self::QwenCode,
        Self::Amp,
        Self::Aider,
        Self::Droid,
        Self::Antigravity,
    ];

    /// Display name, as the vendor spells it.
    pub fn name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
            Self::GeminiCli => "Gemini CLI",
            Self::CursorAgent => "Cursor Agent",
            Self::CopilotCli => "Copilot CLI",
            Self::QwenCode => "Qwen Code",
            Self::Amp => "Amp",
            Self::Aider => "Aider",
            Self::Droid => "Droid",
            Self::Antigravity => "Antigravity",
        }
    }

    /// Name of the executable the vendor installs; what the process table shows and what PATH
    /// lookup uses.
    pub fn executable(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::GeminiCli => "gemini",
            Self::CursorAgent => "cursor-agent",
            Self::CopilotCli => "copilot",
            Self::QwenCode => "qwen",
            Self::Amp => "amp",
            Self::Aider => "aider",
            Self::Droid => "droid",
            Self::Antigravity => "agy",
        }
    }

    /// The agent whose executable is called `name`, as `hive agent start --kind` spells it.
    pub fn from_executable(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.executable() == name)
    }

    /// The agent a command line runs, if any. The process table joins argv with spaces, so a
    /// path with a space spills into the next word; that word still ends in the executable name.
    fn from_command(args: &str) -> Option<Self> {
        let mut words = args.split_whitespace();
        let first = words.next()?;
        if let Some(kind) = Self::from_executable(basename(first)) {
            return Some(kind);
        }
        let second = words.next()?;
        let runs_script = SCRIPT_RUNNERS.contains(&basename(first));
        let path_continues = first.starts_with('/') && second.contains('/');
        if runs_script || path_continues {
            return Self::from_executable(basename(second));
        }
        None
    }
}

/// What an agent in a terminal is doing, judged from the terminal's output timing: agents
/// redraw a spinner or stream text while they work and sit still at their prompt otherwise.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentStatus {
    Working,
    Idle,
    /// The agent explicitly requested user input.
    WaitingInput,
    /// The agent explicitly requested a permission decision.
    WaitingApproval,
    /// The agent finished a response; this does not certify its code or tests.
    Done,
    /// The agent reported a failed turn.
    Failed,
}

impl AgentStatus {
    /// A status deserving attention, independent of terminal output timing.
    pub fn needs_attention(self) -> bool {
        matches!(
            self,
            Self::WaitingInput | Self::WaitingApproval | Self::Done | Self::Failed
        )
    }
    /// User-facing status for an explicit event.
    pub fn label(self) -> &'static str {
        match self {
            Self::Working => "Working",
            Self::Idle => "No active output",
            Self::WaitingInput => "Needs input",
            Self::WaitingApproval => "Needs approval",
            Self::Done => "Response finished",
            Self::Failed => "Turn failed",
        }
    }

    /// The status `activity` shows at `now`.
    pub fn of(activity: &Activity, now: Instant) -> Self {
        if activity.is_streaming(now) {
            Self::Working
        } else {
            Self::Idle
        }
    }

    /// The status a program reported through OSC 7501; `None` when it withdrew its report.
    /// A blocker of unknown kind counts as a permission prompt: Hive never types into a dialog
    /// it cannot name, and Codex reports its approvals without a kind.
    pub fn from_report(state: ProgramState) -> Option<Self> {
        Some(match state {
            ProgramState::Idle => Self::Idle,
            ProgramState::Working => Self::Working,
            ProgramState::Done => Self::Done,
            ProgramState::Error => Self::Failed,
            ProgramState::Blocked(Some(Blocker::Question | Blocker::Auth)) => Self::WaitingInput,
            ProgramState::Blocked(Some(Blocker::Permission) | None) => Self::WaitingApproval,
            ProgramState::Clear => return None,
        })
    }
}

/// Interpreters an agent CLI may be launched through (`node …/bin/claude`, `python3 …/bin/aider`).
const SCRIPT_RUNNERS: [&str; 5] = ["node", "bun", "deno", "python", "python3"];

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reported_states_map_to_statuses_and_unnamed_blockers_are_approvals() {
        let status = |state| AgentStatus::from_report(state);
        assert_eq!(status(ProgramState::Working), Some(AgentStatus::Working));
        assert_eq!(status(ProgramState::Idle), Some(AgentStatus::Idle));
        assert_eq!(status(ProgramState::Done), Some(AgentStatus::Done));
        assert_eq!(status(ProgramState::Error), Some(AgentStatus::Failed));
        assert_eq!(status(ProgramState::Clear), None);
        for (blocker, expected) in [
            (Some(Blocker::Permission), AgentStatus::WaitingApproval),
            (None, AgentStatus::WaitingApproval),
            (Some(Blocker::Question), AgentStatus::WaitingInput),
            (Some(Blocker::Auth), AgentStatus::WaitingInput),
        ] {
            assert_eq!(status(ProgramState::Blocked(blocker)), Some(expected));
        }
    }

    #[test]
    fn every_agent_has_a_distinct_executable() {
        let mut names: Vec<_> = AgentKind::ALL.iter().map(|k| k.executable()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), AgentKind::ALL.len());
    }
}
