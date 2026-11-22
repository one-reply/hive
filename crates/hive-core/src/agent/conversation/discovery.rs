//! How often the agents' files are read for the conversation a terminal runs. The poll runs
//! every 800 ms for every agent, and finding a Codex thread walks every rollout ever written.

use crate::agent::{AgentKind, launch::ResumeSession};
use std::time::{Duration, Instant};

/// How long a lookup stands for the same job. A conversation can change without a new
/// process (`/new` or `/resume` in Codex, `/clear` in Claude Code), so it is looked up again.
const REDISCOVER: Duration = Duration::from_secs(5);

/// What [`super::discover`] found for one terminal's foreground job, and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovery {
    kind: AgentKind,
    /// The job's process group: its member processes come and go as the agent runs tools.
    job: u32,
    at: Instant,
    found: Option<ResumeSession>,
}

impl Discovery {
    /// Blocking: `lookup` (normally [`super::discover`]) for the agent `kind` running as the
    /// process group `job`, unless `previous` looked at the same job less than five seconds
    /// before `now`.
    pub fn refresh(
        previous: Option<&Self>,
        kind: AgentKind,
        job: u32,
        now: Instant,
        lookup: impl FnOnce() -> Option<ResumeSession>,
    ) -> Self {
        if let Some(previous) = previous.filter(|previous| {
            previous.kind == kind
                && previous.job == job
                && now.saturating_duration_since(previous.at) < REDISCOVER
        }) {
            return previous.clone();
        }
        Self {
            kind,
            job,
            at: now,
            found: lookup(),
        }
    }

    /// The conversation found, once the CLI has recorded one.
    pub fn found(&self) -> Option<&ResumeSession> {
        self.found.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn the_same_job_is_looked_up_again_only_after_five_seconds() {
        let lookups = Cell::new(0);
        let lookup = || {
            lookups.set(lookups.get() + 1);
            ResumeSession::new(AgentKind::Codex, "01a1b4e5-cce0-7f72-ac75-8c15a2f6678e")
        };
        let start = Instant::now();
        let first = Discovery::refresh(None, AgentKind::Codex, 700, start, lookup);
        assert_eq!(lookups.get(), 1);
        assert_eq!(
            first.found().map(ResumeSession::id),
            Some("01a1b4e5-cce0-7f72-ac75-8c15a2f6678e")
        );
        let mut latest = first.clone();
        for tick in 1..6 {
            let now = start + Duration::from_millis(800 * tick);
            latest = Discovery::refresh(Some(&latest), AgentKind::Codex, 700, now, lookup);
        }
        assert_eq!(
            lookups.get(),
            1,
            "six polls within five seconds read the files once"
        );
        assert_eq!(latest, first);
        Discovery::refresh(
            Some(&latest),
            AgentKind::Codex,
            700,
            start + REDISCOVER,
            lookup,
        );
        assert_eq!(
            lookups.get(),
            2,
            "a thread switched inside the CLI is found again"
        );
    }

    #[test]
    fn a_new_job_or_agent_is_looked_up_at_once_and_misses_are_retried_on_schedule() {
        let lookups = Cell::new(0);
        let missing = || {
            lookups.set(lookups.get() + 1);
            None
        };
        let start = Instant::now();
        let previous = Discovery::refresh(None, AgentKind::Codex, 700, start, missing);
        let soon = start + Duration::from_millis(800);
        Discovery::refresh(Some(&previous), AgentKind::Codex, 701, soon, missing);
        Discovery::refresh(Some(&previous), AgentKind::ClaudeCode, 700, soon, missing);
        assert_eq!(lookups.get(), 3);
        let unchanged = Discovery::refresh(Some(&previous), AgentKind::Codex, 700, soon, missing);
        assert_eq!(
            lookups.get(),
            3,
            "nothing recorded yet is not retried every poll"
        );
        assert!(unchanged.found().is_none());
    }
}
