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

// `opencode run --format json` 2.2.5 output; ids shortened, payloads replaced with fixture text.
// The second step is the answer; the first ends in a denied tool call.
const OPENCODE: &str = r#"{"type":"step_start","timestamp":1,"sessionID":"ses_t","part":{"id":"prt_1","sessionID":"ses_t","messageID":"msg_1","type":"step-start"}}
{"type":"text","timestamp":2,"sessionID":"ses_t","part":{"id":"prt_2","sessionID":"ses_t","messageID":"msg_1","type":"text","text":"I'll read the file.","time":{"start":1,"end":2}}}
{"type":"tool_use","timestamp":3,"sessionID":"ses_t","part":{"partID":"prt_3","sessionID":"ses_t","messageID":"msg_1","type":"tool","tool":"skill","state":{"status":"error","input":{"id":"fs"},"error":"Unable to load skill fs"}}}
{"type":"step_finish","timestamp":4,"sessionID":"ses_t","part":{"id":"prt_4","sessionID":"ses_t","messageID":"msg_1","type":"step-finish","reason":"tool-calls","cost":0.001,"tokens":{"input":1,"output":1}}}
{"type":"step_start","timestamp":5,"sessionID":"ses_t","part":{"id":"prt_5","sessionID":"ses_t","messageID":"msg_2","type":"step-start"}}
{"type":"text","timestamp":6,"sessionID":"ses_t","part":{"id":"prt_6","sessionID":"ses_t","messageID":"msg_2","type":"text","text":"chore: fixture","time":{"start":5,"end":6}}}
{"type":"step_finish","timestamp":7,"sessionID":"ses_t","part":{"id":"prt_7","sessionID":"ses_t","messageID":"msg_2","type":"step-finish","reason":"stop","cost":0.001,"tokens":{"input":1,"output":1}}}
"#;
const OPENCODE_ERROR: &str = r#"{"type":"error","timestamp":1,"sessionID":"ses_t","error":{"type":"provider.no-route","message":"Model unavailable: anthropic/no-such-model"}}"#;

#[test]
fn opencode_returns_only_the_stopped_step_text() {
    assert_eq!(
        parse(AgentKind::OpenCode, OPENCODE.as_bytes()).unwrap(),
        "chore: fixture"
    );
}

#[test]
fn incomplete_or_failed_opencode_runs_are_rejected() {
    let without_stop: String = OPENCODE.lines().take(6).collect::<Vec<_>>().join("\n");
    for output in [
        without_stop.as_str(),
        OPENCODE_ERROR,
        &format!("{OPENCODE}{OPENCODE_ERROR}"),
        "",
        "not json",
        &OPENCODE[..OPENCODE.len() / 2],
    ] {
        assert!(
            parse(AgentKind::OpenCode, output.as_bytes()).is_err(),
            "{output}"
        );
    }
}

#[test]
fn opencode_accepts_only_a_model_argument() {
    assert_eq!(
        arguments(AgentKind::OpenCode, "-m anthropic/claude-haiku-4-5#low").unwrap(),
        ["-m", "anthropic/claude-haiku-4-5#low"]
    );
    for value in [
        "--auto",
        "--agent build",
        "--session ses_x",
        "--format default",
        "--effort low",
    ] {
        assert!(arguments(AgentKind::OpenCode, value).is_err(), "{value}");
    }
}

#[test]
fn opencode_runs_without_user_config_or_tool_permissions() {
    let directory = process::Directory::new().unwrap();
    let command = cli_command(
        AgentKind::OpenCode,
        std::path::Path::new("/bin/opencode"),
        &["run"],
        &directory,
    );
    let env: HashMap<_, _> = command
        .get_envs()
        .filter_map(|(key, value)| Some((key.to_str()?.to_owned(), value?.to_str()?.to_owned())))
        .collect();
    assert_eq!(
        env["XDG_CONFIG_HOME"],
        directory.0.join("config").to_string_lossy()
    );
    assert_eq!(
        env["OPENCODE_CONFIG_CONTENT"],
        r#"{"permission":{"*":"deny"}}"#
    );
    assert_eq!(env["OPENCODE_DISABLE_PROJECT_CONFIG"], "1");
    let codex = cli_command(
        AgentKind::Codex,
        std::path::Path::new("/bin/codex"),
        &[],
        &directory,
    );
    assert!(codex.get_envs().all(|(key, _)| key != "XDG_CONFIG_HOME"));
}

