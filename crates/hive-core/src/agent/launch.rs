//! Explicit local CLI launches and persisted conversation identities.
use super::AgentKind;
use crate::{command, host::Host};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// Interactive CLIs Hive can launch explicitly.
pub const AGENTS: [AgentKind; 3] = [AgentKind::Codex, AgentKind::ClaudeCode, AgentKind::OpenCode];

/// A supported conversation the user can explicitly resume.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeSession {
    agent: AgentKind,
    id: String,
}
impl ResumeSession {
    /// Capture a validated conversation identifier from an agent event.
    pub fn new(agent: AgentKind, id: &str) -> Option<Self> {
        (matches!(agent, AgentKind::ClaudeCode | AgentKind::Codex) && valid_session_id(id)).then(
            || Self {
                agent,
                id: id.into(),
            },
        )
    }
    /// Agent that owns this conversation.
    pub fn agent(&self) -> AgentKind {
        self.agent
    }
    /// Provider-issued conversation identifier.
    pub fn id(&self) -> &str {
        &self.id
    }
}

pub(crate) fn deserialize_saved<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ResumeSession>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(None);
    };
    let parsed = serde_json::from_value::<ResumeSession>(value)
        .ok()
        .and_then(|session| ResumeSession::new(session.agent, &session.id));
    if parsed.is_none() {
        tracing::warn!("ignoring invalid or unsupported saved agent session");
    }
    Ok(parsed)
}

pub(super) fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A new interactive agent or an explicit resume, always in a new PTY.
#[derive(Clone, Debug)]
pub enum Launch {
    /// Start a new conversation using the CLI's existing permissions and login.
    New(AgentKind),
    /// Resume exactly this conversation, never the ambiguous last session.
    Resume(ResumeSession),
}
impl Launch {
    /// Selected CLI provider.
    pub fn agent(&self) -> AgentKind {
        match self {
            Self::New(agent) => *agent,
            Self::Resume(session) => session.agent,
        }
    }
    /// Blocking: discover the binary before creating a worktree or starting a terminal.
    pub fn executable(&self) -> Result<PathBuf, LaunchError> {
        let agent = self.agent();
        if !AGENTS.contains(&agent) {
            return Err(LaunchError::Unsupported);
        }
        command::executable(agent.executable()).ok_or(LaunchError::Missing(agent.name()))
    }
    /// Blocking: build a direct CLI invocation with optional session-scoped Claude hooks.
    pub fn command(
        &self,
        directory: &Path,
        hook_settings: Option<&str>,
    ) -> Result<Command, LaunchError> {
        let executable = self.executable()?;
        let args = self.arguments(hook_settings)?;
        let mut command = Host::Local.command(
            &executable.to_string_lossy(),
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            Some(directory),
        );
        if let Some(parent) = executable.parent() {
            let mut paths = vec![parent.to_owned()];
            if let Some(path) = std::env::var_os("PATH") {
                paths.extend(std::env::split_paths(&path));
            }
            if let Ok(path) = std::env::join_paths(paths) {
                command.env("PATH", path);
            }
        }
        Ok(command)
    }
    fn arguments(&self, hook_settings: Option<&str>) -> Result<Vec<String>, LaunchError> {
        let mut args = Vec::new();
        if let Self::Resume(session) = self {
            if !valid_session_id(&session.id) {
                return Err(LaunchError::InvalidSession);
            }
            args.push(
                if session.agent == AgentKind::ClaudeCode {
                    "--resume"
                } else {
                    "resume"
                }
                .into(),
            );
            args.push(session.id.clone());
        }
        if self.agent() == AgentKind::ClaudeCode
            && let Some(settings) = hook_settings
        {
            args.extend(["--settings".into(), settings.into()]);
        }
        Ok(args)
    }
}

/// A launch that could not be prepared; no repository is changed by discovery.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    /// The selected CLI is not installed in a supported executable location.
    #[error("{0} CLI not found. Install it, then try again.")]
    Missing(&'static str),
    /// This agent does not support explicit interactive launches.
    #[error("Choose Claude Code, Codex, or OpenCode.")]
    Unsupported,
    /// An invalid saved session must never become CLI flags.
    #[error("The saved agent session id is invalid. Start a new session.")]
    InvalidSession,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_agents_start_interactively_without_submitting_a_prompt() {
        for agent in AGENTS {
            assert!(Launch::New(agent).arguments(None).unwrap().is_empty());
        }
        assert_eq!(
            Launch::New(AgentKind::OpenCode)
                .arguments(Some("{}"))
                .unwrap(),
            Vec::<String>::new()
        );
    }
    #[test]
    fn resume_is_exact_and_does_not_change_permissions() {
        let session = ResumeSession::new(AgentKind::ClaudeCode, "a1-b2").unwrap();
        assert_eq!(
            Launch::Resume(session).arguments(Some("{}")).unwrap(),
            ["--resume", "a1-b2", "--settings", "{}"]
        );
        assert!(
            ResumeSession::new(AgentKind::ClaudeCode, "--dangerously-skip-permissions").is_none()
        );
        assert!(ResumeSession::new(AgentKind::Codex, "$(touch /tmp/no)").is_none());
    }
}
