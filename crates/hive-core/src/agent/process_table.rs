//! The local process table, reduced to what resolves a terminal's foreground job.
mod kernel;

use super::AgentKind;

/// Failure to read the process table.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// The kernel refused the process list or returned one that cannot be read.
    #[error("failed to read the process table: {0}")]
    ProcessList(#[source] std::io::Error),
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
        let processes = kernel::processes().map_err(AgentError::ProcessList)?;
        Ok(Self { processes })
    }

    /// Every process id in the snapshot.
    pub(crate) fn pids(&self) -> impl Iterator<Item = u32> + '_ {
        self.processes.iter().map(|process| process.pid)
    }

    /// A table from `ps -o pid=,pgid=,tpgid=,args=` text, the form captured samples come in;
    /// malformed lines are skipped.
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
    use crate::agent::AgentStatus;
    use std::time::Instant;

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
        let table = ProcessTable::snapshot().expect("the kernel lists processes");
        assert!(table.processes.iter().any(|p| p.pid == 1));
    }
}
