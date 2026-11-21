//! Spawn a macOS login shell without installing process-wide SIGCHLD handlers.
use crate::host::Host;
use alacritty_terminal::event::WindowSize;
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::termios::{InputModes, OptionalActions, Winsize};
use std::ffi::CStr;
use std::fs::File;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};

pub(super) fn spawn(mut command: Command, size: WindowSize) -> io::Result<(Child, File)> {
    let pty = rustix_openpty::openpty(None, Some(&winsize(size)))?;
    let mut modes = rustix::termios::tcgetattr(&pty.controller)?;
    modes.input_modes.insert(InputModes::IUTF8);
    rustix::termios::tcsetattr(&pty.controller, OptionalActions::Now, &modes)?;
    let flags = fcntl_getfl(&pty.controller)?;
    fcntl_setfl(&pty.controller, flags | OFlags::NONBLOCK)?;
    command
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("TERM_PROGRAM", "hive")
        .env_remove("XDG_ACTIVATION_TOKEN")
        .env_remove("DESKTOP_STARTUP_ID");
    remove_inherited_agent_env(&mut command);
    let mut slave = Some(pty.user);
    // Only async-signal-safe syscalls run between fork and exec. login_tty creates
    // the session, attaches its controlling terminal and installs all three stdio fds.
    unsafe {
        command.pre_exec(move || {
            // std inherits the spawning thread's signal mask, and Hive spawns from executor
            // threads that block signals. A child that never unblocks them (Claude Code)
            // would miss SIGWINCH for good and keep drawing at its startup size.
            let mut unblocked = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
            if libc::sigemptyset(unblocked.as_mut_ptr()) != 0
                || libc::sigprocmask(libc::SIG_SETMASK, unblocked.as_ptr(), std::ptr::null_mut())
                    != 0
            {
                return Err(io::Error::last_os_error());
            }
            for signal in [
                libc::SIGCHLD,
                libc::SIGHUP,
                libc::SIGINT,
                libc::SIGQUIT,
                libc::SIGTERM,
                libc::SIGALRM,
            ] {
                if libc::signal(signal, libc::SIG_DFL) == libc::SIG_ERR {
                    return Err(io::Error::last_os_error());
                }
            }
            rustix_openpty::login_tty(slave.take().expect("spawn calls pre_exec once"))?;
            Ok(())
        });
    }
    // After login_tty: the pane keeps its tty and nothing else of Hive's.
    crate::command::keep_only_standard_streams(&mut command);
    let child = command.spawn()?;
    Ok((child, File::from(pty.controller)))
}

pub(super) fn command(cwd: Option<&Path>, shell: Option<&str>) -> io::Result<Command> {
    match shell {
        Some(shell) => Ok(Host::Local.command(shell, &[], cwd)),
        None => login_command(cwd),
    }
}

/// A Hive terminal is a fresh host, not a child of whichever agent launched Hive. Claude Code
/// marks its subprocesses with `CLAUDE*` variables (`CLAUDECODE`, `CLAUDE_CODE_CHILD_SESSION`,
/// session ids, sockets); a `claude` started in the pane would inherit them and run as a nested
/// session with transcript saving off.
fn remove_inherited_agent_env(command: &mut Command) {
    let inherited = std::env::vars_os()
        .map(|(key, _)| key)
        .filter(|key| key.to_string_lossy().starts_with("CLAUDE"));
    for key in inherited {
        command.env_remove(key);
    }
}

pub(super) fn winsize(size: WindowSize) -> Winsize {
    Winsize {
        ws_row: size.num_lines,
        ws_col: size.num_cols,
        ws_xpixel: size.num_cols.saturating_mul(size.cell_width),
        ws_ypixel: size.num_lines.saturating_mul(size.cell_height),
    }
}

fn login_command(cwd: Option<&Path>) -> io::Result<Command> {
    let (user, home, shell) = shell_user()?;
    let shell_name = Path::new(&shell)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    // Keep login's tty bookkeeping and the user's login-shell startup files.
    let exec = format!(
        "exec -a {} {}",
        shlex::try_quote(&format!("-{shell_name}")).map_err(io::Error::other)?,
        shlex::try_quote(&shell).map_err(io::Error::other)?
    );
    let flags = if Path::new(&home).join(".hushlogin").exists() {
        "-qflp"
    } else {
        "-flp"
    };
    let mut command = Host::Local.command(
        "/usr/bin/login",
        &[flags, &user, "/bin/zsh", "-fc", &exec],
        cwd,
    );
    command.env("USER", user).env("HOME", home);
    Ok(command)
}

fn shell_user() -> io::Result<(String, String, String)> {
    let mut buffer = vec![0u8; 1024];
    loop {
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        // getpwuid_r owns no static storage; strings remain valid while buffer lives.
        let status = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                entry.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && buffer.len() < 1024 * 1024 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status));
        }
        if result.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "login user not found",
            ));
        }
        let entry = unsafe { entry.assume_init() };
        let value = |name, pointer: *const libc::c_char| -> io::Result<String> {
            if let Ok(value) = std::env::var(name) {
                return Ok(value);
            }
            if pointer.is_null() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "missing login user field",
                ));
            }
            Ok(unsafe { CStr::from_ptr(pointer) }
                .to_string_lossy()
                .into_owned())
        };
        return Ok((
            value("USER", entry.pw_name)?,
            value("HOME", entry.pw_dir)?,
            value("SHELL", entry.pw_shell)?,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{remove_inherited_agent_env, spawn};
    use alacritty_terminal::event::WindowSize;
    use std::os::fd::AsRawFd;
    use std::process::Command;

    #[test]
    fn a_pane_inherits_no_descriptor_besides_its_tty() {
        let descriptor = crate::command::inheritable_descriptor();
        let parked = descriptor.as_raw_fd();
        let passes = |script: &str| {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            let size = WindowSize {
                num_lines: 24,
                num_cols: 80,
                cell_width: 1,
                cell_height: 1,
            };
            let (mut child, _pty) = spawn(command, size).expect("a pty and /bin/sh");
            child.wait().expect("the shell exits").success()
        };
        assert!(passes("test -e /dev/fd/1"), "the tty is standard output");
        assert!(
            passes(&format!("test ! -e /dev/fd/{parked}")),
            "the shell inherited descriptor {parked}"
        );
    }

    #[test]
    fn child_shell_does_not_inherit_claude_code_markers() {
        let mut command = Command::new("env");
        command
            .env("CLAUDE_CODE_CHILD_SESSION", "1")
            .env("CLAUDECODE", "1")
            .env("HIVE_KEEP", "1");
        // The filter reads the process environment, so the markers must be set there too.
        unsafe {
            std::env::set_var("CLAUDE_CODE_CHILD_SESSION", "1");
            std::env::set_var("CLAUDECODE", "1");
        }
        remove_inherited_agent_env(&mut command);
        let output = command.output().expect("env is always available");
        let env = String::from_utf8_lossy(&output.stdout);
        assert!(!env.contains("CLAUDE_CODE_CHILD_SESSION="), "{env}");
        assert!(!env.contains("CLAUDECODE="), "{env}");
        assert!(env.contains("HIVE_KEEP=1"), "{env}");
    }
}
