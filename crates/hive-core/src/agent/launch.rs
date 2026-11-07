//! Explicit local CLI launches and persisted conversation identities.
use super::AgentKind;
use crate::{command, host::Host};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

/// Interactive CLIs Hive can launch explicitly.
pub const AGENTS: [AgentKind; 4] = [
    AgentKind::Codex,
    AgentKind::ClaudeCode,
    AgentKind::OpenCode,
    AgentKind::Antigravity,
];

/// OpenCode rejects `--session` values without this prefix.
const OPENCODE_SESSION_PREFIX: &str = "ses_";
static NEXT_OPENCODE_SESSION: AtomicU64 = AtomicU64::new(0);

/// A supported conversation the user can explicitly resume.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResumeSession {
    agent: AgentKind,
    id: String,
}
impl ResumeSession {
    /// Capture a validated conversation identifier from an agent event.
    pub fn new(agent: AgentKind, id: &str) -> Option<Self> {
        let supported = match agent {
            AgentKind::ClaudeCode | AgentKind::Codex | AgentKind::Antigravity => true,
            AgentKind::OpenCode => id.starts_with(OPENCODE_SESSION_PREFIX),
            _ => false,
        };
        (supported && valid_session_id(id)).then(|| Self {
            agent,
            id: id.into(),
        })
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

/// A fresh id for `opencode --session`, which creates the session when it does not exist.
/// Unique per process by the counter and across processes by the pid and clock.
pub(crate) fn new_opencode_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!(
        "{OPENCODE_SESSION_PREFIX}hive{nanos:x}{:x}{:x}",
        std::process::id(),
        NEXT_OPENCODE_SESSION.fetch_add(1, Ordering::Relaxed)
    )
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
    /// Check the CLI on the selected host, without starting a conversation.
    pub fn check_executable(&self, host: &Host) -> Result<(), LaunchError> {
        if host.is_local() {
            return self.executable().map(|_| ());
        }
        if !AGENTS.contains(&self.agent()) {
            return Err(LaunchError::Unsupported);
        }
        if matches!(self, Self::Resume(_)) {
            return Err(LaunchError::Remote(
                "Remote Resume is not supported; start a new session".into(),
            ));
        }
        host.check_executable(self.agent().executable())
            .map_err(LaunchError::Remote)
    }

    /// Build a remote interactive launch without local hooks, tokens or session identities.
    pub fn remote_command(&self, host: &Host, directory: &Path) -> Result<Command, LaunchError> {
        self.check_executable(host)?;
        let args = self.arguments(None)?;
        host.terminal_command(
            directory,
            Some((
                self.agent().executable(),
                &args.iter().map(String::as_str).collect::<Vec<_>>(),
            )),
        )
        .map_err(|e| LaunchError::Remote(e.to_string()))
    }
    /// Selected CLI provider.
    pub fn agent(&self) -> AgentKind {
        match self {
            Self::New(agent) => *agent,
            Self::Resume(session) => session.agent,
        }
    }
    /// Gives a new OpenCode conversation its id up front, so it is resumable from the first
    /// moment: OpenCode reports no session events Hive could capture it from later.
    pub fn with_known_session(self) -> Self {
        match self {
            Self::New(AgentKind::OpenCode) => Self::Resume(ResumeSession {
                agent: AgentKind::OpenCode,
                id: new_opencode_session_id(),
            }),
            launch => launch,
        }
    }
    /// The conversation this launch continues, when its id is known before it starts.
    pub fn session(&self) -> Option<&ResumeSession> {
        match self {
            Self::New(_) => None,
            Self::Resume(session) => Some(session),
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
            if let Some(path) = crate::host::environment::path() {
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
                match session.agent {
                    AgentKind::ClaudeCode => "--resume",
                    AgentKind::OpenCode => "--session",
                    AgentKind::Antigravity => "--conversation",
                    _ => "resume",
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
    /// SSH preflight or remote command preparation failed.
    #[error("{0}")]
    Remote(String),
    /// The selected CLI is not installed in a supported executable location.
    #[error("{0} CLI not found. Install it, then try again.")]
    Missing(&'static str),
    /// This agent does not support explicit interactive launches.
    #[error("Choose Claude Code, Codex, OpenCode, or Antigravity.")]
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
    #[test]
    fn antigravity_resumes_its_conversation_by_id() {
        let session = ResumeSession::new(
            AgentKind::Antigravity,
            "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
        )
        .unwrap();
        assert_eq!(
            Launch::Resume(session).arguments(Some("{}")).unwrap(),
            ["--conversation", "a1b2c3d4-e5f6-7890-abcd-ef1234567890"]
        );
        assert!(ResumeSession::new(AgentKind::Antigravity, "-c").is_none());
    }
    #[test]
    fn new_opencode_launch_owns_a_resumable_session_id() {
        let launch = Launch::New(AgentKind::OpenCode).with_known_session();
        let session = launch
            .session()
            .expect("OpenCode gets its id before start")
            .clone();
        assert_eq!(session.agent(), AgentKind::OpenCode);
        assert_eq!(
            ResumeSession::new(AgentKind::OpenCode, session.id()),
            Some(session.clone())
        );
        assert_eq!(
            launch.arguments(Some("{}")).unwrap(),
            ["--session", session.id()]
        );
        let other = Launch::New(AgentKind::OpenCode).with_known_session();
        assert_ne!(other.session(), Some(&session));
    }
    #[test]
    fn other_new_launches_keep_their_unknown_session() {
        for agent in [
            AgentKind::ClaudeCode,
            AgentKind::Codex,
            AgentKind::Antigravity,
        ] {
            assert!(Launch::New(agent).with_known_session().session().is_none());
        }
    }
    #[test]
    fn opencode_ids_without_its_prefix_are_rejected() {
        assert!(ResumeSession::new(AgentKind::OpenCode, "a1-b2").is_none());
        assert!(ResumeSession::new(AgentKind::OpenCode, "ses_--x;").is_none());
        assert!(ResumeSession::new(AgentKind::GeminiCli, "ses_abc").is_none());
    }
}
