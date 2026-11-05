//! Discovery and bounded, non-interactive invocation of the supported local CLIs (Codex and
//! Claude Code; the other `AgentKind`s are only recognized in terminals, not driven here).
use super::Error;
use crate::{agent::AgentKind, host::Host};
use serde::Deserialize;
use std::{path::PathBuf, time::Duration};

mod process;
pub use process::Cancellation;

/// Shown when a saved recipe names an agent that only terminal detection knows.
pub const UNSUPPORTED_GENERATOR: &str =
    "This agent cannot generate commit messages yet. Select Codex, Claude Code or an API provider.";

/// Supported commit CLIs, in picker order.
pub const AGENTS: [AgentKind; 2] = [AgentKind::Codex, AgentKind::ClaudeCode];

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
        let allowed = matches!(flag, "--model" | "-m")
            || match agent {
                AgentKind::Codex => matches!(flag, "-c" | "--config"),
                AgentKind::ClaudeCode => matches!(flag, "--effort" | "--max-budget-usd"),
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
        _ => return Err(Error::Invalid(UNSUPPORTED_GENERATOR)),
    };
    let directory = process::Directory::new().map_err(|source| Error::CliIo { agent, source })?;
    let mut argv = fixed;
    argv.extend(extra.iter().map(String::as_str));
    if agent == AgentKind::Codex {
        argv.push("-");
    }
    let mut command = Host::Local.command(&program.to_string_lossy(), &argv, Some(&directory.0));
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
    let output = process::run(
        agent,
        command,
        prompt,
        cancellation,
        Duration::from_secs(120),
    )?;
    parse(agent, &output)
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

#[cfg(test)]
mod tests;
