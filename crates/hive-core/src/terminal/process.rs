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

pub(super) fn spawn(
    cwd: Option<&Path>,
    shell: Option<&str>,
    size: WindowSize,
) -> io::Result<(Child, File)> {
    let pty = rustix_openpty::openpty(None, Some(&winsize(size)))?;
    let mut modes = rustix::termios::tcgetattr(&pty.controller)?;
    modes.input_modes.insert(InputModes::IUTF8);
    rustix::termios::tcsetattr(&pty.controller, OptionalActions::Now, &modes)?;
    let flags = fcntl_getfl(&pty.controller)?;
    fcntl_setfl(&pty.controller, flags | OFlags::NONBLOCK)?;
    let mut command = match shell {
        Some(shell) => Host::Local.command(shell, &[], cwd),
        None => login_command(cwd)?,
    };
    command
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("TERM_PROGRAM", "hive")
        .env_remove("XDG_ACTIVATION_TOKEN")
        .env_remove("DESKTOP_STARTUP_ID");
    let mut slave = Some(pty.user);
    // Only async-signal-safe syscalls run between fork and exec. login_tty creates
    // the session, attaches its controlling terminal and installs all three stdio fds.
    unsafe {
        command.pre_exec(move || {
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
    let child = command.spawn()?;
    Ok((child, File::from(pty.controller)))
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
