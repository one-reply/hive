//! Discovery and bounded, non-interactive invocation of the supported local CLIs (Codex,
//! Claude Code, OpenCode and Antigravity; the other `AgentKind`s are only recognized in terminals).
use super::Error;
use crate::{agent::AgentKind, host::Host};
use serde::Deserialize;
use std::{collections::HashMap, path::PathBuf, process::Command, time::Duration};

mod process;
pub use process::Cancellation;

/// Shown when a saved recipe names an agent that only terminal detection knows.
pub const UNSUPPORTED_GENERATOR: &str = "This agent cannot generate commit messages yet. Select Codex, Claude Code, OpenCode, Antigravity or an API provider.";

/// Supported commit CLIs, in picker order.
pub const AGENTS: [AgentKind; 4] = [
    AgentKind::Codex,
    AgentKind::ClaudeCode,
    AgentKind::OpenCode,
    AgentKind::Antigravity,
];

// Strict mode soft-denies tool actions in headless runs, even with user allow rules.
const ANTIGRAVITY_SETTINGS: &str = r#"{"toolPermission":"strict","permissions":{"deny":["read_file(*)","write_file(*)","read_url(*)","execute_url(*)","command(*)","mcp(*)"]}}"#;

const TIMEOUT: Duration = Duration::from_secs(120);
/// Deleting the throwaway OpenCode session is local bookkeeping; it must not hold a slot long.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(15);
/// OpenCode has no flag that turns its tools off; denying every permission does, and an
/// empty config home keeps the user's MCP servers, plugins and providers out of the run.
const OPENCODE_ISOLATION: [(&str, &str); 4] = [
    ("OPENCODE_CONFIG_CONTENT", r#"{"permission":{"*":"deny"}}"#),
    ("OPENCODE_DISABLE_PROJECT_CONFIG", "1"),
    ("OPENCODE_DISABLE_CLAUDE_CODE", "1"),
    ("OPENCODE_DISABLE_AUTOUPDATE", "1"),
];

/// Finds an executable on PATH, then in standard macOS CLI installation locations.
/// Blocking metadata access: call off the foreground thread.
pub fn executable(agent: AgentKind) -> Option<PathBuf> {
    crate::command::executable(agent.executable())
}

/// Parses model/effort arguments without expansion. Process protocol and permissions are fixed.
pub fn arguments(agent: AgentKind, value: &str) -> Result<Vec<String>, Error> {
    let args = shlex::split(value).ok_or(Error::Invalid(
        "Arguments contain an unclosed quote or escape.",
    ))?;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(a, b)| (a, Some(b)));
        let allowed = flag == "--model"
            || (flag == "-m" && agent != AgentKind::Antigravity)
            || match agent {
                AgentKind::Codex => matches!(flag, "-c" | "--config"),
                AgentKind::ClaudeCode => matches!(flag, "--effort" | "--max-budget-usd"),
                AgentKind::Antigravity => flag == "--effort",
                _ => false,
            };
        if !allowed {
            return Err(Error::Invalid(
                "Only model/effort arguments are supported. Output, tools, permissions and working directory are managed by Hive.",
            ));
        }
        let value = inline
            .or_else(|| iter.next().map(String::as_str))
            .filter(|v| !v.is_empty() && !v.starts_with('-') && !v.chars().any(char::is_control))
            .ok_or(Error::Invalid("Each argument needs a nonempty value."))?;
        if matches!(flag, "-c" | "--config") {
            let (key, value) = value.split_once('=').ok_or(Error::Invalid(
                "Use -c model_reasoning_effort=low (or medium/high).",
            ))?;
            if key != "model_reasoning_effort"
                || !matches!(
                    value.trim_matches('"'),
                    "minimal" | "low" | "medium" | "high" | "xhigh"
                )
            {
                return Err(Error::Invalid(
                    "Only model_reasoning_effort can be overridden with -c.",
                ));
            }
        }
    }
    Ok(args)
}

pub(super) fn generate(
    agent: AgentKind,
    extra: &str,
    prompt: &str,
    cancellation: &Cancellation,
) -> Result<String, Error> {
    let extra = arguments(agent, extra)?;
    let program = executable(agent).ok_or(Error::Invalid(
        "CLI not found. Install it, then reopen Git settings.",
    ))?;
    let fixed = match agent {
        AgentKind::Codex => vec![
            "exec",
            "--json",
            "--color",
            "never",
            "--ephemeral",
            "--sandbox",
            "read-only",
            "--ignore-user-config",
            "--ignore-rules",
            "--skip-git-repo-check",
        ],
        AgentKind::ClaudeCode => vec![
            "--print",
            "--output-format",
            "json",
            "--no-session-persistence",
            "--safe-mode",
            "--tools",
            "",
            "--strict-mcp-config",
            "--permission-mode",
            "dontAsk",
        ],
        AgentKind::Antigravity => vec![
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--disable-slash-commands",
            "--sandbox",
            "--settings",
            ANTIGRAVITY_SETTINGS,
        ],
        AgentKind::OpenCode => return generate_opencode(&extra, &program, prompt, cancellation),
        _ => return Err(Error::Invalid(UNSUPPORTED_GENERATOR)),
    };
    let directory = process::Directory::new().map_err(|source| Error::CliIo { agent, source })?;
    let mut argv = fixed;
    argv.extend(extra.iter().map(String::as_str));
    if agent == AgentKind::Codex {
        argv.push("-");
    }
    let command = cli_command(agent, &program, &argv, &directory);
    let input = if agent == AgentKind::Antigravity {
        format!(
            "{}\n",
            serde_json::json!({"event": "user", "message": {"content": prompt}})
        )
    } else {
        prompt.to_owned()
    };
    let output = process::run(agent, command, &input, cancellation, TIMEOUT)?;
    parse(agent, &output)
}

