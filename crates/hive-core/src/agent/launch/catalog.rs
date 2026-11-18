//! Local CLI model choices for the pipeline profile picker.
use super::{AgentKind, Launch, LaunchError};
use crate::{
    command::{self, Cancellation},
    host::Host,
};
use serde::Deserialize;
use std::{path::Path, time::Duration};

/// Blocking: query the installed CLI's catalog. Claude's interactive model aliases are local.
/// Custom IDs can still be entered even when a provider's catalog is unavailable.
pub fn pipeline_models(agent: AgentKind) -> Result<Vec<String>, LaunchError> {
    let binary = Launch::New(agent).executable()?;
    let args: &[&str] = match agent {
        AgentKind::Codex => &["debug", "models", "--bundled"],
        AgentKind::OpenCode => &["models", "--standalone"],
        AgentKind::Antigravity => &["models"],
        AgentKind::ClaudeCode => {
            return Ok(["haiku", "sonnet", "opus"].map(str::to_owned).to_vec());
        }
        _ => return Err(LaunchError::Unsupported),
    };
    // Templates are global. Do not load whichever project's config happens to be the app's cwd.
    let mut cmd = Host::Local.command(&binary.to_string_lossy(), args, Some(Path::new("/")));
    cmd.env("PWD", "/").env("NO_COLOR", "1");
    let output = command::run(
        cmd,
        b"",
        &Cancellation::default(),
        Duration::from_secs(12),
        4 * 1024 * 1024,
    )
    .map_err(|error| {
        LaunchError::Pipeline(format!("Could not list {} models: {error}", agent.name()))
    })?;
    if output.code != Some(0) {
        return Err(LaunchError::Pipeline(format!(
            "{} could not list models ({}). Check the CLI login or enter a model ID.",
            agent.name(),
            output.code.map_or_else(
                || "process interrupted".to_owned(),
                |code| format!("exit {code}")
            )
        )));
    }
    parse(agent, &output.stdout)
}

fn parse(agent: AgentKind, stdout: &[u8]) -> Result<Vec<String>, LaunchError> {
    #[derive(Deserialize)]
    struct Catalog {
        models: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        slug: String,
        #[serde(default)]
        visibility: String,
    }
    let invalid = || {
        LaunchError::Pipeline(format!(
            "{} returned an unreadable model catalog",
            agent.name()
        ))
    };
    let mut models: Vec<_> = match agent {
        AgentKind::Codex => serde_json::from_slice::<Catalog>(stdout)
            .map_err(|_| invalid())?
            .models
            .into_iter()
            .filter(|model| model.visibility.is_empty() || model.visibility == "list")
            .map(|model| model.slug)
            .collect(),
        AgentKind::OpenCode => std::str::from_utf8(stdout)
            .map_err(|_| invalid())?
            .lines()
            .map(str::trim)
            .filter(|s| {
                s.split_once('/')
                    .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
            })
            .map(str::to_owned)
            .collect(),
        AgentKind::Antigravity => std::str::from_utf8(stdout)
            .map_err(|_| invalid())?
            .lines()
            .filter_map(|line| line.split_once('\t').map(|(id, _)| id.trim().to_owned()))
            .collect(),
        _ => return Err(LaunchError::Unsupported),
    };
    models.retain(|id| {
        !id.is_empty()
            && id.len() <= 256
            && !id.starts_with('-')
            && !id.chars().any(|ch| ch.is_control() || ch.is_whitespace())
    });
    models.sort();
    models.dedup();
    if models.is_empty() {
        return Err(invalid());
    }
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_local_cli_catalog_shapes_and_omits_hidden_models_and_progress() {
        assert_eq!(parse(AgentKind::Codex, br#"{"models":[{"slug":"gpt-6.2-sol","visibility":"list","context_window":400000},{"slug":"internal","visibility":"hide"}]}"#).unwrap(), ["gpt-6.2-sol"]);
        assert_eq!(
            parse(AgentKind::OpenCode, b"header\nanthropic/claude-haiku-4-5\n").unwrap(),
            ["anthropic/claude-haiku-4-5"]
        );
        assert_eq!(
            parse(
                AgentKind::Antigravity,
                b"Fetching available models...\ngemini-3.8-flash-low\tGemini 3.8 Flash (Low)\n"
            )
            .unwrap(),
            ["gemini-3.8-flash-low"]
        );
        for agent in [
            AgentKind::Codex,
            AgentKind::OpenCode,
            AgentKind::Antigravity,
        ] {
            assert!(parse(agent, b"{truncated").is_err());
            assert!(parse(agent, b"").is_err());
            assert!(parse(agent, &[0xff]).is_err());
        }
    }
}
