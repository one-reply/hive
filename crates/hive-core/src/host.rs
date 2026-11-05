//! Where a project lives. Remote v1 is simply "the same command, run through `ssh`".

pub mod environment;

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
    /// A local command runs in the user's login-shell environment once
    /// [`environment::load`] has captured it, so hooks and CLIs resolve as in a terminal.
    pub fn command(&self, program: &str, args: &[&str], cwd: Option<&Path>) -> Command {
        match self {
            Host::Local => {
                let mut cmd = Command::new(program);
                cmd.args(args);
                if let Some(login) = environment::login() {
                    cmd.envs(login.iter());
                }
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
                // `--` keeps a target that starts with `-` from being read as an ssh option.
                cmd.args(["-o", "BatchMode=yes", "--", target, &script]);
                cmd
            }
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Host::Local)
    }
}

/// POSIX single-quote escaping.
pub(crate) fn quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_command_runs_in_the_login_environment() {
        let login = environment::parse(
            b"__HIVE_ENVIRONMENT_BEGIN__HIVE_LOGIN_SHELL_TEST=1\0__HIVE_ENVIRONMENT_END__",
        )
        .expect("markers are present");
        environment::install(login);
        let cmd = Host::Local.command("git", &["status"], None);
        let value = cmd
            .get_envs()
            .find(|(key, _)| *key == "HIVE_LOGIN_SHELL_TEST")
            .and_then(|(_, value)| value);
        assert_eq!(value, Some(std::ffi::OsStr::new("1")));
    }

    #[test]
    fn ssh_command_is_quoted() {
        let host = Host::Ssh {
            target: "dev".into(),
        };
        let cmd = host.command(
            "git",
            &["log", "--format=%s it's"],
            Some(Path::new("/my dir")),
        );
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(cmd.get_program(), "ssh");
        assert_eq!(args[4], "cd '/my dir' && git log '--format=%s it'\\''s'");
    }

    #[test]
    fn ssh_target_never_becomes_an_option() {
        let host = Host::Ssh {
            target: "-oProxyCommand=touch /tmp/pwned".into(),
        };
        let cmd = host.command("git", &["status"], None);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[2], "--");
        assert_eq!(args[3], "-oProxyCommand=touch /tmp/pwned");
    }
}
