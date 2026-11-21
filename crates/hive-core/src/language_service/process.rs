use super::{Error, Family, Message, wire};
use crate::host::{Host, preamble};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    process::{Pid, Signal, kill_process_group},
};
use std::{
    io::{BufReader, Read, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    time::{Duration, Instant},
};

/// The first stderr bytes say why a server could not start (rustup's missing component, a
/// shell's "not found"); later output may quote source and is never kept.
const STDERR_HEAD: usize = 1024;

pub(super) struct Process {
    child: Child,
    input: ChildStdin,
    stopped: Arc<AtomicBool>,
    stderr_head: Receiver<Vec<u8>>,
}

impl Process {
    pub(super) fn start(
        host: &Host,
        root: &Path,
        family: Family,
        generation: u8,
        sender: SyncSender<Message>,
        stopped: Arc<AtomicBool>,
    ) -> Result<Self, Error> {
        Self::spawn(
            command(host, root, family),
            host,
            root,
            generation,
            sender,
            stopped,
        )
    }

    fn spawn(
        mut command: Command,
        host: &Host,
        root: &Path,
        generation: u8,
        sender: SyncSender<Message>,
        stopped: Arc<AtomicBool>,
    ) -> Result<Self, Error> {
        let fail = |error: std::io::Error| Error::Session {
            host: host.clone(),
            root: root.into(),
            message: error.to_string(),
        };
        command
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // A server lives as long as its project is open.
        crate::command::keep_only_standard_streams(&mut command);
        let mut child = command.spawn().map_err(fail)?;
        let Some(input) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Unavailable("server has no stdin"));
        };
        let (head_sender, stderr_head) = sync_channel(1);
        let mut process = Self {
            child,
            input,
            stopped,
            stderr_head,
        };
        fcntl_setfl(
            &process.input,
            fcntl_getfl(&process.input).map_err(|e| fail(e.into()))? | OFlags::NONBLOCK,
        )
        .map_err(|e| fail(e.into()))?;
        let output = process
            .child
            .stdout
            .take()
            .ok_or(Error::Unavailable("server has no stdout"))?;
        let stderr = process
            .child
            .stderr
            .take()
            .ok_or(Error::Unavailable("server has no stderr"))?;
        std::thread::Builder::new()
            .name("hive-lsp-read".into())
            .spawn(move || forward(output, generation, &sender))
            .map_err(fail)?;
        std::thread::Builder::new()
            .name("hive-lsp-stderr".into())
            .spawn(move || {
                // Drain without retaining arbitrary server logs or source-bearing diagnostics.
                let mut reader = stderr;
                let mut buffer = [0; 4096];
                let mut head = Vec::new();
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => {
                            let kept = count.min(STDERR_HEAD - head.len());
                            head.extend_from_slice(&buffer[..kept]);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(error) => {
                            tracing::debug!(%error,"language-server stderr closed");
                            break;
                        }
                    }
                }
                // The session may already be gone; nobody is waiting then.
                let _ = head_sender.send(head);
            })
            .map_err(fail)?;
        Ok(process)
    }

    /// What the server printed first, once it has exited; `None` while it still runs.
    pub(super) fn startup_output(&self) -> Option<String> {
        let head = self
            .stderr_head
            .recv_timeout(Duration::from_millis(200))
            .ok()?;
        let text: String = String::from_utf8_lossy(&head)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .filter(|character| !character.is_control())
            .collect();
        (!text.is_empty()).then_some(text)
    }

    pub(super) fn write(&mut self, message: &impl serde::Serialize) -> Result<(), Error> {
        self.write_with_deadline(message, Duration::from_secs(5), true)
    }

    pub(super) fn closing(&mut self, message: &impl serde::Serialize) -> Result<(), Error> {
        self.write_with_deadline(message, Duration::from_millis(100), false)
    }

    fn write_with_deadline(
        &mut self,
        message: &impl serde::Serialize,
        timeout: Duration,
        cancellable: bool,
    ) -> Result<(), Error> {
        let bytes = wire::encode(message)?;
        let deadline = Instant::now() + timeout;
        let mut remaining = bytes.as_slice();
        while !remaining.is_empty() {
            if cancellable && self.stopped.load(Ordering::Acquire) {
                return Err(Error::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(Error::TimedOut);
            }
            match self.input.write(remaining) {
                Ok(0) => return Err(Error::Unavailable("server closed stdin")),
                Ok(count) => remaining = &remaining[count..],
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    let timeout = Timespec::try_from(Duration::from_millis(25))
                        .map_err(|e| Error::Protocol(e.to_string()))?;
                    // A signal only ends this wait early; the loop checks the deadline again.
                    match poll(
                        &mut [PollFd::new(&self.input, PollFlags::OUT)],
                        Some(&timeout),
                    ) {
                        Ok(_) | Err(rustix::io::Errno::INTR) => {}
                        Err(error) => return Err(Error::Protocol(error.to_string())),
                    }
                }
                Err(e) => return Err(Error::Protocol(e.to_string())),
            }
        }
        Ok(())
    }
}

