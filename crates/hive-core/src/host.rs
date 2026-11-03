//! Where a project lives. Remote v1 is simply "the same command, run through `ssh`".

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Hash, Debug, Default)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Host {
    #[default]
    Local,
    /// An `ssh` target as the user would type it: `user@box`, or a `~/.ssh/config` alias.
    Ssh { target: String },
}

impl Host {
    /// Build a command to run `program args` on this host, optionally inside `cwd`.
    pub fn command(&self, program: &str, args: &[&str], cwd: Option<&Path>) -> Command {
        match self {
            Host::Local => {
                let mut cmd = Command::new(program);
                cmd.args(args);
                if let Some(dir) = cwd {
                    cmd.current_dir(dir);
                }
                cmd
            }
            Host::Ssh { target } => {
                let mut script = String::new();
                if let Some(dir) = cwd {
                    script.push_str(&format!("cd {} && ", quote(&dir.to_string_lossy())));
                }
                script.push_str(&quote(program));
                for arg in args {
                    script.push(' ');
                    script.push_str(&quote(arg));
                }
                let mut cmd = Command::new("ssh");
                cmd.args(["-o", "BatchMode=yes", target, &script]);
                cmd
            }
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Host::Local)
    }
}

/// POSIX single-quote escaping.
fn quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_command_is_quoted() {
        let host = Host::Ssh { target: "dev".into() };
        let cmd = host.command("git", &["log", "--format=%s it's"], Some(Path::new("/my dir")));
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(cmd.get_program(), "ssh");
        assert_eq!(args[3], "cd '/my dir' && git log '--format=%s it'\\''s'");
    }
}
