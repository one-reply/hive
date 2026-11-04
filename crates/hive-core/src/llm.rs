//! LLM provider credentials. All IO is blocking; call from a background executor.

use std::{fmt, time::Duration};

use crate::keychain;

const VERIFY_TIMEOUT: Duration = Duration::from_secs(10);

/// Providers supported by Hive's AI settings, independently of CLI logins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Provider {
    /// Google AI Studio / Gemini API.
    GoogleAi,
    /// OpenAI API.
    OpenAi,
    /// OpenRouter API.
    OpenRouter,
    /// Anthropic API.
    Anthropic,
}

impl Provider {
    /// Display order in AI settings.
    pub const ALL: [Self; 4] = [
        Self::GoogleAi,
        Self::OpenAi,
        Self::OpenRouter,
        Self::Anthropic,
    ];

    /// Human-readable provider name.
    pub fn name(self) -> &'static str {
        match self {
            Self::GoogleAi => "Google AI",
            Self::OpenAi => "OpenAI",
            Self::OpenRouter => "OpenRouter",
            Self::Anthropic => "Anthropic",
        }
    }

    pub(crate) fn account(self) -> &'static str {
        match self {
            Self::GoogleAi => "google-ai",
            Self::OpenAi => "openai",
            Self::OpenRouter => "openrouter",
            Self::Anthropic => "anthropic",
        }
    }

    fn verification_url(self) -> &'static str {
        match self {
            Self::GoogleAi => "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1",
            Self::OpenAi => "https://api.openai.com/v1/models",
            Self::OpenRouter => "https://openrouter.ai/api/v1/key",
            Self::Anthropic => "https://api.anthropic.com/v1/models?limit=1",
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A locally parsed key; construction does not establish online validity.
/// Deliberately not serializable, cloneable, or printable as plaintext.
pub struct ApiKey(String);

impl ApiKey {
    /// Borrows the secret for an authenticated request. Never log this value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey([REDACTED])")
    }
}

/// Invalid input, without echoing any part of the secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InvalidApiKey {
    /// No key was entered.
    #[error("Enter an API key.")]
    Empty,
    /// Keys must fit into a single HTTP header value.
    #[error("The API key contains whitespace or unsupported characters.")]
    InvalidCharacters,
}

impl TryFrom<String> for ApiKey {
    type Error = InvalidApiKey;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let value = value.trim();
        if value.is_empty() {
            return Err(InvalidApiKey::Empty);
        }
        if value.chars().any(|c| c.is_whitespace() || c.is_control())
            || ureq::http::HeaderValue::from_str(value).is_err()
        {
            return Err(InvalidApiKey::InvalidCharacters);
        }
        Ok(Self(value.to_owned()))
    }
}

/// Credential failures safe to display and log. Raw HTTP errors/bodies are excluded.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The provider rejected verification or is temporarily unavailable.
    #[error("{provider}: verification failed (HTTP {status}). {message}")]
    Verification {
        /// Provider being checked.
        provider: Provider,
        /// HTTP response status.
        status: u16,
        /// Fixed guidance, never a remote response body.
        message: &'static str,
    },
    /// The request did not complete within its deadline.
    #[error("{provider}: verification timed out. Try again.")]
    Timeout {
        /// Provider being checked.
        provider: Provider,
    },
    /// A connection, TLS, or protocol failure.
    #[error("{provider}: could not verify the key. Check your connection and try again.")]
    Connection {
        /// Provider being checked.
        provider: Provider,
    },
    /// A native Keychain operation failed; absence is handled separately.
    #[error("{provider}: could not {operation} the key in Keychain (OSStatus {code}).")]
    Keychain {
        /// Provider whose record was accessed.
        provider: Provider,
        /// Operation that failed.
        operation: &'static str,
        /// Native error code, including denial or a locked Keychain.
        code: i32,
    },
    /// A stored value cannot be used as an API key.
    #[error("{provider}: the stored key is unreadable. Reset it in AI settings.")]
    InvalidStoredKey {
        /// Provider whose record was read.
        provider: Provider,
    },
    /// No secure storage implementation is available on this OS.
    #[error(
        "{provider}: cannot {operation} the key. Secure API key storage is currently supported only on macOS."
    )]
    UnsupportedPlatform {
        /// Provider whose record was accessed.
        provider: Provider,
        /// Operation that could not be performed.
        operation: &'static str,
    },
}

/// Checks Keychain metadata without loading the secret or contacting the provider.
pub fn has_key(provider: Provider) -> Result<bool, Error> {
    keychain::llm::has_key(provider)
}

/// Loads a key on demand; `None` means missing, not denied or unreadable.
pub fn load_key(provider: Provider) -> Result<Option<ApiKey>, Error> {
    keychain::llm::load_key(provider)
}

/// Verifies a key online, then creates or updates its Keychain record.
/// A failed verification never changes the previous record.
pub fn verify_and_save_key(provider: Provider, key: &ApiKey) -> Result<(), Error> {
    verify_and_save_with(
        provider,
        key,
        provider.verification_url(),
        VERIFY_TIMEOUT,
        || keychain::llm::save_key(provider, key),
    )
}

/// Deletes only Hive's local key. Missing records are already successfully deleted.
pub fn delete_key(provider: Provider) -> Result<(), Error> {
    keychain::llm::delete_key(provider)
}

fn verify_and_save_with(
    provider: Provider,
    key: &ApiKey,
    url: &str,
    timeout: Duration,
    save: impl FnOnce() -> Result<(), Error>,
) -> Result<(), Error> {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(3).min(timeout)))
        .timeout_global(Some(timeout))
        .max_redirects(0)
        .http_status_as_error(false)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let (name, value) = authentication(provider, key);
    let mut request = agent.get(url).header(name, value);
    if provider == Provider::Anthropic {
        request = request.header("anthropic-version", "2026-10-21");
    }
    let response = request.call().map_err(|error| match error {
        ureq::Error::Timeout(_) => Error::Timeout { provider },
        _ => Error::Connection { provider },
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::Verification {
            provider,
            status: status.as_u16(),
            message: match status.as_u16() {
                400 | 401 => "Check the API key.",
                403 => "The key lacks permission for verification.",
                429 => "Rate limited. Try again later.",
                500..=599 => "The provider is unavailable. Try again later.",
                _ => "The provider returned an unexpected response.",
            },
        });
    }
    save()
}

fn authentication(provider: Provider, key: &ApiKey) -> (&'static str, ureq::http::HeaderValue) {
    let (name, value) = if provider == Provider::GoogleAi {
        ("x-goog-api-key", key.as_str().to_owned())
    } else {
        ("authorization", format!("Bearer {}", key.as_str()))
    };
    let mut value = ureq::http::HeaderValue::from_str(&value)
        .expect("ApiKey was parsed as a valid HTTP header value");
    value.set_sensitive(true);
    (name, value)
}

#[cfg(test)]
mod tests;
