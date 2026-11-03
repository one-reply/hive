//! Credentials the Claude Code and Codex CLIs store on this machine. We never log in
//! ourselves; the "login" button in the UI just tells the user which CLI to run.

use crate::state::hive_dir;
use base64::Engine as _;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(thiserror::Error, Clone, Debug, PartialEq, Eq)]
pub enum CredentialError {
    #[error("not logged in, run `{0} login`")]
    NotLoggedIn(&'static str),
    #[error("token expired, run `claude`")]
    Expired,
    #[error("unreadable credentials: {0}")]
    Malformed(String),
}

fn home() -> PathBuf {
    hive_dir()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default()
}

/// Claude Code OAuth access token: macOS Keychain first, `~/.claude/.credentials.json` as fallback.
pub fn claude_access_token() -> Result<String, CredentialError> {
    let keychain = Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let raw = match keychain {
        Some(json) => json,
        None => std::fs::read_to_string(home().join(".claude/.credentials.json"))
            .map_err(|_| CredentialError::NotLoggedIn("claude"))?,
    };
    let creds: Value =
        serde_json::from_str(&raw).map_err(|e| CredentialError::Malformed(e.to_string()))?;
    let oauth = &creds["claudeAiOauth"];
    if let Some(expires_ms) = oauth["expiresAt"].as_u64() {
        if UNIX_EPOCH + Duration::from_millis(expires_ms) < SystemTime::now() {
            return Err(CredentialError::Expired);
        }
    }
    oauth["accessToken"]
        .as_str()
        .map(str::to_string)
        .ok_or(CredentialError::NotLoggedIn("claude"))
}

pub struct CodexCredentials {
    pub access_token: String,
    pub account_id: String,
}

/// Codex CLI tokens from `~/.codex/auth.json`.
pub fn codex_credentials() -> Result<CodexCredentials, CredentialError> {
    let raw = std::fs::read_to_string(home().join(".codex/auth.json"))
        .map_err(|_| CredentialError::NotLoggedIn("codex"))?;
    let auth: Value =
        serde_json::from_str(&raw).map_err(|e| CredentialError::Malformed(e.to_string()))?;
    let tokens = &auth["tokens"];
    let access_token = tokens["access_token"]
        .as_str()
        .ok_or(CredentialError::NotLoggedIn("codex"))?
        .to_string();
    let account_id = tokens["account_id"]
        .as_str()
        .map(str::to_string)
        .or_else(|| jwt_claim(tokens["id_token"].as_str()?, "chatgpt_account_id"))
        .ok_or_else(|| CredentialError::Malformed("no account id in auth.json".into()))?;
    Ok(CodexCredentials {
        access_token,
        account_id,
    })
}

/// Reads a claim from a JWT payload, looking inside the `https://api.openai.com/auth` object too.
fn jwt_claim(jwt: &str, claim: &str) -> Option<String> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v[claim]
        .as_str()
        .or_else(|| v["https://api.openai.com/auth"][claim].as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwt_nested_claim() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acc-1"}}"#);
        assert_eq!(
            jwt_claim(&format!("h.{payload}.s"), "chatgpt_account_id").as_deref(),
            Some("acc-1")
        );
    }
}
