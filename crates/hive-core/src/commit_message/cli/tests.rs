use super::*;

#[test]
fn quoted_arguments_are_argv_not_shell_code() {
    assert_eq!(
        arguments(
            AgentKind::Codex,
            "--model 'a b' -c 'model_reasoning_effort=\"low\"'"
        )
        .unwrap(),
        ["--model", "a b", "-c", "model_reasoning_effort=\"low\""]
    );
    assert_eq!(
        arguments(
            AgentKind::ClaudeCode,
            "--model='$(touch forbidden)' --effort low"
        )
        .unwrap()[0],
        "--model=$(touch forbidden)"
    );
    assert!(arguments(AgentKind::Codex, "").unwrap().is_empty());
}

#[test]
fn managed_flags_and_malformed_values_are_rejected() {
    for value in [
        "--model",
        "--model 'broken",
        "--json",
        "--sandbox danger-full-access",
        "--dangerously-bypass-approvals-and-sandbox",
        "-c sandbox_mode=workspace-write",
        "--model x; echo test",
        "--cd /tmp",
        "--tools Bash",
        "-m x --output-format text",
        "--model=",
        "-c features.shell_tool=true",
    ] {
        for agent in AGENTS {
            assert!(arguments(agent, value).is_err(), "{agent:?}: {value}");
        }
    }
}

// NDJSON/result envelopes from the CLI protocol; payloads replaced with fixture text.
const CODEX: &str = r#"{"type":"thread.started","thread_id":"test"}
{"type":"item.completed","item":{"id":"0","type":"reasoning","text":"private thought"}}
{"type":"item.completed","item":{"id":"1","type":"agent_message","text":"feat: fixture"}}
{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}
"#;
const CLAUDE: &str = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":100,"result":"fix: fixture","session_id":"test"}"#;

#[test]
fn only_final_cli_messages_are_returned() {
    assert_eq!(
        parse(AgentKind::Codex, CODEX.as_bytes()).unwrap(),
        "feat: fixture"
    );
    assert_eq!(
        parse(AgentKind::ClaudeCode, CLAUDE.as_bytes()).unwrap(),
        "fix: fixture"
    );
    assert!(
        parse(
            AgentKind::Codex,
            CODEX.replace("turn.completed", "turn.failed").as_bytes()
        )
        .is_err()
    );
    assert!(
        parse(
            AgentKind::Codex,
            CODEX
                .lines()
                .take(3)
                .collect::<Vec<_>>()
                .join("\n")
                .as_bytes()
        )
        .is_err()
    );
    assert!(
        parse(
            AgentKind::ClaudeCode,
            CLAUDE.replace("success", "error_max_turns").as_bytes()
        )
        .is_err()
    );
    assert!(parse(AgentKind::ClaudeCode, b"{}").is_err());
    assert!(parse(AgentKind::Codex, b"not json").is_err());
}

#[test]
#[ignore = "uses installed CLI login and sends a synthetic prompt; opt-in only"]
fn installed_cli_smoke() {
    let name = std::env::var("HIVE_TEST_CLI").expect("set HIVE_TEST_CLI=codex or claude");
    let agent = match name.as_str() {
        "codex" => AgentKind::Codex,
        "claude" => AgentKind::ClaudeCode,
        _ => panic!("unknown HIVE_TEST_CLI"),
    };
    let message = generate(
        agent,
        "",
        "Return exactly this commit message and nothing else: docs: update synthetic fixture",
        &Cancellation::default(),
    )
    .expect("CLI generates text");
    assert!(
        message.contains("docs: update synthetic fixture"),
        "{message:?}"
    );
}
