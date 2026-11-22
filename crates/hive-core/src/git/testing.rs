//! The one Git environment of every test that runs `git`, in this crate and in the app.
//!
//! Git reads the developer's own configuration into every command: `commit.gpgsign` asks for
//! a key, `core.hooksPath` runs their hooks, a global ignore file hides what a test just
//! wrote, `init.defaultBranch` renames the branch a test expects. A fixture that turned these
//! off one by one would always be one setting behind, and could not reach the commands the
//! code under test starts. So the isolation is installed once, as the environment
//! [`Host::Local`] gives every command, and every fixture starts Git through this module.
//!
//! Not covered: Git variables already exported to the test process, such as `GIT_DIR` when
//! the tests run from a Git hook. Variables can be added to a command here, not taken away.

use crate::host::Host;
use crate::host::environment::{self, Environment};
use std::path::Path;
use std::process::Command;

/// What every local command gets once [`isolate`] ran; all of it is read by Git alone.
const ISOLATION: [(&str, &str); 7] = [
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    // Without these Git still reads `~/.config/git/ignore` and `~/.config/git/attributes`,
    // which no configuration file has to mention.
    ("GIT_CONFIG_COUNT", "2"),
    ("GIT_CONFIG_KEY_0", "core.excludesFile"),
    ("GIT_CONFIG_VALUE_0", "/dev/null"),
    ("GIT_CONFIG_KEY_1", "core.attributesFile"),
    ("GIT_CONFIG_VALUE_1", "/dev/null"),
];

/// Makes every command this process starts on [`Host::Local`] from now on run Git without the
/// developer's configuration: the fixture's commands and those of the code under test alike.
/// Call it before the first `git`; calling it again costs nothing.
///
/// # Panics
///
/// When something else installed a login environment first: the isolation would silently not
/// apply, and the tests would depend on the machine again.
pub fn isolate() {
    let isolation = Environment::of(ISOLATION);
    assert!(
        *environment::install(isolation.clone()) == isolation,
        "a login environment was installed before the Git test isolation; install only this one in tests"
    );
}

/// An isolated `git args` in `dir`, for a fixture step that needs more than [`run`] gives,
/// such as its own variables.
pub fn git(dir: &Path, args: &[&str]) -> Command {
    isolate();
    Host::Local.command("git", args, Some(dir))
}

/// Blocking: runs an isolated `git args` in `dir` and returns its trimmed standard output.
///
/// # Panics
///
/// With Git's own message when the command fails: a fixture that cannot be built is a
/// broken test, not a case to handle.
pub fn run(dir: &Path, args: &[&str]) -> String {
    isolate();
    crate::git::run(&Host::Local, dir, args).unwrap_or_else(|error| panic!("{error}"))
}

/// Blocking: creates `dir` as an empty repository on the branch `main`, with an author so
/// that a commit needs nothing else, and with no template: no sample hooks, no `info/exclude`.
pub fn init(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
    for args in [
        &["init", "-q", "-b", "main", "--template="][..],
        &["config", "user.name", "Hive Test"],
        &["config", "user.email", "hive@example.invalid"],
    ] {
        run(dir, args);
    }
}

/// Installs `body` as the `sh` hook `name` of the repository at `repo` and turns its hooks on.
pub fn hook(repo: &Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let hooks = repo.join(".git/hooks");
    let hook = hooks.join(name);
    let installed = std::fs::create_dir_all(&hooks)
        .and_then(|()| std::fs::write(&hook, format!("#!/bin/sh\n{body}")))
        .and_then(|()| std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)));
    installed.unwrap_or_else(|error| panic!("{}: {error}", hook.display()));
    run(
        repo,
        &["config", "core.hooksPath", &hooks.to_string_lossy()],
    );
}

