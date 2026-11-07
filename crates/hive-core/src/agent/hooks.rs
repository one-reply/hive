//! Claude's session-scoped hooks. Payloads never contain prompts or tool arguments after parsing.
use super::AgentStatus;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read},
    os::unix::{fs::DirBuilderExt, net::UnixDatagram},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_EVENT: usize = 64 * 1024;

/// A lifecycle report associated with the terminal that installed its hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    /// Application-owned terminal identity, never a PID.
    pub terminal: u64,
    /// Agent conversation id, when supplied by the CLI.
    pub session: Option<String>,
    /// Explicit lifecycle status; unknown hook events have no effect.
    pub status: Option<AgentStatus>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Input {
    hook_event_name: String,
    session_id: String,
    notification_type: String,
    tool_name: String,
}

/// Parse only the event fields Hive needs; ignore unknown future events and fields.
pub fn parse(terminal: u64, bytes: &[u8]) -> Result<Report, serde_json::Error> {
    let event: Input = serde_json::from_slice(bytes)?;
    let status = match event.hook_event_name.as_str() {
        "SessionStart" => Some(AgentStatus::Idle),
        "UserPromptSubmit" | "PostToolUse" | "PostToolUseFailure" => Some(AgentStatus::Working),
        "PreToolUse" if event.tool_name == "AskUserQuestion" => Some(AgentStatus::WaitingInput),
        "PreToolUse" => Some(AgentStatus::Working),
        "PermissionRequest" => Some(AgentStatus::WaitingApproval),
        "Notification" if event.notification_type == "permission_prompt" => {
            Some(AgentStatus::WaitingApproval)
        }
        "Notification" if event.notification_type == "idle_prompt" => {
            Some(AgentStatus::WaitingInput)
        }
        "Stop" => Some(AgentStatus::Done),
        "StopFailure" => Some(AgentStatus::Failed),
        "SessionEnd" => Some(AgentStatus::Idle),
        _ => None,
    };
    let session = super::launch::valid_session_id(&event.session_id).then_some(event.session_id);
    Ok(Report {
        terminal,
        session,
        status,
    })
}

/// Private per-process datagram endpoint, polled from the background executor.
pub struct Server {
    socket: UnixDatagram,
    path: PathBuf,
}
impl Server {
    /// Blocking: create an owner-only directory and nonblocking socket.
    pub fn bind() -> io::Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("hh-{}-{stamp:x}", std::process::id()));
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let path = directory.join("e");
        let result = (|| {
            let socket = UnixDatagram::bind(&path)?;
            socket.set_nonblocking(true)?;
            Ok(Self { socket, path })
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&directory);
        }
        result
    }

    /// Endpoint passed only to sessions started by this Hive instance.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The owner-only directory holding the endpoint; other per-instance sockets share it.
    pub fn directory(&self) -> &Path {
        self.path.parent().unwrap_or(&self.path)
    }

    /// Blocking IO, but never waits for an event. Bounds work per polling pass.
    pub fn drain(&self) -> io::Result<Vec<Report>> {
        let mut reports = Vec::new();
        let mut buffer = vec![0; MAX_EVENT + 1];
        for _ in 0..256 {
            match self.socket.recv(&mut buffer) {
                Ok(count) if count <= MAX_EVENT => match serde_json::from_slice(&buffer[..count]) {
                    Ok(report) => reports.push(report),
                    Err(error) => tracing::warn!(%error, "invalid agent hook report"),
                },
                Ok(_) => tracing::warn!("agent hook report too large"),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        Ok(reports)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(directory) = self.path.parent()
            && let Err(error) = fs::remove_dir_all(directory)
        {
            tracing::warn!(%error, "could not remove hook endpoint");
        }
    }
}

/// Blocking hook entrypoint: bound stdin, discard sensitive fields, then send one report.
pub fn forward(path: &Path, terminal: u64, input: impl Read) -> io::Result<()> {
    let mut bytes = Vec::new();
    input.take((MAX_EVENT + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_EVENT {
        return Err(io::Error::other("Hook input exceeds 64 KiB"));
    }
    let report = parse(terminal, &bytes).map_err(io::Error::other)?;
    let bytes = serde_json::to_vec(&report).map_err(io::Error::other)?;
    let socket = UnixDatagram::unbound()?;
    socket.set_write_timeout(Some(std::time::Duration::from_millis(200)))?;
    socket.send_to(&bytes, path)?;
    Ok(())
}

/// Additional Claude settings for this launch; never edits the user's global configuration.
pub fn settings(executable: &Path, endpoint: &Path, terminal: u64) -> String {
    let command = format!(
        "{} --agent-hook {} {terminal}",
        crate::host::quote(&executable.to_string_lossy()),
        crate::host::quote(&endpoint.to_string_lossy())
    );
    let mut hooks = serde_json::Map::new();
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PostToolUseFailure",
        "PermissionRequest",
        "Notification",
        "Stop",
        "StopFailure",
        "SessionEnd",
    ] {
        hooks.insert(event.into(), serde_json::json!([{ "hooks": [{ "type": "command", "command": command, "timeout": 2 }] }]));
    }
    serde_json::json!({"hooks": hooks}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_ignores_unknown_events_and_drops_sensitive_fields() {
        let report = parse(42, br#"{"session_id":"c90e-1234","hook_event_name":"PermissionRequest","tool_input":{"command":"secret"},"extra":true}"#).unwrap();
        assert_eq!(report.status, Some(AgentStatus::WaitingApproval));
        assert!(!serde_json::to_string(&report).unwrap().contains("secret"));
        assert_eq!(
            parse(42, br#"{"hook_event_name":"future"}"#)
                .unwrap()
                .status,
            None
        );
        assert_eq!(
            parse(42, br#"{"hook_event_name":"Stop"}"#).unwrap().status,
            Some(AgentStatus::Done)
        );
        assert!(parse(42, b"not json").is_err());
    }
    #[test]
    fn reports_are_consumed_once() {
        let server = Server::bind().unwrap();
        forward(server.path(), 42, &br#"{"hook_event_name":"Stop"}"#[..]).unwrap();
        assert_eq!(server.drain().unwrap().len(), 1);
        assert!(server.drain().unwrap().is_empty());
    }
}
