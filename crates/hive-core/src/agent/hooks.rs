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

/// A report Hive sends itself: a terminal id, a session id and a status.
const MAX_EVENT: usize = 64 * 1024;
/// What Claude writes to a hook. `PreToolUse` and `PermissionRequest` carry the tool's input
/// (a whole file for Write) and `PostToolUse` its output, so events run far past 64 KiB.
/// Parsing streams over it and keeps only four short fields, so memory stays flat.
const MAX_INPUT: u64 = 64 * 1024 * 1024;

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
    Ok(report(terminal, serde_json::from_slice(bytes)?))
}

fn report(terminal: u64, event: Input) -> Report {
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
    Report {
        terminal,
        session,
        status,
    }
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

/// Blocking hook entrypoint: stream bounded stdin, discard sensitive fields, then send one
/// report. Input past [`MAX_INPUT`] fails as truncated JSON.
pub fn forward(path: &Path, terminal: u64, input: impl Read) -> io::Result<()> {
    let input = io::BufReader::new(input.take(MAX_INPUT));
    let event: Input = serde_json::from_reader(input).map_err(io::Error::other)?;
    let report = report(terminal, event);
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
    /// A `PermissionRequest` for a Write of a 300 KiB file, with the event name last.
    fn large_permission_request() -> Vec<u8> {
        let content = "x".repeat(300 * 1024);
        serde_json::to_vec(&serde_json::json!({
            "session_id": "c90e-1234",
            "tool_name": "Write",
            "tool_input": {"file_path": "/repo/big.txt", "content": content},
            "hook_event_name": "PermissionRequest",
        }))
        .unwrap()
    }

    #[test]
    fn an_event_far_past_64_kib_still_reports_its_status() {
        let server = Server::bind().unwrap();
        let event = large_permission_request();
        assert!(event.len() > 4 * MAX_EVENT);
        forward(server.path(), 42, &event[..]).unwrap();
        let reports = server.drain().unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].status, Some(AgentStatus::WaitingApproval));
        assert_eq!(reports[0].session.as_deref(), Some("c90e-1234"));
    }

    #[test]
    fn truncated_or_oversized_input_sends_nothing() {
        let server = Server::bind().unwrap();
        let event = large_permission_request();
        assert!(forward(server.path(), 42, &event[..event.len() - 10]).is_err());
        assert!(
            forward(
                server.path(),
                42,
                &b"{\"hook_event_name\":\"Stop\"} trailing"[..]
            )
            .is_err()
        );
        let endless = std::io::repeat(b' ').take(MAX_INPUT + 1);
        assert!(forward(server.path(), 42, endless).is_err());
        assert!(server.drain().unwrap().is_empty());
    }

    #[test]
    fn reports_are_consumed_once() {
        let server = Server::bind().unwrap();
        forward(server.path(), 42, &br#"{"hook_event_name":"Stop"}"#[..]).unwrap();
        assert_eq!(server.drain().unwrap().len(), 1);
        assert!(server.drain().unwrap().is_empty());
    }
}
