//! CLI credentials and native storage for Hive's own LLM API keys.

pub(crate) mod llm;

use crate::command::{self, Cancellation};
use crate::state::home_dir;
use base64::Engine as _;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `security` may sit behind a Keychain prompt; the usage poll must not hang on it.
const KEYCHAIN_TIMEOUT: Duration = Duration::from_secs(15);
/// A password is small; the bound only guards against unexpected output.
const KEYCHAIN_OUTPUT_LIMIT: usize = 64 * 1024;
/// Exit status of `security` for Apple's errSecItemNotFound.
const SECURITY_ITEM_NOT_FOUND: i32 = 44;

#[derive(thiserror::Error, Clone, Debug, PartialEq, Eq)]
pub enum CredentialError {
    #[error("not logged in, run `{0} login`")]
    NotLoggedIn(&'static str),
    #[error("token expired, run `claude`")]
    Expired,
    #[error("unreadable credentials: {0}")]
    Malformed(String),
    /// The lookup itself failed: no home directory, or the Keychain refused or timed out.
    #[error("credentials unavailable: {0}")]
    Unavailable(String),
}

fn home() -> Result<PathBuf, CredentialError> {
    home_dir().ok_or_else(|| CredentialError::Unavailable("no home directory".into()))
}

/// Claude Code OAuth access token: macOS Keychain first, `~/.claude/.credentials.json` as fallback.
pub fn claude_access_token() -> Result<String, CredentialError> {
    let raw = match keychain_item("Claude Code-credentials")? {
        Some(json) => json,
        None => std::fs::read_to_string(home()?.join(".claude/.credentials.json"))
            .map_err(|_| CredentialError::NotLoggedIn("claude"))?,
    };
    let creds: Value =
        serde_json::from_str(&raw).map_err(|e| CredentialError::Malformed(e.to_string()))?;
    let oauth = &creds["claudeAiOauth"];
    if let Some(expires_ms) = oauth["expiresAt"].as_u64()
        && UNIX_EPOCH + Duration::from_millis(expires_ms) < SystemTime::now()
    {
        return Err(CredentialError::Expired);
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
    let raw = std::fs::read_to_string(home()?.join(".codex/auth.json"))
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

/// Blocking: the password of a generic Keychain item, `None` when there is no such item or
/// no `security` tool on this platform. A refused or stalled lookup is reported, not hidden.
fn keychain_item(service: &str) -> Result<Option<String>, CredentialError> {
    let mut security = Command::new("security");
    security.args(["find-generic-password", "-s", service, "-w"]);
    match command::run(
        security,
        &[],
        &Cancellation::default(),
        KEYCHAIN_TIMEOUT,
        KEYCHAIN_OUTPUT_LIMIT,
    ) {
        Ok(output) => keychain_password(output),
        Err(command::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            tracing::warn!(service, %error, "keychain lookup failed");
            Err(CredentialError::Unavailable(error.to_string()))
        }
    }
}

fn keychain_password(output: command::Output) -> Result<Option<String>, CredentialError> {
    match output.code {
        Some(0) => Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        )),
        Some(SECURITY_ITEM_NOT_FOUND) => Ok(None),
        code => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            tracing::warn!(?code, %stderr, "keychain lookup failed");
            Err(CredentialError::Unavailable(format!(
                "security exited with {code:?}: {stderr}"
            )))
        }
    }
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

    fn output(code: Option<i32>, stdout: &str, stderr: &str) -> command::Output {
        command::Output {
            code,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn keychain_password_distinguishes_missing_item_from_failure() {
        assert_eq!(
            keychain_password(output(Some(0), "{\"token\":1}\n", "")),
            Ok(Some("{\"token\":1}".into()))
        );
        assert_eq!(
            keychain_password(output(Some(44), "", "not found")),
            Ok(None)
        );
        assert!(matches!(
            keychain_password(output(Some(36), "", "User interaction is not allowed.")),
            Err(CredentialError::Unavailable(message)) if message.contains("36")
        ));
        assert!(matches!(
            keychain_password(output(None, "", "")),
            Err(CredentialError::Unavailable(_))
        ));
    }

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
