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

/// `name` in `dir`, executable or not.
fn file_in(dir: &Path, name: &str, executable: bool) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, "#!/bin/sh\necho resolved\n").unwrap();
    let mode = if executable { 0o755 } else { 0o644 };
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    path
}

#[test]
fn a_local_program_is_found_on_the_login_path_so_spawn_needs_no_fork() {
    let scratch = Scratch::new("resolve");
    let (first, second) = (scratch.0.join("first"), scratch.0.join("second"));
    file_in(&first, "hive-tool", false);
    fs::create_dir_all(first.join("hive-folder")).unwrap();
    let tool = file_in(&second, "hive-tool", true);
    file_in(&second, "hive-folder", true);
    let path = std::env::join_paths([&first, &second]).unwrap();
    let mut variables = b"__HIVE_ENVIRONMENT_BEGIN__PATH=".to_vec();
    variables.extend_from_slice(path.as_bytes());
    variables.extend_from_slice(b"\0__HIVE_ENVIRONMENT_END__");
    let login = environment::parse(&variables).expect("markers are present");

    let command = local_command("hive-tool", &["--flag"], Some(&scratch.0), Some(&login));
    // A path makes std take `posix_spawn`; a bare name with PATH set makes it fork the app.
    assert_eq!(command.get_program(), tool.as_os_str());
    assert_eq!(command.get_args().collect::<Vec<_>>(), ["--flag"]);
    assert!(command.get_envs().any(|(key, _)| key == "PATH"));
    let output = local_command("hive-tool", &[], None, Some(&login))
        .output()
        .unwrap();
    assert_eq!(output.stdout, b"resolved\n");

    // A directory and a file without the execute bit are skipped, as `execvp` skips them.
    assert_eq!(
        environment::resolve("hive-folder", &path),
        Some(second.join("hive-folder"))
    );
    assert_eq!(environment::resolve("hive-missing", &path), None);
    assert_eq!(environment::resolve("dir/hive-tool", &path), None);
    // An entry relative to the child's directory comes first: leave the search to `execvp`.
    let relative = std::env::join_paths([Path::new("bin"), &second]).unwrap();
    assert_eq!(environment::resolve("hive-tool", &relative), None);
    let command = local_command("git", &[], None, None);
    assert_eq!(
        command.get_program(),
        "git",
        "nothing to resolve before the login shell answers"
    );
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

/// The `-o` values `command` passes before the destination.
fn ssh_options(command: &Command) -> Vec<String> {
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let end = args.iter().position(|arg| arg == "--").unwrap();
    args[..end]
        .windows(2)
        .filter(|pair| pair[0] == "-o")
        .map(|pair| pair[1].clone())
        .collect()
}

#[test]
fn batch_commands_ignore_masters_forwards_and_commands_from_the_users_ssh_config() {
    let batch = [
        remote().command("git", &["status"], Some(Path::new("/srv/app"))),
        remote().login_command("command -v codex", None),
    ];
    for command in &batch {
        let options = ssh_options(command);
        for expected in [
            "ControlMaster=no",
            "ClearAllForwardings=yes",
            "RemoteCommand=none",
            "PermitLocalCommand=no",
            "BatchMode=yes",
        ] {
            assert!(
                options.iter().any(|o| o == expected),
                "{expected}: {options:?}"
            );
        }
        assert_eq!(command.get_args().next().unwrap(), "-T");
    }
    // A terminal keeps the user's masters and forwards, but still sends Hive's command.
    let terminal = remote()
        .terminal_command(Path::new("/srv/app"), None)
        .unwrap();
    let options = ssh_options(&terminal);
    assert!(options.iter().any(|o| o == "RemoteCommand=none"));
    assert!(!options.iter().any(|o| o.starts_with("ControlMaster")));
    assert!(!options.iter().any(|o| o.starts_with("ClearAllForwardings")));
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

/// A home as Ubuntu and nvm leave it: `.profile` sources `.bashrc`, which returns at once
/// for a non-interactive shell, and the installer appended its `PATH` to `.bashrc` and
/// `.zshrc`. Both also print a banner and read a line, as some startup files do.
fn home_with_interactive_tools(home: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(home.join(".nvm/bin")).unwrap();
    let tool = home.join(".nvm/bin/hive-nvm-cli");
    fs::write(&tool, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        home.join(".profile"),
        "if [ -n \"$BASH_VERSION\" ] && [ -f \"$HOME/.bashrc\" ]; then . \"$HOME/.bashrc\"; fi\n",
    )
    .unwrap();
    let interactive =
        "echo 'Welcome from rc'\nread -r hive_answer\nexport PATH=\"$HOME/.nvm/bin:$PATH\"\n";
    fs::write(
        home.join(".bashrc"),
        format!("case $- in *i*) ;; *) return ;; esac\n{interactive}"),
    )
    .unwrap();
    fs::write(home.join(".zshrc"), interactive).unwrap();
}

#[test]
fn remote_tools_from_interactive_startup_files_are_found_and_stdin_reaches_the_command() {
    let scratch = Scratch::new("interactive");
    let home = scratch.0.join("home");
    home_with_interactive_tools(&home);
    let check = remote().login_command("command -v hive-nvm-cli", None);
    let echo = remote().login_command("cat", None);
    let shells = login_shell::tests::installed_shells();
    for shell in shells
        .iter()
        .filter(|shell| shell.ends_with("bash") || shell.ends_with("zsh"))
    {
        let output = as_login_shell(&check, shell, &home).output().unwrap();
        assert!(
            output.status.success(),
            "{}: {}",
            shell.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            preamble::strip(&output.stdout).unwrap(),
            format!("{}\n", home.join(".nvm/bin/hive-nvm-cli").display()).as_bytes(),
            "{}",
            shell.display()
        );
    }
    for shell in &shells {
        let mut child = as_login_shell(&echo, shell, &home)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(
            child.stdin.as_mut().unwrap(),
            b"Content-Length: 2\r\n\r\n{}",
        )
        .unwrap();
        drop(child.stdin.take());
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", shell.display());
        assert_eq!(
            preamble::strip(&output.stdout).unwrap(),
            b"Content-Length: 2\r\n\r\n{}",
            "{}: a startup file must not read the command's input",
            shell.display()
        );
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("job control"),
            "{}",
            shell.display()
        );
    }
    assert!(shells.iter().any(|shell| shell.ends_with("bash")));
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
