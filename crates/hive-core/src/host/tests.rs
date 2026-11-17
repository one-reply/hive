use super::*;
use std::{
    fs,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

/// A per-test directory under the system temporary directory, removed afterwards.
struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "hive-host-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn remote() -> Host {
    Host::Ssh {
        target: "home".into(),
    }
}

#[test]
fn ssh_cwd_keeps_non_utf8_bytes_and_quotes_metacharacters() {
    let path = PathBuf::from(OsString::from_vec(b"/tmp/\xff'$(touch nope)".to_vec()));
    let command = remote().command("pwd", &[], Some(&path));
    let script = read_by_sh(command.get_args().last().unwrap());
    assert_eq!(script, b"cd '/tmp/\xff'\\''$(touch nope)' && pwd");
    let terminal = remote().terminal_command(&path, None).unwrap();
    let script = read_by_sh(terminal.get_args().last().unwrap());
    assert!(script.starts_with(b"cd '/tmp/\xff'\\''$(touch nope)' && exec "));
    assert!(!script.windows(3).any(|bytes| bytes == [0xef, 0xbf, 0xbd]));
}

/// What `sh` receives as its script from `exec sh -c '…'`: the real `/bin/sh` unquotes it.
fn read_by_sh(line: &OsStr) -> Vec<u8> {
    let quoted = line
        .as_bytes()
        .strip_prefix(b"exec sh -c ")
        .expect("remote line starts with exec sh -c");
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
    let command = remote()
        .terminal_command(Path::new("/srv/it's\\here!"), None)
        .unwrap();
    let line = command.get_args().last().unwrap();
    assert_eq!(
        read_by_sh(line),
        b"cd '/srv/it'\\''s\\here!' && exec \"${SHELL:-/bin/sh}\" -l"
    );
    // Outside `sh -c '…'` nothing is left for the login shell to expand, and a backslash
    // only ever appears as `'\''`, `'\\'` or `'\!'`, which fish and csh read as sh does.
    let line = line.to_str().unwrap();
    let literal = line
        .replace("'\\''", "")
        .replace("'\\\\'", "")
        .replace("'\\!'", "");
    assert!(!literal.contains('\\') && !literal.contains('!'));
    assert!(line.starts_with("exec sh -c 'cd "));
    assert!(line.ends_with('\''));
}

#[test]
fn remote_terminal_uses_remote_cwd_and_never_local_current_dir() {
    let command = remote()
        .terminal_command(
            Path::new("/srv/my app's code"),
            Some(("codex", &["--model", "a b"])),
        )
        .unwrap();
    assert!(command.get_current_dir().is_none());
    let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy()).collect();
    assert_eq!(args[0], "-tt");
    let script = String::from_utf8(read_by_sh(command.get_args().last().unwrap())).unwrap();
    assert!(script.starts_with("cd '/srv/my app'\\''s code' && login=-lc"));
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
    let cmd = remote().command(
        "git",
        &["log", "--format=%s it's"],
        Some(Path::new("/my dir")),
    );
    assert_eq!(cmd.get_program(), "/usr/bin/ssh");
    assert_eq!(
        read_by_sh(cmd.get_args().last().unwrap()),
        b"cd '/my dir' && git log '--format=%s it'\\''s'"
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

#[test]
fn local_login_command_marks_where_the_script_output_starts() {
    let scratch = Scratch::new("local-login");
    let output = Host::Local
        .login_command("pwd -P", Some(&scratch.0))
        .output()
        .unwrap();
    assert!(output.status.success());
    let physical = fs::canonicalize(&scratch.0).unwrap();
    assert_eq!(
        preamble::strip(&output.stdout).unwrap(),
        format!("{}\n", physical.display()).as_bytes()
    );
}

/// Startup files for every login shell: each prints a banner on stdout and exports where it
/// came from. `.cshrc` is the only file csh and tcsh read for `-c`.
fn home_with_profiles(home: &Path) {
    fs::write(
        home.join(".profile"),
        "echo 'Welcome from .profile'\nHIVE_PROFILE=profile\nexport HIVE_PROFILE\n",
    )
    .unwrap();
    fs::write(
        home.join(".zprofile"),
        "echo 'Welcome from .zprofile'\nexport HIVE_PROFILE=zprofile\n",
    )
    .unwrap();
    fs::write(
        home.join(".cshrc"),
        "echo 'Welcome from .cshrc'\nsetenv HIVE_PROFILE cshrc\n",
    )
    .unwrap();
    fs::create_dir_all(home.join(".config/fish")).unwrap();
    fs::write(
        home.join(".config/fish/config.fish"),
        "echo 'Welcome from config.fish'\nset -gx HIVE_PROFILE fish\n",
    )
    .unwrap();
}

#[test]
fn login_command_reads_the_profile_and_drops_its_banner_in_every_login_shell() {
    let scratch = Scratch::new("profiles");
    let home = scratch.0.join("home");
    let worktree = scratch.0.join("work tree's \\ dir!");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&worktree).unwrap();
    home_with_profiles(&home);
    let command = remote().login_command(
        "printf '%s:%s' \"$HIVE_PROFILE\" \"$(pwd -P)\"",
        Some(&worktree),
    );
    let physical = fs::canonicalize(&worktree).unwrap();
    let shells = login_shell::tests::installed_shells();
    for shell in &shells {
        let output = as_login_shell(&command, shell, &home).output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{}: {stderr}", shell.display());
        let profile = match shell.file_name().unwrap().to_str().unwrap() {
            "zsh" => "zprofile",
            "csh" | "tcsh" => "cshrc",
            "fish" => "fish",
            _ => "profile",
        };
        assert_eq!(
            String::from_utf8_lossy(preamble::strip(&output.stdout).unwrap()),
            format!("{profile}:{}", physical.display()),
            "{}",
            shell.display()
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).starts_with("Welcome from"),
            "{}: the banner is printed, then dropped",
            shell.display()
        );
    }
    assert!(shells.iter().any(|shell| shell.ends_with("sh")));
}

#[test]
fn a_remote_check_finds_executables_through_the_login_environment() {
    let scratch = Scratch::new("check");
    let home = scratch.0.join("home");
    fs::create_dir_all(home.join("tools")).unwrap();
    fs::write(
        home.join(".profile"),
        "echo banner\nPATH=\"$HOME/tools:$PATH\"\nexport PATH\n",
    )
    .unwrap();
    let tool = home.join("tools/hive-agent-cli");
    fs::write(&tool, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&tool, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let check = |program: &str| {
        let command =
            remote().login_command(&format!("command -v {} >/dev/null", quote(program)), None);
        as_login_shell(&command, Path::new("/bin/sh"), &home)
            .output()
            .unwrap()
            .status
            .success()
    };
    assert!(check("hive-agent-cli"));
    assert!(!check("hive-missing-cli"));
}
