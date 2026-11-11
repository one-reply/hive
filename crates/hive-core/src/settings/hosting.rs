//! Last selected hosting servers, without account names or credentials.
use crate::git::hosting::{Provider, Server};
use serde::{Deserialize, Serialize};

/// Servers shown in Git settings. Missing, malformed, or future fields are tolerated.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HostingServers {
    #[serde(deserialize_with = "crate::state::lenient::or_default")]
    github: Option<Server>,
    #[serde(deserialize_with = "crate::state::lenient::or_default")]
    gitlab: Option<Server>,
}
impl HostingServers {
    /// Last chosen server, falling back to the public service.
    pub fn server(&self, provider: Provider) -> Server {
        match provider {
            Provider::GitHub => &self.github,
            Provider::GitLab => &self.gitlab,
        }
        .clone()
        .unwrap_or_else(|| {
            Server::parse(provider.default_host()).expect("built-in hostname is valid")
        })
    }
    /// Remember a validated server; return whether it changed.
    pub fn set(&mut self, provider: Provider, server: Server) -> bool {
        if self.server(provider) == server {
            return false;
        }
        *match provider {
            Provider::GitHub => &mut self.github,
            Provider::GitLab => &mut self.gitlab,
        } = Some(server);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    #[test]
    fn old_settings_default_to_public_hosts_and_selected_server_roundtrips() {
        let mut settings: Settings =
            serde_json::from_str(r#"{"theme":"dark","future":true}"#).unwrap();
        assert_eq!(
            settings.hosting_servers.server(Provider::GitHub).as_str(),
            "github.com"
        );
        assert_eq!(
            settings.hosting_servers.server(Provider::GitLab).as_str(),
            "gitlab.com"
        );
        assert!(settings.hosting_servers.set(
            Provider::GitLab,
            Server::parse("code.example:8443").unwrap()
        ));
        let restored: Settings =
            serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(
            restored.hosting_servers.server(Provider::GitLab).as_str(),
            "code.example:8443"
        );
    }
    #[test]
    fn invalid_saved_host_cannot_become_a_cli_argument_or_erase_the_other_host() {
        for value in [r#""https://evil.test/path""#, "42", "null", r#""--option""#] {
            let settings: Settings=serde_json::from_str(&format!(r#"{{"hosting_servers":{{"github":{value},"gitlab":"code.example","future":true}}}}"#)).unwrap();
            assert_eq!(
                settings.hosting_servers.server(Provider::GitHub).as_str(),
                "github.com"
            );
            assert_eq!(
                settings.hosting_servers.server(Provider::GitLab).as_str(),
                "code.example"
            );
        }
        let settings: Settings = serde_json::from_str(r#"{"hosting_servers":42}"#).unwrap();
        assert_eq!(
            settings.hosting_servers.server(Provider::GitLab).as_str(),
            "gitlab.com"
        );
    }
}
