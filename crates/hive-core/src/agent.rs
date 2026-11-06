//! Coding agents running interactively in a terminal: which agent holds a PTY's foreground job.
//!
//! The terminal's shell knows nothing about agents, and the agents print no marker we could
//! rely on, so detection reads the process table: the shell's controlling terminal reports
//! its foreground process group (`tpgid`), and the command lines in that group name the agent.
//! One `ps` snapshot serves every open terminal.

pub mod conversation;
pub mod draft;
pub mod hooks;
pub mod launch;

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::command::{self, Cancellation};
use crate::host::Host;
use crate::terminal::Activity;

/// `ps` normally answers in milliseconds; a wedged system must not stall the poll forever.
const PS_TIMEOUT: Duration = Duration::from_secs(5);
/// Enough for thousands of processes with long command lines.
const PS_OUTPUT_LIMIT: usize = 8 * 1024 * 1024;

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

    /// Name of the executable the vendor installs; what `ps` shows and what PATH lookup uses.
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

    fn from_executable(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.executable() == name)
    }

    /// The agent a command line runs, if any. `ps` joins argv with spaces, so a path with a
    /// space spills into the next word; that word still ends in the executable name.
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
}

/// Interpreters an agent CLI may be launched through (`node …/bin/claude`, `python3 …/bin/aider`).
const SCRIPT_RUNNERS: [&str; 5] = ["node", "bun", "deno", "python", "python3"];

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Failure to read the process table.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// `ps` could not be started, timed out or printed more than the budget.
    #[error("failed to run ps: {0}")]
    Run(#[source] command::Error),
    /// `ps` ran and exited non-zero.
    #[error("ps exited with {code:?}: {stderr}")]
    Failed { code: Option<i32>, stderr: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Process {
    pid: u32,
    pgid: u32,
    /// Foreground process group of the process's controlling terminal; 0 or -1 without one.
    tpgid: i64,
    args: String,
}

/// A point-in-time view of local processes, enough to resolve terminal foreground jobs.
#[derive(Clone, Debug, Default)]
pub struct ProcessTable {
    processes: Vec<Process>,
}

impl ProcessTable {
    /// Reads the local process table. Blocking: run it off the UI thread.
    pub fn snapshot() -> Result<Self, AgentError> {
        let ps = Host::Local.command("/bin/ps", &["-ax", "-o", "pid=,pgid=,tpgid=,args="], None);
        let output = command::run(
            ps,
            &[],
            &Cancellation::default(),
            PS_TIMEOUT,
            PS_OUTPUT_LIMIT,
        )
        .map_err(AgentError::Run)?;
        if output.code != Some(0) {
            return Err(AgentError::Failed {
                code: output.code,
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(Self::parse(&String::from_utf8_lossy(&output.stdout)))
    }

    /// Every process id in the snapshot.
    pub(crate) fn pids(&self) -> impl Iterator<Item = u32> + '_ {
        self.processes.iter().map(|process| process.pid)
    }

    /// Parses `ps -o pid=,pgid=,tpgid=,args=` output; malformed lines are skipped.
    pub fn parse(output: &str) -> Self {
        let processes = output.lines().filter_map(parse_line).collect();
        Self { processes }
    }

    /// The agent running as the foreground job of the terminal whose shell is `shell_pid`.
    /// `None` while the shell itself is in the foreground or runs something else.
    pub fn foreground_agent(&self, shell_pid: u32) -> Option<AgentKind> {
        let shell = self.processes.iter().find(|p| p.pid == shell_pid)?;
        let foreground = Self::foreground_group(shell)?;
        if foreground == shell.pgid {
            return AgentKind::from_command(&shell.args);
        }
        // A job may start in a wrapper (a subshell, `npx`) whose child is the agent.
        self.processes
            .iter()
            .filter(|p| p.pgid == foreground)
            .find_map(|p| AgentKind::from_command(&p.args))
    }

    /// Every process of the foreground job of the terminal whose shell is `shell_pid`,
    /// including the shell itself when nothing else is in the foreground.
    pub fn foreground_pids(&self, shell_pid: u32) -> Vec<u32> {
        let Some(foreground) = self
            .processes
            .iter()
            .find(|p| p.pid == shell_pid)
            .and_then(Self::foreground_group)
        else {
            return Vec::new();
        };
        self.processes
            .iter()
            .filter(|p| p.pgid == foreground)
            .map(|p| p.pid)
            .collect()
    }

    fn foreground_group(shell: &Process) -> Option<u32> {
        u32::try_from(shell.tpgid).ok().filter(|&group| group > 0)
    }
}

fn parse_line(line: &str) -> Option<Process> {
    let mut rest = line.trim_start();
    let mut field = || {
        let end = rest.find(char::is_whitespace)?;
        let (value, tail) = rest.split_at(end);
        rest = tail.trim_start();
        Some(value)
    };
    let pid = field()?.parse().ok()?;
    let pgid = field()?.parse().ok()?;
    let tpgid = field()?.parse().ok()?;
    Some(Process {
        pid,
        pgid,
        tpgid,
        args: rest.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured on macOS with two Hive terminals (zsh 51000 running `claude`, zsh 52000 idle)
    /// plus unrelated processes, trimmed.
    const SAMPLE: &str = "\
    1     1     0 /sbin/launchd
18450 18450     0 /Users/me/.codex/packages/app-server-daemon/releases/0.166.0-aarch64-apple-darwin/bin/codex app-server --listen unix://
51000 51000 98260 -zsh
98260 98260 98260 claude
98948 98260 98260 node /Users/me/.npm/_npx/705d23756ff7dacc/node_modules/.bin/mcp-remote http://10.22.16.119:3100/mcp
52000 52000 52000 -zsh
53000 53000 53500 -zsh
53500 53500 53500 vim claude.md
54000 54000 54100 -zsh
54100 54100 54100 zsh
54101 54100 54100 /opt/homebrew/bin/codex
55000 55000 55100 -zsh
55100 55100 55100 /Users/me/Library/Application Support/Claude/claude-code/2.1.317/claude --resume
56000 56000 56100 -zsh
56100 56100 56100 /Users/me/.nvm/versions/node/v24.14.0/bin/node /Users/me/.nvm/versions/node/v24.14.0/bin/claude
69765 69765     0 /opt/homebrew/Cellar/opencode-v2/2.2.5/bin/opencode serve --service
57000 57000 57100 -zsh
57100 57100 57100 opencode
58000 58000 58100 -zsh
58100 58100 58100 /opt/homebrew/bin/python3 /Users/me/.local/bin/aider --model sonnet
59000 59000 59100 -zsh
59100 59100 59100 node /Users/me/.nvm/versions/node/v24.14.0/bin/gemini
garbage line
";

    #[test]
    fn agent_in_the_foreground_is_found() {
        let table = ProcessTable::parse(SAMPLE);
        assert_eq!(table.foreground_agent(51000), Some(AgentKind::ClaudeCode));
    }

    #[test]
    fn foreground_pids_cover_the_whole_job() {
        let table = ProcessTable::parse(SAMPLE);
        assert_eq!(table.foreground_pids(51000), [98260, 98948]);
        assert_eq!(table.foreground_pids(54000), [54100, 54101]);
        assert_eq!(table.foreground_pids(52000), [52000]);
        assert!(table.foreground_pids(1).is_empty());
        assert!(table.foreground_pids(4242).is_empty());
    }

    #[test]
    fn idle_shell_has_no_agent() {
        let table = ProcessTable::parse(SAMPLE);
        assert_eq!(table.foreground_agent(52000), None);
    }

    #[test]
    fn other_programs_and_arguments_named_like_agents_do_not_count() {
        let table = ProcessTable::parse(SAMPLE);
        assert_eq!(table.foreground_agent(53000), None);
    }

    #[test]
    fn agent_started_through_a_subshell_is_found() {
        let table = ProcessTable::parse(SAMPLE);
        assert_eq!(table.foreground_agent(54000), Some(AgentKind::Codex));
    }

    #[test]
    fn executable_path_with_a_space_is_found() {
        let table = ProcessTable::parse(SAMPLE);
        assert_eq!(table.foreground_agent(55000), Some(AgentKind::ClaudeCode));
    }

    #[test]
    fn script_run_through_node_is_found() {
        let table = ProcessTable::parse(SAMPLE);
        assert_eq!(table.foreground_agent(56000), Some(AgentKind::ClaudeCode));
    }

    #[test]
    fn opencode_tui_is_found_but_its_background_service_is_not() {
        let table = ProcessTable::parse(SAMPLE);
        assert_eq!(table.foreground_agent(57000), Some(AgentKind::OpenCode));
        assert_eq!(table.foreground_agent(69765), None);
    }

    #[test]
    fn python_and_node_launched_agents_are_found() {
        let table = ProcessTable::parse(SAMPLE);
        assert_eq!(table.foreground_agent(58000), Some(AgentKind::Aider));
        assert_eq!(table.foreground_agent(59000), Some(AgentKind::GeminiCli));
    }

    #[test]
    fn every_agent_has_a_distinct_executable() {
        let mut names: Vec<_> = AgentKind::ALL.iter().map(|k| k.executable()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), AgentKind::ALL.len());
    }

    #[test]
    fn unknown_shell_and_daemons_without_a_terminal_are_ignored() {
        let table = ProcessTable::parse(SAMPLE);
        assert_eq!(table.foreground_agent(99999), None);
        assert_eq!(table.foreground_agent(18450), None);
    }

    /// A real PTY: an executable named `codex` in the foreground is seen as Codex, and its
    /// output timing gives the status (`yes` floods output, `sleep` prints nothing).
    #[test]
    fn agent_in_a_real_terminal_is_detected_with_its_status() {
        use crate::terminal::{Terminal, TerminalSize};
        use std::time::Duration;

        let dir = std::env::temp_dir().join(format!("hive-agent-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let busy = dir.join("busy").join("codex");
        let quiet = dir.join("quiet").join("codex");
        for (link, target) in [(&busy, "/usr/bin/yes"), (&quiet, "/bin/sleep")] {
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            let _ = std::fs::remove_file(link);
            std::os::unix::fs::symlink(target, link).unwrap();
        }
        let spawn = || Terminal::spawn(None, Some("/bin/sh"), TerminalSize::new(80, 24)).unwrap();
        let (working, idle) = (spawn(), spawn());
        working.write(format!("{} > /dev/tty\n", busy.display()));
        idle.write(format!("{} 30\n", quiet.display()));

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let table = ProcessTable::snapshot().unwrap();
            let now = Instant::now();
            let seen = (
                table.foreground_agent(working.shell_pid()),
                AgentStatus::of(&working.activity(), now),
                table.foreground_agent(idle.shell_pid()),
                AgentStatus::of(&idle.activity(), now),
            );
            let expected = (
                Some(AgentKind::Codex),
                AgentStatus::Working,
                Some(AgentKind::Codex),
                AgentStatus::Idle,
            );
            if seen == expected {
                break;
            }
            assert!(Instant::now() < deadline, "last seen: {seen:?}");
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_snapshot_parses() {
        let table = ProcessTable::snapshot().expect("ps runs on macOS");
        assert!(table.processes.iter().any(|p| p.pid == 1));
    }
}
