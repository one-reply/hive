//! Local hosting CLI installation and browser authentication; credentials stay with the CLI.
#[cfg(feature = "desktop")]
mod install;
#[cfg(test)]
mod tests;

use crate::{
    command::{self, Cancellation},
    host::Host,
};
use serde::Deserialize;
use std::{path::PathBuf, process::Command, time::Duration};

#[cfg(feature = "desktop")]
pub use install::install;

/// Hosting service for PR/MR metadata and authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    /// GitHub.com or GitHub Enterprise.
    GitHub,
    /// GitLab.com or a self-managed GitLab instance.
    GitLab,
}
impl Provider {
    /// Supported providers, in settings display order.
    pub const ALL: [Self; 2] = [Self::GitHub, Self::GitLab];
    /// Product name.
    pub fn name(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
        }
    }
    /// Official CLI executable name.
    pub fn cli(self) -> &'static str {
        match self {
            Self::GitHub => "gh",
            Self::GitLab => "glab",
        }
    }
    /// Public hosting service.
    pub fn default_host(self) -> &'static str {
        match self {
            Self::GitHub => "github.com",
            Self::GitLab => "gitlab.com",
        }
    }
    /// Existing system installation, or Hive's verified private installation.
    pub fn executable(self) -> Option<PathBuf> {
        let executable = command::executable(self.cli());
        #[cfg(feature = "desktop")]
        let executable = executable.or_else(|| {
            let path = self.managed_path();
            use std::os::unix::fs::PermissionsExt;
            path.metadata()
                .ok()
                .filter(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .map(|_| path)
        });
        executable
    }
    #[cfg(feature = "desktop")]
    fn managed_path(self) -> PathBuf {
        crate::state::hive_dir().join("tools").join(self.cli())
    }
}

/// Validated server name, never a URL, command option, or repository path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(transparent)]
pub struct Server(String);
impl<'de> Deserialize<'de> for Server {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}
impl Server {
    /// Accept a DNS hostname and optional TCP port. HTTPS is always used.
    pub fn parse(value: &str) -> Result<Self, Error> {
        let value = value.trim();
        let (host, port) = value
            .split_once(':')
            .map_or((value, None), |(h, p)| (h, Some(p)));
        let valid = !host.is_empty()
            && host.len() <= 253
            && host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
            && port.is_none_or(|p| p.parse::<u16>().is_ok_and(|p| p > 0));
        if !valid {
            return Err(Error::InvalidServer);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }
    /// Validated hostname, including its optional port.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Read-only verification result; account identity comes from the provider's API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    /// Install the official CLI before signing in.
    MissingCli,
    /// Credentials are missing or expired.
    SignedOut,
    /// The server accepted the CLI's credentials.
    Connected(String),
    /// Authentication succeeded, but the CLI reported non-Keychain credential storage.
    ConnectedWithWarning(String),
}

/// Safe instructions extracted from CLI output, never an access token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicePrompt {
    /// Short one-time code the user enters in the browser.
    pub code: String,
    /// HTTPS verification page on the chosen server.
    pub url: String,
}

/// Hosting setup failure with no raw credential-bearing process output.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Invalid hostname input.
    #[error("Enter a hostname such as gitlab.com or git.example.com, without https:// or a path.")]
    InvalidServer,
    /// No suitable CLI installation.
    #[error("The hosting CLI is not installed. Choose Install & Connect.")]
    MissingCli,
    /// A bounded command was cancelled, timed out, or failed to start.
    #[error("Hosting command failed: {0}")]
    Command(#[from] command::Error),
    /// Account cannot access this API.
    #[error("The server denied access. Check your account permissions or sign in again.")]
    AccessDenied,
    /// Network or provider error.
    #[error("Could not reach or verify the server. Check the hostname and network, then retry.")]
    Unavailable,
    /// API JSON missing the expected account identity.
    #[error("The server returned an invalid account response.")]
    InvalidResponse,
    /// Login did not finish successfully.
    #[error(
        "Sign-in did not complete. Retry and approve access in your browser. Self-managed GitLab may require an OAuth application configured in glab."
    )]
    SignIn,
    /// Public OAuth application identifier was malformed.
    #[error(
        "Enter the OAuth Application ID supplied by your GitLab administrator, or leave it empty to use glab configuration."
    )]
    InvalidClientId,
    /// Storage/download validation failure; only safe, owned context is included.
    #[error("CLI installation failed: {0}")]
    Install(String),
}

/// Credential storage notice; authentication is still valid.
pub const STORAGE_WARNING: &str = "The CLI reported storing credentials outside Keychain. Review its credential storage settings.";

impl Connection {
    /// Verified account name, including a successful login with a storage warning.
    pub fn username(&self) -> Option<&str> {
        match self {
            Self::Connected(name) | Self::ConnectedWithWarning(name) => Some(name),
            _ => None,
        }
    }
}

pub(crate) fn cli_command_on(
    provider: Provider,
    host: &Host,
    cwd: Option<&std::path::Path>,
    args: &[&str],
) -> Option<Command> {
    let executable = if *host == Host::Local {
        provider.executable()?
    } else {
        provider.cli().into()
    };
    let executable = executable.to_string_lossy();
    let mut arguments = vec![
        "-u",
        "CI",
        "-u",
        "GITLAB_CI",
        "-u",
        "GH_FORCE_TTY",
        "-u",
        "GH_DEBUG",
        "-u",
        "GLAB_DEBUG_HTTP",
        "NO_COLOR=1",
        "CLICOLOR=0",
        "LC_ALL=C",
        "GH_PROMPT_DISABLED=1",
        "GH_NO_UPDATE_NOTIFIER=1",
        "GH_PAGER=cat",
        "GLAB_NO_PROMPT=1",
        "NO_PROMPT=1",
        "GLAB_SKIP_UPDATE_CHECK=1",
        "GLAB_PAGER=cat",
        &executable,
    ];
    arguments.extend_from_slice(args);
    Some(host.command("env", &arguments, cwd))
}
fn cli_command(provider: Provider, args: &[&str]) -> Result<Command, Error> {
    cli_command_on(provider, &Host::Local, None, args).ok_or(Error::MissingCli)
}

