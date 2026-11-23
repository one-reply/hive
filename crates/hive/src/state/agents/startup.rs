//! A Claude Code that stopped at a prompt of its own before it could say so.
//!
//! Claude Code asks whether to trust a new folder (the worktree it starts in, or one given
//! with `--add-dir`) before it loads hooks or starts the REPL that speaks OSC 7501. Neither
//! source reports anything then, and its silent terminal reads as idle, so a pipeline step
//! waited with no **Needs you** and no notification. Silence after a Hive launch with no report
//! at all is the only sign there is.
use super::*;

/// How long a Claude Code started by Hive may sit silent, having reported nothing, before it
/// counts as waiting for the user. A normal start sends its first hook within a second or two,
/// while the screen is still being drawn.
const STARTUP_PROMPT_GRACE: Duration = Duration::from_secs(5);

impl Agents {
    /// Marks each Hive-launched Claude Code still silent at a startup prompt as waiting for
    /// approval, through the same path as a reported prompt: status, notification and the
    /// pipeline's **Needs you**. Its first real report replaces the guess.
    pub(super) fn settle_starts(&mut self, now: Instant, cx: &mut Context<Self>) {
        if self.hooks.is_none() {
            return;
        }
        let stuck: Vec<EntityId> = self
            .watched
            .iter()
            .filter(|watched| {
                // A remote CLI's hooks do not reach this socket, so its silence says nothing.
                watched.probe.host.is_local()
                    && at_startup_prompt(
                        watched.probe.launched_agent,
                        watched.sighted,
                        watched.reported_at.is_some(),
                        watched.probe.activity.quiet_for(now),
                    )
            })
            .map(|watched| watched.id)
            .collect();
        for id in stuck {
            self.report_status(id, Some(AgentStatus::WaitingApproval), cx);
        }
    }
}

/// Whether the agent Hive launched in a terminal waits at a prompt it shows before it can
/// report: Hive started Claude Code with hooks, the poll still sees it there, it has never
/// reported, and it drew something and then went silent for [`STARTUP_PROMPT_GRACE`].
/// Anything reported since the launch, even later withdrawn, means it got past its start.
fn at_startup_prompt(
    launched: Option<AgentKind>,
    sighted: Option<AgentKind>,
    has_reported: bool,
    quiet_for: Option<Duration>,
) -> bool {
    let claude = Some(AgentKind::ClaudeCode);
    launched == claude
        && sighted == claude
        && !has_reported
        && quiet_for.is_some_and(|quiet| quiet >= STARTUP_PROMPT_GRACE)
}

#[cfg(test)]
mod tests {
    use super::{AgentKind, Duration, STARTUP_PROMPT_GRACE, at_startup_prompt};

    const CLAUDE: Option<AgentKind> = Some(AgentKind::ClaudeCode);

    #[test]
    fn a_silent_claude_that_never_reported_waits_at_a_startup_prompt() {
        assert!(at_startup_prompt(
            CLAUDE,
            CLAUDE,
            false,
            Some(STARTUP_PROMPT_GRACE)
        ));
    }

    #[test]
    fn a_start_still_drawing_or_not_yet_drawn_is_not_a_prompt() {
        let moment = Some(STARTUP_PROMPT_GRACE - Duration::from_millis(1));
        assert!(!at_startup_prompt(CLAUDE, CLAUDE, false, moment));
        // Nothing on screen yet: the CLI is still loading, not asking.
        assert!(!at_startup_prompt(CLAUDE, CLAUDE, false, None));
    }

    #[test]
    fn a_claude_that_reported_once_is_past_its_start() {
        assert!(!at_startup_prompt(
            CLAUDE,
            CLAUDE,
            true,
            Some(STARTUP_PROMPT_GRACE * 10)
        ));
    }

    #[test]
    fn only_a_claude_launched_by_hive_and_still_running_owes_a_report() {
        let quiet = Some(STARTUP_PROMPT_GRACE);
        // Typed into a shell by the user: Hive gave it no hooks.
        assert!(!at_startup_prompt(None, CLAUDE, false, quiet));
        // Other CLIs speak OSC 7501 or nothing; their silence is plain idle.
        let codex = Some(AgentKind::Codex);
        assert!(!at_startup_prompt(codex, codex, false, quiet));
        // Claude answered "No, exit": the shell is back in front.
        assert!(!at_startup_prompt(CLAUDE, None, false, quiet));
    }
}
