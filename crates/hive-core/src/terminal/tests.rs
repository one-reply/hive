use super::*;
use std::time::Duration;

fn shell() -> Terminal {
    Terminal::spawn(None, Some("/bin/sh"), TerminalSize::new(80, 24)).unwrap()
}

fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "terminal did not complete in time"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn echo_and_natural_exit_publish_final_snapshot() {
    let mut terminal = shell();
    let events = terminal.events().unwrap();
    let grid = Arc::downgrade(terminal.term());
    terminal.write(b"printf '\\033]2;natural-exit\\007'; printf 'hive-ok\\n'; exit\n".to_vec());
    assert!(
        terminal
            .control()
            .wait_until(Instant::now() + Duration::from_secs(3))
    );
    let update = events.try_recv().unwrap();
    assert!(update.exited());
    assert_eq!(update.title(), Some("natural-exit"));
    let text: String = terminal
        .term()
        .lock()
        .renderable_content()
        .display_iter
        .map(|c| c.c)
        .collect();
    assert!(text.contains("hive-ok"));
    drop(terminal);
    wait_for(|| grid.upgrade().is_none());
}

#[test]
fn missing_shell_returns_a_launch_error() {
    assert!(matches!(
        Terminal::spawn(
            None,
            Some("/nonexistent/hive-regression-shell"),
            TerminalSize::new(80, 24)
        ),
        Err(TerminalError::Pty(_))
    ));
}

#[test]
fn spawning_a_pty_does_not_install_a_sigchld_handler() {
    const ISOLATED: &str = "HIVE_TEST_SIGCHLD_ISOLATED";
    if std::env::var_os(ISOLATED).is_none() {
        let executable = std::env::current_exe().unwrap();
        let output = crate::host::Host::Local
            .command(
                executable.to_str().unwrap(),
                &["--exact", "terminal::tests::spawning_a_pty_does_not_install_a_sigchld_handler", "--nocapture"],
                None,
            )
            .env(ISOLATED, "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    let handler = || unsafe {
        let mut action = std::mem::MaybeUninit::<libc::sigaction>::uninit();
        assert_eq!(libc::sigaction(libc::SIGCHLD, std::ptr::null(), action.as_mut_ptr()), 0);
        action.assume_init().sa_sigaction
    };
    let before = handler();
    let terminal = shell();
    assert_eq!(handler(), before, "PTY exit is watched by kqueue, not a global signal socket");
    for _ in 0..128 {
        assert!(crate::host::Host::Local.command("/usr/bin/true", &[], None).status().unwrap().success());
    }
    terminal.write(b"printf '\\033]2;after-child-exits\\007'\n".to_vec());
    wait_for(|| terminal.events.as_ref().unwrap().try_recv().is_ok_and(|update| update.title() == Some("after-child-exits")));
    let control = terminal.control();
    drop(terminal);
    assert!(control.wait_until(Instant::now() + Duration::from_secs(3)));
}

#[test]
fn three_cycles_of_fifty_terminals_release_every_grid() {
    for _ in 0..3 {
        let terminals: Vec<_> = (0..50).map(|_| shell()).collect();
        let grids: Vec<_> = terminals
            .iter()
            .map(|terminal| Arc::downgrade(terminal.term()))
            .collect();
        let controls: Vec<_> = terminals.iter().map(Terminal::control).collect();
        drop(terminals);
        assert!(
            controls
                .iter()
                .all(|control| control.sender.upgrade().is_none())
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        assert!(controls.iter().all(|control| control.wait_until(deadline)));
        wait_for(|| grids.iter().all(|grid| grid.upgrade().is_none()));
    }
}

#[test]
fn shutdown_kills_hup_ignoring_session_jobs_but_leaves_another_session_alive() {
    let terminal = shell();
    let outsider = shell();
    let path = std::env::temp_dir().join(format!("hive-pty-jobs-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    // Interactive macOS sh enables history expansion; disable it before using $!.
    terminal.write(b"set +H\n".to_vec());
    // The shell, its background job and the foreground sleep all inherit ignored HUP.
    terminal.write(format!("trap '' HUP; /bin/sh -c 'trap \"\" HUP; while :; do sleep 1; done' & printf '%s %s\\n' \"$$\" \"$!\" > '{}'; sleep 60\n", path.display()).into_bytes());
    wait_for(|| {
        std::fs::read_to_string(&path).is_ok_and(|text| text.split_whitespace().count() == 2)
    });
    let pids: Vec<i32> = std::fs::read_to_string(&path)
        .unwrap()
        .split_whitespace()
        .map(|pid| pid.parse().unwrap())
        .collect();
    let control = terminal.control();
    let grid = Arc::downgrade(terminal.term());
    let start = Instant::now();
    drop(terminal);
    assert!(control.wait_until(start + Duration::from_millis(2500)));
    assert!(!outsider.control().is_closed());
    wait_for(|| grid.upgrade().is_none());
    for pid in pids {
        let output = crate::host::Host::Local
            .command("/bin/ps", &["-p", &pid.to_string(), "-o", "stat="], None)
            .output()
            .unwrap();
        let status = String::from_utf8_lossy(&output.stdout);
        assert!(
            status.trim().is_empty() || status.trim().starts_with('Z'),
            "session process {pid} survived: {status}"
        );
    }
    std::fs::remove_file(path).unwrap();
    let outsider_control = outsider.control();
    drop(outsider);
    assert!(outsider_control.wait_until(Instant::now() + Duration::from_secs(3)));
}
