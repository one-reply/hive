use super::{ApiKey, Provider, authentication, load_key};
use serde::de::DeserializeOwned;
use std::time::Duration;

const RESPONSE_LIMIT: u64 = 16 * 1024 * 1024;

/// A model-list or generation failure, without credentials or remote response contents.
#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    /// Credential access failed.
    #[error(transparent)]
    Credential(#[from] super::Error),
    /// No local key is configured.
    #[error("{0}: add an API key in AI settings.")]
    MissingKey(Provider),
    /// HTTP rejection with fixed guidance.
    #[error("{provider}: HTTP {status}. {guidance}")]
    Http {
        /// Provider contacted.
        provider: Provider,
        /// Response status code.
        status: u16,
        /// Safe guidance, never a remote response body.
        guidance: &'static str,
    },
    /// Transport or deadline failure.
    #[error("{provider}: {message}")]
    Transport {
        /// Provider contacted.
        provider: Provider,
        /// Fixed diagnostic.
        message: &'static str,
    },
    /// Invalid, unsupported or incomplete remote data.
    #[error("{provider}: {message}")]
    Response {
        /// Provider contacted.
        provider: Provider,
        /// Fixed diagnostic.
        message: &'static str,
    },
}

pub(super) struct Client {
    agent: ureq::Agent,
    pub(super) provider: Provider,
    key: ApiKey,
}

impl Client {
    pub(super) fn new(provider: Provider, timeout: Duration) -> Result<Self, RequestError> {
        let key = load_key(provider)?.ok_or(RequestError::MissingKey(provider))?;
        Ok(Self::with_key(provider, key, timeout))
    }

    pub(super) fn with_key(provider: Provider, key: ApiKey, timeout: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(5).min(timeout)))
            .timeout_global(Some(timeout))
            .max_redirects(0)
            .http_status_as_error(false)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            provider,
            key,
        }
    }

    pub(super) fn get<T: DeserializeOwned>(
        &self,
        url: &str,
        query: &[(&str, &str)],
    ) -> Result<T, RequestError> {
        let (name, value) = authentication(self.provider, &self.key);
        let mut request = self
            .agent
            .get(url)
            .header(name, value)
            .query_pairs(query.iter().copied());
        if self.provider == Provider::Anthropic {
            request = request.header("anthropic-version", "2026-10-21");
        }
        self.decode(request.call())
    }

    pub(super) fn post<T: DeserializeOwned>(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<T, RequestError> {
        let body =
            serde_json::to_vec(body).map_err(|_| self.invalid("Could not encode the request."))?;
        let (name, value) = authentication(self.provider, &self.key);
        let mut request = self
            .agent
            .post(url)
            .header(name, value)
            .header("content-type", "application/json");
        if self.provider == Provider::Anthropic {
            request = request.header("anthropic-version", "2026-10-21");
        }
        self.decode(request.send(body.as_slice()))
    }

    fn decode<T: DeserializeOwned>(
        &self,
        response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<T, RequestError> {
        let mut response = response.map_err(|e| self.transport(e))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(RequestError::Http {
                provider: self.provider,
                status,
                guidance: match status {
                    401 | 403 => "Check your API key and model permissions in AI settings.",
                    400 | 404 | 422 => "Check the selected model and prompt size in Git settings.",
                    429 => "Rate or credit limit reached. Try again later.",
                    500..=599 => "The provider is unavailable. Try again later.",
                    _ => "Unexpected response. Try again later.",
                },
            });
        }
        let bytes = response
            .body_mut()
            .with_config()
            .limit(RESPONSE_LIMIT)
            .read_to_vec()
            .map_err(|e| self.transport(e))?;
        serde_json::from_slice(&bytes)
            .map_err(|_| self.invalid("Invalid JSON response. Try again."))
    }

    fn transport(&self, error: ureq::Error) -> RequestError {
        RequestError::Transport {
            provider: self.provider,
            message: match error {
                ureq::Error::Timeout(_) => "Request timed out. Try again.",
                ureq::Error::BodyExceedsLimit(_) => "Response exceeds the size limit.",
                _ => "Request failed. Check your connection and try again.",
            },
        }
    }

    pub(super) fn invalid(&self, message: &'static str) -> RequestError {
        RequestError::Response {
            provider: self.provider,
            message,
        }
    }
}

#[cfg(test)]
mod tests;
