//! Session-only MCP configuration for local agent launches. Identity is in argv, not shell env.
use super::super::AgentKind;
use serde_json::json;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    io,
    path::Path,
    process::Command,
};

/// Add this pane's MCP server without replacing other servers or changing permissions.
/// OpenCode 2 and Antigravity have no verified session-only MCP override; they retain CLI access.
pub fn attach(
    command: &mut Command,
    agent: AgentKind,
    socket: &Path,
    terminal: u64,
    executable: &Path,
) -> io::Result<()> {
    if !matches!(agent, AgentKind::Codex | AgentKind::ClaudeCode) {
        return Ok(());
    }
    let executable = executable.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Hive executable path must be UTF-8 for MCP config",
        )
    })?;
    let socket_text = socket.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Hive socket path must be UTF-8 for MCP config",
        )
    })?;
    let args = [
        "mcp".to_owned(),
        "--socket".into(),
        socket_text.into(),
        "--terminal".into(),
        terminal.to_string(),
    ];
    let mut hash = DefaultHasher::new();
    socket.hash(&mut hash);
    let name = format!("hive_{:016x}_{terminal}", hash.finish());
    match agent {
        AgentKind::ClaudeCode => {
            command.args([
                "--mcp-config",
                &json!({"mcpServers": {name: {"type":"stdio", "command":executable, "args":args}}})
                    .to_string(),
            ]);
        }
        AgentKind::Codex => {
            let prefix = format!("mcp_servers.{name}");
            for (key, value) in [
                ("command", json!(executable)),
                ("args", json!(args)),
                ("tool_timeout_sec", json!(86405)),
            ] {
                command.args(["-c", &format!("{prefix}.{key}={value}")]);
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(agent: AgentKind, socket: &str, terminal: u64) -> Vec<String> {
        let mut command = Command::new("agent");
        command.args(["--resume", "saved"]);
        command.env("KEEP_SETTING", "yes");
        attach(
            &mut command,
            agent,
            Path::new(socket),
            terminal,
            Path::new("/Apps/Hive's app/hive"),
        )
        .unwrap();
        assert!(command.get_envs().any(
            |(key, value)| key == "KEEP_SETTING" && value == Some(std::ffi::OsStr::new("yes"))
        ));
        command
            .get_args()
            .map(|arg| arg.to_str().unwrap().to_owned())
            .collect()
    }
    #[test]
    fn claude_keeps_resume_and_adds_exact_identity_without_strict_config() {
        let args = args(AgentKind::ClaudeCode, "/tmp/one/a", 42);
        assert_eq!(&args[..3], ["--resume", "saved", "--mcp-config"]);
        let config: serde_json::Value = serde_json::from_str(&args[3]).unwrap();
        let servers = config["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 1);
        let server = servers.values().next().unwrap();
        assert_eq!(
            server["args"],
            json!(["mcp", "--socket", "/tmp/one/a", "--terminal", "42"])
        );
        assert_eq!(server["command"], "/Apps/Hive's app/hive");
    }
    #[test]
    fn codex_panes_and_instances_get_distinct_configs() {
        let first = args(AgentKind::Codex, "/tmp/one/a", 1);
        assert_ne!(first, args(AgentKind::Codex, "/tmp/one/a", 2));
        assert_ne!(first, args(AgentKind::Codex, "/tmp/two/a", 1));
        assert!(first.iter().any(|arg| arg.contains("--terminal\",\"1")));
        assert!(!first.iter().any(|arg| arg.contains("approval")
            || arg.contains("sandbox")
            || arg.contains("HOME")));
    }
    #[test]
    fn unverified_clients_do_not_receive_an_mcp_override() {
        assert_eq!(
            args(AgentKind::OpenCode, "/tmp/one/a", 1),
            ["--resume", "saved"]
        );
        assert_eq!(
            args(AgentKind::Antigravity, "/tmp/one/a", 1),
            ["--resume", "saved"]
        );
    }
}