/// OpenCode keeps every `run` in the user's session list; the throwaway session is deleted
/// afterwards, whatever the outcome, because a failed or cancelled run may still have saved it.
fn generate_opencode(
    extra: &[String],
    program: &std::path::Path,
    prompt: &str,
    cancellation: &Cancellation,
) -> Result<String, Error> {
    let agent = AgentKind::OpenCode;
    let directory = process::Directory::new().map_err(|source| Error::CliIo { agent, source })?;
    let session = crate::agent::launch::new_opencode_session_id();
    let mut argv = vec![
        "run",
        "--format",
        "json",
        "--standalone",
        "--session",
        &session,
    ];
    argv.extend(extra.iter().map(String::as_str));
    let command = cli_command(agent, program, &argv, &directory);
    let result = process::run(agent, command, prompt, cancellation, TIMEOUT)
        .and_then(|output| parse(agent, &output));
    let delete = ["session", "delete", "--standalone", &session];
    let cleanup = cli_command(agent, program, &delete, &directory);
    if let Err(error) = crate::command::run(
        cleanup,
        b"",
        &Cancellation::default(),
        CLEANUP_TIMEOUT,
        64 * 1024,
    ) {
        tracing::warn!(%error, "could not delete the OpenCode commit message session");
    }
    result
}

/// A generator process in its private temporary directory, with the environment it needs.
fn cli_command(
    agent: AgentKind,
    program: &std::path::Path,
    argv: &[&str],
    directory: &process::Directory,
) -> Command {
    let mut command = Host::Local.command(&program.to_string_lossy(), argv, Some(&directory.0));
    // Node-based installs need their sibling runtime even when launched from Finder's short PATH.
    if let Some(parent) = program.parent() {
        let mut paths = vec![parent.to_path_buf()];
        if let Some(path) = crate::host::environment::path() {
            paths.extend(std::env::split_paths(&path));
        }
        if let Ok(path) = std::env::join_paths(paths) {
            command.env("PATH", path);
        }
    }
    if agent == AgentKind::OpenCode {
        // Credentials live in OpenCode's data directory, which stays the user's.
        command.env("XDG_CONFIG_HOME", directory.0.join("config"));
        command.envs(OPENCODE_ISOLATION);
    }
    command
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    subtype: String,
    is_error: bool,
    result: String,
    item: Option<Item>,
    part: Option<Part>,
}
/// An OpenCode message part: text arrives per message, and a step ends with a reason.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Part {
    #[serde(rename = "messageID")]
    message: String,
    text: String,
    reason: String,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Item {
    #[serde(rename = "type")]
    kind: String,
    text: String,
}

fn parse(agent: AgentKind, output: &[u8]) -> Result<String, Error> {
    let invalid = || {
        Error::Invalid(
            "CLI returned no complete message. Check its version, login and model, then try again.",
        )
    };
    if agent == AgentKind::ClaudeCode {
        let event: Event = serde_json::from_slice(output).map_err(|_| invalid())?;
        if event.kind != "result" || event.subtype != "success" || event.is_error {
            return Err(invalid());
        }
        return super::message(event.result);
    }
    if agent == AgentKind::OpenCode {
        return parse_opencode(output)
            .ok_or_else(invalid)
            .and_then(super::message);
    }
    if agent == AgentKind::Antigravity {
        return parse_antigravity(output)
            .ok_or_else(invalid)
            .and_then(super::message);
    }
    let mut message = None;
    let mut completed = false;
    for line in output
        .split(|b| *b == b'\n')
        .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
    {
        let event: Event = serde_json::from_slice(line).map_err(|_| invalid())?;
        match event.kind.as_str() {
            "turn.failed" | "error" => return Err(invalid()),
            "turn.completed" => completed = true,
            "item.completed" => {
                if let Some(item) = event.item
                    && item.kind == "agent_message"
                {
                    message = Some(item.text);
                }
            }
            _ => {}
        }
    }
    if !completed {
        return Err(invalid());
    }
    super::message(message.ok_or_else(invalid)?)
}

#[derive(Deserialize)]
struct AntigravityEvent {
    event: String,
    result: Option<AntigravityResult>,
}

#[derive(Deserialize)]
struct AntigravityResult {
    status: String,
    response: String,
}

fn parse_antigravity(output: &[u8]) -> Option<String> {
    let mut answer = None;
    for line in output
        .split(|b| *b == b'\n')
        .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
    {
        let event: AntigravityEvent = serde_json::from_slice(line).ok()?;
        if event.event == "result" {
            let result = event.result?;
            if result.status != "SUCCESS" || answer.is_some() {
                return None;
            }
            answer = Some(result.response);
        }
    }
    answer
}

/// The text of the message whose step stopped on its own. Earlier steps end in tool calls,
/// and their narration ("I'll read the file") is not the answer.
fn parse_opencode(output: &[u8]) -> Option<String> {
    let mut texts: HashMap<String, String> = HashMap::new();
    let mut answer = None;
    for line in output
        .split(|b| *b == b'\n')
        .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
    {
        let event: Event = serde_json::from_slice(line).ok()?;
        let part = event.part.unwrap_or_default();
        match event.kind.as_str() {
            "error" => return None,
            "text" => texts.entry(part.message).or_default().push_str(&part.text),
            "step_finish" if part.reason == "stop" => answer = texts.remove(&part.message),
            _ => {}
        }
    }
    answer
}

#[cfg(test)]
mod tests;
