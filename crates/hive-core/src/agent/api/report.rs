//! An attempt's local subprocesses prove their pane identity without exposing a secret to the model.
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    fs::File,
    io::{self, Read},
    process::Command,
};

/// Environment inherited by the reporting CLI or forwarded only to the pane's Hive MCP server.
pub const REPORT_GRANT_ENV: &str = "HIVE_PIPELINE_GRANT";

/// Ephemeral bearer grant, never persisted or included in prompts.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReportGrant(String);

impl fmt::Debug for ReportGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReportGrant([redacted])")
    }
}

impl ReportGrant {
    /// Blocking: create a fresh capability before spawning one pipeline PTY.
    pub fn generate() -> io::Result<Self> {
        let mut bytes = [0_u8; 32];
        File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        Ok(Self(URL_SAFE_NO_PAD.encode(bytes)))
    }

    /// Read the caller's grant when constructing a CLI or MCP report.
    pub fn inherited() -> Option<Self> {
        std::env::var(REPORT_GRANT_ENV)
            .ok()
            .filter(|v| v.len() == 43)
            .map(Self)
    }

    /// Give this grant to a pipeline subprocess.
    pub fn attach(&self, command: &mut Command) {
        command.env(REPORT_GRANT_ENV, &self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn each_pane_gets_a_distinct_grant_kept_out_of_debug_output() {
        let first = ReportGrant::generate().unwrap();
        assert_ne!(first, ReportGrant::generate().unwrap());
        let mut command = Command::new("test");
        first.attach(&mut command);
        assert!(!format!("{first:?}").contains(&first.0));
        assert_eq!(first.0.len(), 43);
        assert_eq!(command.get_envs().next().unwrap().0, REPORT_GRANT_ENV);
    }
}
