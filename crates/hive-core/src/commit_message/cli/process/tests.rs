use super::*;
use crate::host::Host;
use std::time::Instant;

fn command(script: &str) -> Command {
    Host::Local.command("/bin/sh", &["-c", script], None)
}

#[test]
fn full_stdin_and_both_output_pipes_are_serviced() {
    let control = Cancellation::default();
    let prompt = "x".repeat(256 * 1024);
    let output = run(
        AgentKind::Codex,
        command("head -c 65536 /dev/zero >&2; cat"),
        &prompt,
        &control,
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(output, prompt.as_bytes());
}

#[test]
fn timeout_cancellation_nonzero_and_output_limits_terminate_children() {
    let control = Cancellation::default();
    let start = Instant::now();
    let result = run(
        AgentKind::Codex,
        command("sleep 30 & wait"),
        "",
        &control,
        Duration::from_millis(40),
    );
    assert!(result.unwrap_err().to_string().contains("timed out"));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(
        run(
            AgentKind::Codex,
            command("exit 1"),
            "",
            &control,
            Duration::from_secs(1)
        )
        .is_err()
    );
    assert!(
        run(
            AgentKind::Codex,
            command("yes"),
            "",
            &control,
            Duration::from_secs(5)
        )
        .unwrap_err()
        .to_string()
        .contains("1 MiB")
    );
    std::thread::scope(|scope| {
        let task = scope.spawn(|| {
            run(
                AgentKind::Codex,
                command("sleep 30"),
                "",
                &control,
                Duration::from_secs(20),
            )
        });
        std::thread::sleep(Duration::from_millis(40));
        control.cancel();
        assert!(task.join().unwrap().is_err());
    });
}
