//! Where a project lives. Remote v1 is simply "the same command, run through `ssh`".

pub mod environment;
mod login_shell;
pub mod preamble;
pub mod ssh_config;

pub(crate) use login_shell::quote;
use login_shell::{exec_sh, in_login_environment, quote_path};
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::{
    ffi::{OsStr, OsString},
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
    /// A remote one runs with `sh`, whatever the account's login shell is.
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
                let mut script = change_directory(cwd);
                script.push(quote(program));
                for arg in args {
                    script.push(" ");
                    script.push(quote(arg));
                }
                ssh(target, false, &script)
            }
        }
    }

    /// Run the POSIX `script` with `sh` in the account's login environment, inside `cwd`, so
    /// tools resolve as in the user's terminal. Locally that is the environment
    /// [`environment::load`] captured; over SSH the login shell reads its profile first.
    ///
    /// stdout carries whatever startup files print, then [`preamble::MARKER`], then the
    /// script's own output: read it with [`preamble::skip`] or [`preamble::strip`].
    pub fn login_command(&self, script: &str, cwd: Option<&Path>) -> Command {
        let action = format!("printf %s {}; {script}", preamble::MARKER);
        match self {
            Host::Local => self.command("sh", &["-c", &action], cwd),
            Host::Ssh { target } => {
                let mut posix = change_directory(cwd);
                posix.push(in_login_environment(OsStr::new(&action)));
                ssh(target, false, &posix)
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
        let mut posix = change_directory(Some(cwd));
        match program {
            Some((program, args)) => {
                let mut action = format!("exec {}", quote(program));
                for arg in args {
                    action.push(' ');
                    action.push_str(&quote(arg));
                }
                posix.push(in_login_environment(OsStr::new(&action)));
            }
            // `-l` on its own is the one form every login shell, csh included, accepts.
            None => posix.push("exec \"${SHELL:-/bin/sh}\" -l"),
        }
        Ok(ssh(target, true, &posix))
    }

    /// Check a remote CLI in the login environment without starting the agent.
    pub fn check_executable(&self, program: &str) -> Result<(), String> {
        let script = format!("command -v {} >/dev/null", quote(program));
        let output = crate::command::run(
            self.login_command(&script, None),
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

/// `cd <dir> && `, the start of a remote POSIX script, or nothing without a directory.
fn change_directory(cwd: Option<&Path>) -> OsString {
    let mut script = OsString::new();
    if let Some(dir) = cwd {
        script.push("cd ");
        script.push(quote_path(dir));
        script.push(" && ");
    }
    script
}

/// sshd hands the command line to the account's login shell, so `posix` travels as
/// `exec sh -c <posix>` written for any of them.
fn ssh(target: &str, interactive: bool, posix: &OsStr) -> Command {
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
    command.arg(exec_sh(posix));
    command
}

/// What sshd runs for `command` on an account whose login shell is `shell`: `shell -c <line>`
/// in `home`, with sshd's small environment, here on this machine.
#[cfg(test)]
pub(crate) fn as_login_shell(command: &Command, shell: &Path, home: &Path) -> Command {
    assert_eq!(command.get_program(), "/usr/bin/ssh");
    let line = command
        .get_args()
        .last()
        .expect("ssh passes one command line");
    let mut local = Command::new(shell);
    local
        .arg("-c")
        .arg(line)
        .current_dir(home)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .env("SHELL", shell);
    local
}

#[cfg(test)]
mod tests;
