//! Real binary/stdio coverage without GPUI, installed agents, network or user configuration.
use hive_core::agent::api::{Call, Outcome, Request, Response};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: mpsc::Receiver<Value>,
    directory: PathBuf,
}
impl Client {
    fn new(socket: &std::path::Path, terminal: u64) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "hm-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_hive"))
            .args(["mcp", "--socket"])
            .arg(socket)
            .args(["--terminal", &terminal.to_string()])
            .env_remove("HIVE_SOCKET")
            .env_remove("HIVE_TERMINAL")
            .env_remove("HIVE_BIN")
            .current_dir(&directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (sender, output) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let value = serde_json::from_str(&line).expect("stdout is only MCP JSON");
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            input,
            output,
            directory,
        }
    }
    fn send(&mut self, value: Value) {
        self.raw(&format!("{value}\n"));
    }
    fn raw(&mut self, text: &str) {
        self.input
            .as_mut()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }
    fn receive(&self) -> Value {
        self.output
            .recv_timeout(Duration::from_secs(15))
            .expect("MCP response")
    }
    fn initialize(&mut self) -> Value {
        self.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"hive-test","version":"1"}}}));
        let response = self.receive();
        assert_eq!(response["result"]["protocolVersion"], "2025-11-25");
        assert!(response["result"]["capabilities"]["tools"].is_object());
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        response
    }
    fn call(&mut self, id: u64, name: &str, args: Value) {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}}));
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}
struct Socket {
    path: PathBuf,
    listener: UnixListener,
}
impl Socket {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = UnixListener::bind(&path).unwrap();
        Self { path, listener }
    }
    fn answer(&self, expected: u64) {
        let (mut stream, _) = self.listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        let call: Call = serde_json::from_str(&line).unwrap();
        assert_eq!(call.from, Some(expected));
        assert_eq!(call.request, Request::List);
        writeln!(
            stream,
            "{}",
            serde_json::to_string(&Outcome::Result(Response::Agents { agents: vec![] })).unwrap()
        )
        .unwrap();
    }
}
impl Drop for Socket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[test]
fn handshake_discovery_and_calls_keep_two_instances_and_two_panes_separate() {
    let one = Socket::new();
    let two = Socket::new();
    let mut first = Client::new(&one.path, 11);
    let mut neighbour = Client::new(&one.path, 12);
    let mut other = Client::new(&two.path, 11);
    for (client, terminal, path) in [
        (&mut first, 11, &one.path),
        (&mut neighbour, 12, &one.path),
        (&mut other, 11, &two.path),
    ] {
        let info = client.initialize();
        let instructions = info["result"]["instructions"].as_str().unwrap();
        assert!(instructions.contains(&format!("terminal {terminal}")));
        assert!(instructions.contains(path.to_str().unwrap()));
        client.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
        let list = client.receive();
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 6);
        for tool in tools {
            assert!(tool["description"].as_str().unwrap().len() > 40);
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }
    first.call(3, "list", json!({}));
    one.answer(11);
    neighbour.call(3, "list", json!({}));
    one.answer(12);
    other.call(3, "list", json!({}));
    two.answer(11);
    for client in [&first, &neighbour, &other] {
        assert_eq!(client.receive()["result"]["isError"], false);
    }
}

#[test]
fn unavailable_invalid_and_oversized_input_do_not_break_later_calls() {
    let mut client = Client::new(std::path::Path::new("/nonexistent-hive-test/socket"), 42);
    client.initialize();
    client.call(2, "list", json!({}));
    assert_eq!(client.receive()["result"]["isError"], true);
    client.call(3, "send", json!({"target":"42","text":"secret","from":99}));
    assert!(client.receive()["error"].is_object());
    client.raw("not-json\n");
    // Syntax errors have no correlatable ID and are discarded by the official SDK.
    client.raw(&format!("{}\n", "x".repeat(65537)));
    // Oversized lines are discarded by the official SDK. The next valid request must work.
    client.send(json!({"jsonrpc":"2.0","id":4,"method":"ping"}));
    let mut reply = client.receive();
    if reply["id"] != 4 {
        reply = client.receive();
    }
    assert_eq!(reply["id"], 4);
    assert!(reply["result"].is_object());
}

