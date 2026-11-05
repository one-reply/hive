//! The user's login-shell environment, captured once so that commands Hive runs see the same
//! `PATH` as a terminal.
//!
//! An app launched from Finder or the Dock inherits launchd's minimal environment
//! (`PATH=/usr/bin:/bin:/usr/sbin:/sbin`), so git hooks and CLIs installed by Homebrew, nvm or
//! into `~/.local/bin` are not found. Like Zed, Hive asks `$SHELL -l -i` to print its environment
//! and applies it to every [`Host::Local`](super::Host::Local) command.

use crate::command::{self, Cancellation};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

/// Startup files print banners and warnings before the shell runs our script; the markers
/// separate the environment from that noise.
const BEGIN: &str = "__HIVE_ENVIRONMENT_BEGIN__";
const END: &str = "__HIVE_ENVIRONMENT_END__";
/// A shell that waits for input or hangs in a startup file must not block forever; slow
/// version managers (nvm, conda) finish in a couple of seconds.
const TIMEOUT: Duration = Duration::from_secs(15);
/// Environments run into kilobytes, not megabytes; this also bounds a chatty startup file.
const OUTPUT_LIMIT: usize = 1024 * 1024;
/// Bookkeeping of the capturing shell itself, meaningless for the commands Hive runs.
const SHELL_OWN: [&str; 4] = ["PWD", "OLDPWD", "SHLVL", "_"];

/// Variables exported by a login shell, in the order it printed them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Environment {
    vars: Vec<(OsString, OsString)>,
}

impl Environment {
    /// Every `KEY=VALUE` pair, ready for [`Command::envs`].
    pub fn iter(&self) -> impl Iterator<Item = (&OsStr, &OsStr)> {
        self.vars
            .iter()
            .map(|(key, value)| (key.as_os_str(), value.as_os_str()))
    }

    /// The value of `key`, if the shell exported it.
    pub fn get(&self, key: &str) -> Option<&OsStr> {
        self.vars
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_os_str())
    }

    /// Number of exported variables.
    pub fn len(&self) -> usize {
        self.vars.len()
    }

    /// Whether the shell exported nothing.
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }
}

/// Why the login shell's environment could not be captured; Hive then keeps the process
/// environment it was launched with.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// `SHELL` is unset or empty, so there is no login shell to ask.
    #[error("SHELL is not set")]
    NoShell,
    /// The shell could not be started or did not finish within the deadline.
    #[error("`{shell} -l -i -c env` did not finish: {source}")]
    Run {
        /// The shell that was started.
        shell: String,
        /// The process or deadline failure.
        #[source]
        source: command::Error,
    },
    /// The shell exited without printing an environment between the markers.
    #[error(
        "`{shell} -l -i -c env` exited with {code:?} without printing its environment: {stderr}"
    )]
    Unparsed {
        /// The shell that was started.
        shell: String,
        /// Its exit code, when it exited normally.
        code: Option<i32>,
        /// What it printed to stderr, trimmed.
        stderr: String,
    },
}

static LOGIN: OnceLock<Environment> = OnceLock::new();

/// Blocking: capture `$SHELL`'s login environment and apply it to every later local command.
/// Call once at startup, off the UI thread; until it returns, commands keep the process environment.
pub fn load() -> Result<&'static Environment, CaptureError> {
    let shell = std::env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .ok_or(CaptureError::NoShell)?;
    let environment = capture(Path::new(&shell))?;
    Ok(install(environment))
}

/// Remember `environment` as the login environment for every later local command.
/// The first installed environment wins; later calls return it unchanged.
pub fn install(environment: Environment) -> &'static Environment {
    LOGIN.get_or_init(|| environment)
}

/// The captured login environment, once [`load`] has succeeded.
pub fn login() -> Option<&'static Environment> {
    LOGIN.get()
}

/// `PATH` as the user's terminal sees it, or the process `PATH` until the login shell answers.
pub fn path() -> Option<OsString> {
    login()
        .and_then(|environment| environment.get("PATH"))
        .map(OsStr::to_os_string)
        .or_else(|| std::env::var_os("PATH"))
}

/// Blocking: start `shell` as an interactive login shell and parse the environment it prints.
pub fn capture(shell: &Path) -> Result<Environment, CaptureError> {
    // `command env` bypasses aliases and functions; `-0` keeps multi-line values intact.
    let script = format!("printf '\\n{BEGIN}'; command env -0; printf '{END}'");
    let mut command = Command::new(shell);
    command.args(["-l", "-i", "-c", &script]);
    let name = || shell.to_string_lossy().into_owned();
    let output = command::run(
        command,
        b"",
        &Cancellation::default(),
        TIMEOUT,
        OUTPUT_LIMIT,
    )
    .map_err(|source| CaptureError::Run {
        shell: name(),
        source,
    })?;
    parse(&output.stdout).ok_or_else(|| CaptureError::Unparsed {
        shell: name(),
        code: output.code,
        stderr: String::from_utf8_lossy(&output.stderr).trim().into(),
    })
}