/// The server for `family` in the login environment, where it resolves as in a terminal.
fn command(host: &Host, root: &Path, family: Family) -> Command {
    let script = match family {
        // A rustup proxy must report a missing toolchain, never download one.
        Family::Rust => "export RUSTUP_AUTO_INSTALL=0; exec rust-analyzer",
        // Only the worktree's own install or PATH: a parent directory is outside the project.
        Family::TypeScript => {
            "if [ -x node_modules/.bin/typescript-language-server ]; then exec node_modules/.bin/typescript-language-server --stdio; fi; exec typescript-language-server --stdio"
        }
    };
    host.login_command(script, Some(root))
}

/// Send the server's frames to the worker, after whatever the account's startup files
/// printed before the server started.
fn forward(output: ChildStdout, generation: u8, sender: &SyncSender<Message>) {
    let mut reader = BufReader::new(output);
    let started = match preamble::skip(&mut reader) {
        Ok(true) => Ok(()),
        // Nothing started: to the worker, the same as a server that closed stdout at once.
        Ok(false) => Err(Ok(None)),
        Err(error) => Err(Err(Error::Protocol(format!(
            "language server did not start: {error}"
        )))),
    };
    if let Err(result) = started {
        let _ = sender.send(Message::Incoming(generation, result));
        return;
    }
    loop {
        let result = wire::read(&mut reader);
        let ended = !matches!(result, Ok(Some(_)));
        if sender.send(Message::Incoming(generation, result)).is_err() || ended {
            break;
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // Keep the child unreaped until its group is signalled, avoiding PID reuse.
        if let Some(pid) = Pid::from_raw(self.child.id() as i32)
            && let Err(error) = kill_process_group(pid, Signal::KILL)
            && error != rustix::io::Errno::SRCH
        {
            tracing::debug!(%error,"language-server group cleanup");
        }
        if let Err(error) = self.child.wait() {
            tracing::warn!(%error,"could not reap language server");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::as_login_shell;
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::mpsc};

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("hive-lsp-{name}-{}", std::process::id()));
            fs::create_dir_all(path.join("home")).unwrap();
            Self(path)
        }
        fn executable(&self, relative: &str, script: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// An account whose profile greets every shell, login or not, on stdout.
    const BANNER_SHELL: &str = "#!/bin/sh\necho 'Welcome to build-01'\ncase $1 in -c|-lc) exec /bin/sh -c \"$2\" ;; esac\nexit 64\n";

    fn first_message(scratch: &Scratch, root: &Path) -> Result<Option<wire::Incoming>, Error> {
        let shell = scratch.executable("login-shell", BANNER_SHELL);
        let host = Host::Ssh {
            target: "build-01".into(),
        };
        let command = command(&host, root, Family::TypeScript);
        let (sender, receiver) = mpsc::sync_channel(4);
        let process = Process::spawn(
            as_login_shell(&command, &shell, &scratch.0.join("home")),
            &host,
            root,
            0,
            sender,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let message = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        drop(process);
        match message {
            Message::Incoming(0, result) => result,
            _ => panic!("the reader sends only incoming frames"),
        }
    }

    #[test]
    fn a_remote_login_banner_does_not_break_the_first_frame() {
        let scratch = Scratch::new("banner");
        let root = scratch.0.join("work tree's");
        let body = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"ready"}}"#;
        scratch.executable(
            "work tree's/node_modules/.bin/typescript-language-server",
            &format!(
                "#!/bin/sh\nprintf 'Content-Length: {}\\r\\n\\r\\n%s' '{body}'\nexec cat >/dev/null\n",
                body.len()
            ),
        );
        let first = first_message(&scratch, &root);
        assert!(
            matches!(&first, Ok(Some(wire::Incoming::Call { method, .. })) if method == "window/logMessage"),
            "{:?}",
            first.err()
        );
    }

    #[test]
    fn a_server_inherits_no_descriptor_besides_its_standard_streams() {
        use std::os::fd::AsRawFd;
        let descriptor = crate::command::inheritable_descriptor();
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            &format!(
                "test -e /dev/fd/{} && echo inherited >&2; exit 0",
                descriptor.as_raw_fd()
            ),
        ]);
        let (sender, _frames) = mpsc::sync_channel(4);
        let process = Process::spawn(
            command,
            &Host::Local,
            Path::new("/"),
            0,
            sender,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        // The head arrives when the shell has exited and its stderr is closed.
        let head = process.stderr_head.recv().unwrap();
        assert_eq!(String::from_utf8_lossy(&head), "");
    }

    #[test]
    fn a_shell_that_exits_before_the_server_reads_as_a_closed_stdout() {
        let scratch = Scratch::new("missing-root");
        // `cd` fails remotely, so the script never prints the marker.
        let first = first_message(&scratch, &scratch.0.join("gone"));
        assert!(matches!(first, Ok(None)), "{:?}", first.err());
    }
}