pub(crate) fn has_http_status(message: &str, status: &str) -> bool {
    message
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != ':' && c != '-')
        .any(|word| word == status || word.strip_prefix(status) == Some(":"))
}

/// Blocking: verify the chosen host using existing CLI credentials, without exposing tokens.
pub fn check(
    provider: Provider,
    server: &Server,
    cancellation: &Cancellation,
) -> Result<Connection, Error> {
    if provider.executable().is_none() {
        return Ok(Connection::MissingCli);
    }
    let output = command::run(
        cli_command(provider, &["api", "user", "--hostname", server.as_str()])?,
        &[],
        cancellation,
        Duration::from_secs(20),
        1024 * 1024,
    )?;
    account_response(provider, output)
}

fn account_response(provider: Provider, output: command::Output) -> Result<Connection, Error> {
    if output.code != Some(0) {
        let message = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        if [
            "not logged",
            "auth login",
            "authentication required",
            "could not find token",
        ]
        .iter()
        .any(|s| message.contains(s))
            || has_http_status(&message, "401")
        {
            return Ok(Connection::SignedOut);
        }
        return Err(if has_http_status(&message, "403") {
            Error::AccessDenied
        } else {
            Error::Unavailable
        });
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Account {
        login: String,
        username: String,
    }
    let account: Account =
        serde_json::from_slice(&output.stdout).map_err(|_| Error::InvalidResponse)?;
    let name = match provider {
        Provider::GitHub => account.login,
        Provider::GitLab => account.username,
    };
    if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return Err(Error::InvalidResponse);
    }
    Ok(Connection::Connected(name))
}

fn login_arguments(provider: Provider, server: &Server) -> Vec<&str> {
    let mut args = vec!["auth", "login", "--hostname", server.as_str(), "--web"];
    match provider {
        Provider::GitHub => args.extend(["--skip-ssh-key"]),
        Provider::GitLab => args.extend(["--api-protocol", "https"]),
    }
    args
}

/// Blocking: use the official browser sign-in flow. Cancel terminates its process group.
/// GitHub's device code is reported while the command is waiting; GitLab opens its browser.
pub fn sign_in(
    provider: Provider,
    server: &Server,
    cancellation: &Cancellation,
    client_id: Option<&str>,
    mut prompt: impl FnMut(DevicePrompt),
) -> Result<Connection, Error> {
    if let Some(client_id) = client_id.filter(|value| !value.is_empty()) {
        if provider != Provider::GitLab
            || server.as_str() == "gitlab.com"
            || !valid_client_id(client_id)
        {
            return Err(Error::InvalidClientId);
        }
        let output = command::run(
            cli_command(
                provider,
                &[
                    "config",
                    "set",
                    "client_id",
                    client_id,
                    "--host",
                    server.as_str(),
                ],
            )?,
            &[],
            cancellation,
            Duration::from_secs(10),
            4096,
        )?;
        if output.code != Some(0) {
            return Err(Error::SignIn);
        }
    }
    let mut last_prompt = None;
    let output = command::run_with_progress(
        cli_command(provider, &login_arguments(provider, server))?,
        &[],
        cancellation,
        Duration::from_secs(15 * 60),
        1024 * 1024,
        |_, stderr| {
            if let Some(value) = device_prompt(stderr, server)
                && last_prompt.as_ref() != Some(&value)
            {
                prompt(value.clone());
                last_prompt = Some(value);
            }
        },
    )?;
    if output.code != Some(0) {
        return Err(Error::SignIn);
    }
    let connection = check(provider, server, cancellation)?;
    Ok(storage_notice(connection, &output))
}

fn device_prompt(bytes: &[u8], server: &Server) -> Option<DevicePrompt> {
    let text = std::str::from_utf8(bytes).ok()?;
    let code = text.lines().find_map(|line| {
        line.split_once("First copy your one-time code: ")
            .map(|(_, code)| code.trim())
    })?;
    if !(4..=32).contains(&code.len())
        || !code
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-')
    {
        return None;
    }
    let url = text.lines().find_map(|line| {
        line.split_once("to continue in your web browser: ")
            .map(|(_, url)| url.trim())
    })?;
    let rest = url.strip_prefix("https://")?;
    if rest.split('/').next()? != server.as_str() || url.chars().any(char::is_whitespace) {
        return None;
    }
    Some(DevicePrompt {
        code: code.into(),
        url: url.into(),
    })
}

fn valid_client_id(value: &str) -> bool {
    (1..=256).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn storage_notice(connection: Connection, output: &command::Output) -> Connection {
    match connection {
        Connection::Connected(name) if insecure_storage(output) => {
            Connection::ConnectedWithWarning(name)
        }
        other => other,
    }
}

fn insecure_storage(output: &command::Output) -> bool {
    let messages = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .to_ascii_lowercase();
    ["plain text", "plaintext", "insecure storage"]
        .iter()
        .any(|value| messages.contains(value))
}
