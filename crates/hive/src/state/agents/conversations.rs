//! Which conversation each terminal's agent runs, saved with the tile for the next launch.
use std::collections::HashMap;

use gpui_kit::*;
use hive_core::agent::{AgentKind, launch::ResumeSession};

use super::Agents;

impl Agents {
    /// Keeps each terminal's saved conversation in step with the agent the poll found in it,
    /// so a restart resumes exactly what was running and nothing that already ended.
    pub(super) fn track_conversations(
        &self,
        seen: &HashMap<EntityId, (AgentKind, Option<ResumeSession>)>,
        cx: &mut App,
    ) {
        for watched in &self.watched {
            let Some(session) = watched.session.upgrade() else {
                continue;
            };
            let (current, launched) = {
                let session = session.read(cx);
                (session.resume.clone(), session.launched_agent.is_some())
            };
            if let Some(next) = next_resume(current.as_ref(), launched, seen.get(&watched.id)) {
                session.update(cx, |session, cx| session.set_resume(next, cx));
            }
        }
    }
}

/// Only Antigravity's per-workspace cache can name one conversation for two terminals; neither
/// can tell which one owns it, so both keep what they saved before.
pub(super) fn forget_shared(conversations: &mut [&mut Option<ResumeSession>]) {
    let mut counts: HashMap<ResumeSession, usize> = HashMap::new();
    for conversation in conversations
        .iter()
        .filter_map(|conversation| conversation.as_ref())
    {
        *counts.entry(conversation.clone()).or_default() += 1;
    }
    for conversation in conversations.iter_mut() {
        if conversation
            .as_ref()
            .is_some_and(|found| counts.get(found).copied().unwrap_or_default() > 1)
        {
            **conversation = None;
        }
    }
}

/// The conversation a terminal should save after one poll; `None` keeps the current one.
/// `seen` is the foreground agent and the conversation discovered for it, if any.
fn next_resume(
    current: Option<&ResumeSession>,
    launched: bool,
    seen: Option<&(AgentKind, Option<ResumeSession>)>,
) -> Option<Option<ResumeSession>> {
    match seen {
        Some((_, Some(found))) => (current != Some(found)).then(|| Some(found.clone())),
        // A launched terminal is its agent: the launch or its hooks own the id until it exits.
        _ if launched => None,
        // The same agent has not recorded its conversation yet (Codex before a first message).
        Some((kind, None)) if current.is_some_and(|current| current.agent() == *kind) => None,
        // The shell is back in the foreground or runs another agent.
        _ => current.is_some().then_some(None),
    }
}

#[cfg(test)]
mod tests {
    // Not a glob: `gpui_kit::*` would shadow `#[test]` with GPUI's test macro.
    use super::{AgentKind, ResumeSession, forget_shared, next_resume};

    fn claude(id: &str) -> ResumeSession {
        ResumeSession::new(AgentKind::ClaudeCode, id).unwrap()
    }

    #[test]
    fn discovered_conversation_is_saved_once() {
        let found = claude("a1");
        let seen = (AgentKind::ClaudeCode, Some(found.clone()));
        assert_eq!(
            next_resume(None, false, Some(&seen)),
            Some(Some(found.clone()))
        );
        assert_eq!(next_resume(Some(&found), false, Some(&seen)), None);
        assert_eq!(
            next_resume(Some(&claude("old")), true, Some(&seen)),
            Some(Some(found))
        );
    }

    #[test]
    fn shell_back_in_the_foreground_forgets_the_conversation() {
        assert_eq!(next_resume(Some(&claude("a1")), false, None), Some(None));
        assert_eq!(next_resume(None, false, None), None);
        let codex = (AgentKind::Codex, None);
        assert_eq!(
            next_resume(Some(&claude("a1")), false, Some(&codex)),
            Some(None)
        );
    }

    #[test]
    fn undiscovered_conversation_of_the_same_agent_is_kept() {
        let seen = (AgentKind::ClaudeCode, None);
        assert_eq!(next_resume(Some(&claude("a1")), false, Some(&seen)), None);
    }

    #[test]
    fn launched_agent_keeps_the_id_its_launch_or_hooks_gave() {
        assert_eq!(next_resume(Some(&claude("a1")), true, None), None);
        let opencode = (AgentKind::OpenCode, None);
        let id = ResumeSession::new(AgentKind::OpenCode, "ses_hive1").unwrap();
        assert_eq!(next_resume(Some(&id), true, Some(&opencode)), None);
    }

    #[test]
    fn one_conversation_seen_in_two_terminals_belongs_to_neither() {
        let shared = ResumeSession::new(AgentKind::Antigravity, "a1b2").unwrap();
        let mut first = Some(shared.clone());
        let mut second = Some(shared);
        let mut own = Some(claude("c3"));
        let mut none = None;
        forget_shared(&mut [&mut first, &mut second, &mut own, &mut none]);
        assert_eq!((first, second, none), (None, None, None));
        assert_eq!(own, Some(claude("c3")));
    }
}
