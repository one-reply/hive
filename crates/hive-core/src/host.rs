//! Where a project lives. Remote v1 is simply "the same command, run through `ssh`".

pub mod environment;
pub mod ssh_config;

use serde::{Deserialize, Serialize};
use std::process::Command;
use std::{
    ffi::{OsStr, OsString},
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::Path,
};

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
                let mut script = OsString::new();
                if let Some(dir) = cwd {
                    script.push("cd ");
                    script.push(quote_path(dir));
                    script.push(" && ");
                }
                script.push(quote(program));
                for arg in args {
                    script.push(" ");
                    script.push(quote(arg));
                }
                ssh(target, false, &script)
            }
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Host::Local)
    }

    /// Reject command fragments: SSH destinations are one alias or user@host, never flags.
    pub fn validate(&self) -> std::io::Result<()> {
        if let Self::Ssh { target } = self
            && (target.is_empty()
                || target.starts_with('-')
                || target.chars().any(|c| c.is_whitespace() || c.is_control()))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Enter one SSH alias or user@host, without options",
            ));
        }
        Ok(())
    }

    /// Open a remote login shell, or a CLI in that shell's environment, with a real PTY.
    pub fn terminal_command(
        &self,
        cwd: &Path,
        program: Option<(&str, &[&str])>,
    ) -> std::io::Result<Command> {
        self.validate()?;
        let Self::Ssh { target } = self else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "SSH terminal requires a remote host",
            ));
        };
        let action = match program {
            Some((program, args)) => {
                let mut action = format!("exec {}", quote(program));
                for arg in args {
                    action.push(' ');
                    action.push_str(&quote(arg));
                }
                format!("exec \"${{SHELL:-/bin/sh}}\" -lc {}", quote(&action))
            }
            None => "exec \"${SHELL:-/bin/sh}\" -l".into(),
        };
        let mut posix = OsString::from("cd ");
        posix.push(quote_path(cwd));
        posix.push(format!(" && {action}"));
        // sshd hands this line to the account's login shell, which may be fish or csh; neither
        // reads `${SHELL:-…}`. `exec sh -c '…'` means the same in all of them.
        let mut script = OsString::from("exec sh -c ");
        script.push(quote_for_login_shell(&posix));
        Ok(ssh(target, true, &script))
    }

    /// Check a remote CLI in the login environment without starting the agent.
    pub fn check_executable(&self, program: &str) -> Result<(), String> {
        let action = format!("command -v {} >/dev/null", quote(program));
        let script = format!("exec \"${{SHELL:-/bin/sh}}\" -lc {}", quote(&action));
        let output = crate::command::run(
            self.command("sh", &["-c", &script], None),
            &[],
            &crate::command::Cancellation::default(),
            std::time::Duration::from_secs(15),
            65536,
        )
        .map_err(|e| e.to_string())?;
        if output.code == Some(0) {
            Ok(())
        } else {
            Err(format!(
                "{program} is unavailable on {self:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            ))
        }
    }
}

fn ssh(target: &str, interactive: bool, script: &OsStr) -> Command {
    let mut command = Command::new("/usr/bin/ssh");
    command.args([
        if interactive { "-tt" } else { "-T" },
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "--",
        target,
    ]);
    command.arg(script);
    command
}

fn quote_path(path: &Path) -> OsString {
    let mut bytes = vec![b'\''];
    for &byte in path.as_os_str().as_bytes() {
        if byte == b'\'' {
            bytes.extend_from_slice(b"'\\''");
        } else {
            bytes.push(byte);
        }
    }
    bytes.push(b'\'');
    OsString::from_vec(bytes)
}

/// Single-quotes `value` so that sh, fish and csh read back the same bytes. Inside single
/// quotes fish treats `\\` and `\'` as escapes, so a backslash is written outside them.
fn quote_for_login_shell(value: &OsStr) -> OsString {
    let mut bytes = vec![b'\''];
    for &byte in value.as_bytes() {
        match byte {
            b'\'' => bytes.extend_from_slice(b"'\\''"),
            b'\\' => bytes.extend_from_slice(b"'\\\\'"),
            _ => bytes.push(byte),
        }
    }
    bytes.push(b'\'');
    OsString::from_vec(bytes)
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
    fn ssh_cwd_keeps_non_utf8_bytes_and_quotes_metacharacters() {
        let host = Host::Ssh {
            target: "home".into(),
        };
        let path =
            std::path::PathBuf::from(OsString::from_vec(b"/tmp/\xff'$(touch nope)".to_vec()));
        let command = host.command("pwd", &[], Some(&path));
        let script = command.get_args().last().unwrap().as_bytes();
        assert!(script.starts_with(b"cd '/tmp/\xff'\\''$(touch nope)' && "));
        let terminal = host.terminal_command(&path, None).unwrap();
        let script = read_by_sh(terminal.get_args().last().unwrap());
        assert!(script.starts_with(b"cd '/tmp/\xff'\\''$(touch nope)' && exec "));
        assert!(!script.windows(3).any(|bytes| bytes == [0xef, 0xbf, 0xbd]));
    }

    /// What `sh` receives as its script from `exec sh -c '…'`: the real `/bin/sh` unquotes it.
    fn read_by_sh(line: &OsStr) -> Vec<u8> {
        let quoted = line
            .as_bytes()
            .strip_prefix(b"exec sh -c ")
            .expect("terminal line starts with exec sh -c");
        let mut print = b"printf %s ".to_vec();
        print.extend_from_slice(quoted);
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(OsString::from_vec(print))
            .output()
            .unwrap();
        assert!(output.status.success());
        output.stdout
    }

    #[test]
    fn remote_terminal_line_reads_the_same_in_sh_fish_and_csh() {
        let host = Host::Ssh {
            target: "home".into(),
        };
        let command = host
            .terminal_command(Path::new("/srv/it's\\here"), None)
            .unwrap();
        let line = command.get_args().last().unwrap();
        assert_eq!(
            read_by_sh(line),
            b"cd '/srv/it'\\''s\\here' && exec \"${SHELL:-/bin/sh}\" -l"
        );
        // Outside `sh -c '…'` nothing is left for the login shell to expand, and a backslash
        // only ever appears as `'\''` or `'\\'`, which fish and csh read as sh does.
        let line = line.to_str().unwrap();
        let literal = line.replace("'\\''", "").replace("'\\\\'", "");
        assert!(!literal.contains('\\'));
        assert!(line.starts_with("exec sh -c 'cd "));
        assert!(line.ends_with('\''));
    }

    #[test]
    fn remote_terminal_uses_remote_cwd_and_never_local_current_dir() {
        let host = Host::Ssh {
            target: "home".into(),
        };
        let command = host
            .terminal_command(
                Path::new("/srv/my app's code"),
                Some(("codex", &["--model", "a b"])),
            )
            .unwrap();
        assert!(command.get_current_dir().is_none());
        let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy()).collect();
        assert_eq!(args[0], "-tt");
        let script = String::from_utf8(read_by_sh(command.get_args().last().unwrap())).unwrap();
        assert!(script.starts_with("cd '/srv/my app'\\''s code' && exec"));
        assert!(script.contains("${SHELL:-/bin/sh}"));
        assert!(
            !args
                .iter()
                .any(|arg| arg.contains("StrictHostKeyChecking=no"))
        );
        assert!(
            Host::Ssh {
                target: "home -A".into()
            }
            .validate()
            .is_err()
        );
    }

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
        assert_eq!(cmd.get_program(), "/usr/bin/ssh");
        assert_eq!(
            args.last().unwrap(),
            "cd '/my dir' && git log '--format=%s it'\\''s'"
        );
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
        assert_eq!(args[args.len() - 3], "--");
        assert_eq!(args[args.len() - 2], "-oProxyCommand=touch /tmp/pwned");
        assert!(host.validate().is_err());
    }
}
