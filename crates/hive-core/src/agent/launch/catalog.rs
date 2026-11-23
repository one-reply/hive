//! Local CLI model choices for the pipeline profile picker.
use super::{AgentKind, Launch, LaunchError};
use crate::{
    command::{self, Cancellation},
    host::Host,
};
use serde::{Deserialize, de::IgnoredAny};
use std::{collections::HashMap, path::Path, time::Duration};

/// Aliases every supported Claude Code release resolves to the latest model of its family.
const CLAUDE_ALIASES: [&str; 3] = ["haiku", "sonnet", "opus"];
/// Families whose alias only newer CLI releases know. Each is offered once the CLI's own
/// catalog lists a model of that family, so an older CLI is never handed a name it rejects.
const CLAUDE_NEWER_FAMILIES: [&str; 1] = ["fable"];

/// Blocking: query the installed CLI's catalog. Claude Code has no listing command, so its
/// aliases are combined with the full model IDs its global config caches for the account.
/// Custom IDs can still be entered even when a provider's catalog is unavailable.
pub fn pipeline_models(agent: AgentKind) -> Result<Vec<String>, LaunchError> {
    let binary = Launch::New(agent).executable()?;
    let args: &[&str] = match agent {
        AgentKind::Codex => &["debug", "models", "--bundled"],
        AgentKind::OpenCode => &["models", "--standalone"],
        AgentKind::Antigravity => &["models"],
        AgentKind::ClaudeCode => return Ok(claude_models(read_claude_config().as_deref())),
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
    models.retain(|id| is_model_id(id));
    models.sort();
    models.dedup();
    if models.is_empty() {
        return Err(invalid());
    }
    Ok(models)
}

/// A model ID is passed as one `--model` argument, so it must not read as a flag or split.
fn is_model_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && !id.starts_with('-')
        && !id.chars().any(|ch| ch.is_control() || ch.is_whitespace())
}

/// A missing or unreadable config only costs the full IDs; the aliases still work.
fn read_claude_config() -> Option<Vec<u8>> {
    let path = crate::agent::claude_config::locations()?.global_config;
    std::fs::read(&path)
        .inspect_err(|error| {
            tracing::debug!(path = %path.display(), %error, "Claude Code config not read");
        })
        .ok()
}

/// The aliases plus every model the CLI caches prices or picker options for. The cache is
/// Claude Code's private format, so any shape it no longer has falls back to the aliases.
fn claude_models(config: Option<&[u8]>) -> Vec<String> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Config {
        #[serde(rename = "additionalModelCostsCache")]
        costs: HashMap<String, IgnoredAny>,
        #[serde(rename = "additionalModelOptionsCache")]
        options: Vec<PickerOption>,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct PickerOption {
        value: String,
    }
    let config = config.map_or_else(Config::default, |bytes| {
        serde_json::from_slice(bytes).unwrap_or_else(|error| {
            tracing::debug!(%error, "Claude Code config has no readable model cache");
            Config::default()
        })
    });
    let mut models: Vec<String> = config
        .costs
        .into_keys()
        .chain(config.options.into_iter().map(|option| option.value))
        .filter(|id| is_model_id(id))
        .collect();
    let newer = CLAUDE_NEWER_FAMILIES.into_iter().filter(|family| {
        let prefix = format!("claude-{family}-");
        models.iter().any(|id| id.starts_with(&prefix))
    });
    let aliases: Vec<_> = CLAUDE_ALIASES.into_iter().chain(newer).collect();
    models.extend(aliases.into_iter().map(str::to_owned));
    models.sort();
    models.dedup();
    models
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

    /// Trimmed from a real `~/.claude.json` of Claude Code 2.1.327: unrelated keys and
    /// pricing objects are kept so the parser is tested against what the CLI writes.
    const CLAUDE_CONFIG: &[u8] = br#"{
        "numStartups": 412,
        "additionalModelOptionsCache": [
            {"value": "claude-fable-5-2[1m]", "label": "Fable", "description": "Fable 5.2"}
        ],
        "additionalModelOptionsAnsweredAt": 1763459200000,
        "additionalModelCostsCache": {
            "claude-opus-5-5": {"inputTokens": 4, "outputTokens": 20, "fast": {"inputTokens": 8}},
            "claude-sonnet-5-5": {"inputTokens": 2, "outputTokens": 10},
            "claude-haiku-4-5-20251001": {"inputTokens": 1, "outputTokens": 5},
            "claude-fable-5-2": {"inputTokens": 10, "outputTokens": 50}
        },
        "projects": {"/Users/someone/project": {"allowedTools": []}}
    }"#;

    #[test]
    fn claude_lists_versioned_models_cached_by_the_cli() {
        assert_eq!(
            claude_models(Some(CLAUDE_CONFIG)),
            [
                "claude-fable-5-2",
                "claude-fable-5-2[1m]",
                "claude-haiku-4-5-20251001",
                "claude-opus-5-5",
                "claude-sonnet-5-5",
                "fable",
                "haiku",
                "opus",
                "sonnet",
            ]
        );
    }

    #[test]
    fn claude_offers_a_newer_alias_only_when_the_cli_knows_its_family() {
        let config = br#"{"additionalModelCostsCache": {"claude-opus-5-5": {}}}"#;
        assert_eq!(
            claude_models(Some(config)),
            ["claude-opus-5-5", "haiku", "opus", "sonnet"]
        );
    }

    #[test]
    fn claude_falls_back_to_aliases_without_a_readable_cache() {
        let aliases = ["haiku", "opus", "sonnet"];
        assert_eq!(claude_models(None), aliases);
        assert_eq!(claude_models(Some(b"")), aliases);
        assert_eq!(claude_models(Some(b"{\"additionalModelCost")), aliases);
        assert_eq!(claude_models(Some(&[0xff])), aliases);
        assert_eq!(claude_models(Some(b"{}")), aliases);
        assert_eq!(
            claude_models(Some(
                br#"{"additionalModelCostsCache": ["claude-opus-5-5"]}"#
            )),
            aliases
        );
    }

    #[test]
    fn claude_drops_cached_ids_that_cannot_be_one_argument() {
        let config = br#"{
            "additionalModelCostsCache": {"--dangerous": {}, "two words": {}, "": {}},
            "additionalModelOptionsCache": [{"label": "no value"}, {"value": "claude-opus-5-5"}]
        }"#;
        assert_eq!(
            claude_models(Some(config)),
            ["claude-opus-5-5", "haiku", "opus", "sonnet"]
        );
    }
}