/// Makes the hook `name` of `repo` say that it started and then idle; the returned channel
/// yields once it runs. A command that is stopped ends the idling at once; left alone the hook
/// ends by itself, so a command that ignores its stop fails the test instead of hanging it.
pub fn stalling_hook(repo: &Path, name: &str) -> std::sync::mpsc::Receiver<()> {
    let started = repo.join(".git/hook-started");
    let made = Host::Local
        .command("mkfifo", &[&started.to_string_lossy()], None)
        .status();
    assert!(
        made.is_ok_and(|status| status.success()),
        "mkfifo {started:?}"
    );
    // Writing to the pipe waits for the reader below, so the announcement is read while the
    // hook is still running.
    hook(
        repo,
        name,
        &format!("echo started > '{}'\nsleep 5\n", started.display()),
    );
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if std::fs::read_to_string(started).is_ok_and(|text| text == "started\n") {
            let _ = sender.send(());
        }
    });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("hive-git-testing-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A home folder whose Git configuration breaks every assumption a fixture makes.
    fn hostile_home(scratch: &Scratch) -> PathBuf {
        let home = scratch.0.join("home");
        let hooks = home.join("hooks");
        fs::create_dir_all(home.join(".config/git")).unwrap();
        fs::create_dir_all(&hooks).unwrap();
        fs::write(
            home.join(".gitconfig"),
            format!(
                "[commit]\n\tgpgsign = true\n[init]\n\tdefaultBranch = hostile\n[core]\n\thooksPath = {}\n",
                hooks.display()
            ),
        )
        .unwrap();
        fs::write(home.join(".config/git/ignore"), "*\n").unwrap();
        fs::write(home.join(".config/git/attributes"), "* text eol=crlf\n").unwrap();
        fs::write(hooks.join("pre-commit"), "#!/bin/sh\nexit 1\n").unwrap();
        let made_executable = Host::Local
            .command(
                "chmod",
                &["+x", hooks.join("pre-commit").to_str().unwrap()],
                None,
            )
            .status()
            .unwrap();
        assert!(made_executable.success());
        home
    }

    #[test]
    fn a_developers_configuration_ignore_file_and_hooks_do_not_reach_git() {
        let scratch = Scratch::new("hostile");
        let home = hostile_home(&scratch);
        let repo = scratch.0.join("repo");
        fs::create_dir_all(&repo).unwrap();
        // The child's own home: the test process keeps its environment.
        let at_home = |args: &[&str]| {
            let mut git = git(&repo, args);
            git.env("HOME", &home).env_remove("XDG_CONFIG_HOME");
            let output = git.output().unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout)
                .trim_end()
                .to_owned()
        };
        at_home(&["init", "-q"]);
        assert_ne!(at_home(&["branch", "--show-current"]), "hostile");
        fs::write(repo.join("notes.txt"), "one\ntwo\n").unwrap();
        // The home's ignore file would hide it.
        assert_eq!(at_home(&["status", "--porcelain"]), "?? notes.txt");
        at_home(&["config", "user.name", "Hive Test"]);
        at_home(&["config", "user.email", "hive@example.invalid"]);
        at_home(&["add", "notes.txt"]);
        // Neither the signing key nor the home's failing hook is asked.
        at_home(&["commit", "-q", "-m", "notes"]);
        // And its attributes would have marked the file for CRLF conversion.
        assert!(
            at_home(&["ls-files", "--eol", "notes.txt"]).contains("attr/ "),
            "no attribute applies"
        );
        let origins = at_home(&["config", "--show-origin", "--name-only", "--list"]);
        assert!(
            origins
                .lines()
                .all(|line| line.starts_with("file:.git/config")
                    || line.starts_with("command line:")),
            "{origins}"
        );
    }

    #[test]
    fn a_new_repository_is_on_main_with_an_author_and_no_hooks() {
        let scratch = Scratch::new("fresh");
        let repo = scratch.0.join("fresh");
        init(&repo);
        assert_eq!(run(&repo, &["branch", "--show-current"]), "main");
        run(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
        assert_eq!(
            run(&repo, &["log", "--format=%an <%ae>"]),
            "Hive Test <hive@example.invalid>"
        );
        assert!(!repo.join(".git/hooks").exists());
    }

    /// Every fixture starts Git through this module: a `Command` built by hand misses the
    /// isolation, and nothing would say so until it failed on someone's machine.
    #[test]
    fn no_test_starts_git_around_the_isolation() {
        // Assembled so that this file does not contain what it looks for.
        let by_hand = ["Command::new(", "\"git\")"].concat();
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut offenders = Vec::new();
        let mut folders = vec![crates.to_owned()];
        while let Some(folder) = folders.pop() {
            for entry in fs::read_dir(&folder).unwrap().map(Result::unwrap) {
                let path = entry.path();
                if path.is_dir() {
                    folders.push(path);
                } else if path.extension().is_some_and(|extension| extension == "rs")
                    && fs::read_to_string(&path).unwrap().contains(&by_hand)
                {
                    offenders.push(path);
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "start Git with git::testing instead: {offenders:?}"
        );
    }
}
