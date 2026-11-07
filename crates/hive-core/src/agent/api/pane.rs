//! What a local terminal gets so `hive agent …` reaches this app as that terminal.
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What [`environment`] sets; a Hive started from another Hive's pane inherits them.
const VARIABLES: [&str; 3] = ["HIVE_SOCKET", "HIVE_TERMINAL", "HIVE_BIN"];

/// Blocking: create `directory` with a `hive` link to `executable`, for terminals' `PATH`.
///
/// The directory belongs to one app instance, so a debug build and the installed app never
/// overwrite each other's launcher.
pub fn link_launcher(directory: &Path, executable: &Path) -> io::Result<PathBuf> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)?;
    std::os::unix::fs::symlink(executable, directory.join("hive"))?;
    Ok(directory.to_owned())
}

/// Identify `terminal` to the `hive` CLI and put `launcher` (from [`link_launcher`]) first on
/// the command's `PATH`. `HIVE_BIN` names the executable for shells that rebuild `PATH`.
pub fn environment(
    command: &mut Command,
    socket: &Path,
    terminal: u64,
    launcher: Option<&Path>,
    executable: &Path,
) {
    command
        .env("HIVE_SOCKET", socket)
        .env("HIVE_TERMINAL", terminal.to_string())
        .env("HIVE_BIN", executable);
    let Some(launcher) = launcher else {
        return;
    };
    let path = command_path(command).or_else(crate::host::environment::path);
    let paths =
        std::iter::once(launcher.to_owned()).chain(path.iter().flat_map(std::env::split_paths));
    match std::env::join_paths(paths) {
        Ok(path) => {
            command.env("PATH", path);
        }
        Err(error) => tracing::warn!(%error, "could not add the hive launcher to PATH"),
    }
}

/// Drops identity inherited from an outer Hive, so a pane without this app's API cannot reach
/// the outer app's panes by id. [`environment`] afterwards sets this app's values.
pub fn clear_environment(command: &mut Command) {
    for variable in VARIABLES {
        command.env_remove(variable);
    }
}

/// `PATH` the command already sets, such as the CLI directory of an agent launch.
fn command_path(command: &Command) -> Option<OsString> {
    command
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .and_then(|(_, value)| value.map(OsString::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("hive-pane-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        directory
    }

    fn variable(command: &Command, name: &str) -> Option<OsString> {
        command
            .get_envs()
            .find(|(key, _)| *key == name)
            .and_then(|(_, value)| value.map(OsString::from))
    }

    #[test]
    fn launcher_links_hive_to_the_running_executable() {
        let directory = scratch("launcher").join("bin");
        let executable = Path::new("/Applications/Hive.app/Contents/MacOS/hive");
        assert_eq!(link_launcher(&directory, executable).unwrap(), directory);
        assert_eq!(fs::read_link(directory.join("hive")).unwrap(), executable);
        let _ = fs::remove_dir_all(directory.parent().unwrap());
    }

    #[test]
    fn terminal_learns_its_identity_and_finds_hive_first_on_path() {
        let mut command = Command::new("true");
        command.env("PATH", "/opt/claude/bin:/usr/bin");
        environment(
            &mut command,
            Path::new("/tmp/hh/a"),
            42,
            Some(Path::new("/tmp/hh/bin")),
            Path::new("/Apps/hive"),
        );
        assert_eq!(variable(&command, "HIVE_SOCKET").unwrap(), "/tmp/hh/a");
        assert_eq!(variable(&command, "HIVE_TERMINAL").unwrap(), "42");
        assert_eq!(variable(&command, "HIVE_BIN").unwrap(), "/Apps/hive");
        assert_eq!(
            variable(&command, "PATH").unwrap(),
            "/tmp/hh/bin:/opt/claude/bin:/usr/bin"
        );
    }

    #[test]
    fn a_shell_without_its_own_path_gets_the_login_path_after_the_launcher() {
        let mut command = Command::new("true");
        environment(
            &mut command,
            Path::new("/tmp/hh/a"),
            1,
            Some(Path::new("/tmp/hh/bin")),
            Path::new("/Apps/hive"),
        );
        let path = variable(&command, "PATH").unwrap();
        let mut entries = std::env::split_paths(&path);
        assert_eq!(entries.next().unwrap(), Path::new("/tmp/hh/bin"));
        assert!(entries.next().is_some(), "login or process PATH follows");
    }

    #[test]
    fn identity_inherited_from_an_outer_hive_is_dropped_then_replaced() {
        let mut command = Command::new("env");
        command
            .env("HIVE_SOCKET", "/outer/a")
            .env("HIVE_TERMINAL", "7");
        clear_environment(&mut command);
        let output = command.output().expect("env is always available");
        let env = String::from_utf8_lossy(&output.stdout);
        assert!(
            !env.contains("HIVE_SOCKET=") && !env.contains("HIVE_TERMINAL="),
            "{env}"
        );
        environment(
            &mut command,
            Path::new("/inner/a"),
            2,
            None,
            Path::new("/Apps/hive"),
        );
        assert_eq!(variable(&command, "HIVE_SOCKET").unwrap(), "/inner/a");
    }

    #[test]
    fn without_a_launcher_path_is_left_alone() {
        let mut command = Command::new("true");
        environment(
            &mut command,
            Path::new("/tmp/hh/a"),
            1,
            None,
            Path::new("/Apps/hive"),
        );
        assert!(variable(&command, "PATH").is_none());
        assert_eq!(variable(&command, "HIVE_TERMINAL").unwrap(), "1");
    }
}