#[test]
#[ignore = "uses installed CLI login and sends a synthetic prompt; opt-in only"]
fn installed_cli_smoke() {
    let name =
        std::env::var("HIVE_TEST_CLI").expect("set HIVE_TEST_CLI=codex, claude, opencode or agy");
    let arguments = std::env::var("HIVE_TEST_CLI_ARGS").unwrap_or_default();
    let agent = match name.as_str() {
        "codex" => AgentKind::Codex,
        "claude" => AgentKind::ClaudeCode,
        "opencode" => AgentKind::OpenCode,
        "agy" => AgentKind::Antigravity,
        _ => panic!("unknown HIVE_TEST_CLI"),
    };
    let message = generate(
        agent,
        &arguments,
        "Return exactly this commit message and nothing else: docs: update synthetic fixture",
        &Cancellation::default(),
    )
    .expect("CLI generates text");
    assert!(
        message.contains("docs: update synthetic fixture"),
        "{message:?}"
    );
}

// Captured agy 1.5.3 stream-json output; IDs, paths and response replaced with fixture text.
const ANTIGRAVITY: &str = r#"{"event":"init","init":{"cwd":"/tmp/fixture"}}
{"event":"step_update","step_update":{"state":"DONE","step_type":"agent_response","text_delta":"intermediate text"}}
{"event":"result","result":{"conversation_id":"fixture","status":"SUCCESS","response":"docs: fixture\n","duration_seconds":1,"num_turns":1,"usage":{"input_tokens":1}}}
"#;

#[test]
fn antigravity_is_a_generator_and_accepts_model_and_effort() {
    assert!(AGENTS.contains(&AgentKind::Antigravity));
    assert_eq!(
        arguments(
            AgentKind::Antigravity,
            "--model gemini-3.8-flash-low --effort low"
        )
        .unwrap(),
        ["--model", "gemini-3.8-flash-low", "--effort", "low"]
    );
    for value in [
        "-m model",
        "--settings {}",
        "--continue",
        "--agent custom",
        "--mode auto",
        "--print hi",
        "--input-format text",
        "--disable-slash-commands=false",
    ] {
        assert!(arguments(AgentKind::Antigravity, value).is_err(), "{value}");
    }
}

#[test]
fn antigravity_returns_only_the_successful_result() {
    assert_eq!(
        parse(AgentKind::Antigravity, ANTIGRAVITY.as_bytes()).unwrap(),
        "docs: fixture"
    );
    for status in [
        "ERROR",
        "CANCELED",
        "INTERRUPTED",
        "INVALID",
        "WAITING",
        "RUNNING",
        "FUTURE",
    ] {
        assert!(
            parse(
                AgentKind::Antigravity,
                ANTIGRAVITY.replace("SUCCESS", status).as_bytes()
            )
            .is_err()
        );
    }
    for output in [
        "",
        "{}",
        "not json",
        "{\"event\":\"result\",\"result\":{}}",
        ANTIGRAVITY.lines().next().unwrap(),
        &ANTIGRAVITY[..ANTIGRAVITY.len() / 2],
        &ANTIGRAVITY.replace("docs: fixture\\n", ""),
        &format!("{ANTIGRAVITY}{ANTIGRAVITY}"),
        &format!("{ANTIGRAVITY}{{broken"),
    ] {
        assert!(
            parse(AgentKind::Antigravity, output.as_bytes()).is_err(),
            "{output}"
        );
    }
}
