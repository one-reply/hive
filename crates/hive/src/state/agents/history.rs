//! Past conversations of a worktree, and resuming one without a second writer.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui_kit::*;
use hive_core::agent::conversation::history::{self, Conversation};
use hive_core::agent::launch::{Launch, ResumeSession};
use hive_core::host::Host;

use super::Agents;

impl Agents {
    /// Reads the CLIs' stores in the background. Remote hosts keep no local history, and
    /// Resume is refused there anyway.
    pub(crate) fn history(
        &self,
        host: &Host,
        directory: &Path,
        cx: &App,
    ) -> Task<Vec<Conversation>> {
        if !host.is_local() {
            return Task::ready(Vec::new());
        }
        let directory = directory.to_path_buf();
        cx.background_executor()
            .spawn(async move { history::list(&directory) })
    }

    /// The live terminal of each conversation open in Hive.
    pub(crate) fn running(&self, cx: &App) -> HashMap<ResumeSession, EntityId> {
        running_conversations(self.watched.iter().filter_map(|watched| {
            let session = watched.session.upgrade()?;
            let session = session.read(cx);
            Some((watched.id, session.exited, session.resume.clone()))
        }))
    }

    /// Shows the tile that already runs `conversation`, or opens one that resumes it: two
    /// CLIs appending to one transcript would interleave it.
    pub(crate) fn resume(
        &mut self,
        host: Host,
        worktree: PathBuf,
        conversation: ResumeSession,
        cx: &mut Context<Self>,
    ) {
        match self.running(cx).get(&conversation) {
            Some(&terminal) => self.reveal(terminal, cx),
            None => self.start(host, worktree, Some(Launch::Resume(conversation)), cx),
        }
    }
}

/// The first live terminal wins when two report one conversation.
fn running_conversations(
    terminals: impl IntoIterator<Item = (EntityId, bool, Option<ResumeSession>)>,
) -> HashMap<ResumeSession, EntityId> {
    let mut running = HashMap::new();
    for (terminal, exited, conversation) in terminals {
        if let (false, Some(conversation)) = (exited, conversation) {
            running.entry(conversation).or_insert(terminal);
        }
    }
    running
}

#[cfg(test)]
mod tests {
    use super::running_conversations;
    use gpui_kit::EntityId;
    use hive_core::agent::{AgentKind, launch::ResumeSession};

    fn conversation(id: &str) -> Option<ResumeSession> {
        ResumeSession::new(AgentKind::ClaudeCode, id)
    }

    #[test]
    fn exited_terminals_and_plain_shells_run_no_conversation() {
        let running = running_conversations([
            (EntityId::from(1), true, conversation("a")),
            (EntityId::from(2), false, None),
        ]);
        assert!(running.is_empty());
    }

    #[test]
    fn first_terminal_owns_a_conversation_two_report() {
        let running = running_conversations([
            (EntityId::from(1), false, conversation("a")),
            (EntityId::from(2), false, conversation("a")),
            (EntityId::from(3), false, conversation("b")),
        ]);
        assert_eq!(
            running.get(&conversation("a").unwrap()),
            Some(&EntityId::from(1))
        );
        assert_eq!(
            running.get(&conversation("b").unwrap()),
            Some(&EntityId::from(3))
        );
    }
}
