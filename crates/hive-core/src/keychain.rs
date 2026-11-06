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
const CLAUDE_LOGIN: &str = "claude login";
const CODEX_LOGIN: &str = "codex login";
const CURSOR_LOGIN: &str = "cursor-agent login";
/// `agy` signs in on its first interactive run; it has no separate login command.
const ANTIGRAVITY_LOGIN: &str = "agy";

#[derive(thiserror::Error, Clone, Debug, PartialEq, Eq)]
pub enum CredentialError {
    /// No saved session; carries the command that signs in.
    #[error("not logged in, run `{0}`")]
    NotLoggedIn(&'static str),
    /// The saved session is past its expiry; carries the command that renews it.
    #[error("token expired, run `{0}`")]
    Expired(&'static str),
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
    let raw = match keychain_item("Claude Code-credentials", None)? {
        Some(json) => json,
        None => std::fs::read_to_string(home()?.join(".claude/.credentials.json"))
            .map_err(|_| CredentialError::NotLoggedIn(CLAUDE_LOGIN))?,
    };
    let creds: Value =
        serde_json::from_str(&raw).map_err(|e| CredentialError::Malformed(e.to_string()))?;
    let oauth = &creds["claudeAiOauth"];
    if let Some(expires_ms) = oauth["expiresAt"].as_u64()
        && UNIX_EPOCH + Duration::from_millis(expires_ms) < SystemTime::now()
    {
        return Err(CredentialError::Expired("claude"));
    }
    oauth["accessToken"]
        .as_str()
        .map(str::to_string)
        .ok_or(CredentialError::NotLoggedIn(CLAUDE_LOGIN))
}

pub struct CodexCredentials {
    pub access_token: String,
    pub account_id: String,
}

/// Codex CLI tokens from `~/.codex/auth.json`.
pub fn codex_credentials() -> Result<CodexCredentials, CredentialError> {
    let raw = std::fs::read_to_string(home()?.join(".codex/auth.json"))
        .map_err(|_| CredentialError::NotLoggedIn(CODEX_LOGIN))?;
    let auth: Value =
        serde_json::from_str(&raw).map_err(|e| CredentialError::Malformed(e.to_string()))?;
    let tokens = &auth["tokens"];
    let access_token = tokens["access_token"]
        .as_str()
        .ok_or(CredentialError::NotLoggedIn(CODEX_LOGIN))?
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

/// A `cursor-agent login` session.
pub struct CursorSession {
    pub access_token: String,
    /// The account id from the token's `sub` claim (`auth0|<id>`).
    pub user_id: String,
}

impl CursorSession {
    /// The cookie `cursor.com`'s dashboard API authenticates with.
    pub fn cookie(&self) -> String {
        format!(
            "WorkosCursorSessionToken={}%3A%3A{}",
            self.user_id, self.access_token
        )
    }
}

/// Blocking: the `cursor-agent` session from the login Keychain, else `~/.cursor/auth.json`
/// (where it writes with `AGENT_CLI_CREDENTIAL_STORE=file`).
pub fn cursor_session() -> Result<CursorSession, CredentialError> {
    let access_token = match keychain_item("cursor-access-token", Some("cursor-user"))? {
        Some(token) => token,
        None => {
            let raw = std::fs::read_to_string(home()?.join(".cursor/auth.json"))
                .map_err(|_| CredentialError::NotLoggedIn(CURSOR_LOGIN))?;
            let auth: Value = serde_json::from_str(&raw)
                .map_err(|e| CredentialError::Malformed(e.to_string()))?;
            auth["accessToken"]
                .as_str()
                .ok_or(CredentialError::NotLoggedIn(CURSOR_LOGIN))?
                .to_owned()
        }
    };
    cursor_session_from(access_token, SystemTime::now())
}

fn cursor_session_from(
    access_token: String,
    now: SystemTime,
) -> Result<CursorSession, CredentialError> {
    let malformed = || CredentialError::Malformed("cursor token has no user id".into());
    let subject = jwt_claim(&access_token, "sub").ok_or_else(malformed)?;
    let user_id = subject.rsplit('|').next().unwrap_or_default().to_owned();
    // The id goes into a cookie unescaped; anything beyond these would change its meaning.
    let valid = !user_id.is_empty()
        && user_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if !valid {
        return Err(malformed());
    }
    if jwt_expiry(&access_token).is_some_and(|expiry| expiry < now) {
        return Err(CredentialError::Expired(CURSOR_LOGIN));
    }
    Ok(CursorSession {
        access_token,
        user_id,
    })
}

/// Blocking: the Google access token Antigravity saved at sign-in. `agy` keeps it through
/// `go-keyring` (Keychain service `gemini`, account `antigravity`), or in
/// `~/.gemini/antigravity-cli/antigravity-oauth-token` when it cannot use the keyring.
/// Only a running Antigravity renews it, so an expired one asks for a run of `agy`.
pub fn antigravity_access_token() -> Result<String, CredentialError> {
    let raw = match keychain_item("gemini", Some("antigravity"))? {
        Some(raw) => raw,
        None => {
            std::fs::read_to_string(home()?.join(".gemini/antigravity-cli/antigravity-oauth-token"))
                .map_err(|_| CredentialError::NotLoggedIn(ANTIGRAVITY_LOGIN))?
        }
    };
    antigravity_token_from(&raw, SystemTime::now())
}

/// `go-keyring` stores either the JSON itself or `go-keyring-base64:` and its base64;
/// the token sits at the top level or under `token`, in snake or camel case.
fn antigravity_token_from(raw: &str, now: SystemTime) -> Result<String, CredentialError> {
    let raw = raw.trim();
    let json = match raw.strip_prefix("go-keyring-base64:") {
        Some(encoded) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded.trim())
                .map_err(|e| CredentialError::Malformed(e.to_string()))?;
            String::from_utf8(bytes).map_err(|e| CredentialError::Malformed(e.to_string()))?
        }
        None => raw.to_owned(),
    };
    let root: Value =
        serde_json::from_str(&json).map_err(|e| CredentialError::Malformed(e.to_string()))?;
    let token = match &root["token"] {
        nested @ Value::Object(_) => nested,
        _ => &root,
    };
    let access_token = ["access_token", "accessToken"]
        .iter()
        .filter_map(|key| token[key].as_str())
        .find(|value| !value.trim().is_empty())
        .ok_or(CredentialError::NotLoggedIn(ANTIGRAVITY_LOGIN))?;
    let expiry = ["expiry", "expires_at", "expiresAt"]
        .iter()
        .find_map(|key| token[key].as_str())
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok());
    if expiry.is_some_and(|expiry| expiry.timestamp() < unix_seconds(now)) {
        return Err(CredentialError::Expired(ANTIGRAVITY_LOGIN));
    }
    Ok(access_token.trim().to_owned())
}