#[test]
fn cancellation_closes_the_inflight_socket_and_keeps_stdio_usable() {
    let socket = Socket::new();
    let mut client = Client::new(&socket.path, 42);
    client.initialize();
    client.call(2, "wait", json!({"target":"7"}));
    let (stream, _) = socket.listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(!line.is_empty());
    client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2,"reason":"test"}}));
    line.clear();
    assert_eq!(
        reader.read_line(&mut line).unwrap(),
        0,
        "cancel releases the API socket"
    );
    client.send(json!({"jsonrpc":"2.0","id":3,"method":"ping"}));
    let mut reply = client.receive();
    if reply["id"] == 2 {
        reply = client.receive();
    }
    assert_eq!(reply["id"], 3);
}

#[test]
fn legacy_skill_still_works_without_identity() {
    let output = Command::new(env!("CARGO_BIN_EXE_hive"))
        .arg("--skill")
        .env_remove("HIVE_SOCKET")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("hive agent")
    );
}

#[test]
fn eof_cancels_pending_call_and_exits_without_waiting_for_its_deadline() {
    let socket = Socket::new();
    let mut client = Client::new(&socket.path, 42);
    client.initialize();
    client.call(2, "wait", json!({"target":"7","timeout":86400}));
    let (stream, _) = socket.listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    client.input.take();
    line.clear();
    assert_eq!(reader.read_line(&mut line).unwrap(), 0);
    assert!(matches!(
        client.output.recv_timeout(Duration::from_secs(10)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
    assert!(client.child.wait().unwrap().success());
}

#[test]
fn all_mutating_tools_forward_typed_calls_and_preserve_api_errors() {
    let socket = Socket::new();
    let mut client = Client::new(&socket.path, 42);
    client.initialize();
    for (method, args) in [
        (
            "start",
            json!({"name":"helper","kind":"claude","prompt":"Review","tab":true,"timeout":2.5}),
        ),
        (
            "send",
            json!({"target":"7","text":"Hello\n世界","wait":true,"timeout":2.5}),
        ),
        ("read", json!({"target":"helper","lines":20})),
        ("wait", json!({"target":"7","timeout":2.5})),
        ("interrupt", json!({"target":"7"})),
    ] {
        client.call(2, method, args);
        let (mut stream, _) = socket.listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        let call: Call = serde_json::from_str(&line).unwrap();
        assert_eq!(call.from, Some(42));
        assert_eq!(
            serde_json::to_value(&call.request).unwrap()["method"],
            method
        );
        match call.request {
            Request::Start {
                name,
                kind,
                prompt,
                tab,
                timeout_ms,
            } => {
                assert_eq!(name, "helper");
                assert_eq!(kind, hive_core::agent::AgentKind::ClaudeCode);
                assert_eq!(prompt.as_deref(), Some("Review"));
                assert!(tab);
                assert_eq!(timeout_ms, Some(2500));
            }
            Request::Send {
                text,
                wait,
                timeout_ms,
                ..
            } => {
                assert_eq!(text, "Hello\n世界");
                assert!(wait);
                assert_eq!(timeout_ms, Some(2500));
            }
            Request::Read { lines, .. } => assert_eq!(lines, Some(20)),
            Request::Wait { timeout_ms, .. } => assert_eq!(timeout_ms, Some(2500)),
            Request::Interrupt { .. } => {}
            Request::List => panic!("unexpected list"),
        }
        writeln!(
            stream,
            "{}",
            json!({"error":{"code":"agent_blocked","message":"Approval required"}})
        )
        .unwrap();
        let response = client.receive();
        assert_eq!(response["result"]["isError"], true);
        let content: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(content["error"]["code"], "agent_blocked");
    }
}