/// The variables between the markers, as `env -0` printed them; `None` when a marker is missing.
pub fn parse(output: &[u8]) -> Option<Environment> {
    let begin = find(output, BEGIN.as_bytes())? + BEGIN.len();
    let body = &output[begin..];
    let body = &body[..find(body, END.as_bytes())?];
    let vars =
        body.split(|byte| *byte == 0)
            .filter_map(|entry| {
                let separator = entry.iter().position(|byte| *byte == b'=')?;
                let (key, value) = (&entry[..separator], &entry[separator + 1..]);
                (!key.is_empty() && !SHELL_OWN.contains(&String::from_utf8_lossy(key).as_ref()))
                    .then(|| {
                        (
                            OsStr::from_bytes(key).into(),
                            OsStr::from_bytes(value).into(),
                        )
                    })
            })
            .collect();
    Some(Environment { vars })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from `zsh -l -i` on macOS with an `nvm` startup file: a warning before the
    /// marker, a multi-line value, an `=` inside a value and the shell's own bookkeeping.
    const SAMPLE: &[u8] = b".autocomplete__key-bindings:33: terminfo[kcbt]: parameter not set\n\
        \n__HIVE_ENVIRONMENT_BEGIN__\
        PATH=/Users/vitalii/.nvm/versions/node/v24.14.0/bin:/opt/homebrew/bin:/usr/bin:/bin\0\
        NVM_DIR=/Users/vitalii/.nvm\0\
        LESS=-R\n-F\0\
        CLAUDE_CODE_OAUTH_SCOPES=user:inference user:profile\0\
        GIT_CONFIG_PARAMETERS='user.name=Vitalii'\0\
        PWD=/Users/vitalii\0OLDPWD=/Users/vitalii\0SHLVL=1\0_=/usr/bin/env\0\
        __HIVE_ENVIRONMENT_END__";

    #[test]
    fn parses_captured_login_shell_output() {
        let environment = parse(SAMPLE).expect("markers are present");
        assert_eq!(
            environment.get("PATH").unwrap(),
            "/Users/vitalii/.nvm/versions/node/v24.14.0/bin:/opt/homebrew/bin:/usr/bin:/bin"
        );
        assert_eq!(environment.get("NVM_DIR").unwrap(), "/Users/vitalii/.nvm");
        assert_eq!(environment.get("LESS").unwrap(), "-R\n-F");
        assert_eq!(
            environment.get("GIT_CONFIG_PARAMETERS").unwrap(),
            "'user.name=Vitalii'"
        );
        for own in SHELL_OWN {
            assert!(
                environment.get(own).is_none(),
                "{own} belongs to the capturing shell"
            );
        }
        assert_eq!(environment.len(), 5);
    }

    #[test]
    fn missing_markers_are_not_an_environment() {
        assert!(parse(b"").is_none());
        assert!(parse(b"__HIVE_ENVIRONMENT_BEGIN__PATH=/bin\0").is_none());
        assert!(parse(b"PATH=/bin\0__HIVE_ENVIRONMENT_END__").is_none());
    }

    #[test]
    fn captures_a_real_login_shell() {
        let environment = capture(Path::new("/bin/sh")).expect("sh prints its environment");
        assert!(environment.get("PATH").is_some());
        assert!(environment.get("HOME").is_some());
        assert!(environment.get("SHLVL").is_none());
    }

    #[test]
    fn path_falls_back_to_the_process_until_installed() {
        assert_eq!(path(), std::env::var_os("PATH"));
    }

    /// Reproduces the Finder launch: run with launchd's PATH, e.g.
    /// `env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin:$HOME/.cargo/bin HOME=$HOME SHELL=$SHELL USER=$USER
    /// cargo test -p hive-core -- --ignored commit_hook`, and the hook must still find `npx`.
    #[test]
    #[ignore = "needs npx on the login shell's PATH and a real git; opt-in only"]
    fn commit_hook_runs_in_the_login_environment() {
        use std::os::unix::fs::PermissionsExt;
        let repo = std::env::temp_dir().join(format!("hive-hook-{}", std::process::id()));
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| crate::git::run(&crate::host::Host::Local, &repo, args);
        git(&["init", "-b", "main"]).unwrap();
        git(&["config", "user.name", "Hive Test"]).unwrap();
        git(&["config", "user.email", "hive@example.invalid"]).unwrap();
        git(&["config", "commit.gpgsign", "false"]).unwrap();
        let hook = repo.join(".git/hooks/pre-commit");
        std::fs::write(&hook, "#!/bin/sh\nnpx --version || exit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(repo.join("notes.txt"), "one\n").unwrap();
        git(&["add", "notes.txt"]).unwrap();
        load().expect("the login shell prints its environment");
        git(&["commit", "-m", "hook finds npx"]).expect("pre-commit finds npx on the login PATH");
        assert_eq!(git(&["log", "--format=%s"]).unwrap(), "hook finds npx");
        std::fs::remove_dir_all(&repo).unwrap();
    }
}