fn unix_seconds(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// Blocking: the password of a generic Keychain item, `None` when there is no such item or
/// no `security` tool on this platform. A refused or stalled lookup is reported, not hidden.
fn keychain_item(service: &str, account: Option<&str>) -> Result<Option<String>, CredentialError> {
    let mut security = Command::new("security");
    security.args(["find-generic-password", "-s", service]);
    if let Some(account) = account {
        security.args(["-a", account]);
    }
    security.arg("-w");
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
    let v = jwt_payload(jwt)?;
    v[claim]
        .as_str()
        .or_else(|| v["https://api.openai.com/auth"][claim].as_str())
        .map(str::to_string)
}

/// The JWT's `exp` claim.
fn jwt_expiry(jwt: &str) -> Option<SystemTime> {
    let seconds = jwt_payload(jwt)?["exp"].as_u64()?;
    Some(UNIX_EPOCH + Duration::from_secs(seconds))
}

fn jwt_payload(jwt: &str) -> Option<Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
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

    fn jwt(payload: &str) -> String {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        format!("h.{payload}.s")
    }

    #[test]
    fn cursor_session_takes_the_user_id_from_sub_and_rejects_expired_tokens() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let token = jwt(r#"{"sub":"auth0|user_01ABC","exp":2000}"#);
        let session = cursor_session_from(token.clone(), now).unwrap();
        assert_eq!(session.user_id, "user_01ABC");
        assert_eq!(
            session.cookie(),
            format!("WorkosCursorSessionToken=user_01ABC%3A%3A{token}")
        );
        assert_eq!(
            cursor_session_from(jwt(r#"{"sub":"auth0|u","exp":500}"#), now).err(),
            Some(CredentialError::Expired(CURSOR_LOGIN))
        );
        for sub in [
            r#"{"exp":2000}"#,
            r#"{"sub":"auth0|a;b"}"#,
            r#"{"sub":"auth0|"}"#,
        ] {
            assert!(matches!(
                cursor_session_from(jwt(sub), now),
                Err(CredentialError::Malformed(_))
            ));
        }
    }

    #[test]
    fn antigravity_token_reads_every_saved_shape() {
        let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let nested =
            r#"{"email":"me","token":{"access_token":"at-1","expiry":"2030-01-02T03:04:05Z"}}"#;
        assert_eq!(antigravity_token_from(nested, now).unwrap(), "at-1");
        assert_eq!(
            antigravity_token_from(r#"{"accessToken":" at-2 "}"#, now).unwrap(),
            "at-2"
        );
        let wrapped = format!(
            "go-keyring-base64:{}",
            base64::engine::general_purpose::STANDARD.encode(r#"{"access_token":"at-3"}"#)
        );
        assert_eq!(antigravity_token_from(&wrapped, now).unwrap(), "at-3");
        assert_eq!(
            antigravity_token_from(
                r#"{"access_token":"at","expiry":"2020-01-01T00:00:00Z"}"#,
                now
            ),
            Err(CredentialError::Expired(ANTIGRAVITY_LOGIN))
        );
        assert_eq!(
            antigravity_token_from(r#"{"token":{"refresh_token":"rt"}}"#, now),
            Err(CredentialError::NotLoggedIn(ANTIGRAVITY_LOGIN))
        );
        assert!(matches!(
            antigravity_token_from("go-keyring-base64:!!", now),
            Err(CredentialError::Malformed(_))
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
