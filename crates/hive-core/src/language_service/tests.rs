use super::*;
use lsp_types::{notification::DidOpenTextDocument, request::HoverRequest, *};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hive-lsp-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let bin = path.join("node_modules/.bin/typescript-language-server");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(&bin, fake_script()).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn ready(events: &async_channel::Receiver<Event>) {
    loop {
        match events.recv_blocking().unwrap() {
            Event::Ready(capabilities) => {
                assert!(capabilities.hover_provider.is_some());
                return;
            }
            Event::Failed(error) => panic!("{error}"),
            Event::Diagnostics(_) => {}
        }
    }
}
fn starts(fixture: &Fixture) -> usize {
    fs::read_to_string(fixture.0.join("starts"))
        .unwrap()
        .lines()
        .count()
}
fn receive<R: lsp_types::request::Request>(
    mut request: super::Request<R>,
) -> Result<R::Result, Error> {
    let result = request.receiver.recv_blocking().unwrap();
    request.complete = true;
    serde_json::from_value(result?).map_err(|e| Error::Protocol(e.to_string()))
}

enum Never {}
impl lsp_types::request::Request for Never {
    type Params = Value;
    type Result = Value;
    const METHOD: &'static str = "test/never";
}
enum Crash {}
impl lsp_types::request::Request for Crash {
    type Params = Value;
    type Result = Value;
    const METHOD: &'static str = "test/crash";
}

enum Ask {}
impl lsp_types::request::Request for Ask {
    type Params = Value;
    type Result = Value;
    const METHOD: &'static str = "test/ask";
}

enum Burst {}
impl lsp_types::request::Request for Burst {
    type Params = Value;
    type Result = Value;
    const METHOD: &'static str = "test/burst";
}

#[test]
fn diagnostic_events_are_bounded_and_sanitized_before_reaching_the_consumer() {
    enum Publish {}
    impl lsp_types::request::Request for Publish {
        type Params = PublishDiagnosticsParams;
        type Result = bool;
        const METHOD: &'static str = "test/diagnostics";
    }
    let fixture = Fixture::new();
    let (session, events) = Session::start(Host::Local, fixture.0.clone(), Family::TypeScript);
    ready(&events);
    let uri = position::uri(&fixture.0.join("file.ts")).unwrap();
    let messages = [
        "see [notes](file:///x.command) or <https://docs.rs>".to_owned(),
        "😀".repeat(3000),
        format!("{}[x](file:///x)", "a".repeat(2047)),
    ];
    let publish = session
        .request::<Publish>(PublishDiagnosticsParams::new(
            uri.clone(),
            messages
                .into_iter()
                .map(|message| Diagnostic {
                    range: Range::new(Position::new(0, 2), Position::new(0, 3)),
                    message,
                    ..Default::default()
                })
                .collect(),
            Some(7),
        ))
        .unwrap();
    let Event::Diagnostics(params) = events.recv_blocking().unwrap() else {
        panic!("expected a prepared diagnostic publication");
    };
    assert_eq!(params.uri, uri);
    assert_eq!(params.version, Some(7));
    assert_eq!(params.diagnostics.len(), 3);
    assert_eq!(
        params.diagnostics[0].message,
        r"see \[notes](file:///x.command) or <https://docs.rs>"
    );
    assert_eq!(params.diagnostics[1].message, "😀".repeat(2048));
    assert_eq!(
        params.diagnostics[2].message,
        format!("{}[", "a".repeat(2047))
    );
    assert_eq!(
        params.diagnostics[0].range,
        Range::new(Position::new(0, 2), Position::new(0, 3))
    );
    assert!(receive(publish).unwrap());
    drop(session);
    while events.recv_blocking().is_ok() {}
}

/// Diagnostics the fake publishes for `test/burst`: more than the event queue holds.
const BURST: usize = 100;

#[test]
fn a_diagnostics_burst_the_app_reads_late_loses_no_file_update() {
    let fixture = Fixture::new();
    let (session, events) = Session::start(Host::Local, fixture.0.clone(), Family::TypeScript);
    ready(&events);
    let burst = session.request::<Burst>(Value::Null).unwrap();
    // The app reads nothing until the server has sent it all: the worker must wait for room
    // in the event queue, not drop what does not fit.
    let sent = fixture.0.join("burst-sent");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !sent.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the fake never finished the burst"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Bounded: an update that was dropped would otherwise be waited for forever.
    let mut line = 0;
    while line < BURST {
        match events.try_recv() {
            Ok(Event::Diagnostics(params)) => {
                assert_eq!(params.uri.as_str(), format!("file:///file{line}.ts"));
                line += 1;
            }
            Ok(_) => panic!("a healthy session must not restart during a diagnostics burst"),
            Err(async_channel::TryRecvError::Empty) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "only {line} of {BURST} file updates arrived"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(async_channel::TryRecvError::Closed) => panic!("the session ended"),
        }
    }
    assert_eq!(receive(burst).unwrap(), Value::Bool(true));
    assert_eq!(
        fs::read_to_string(fixture.0.join("starts"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    drop(session);
    while events.recv_blocking().is_ok() {}
    assert!(
        fs::read_to_string(fixture.0.join("messages"))
            .unwrap()
            .contains("shutdown\nexit\n")
    );
}

enum Flood {}
impl lsp_types::request::Request for Flood {
    type Params = Value;
    type Result = Value;
    const METHOD: &'static str = "test/flood";
}

/// Diagnostics the fake publishes for `test/flood` before it reads its next message: more
/// than the worker queue, the reader's buffer and the stdout pipe hold together.
const FLOOD: usize = 400;

fn open(session: &Session, uri: &Uri, text: String) -> Result<(), Error> {
    session.notify::<DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            language_id: "typescript".into(),
            version: 1,
            text,
        },
    })
}

fn hover_text(session: &Session, uri: &Uri) -> String {
    let hover = receive(
        session
            .request::<HoverRequest>(HoverParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri: uri.clone() },
                    position: Position::new(0, 0),
                },
                work_done_progress_params: Default::default(),
            })
            .unwrap(),
    )
    .unwrap()
    .unwrap();
    match hover.contents {
        HoverContents::Markup(MarkupContent { value, .. }) => value,
        _ => panic!("the fake answers hover with the opened text"),
    }
}

#[test]
fn a_server_that_writes_a_burst_before_reading_more_input_keeps_its_session() {
    let fixture = Fixture::new();
    let (session, events) = Session::start(Host::Local, fixture.0.clone(), Family::TypeScript);
    ready(&events);
    let uri = position::uri(&fixture.0.join("file.ts")).unwrap();
    let flood = session.request::<Flood>(Value::Null).unwrap();
    // Larger than a pipe buffer: the server takes it only after the whole burst is out.
    let text = "x".repeat(1024 * 1024);
    open(&session, &uri, text.clone()).unwrap();
    for published in 0..FLOOD {
        match events.recv_blocking().unwrap() {
            Event::Diagnostics(params) => {
                assert_eq!(params.uri.as_str(), format!("file:///flood{published}.ts"));
            }
            Event::Failed(error) => panic!("a server busy writing is not stuck: {error}"),
            Event::Ready(_) => panic!("the session restarted"),
        }
    }
    assert_eq!(receive(flood).unwrap(), Value::Bool(true));
    // Input reached the server whole and in order: the hover after it sees the opened text.
    assert_eq!(hover_text(&session, &uri), text);
    assert_eq!(starts(&fixture), 1);
    drop(session);
    while events.recv_blocking().is_ok() {}
}

enum Stall {}
impl lsp_types::request::Request for Stall {
    type Params = Value;
    type Result = Value;
    const METHOD: &'static str = "test/stall";
}
enum Blob {}
impl lsp_types::notification::Notification for Blob {
    type Params = String;
    const METHOD: &'static str = "test/blob";
}
/// More than any pipe holds, so it stays queued behind a server that does not read.
const BLOB: usize = 2 * 1024 * 1024;

#[test]
fn input_a_server_never_reads_is_bounded_and_ends_the_session() {
    let fixture = Fixture::new();
    let (session, events) = Session::start(Host::Local, fixture.0.clone(), Family::TypeScript);
    ready(&events);
    let stall = session.request::<Stall>(Value::Null).unwrap();
    let blob = "x".repeat(BLOB);
    let error = loop {
        match session.notify::<Blob>(blob.clone()) {
            Ok(()) | Err(Error::Busy) => {}
            Err(error) => panic!("the worker stopped before reporting: {error}"),
        }
        match events.try_recv() {
            Ok(Event::Failed(error)) => break error,
            Ok(_) | Err(async_channel::TryRecvError::Empty) => {}
            Err(async_channel::TryRecvError::Closed) => panic!("the session ended silently"),
        }
    };
    assert!(
        error
            .to_string()
            .contains("server stopped reading its input"),
        "{error}"
    );
    assert!(receive(stall).is_err());
    drop(session);
    while events.recv_blocking().is_ok() {}
}

#[test]
fn stopping_a_session_does_not_wait_on_input_the_server_never_reads() {
    let fixture = Fixture::new();
    let (session, events) = Session::start(Host::Local, fixture.0.clone(), Family::TypeScript);
    ready(&events);
    let stall = session.request::<Stall>(Value::Null).unwrap();
    session.notify::<Blob>("x".repeat(BLOB)).unwrap();
    drop(stall);
    drop(session);
    // Ends once the worker has given up on the queued input and cleaned up the process.
    while events.recv_blocking().is_ok() {}
    let messages = fs::read_to_string(fixture.0.join("messages")).unwrap();
    assert!(!messages.contains("shutdown"), "{messages}");
}

#[test]
fn a_server_that_dies_with_input_queued_restarts_once() {
    let fixture = Fixture::new();
    let (session, events) = Session::start(Host::Local, fixture.0.clone(), Family::TypeScript);
    ready(&events);
    let stall = session.request::<Stall>(Value::Null).unwrap();
    session.notify::<Blob>("x".repeat(BLOB)).unwrap();
    let pid = fs::read_to_string(fixture.0.join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(pid).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    assert!(receive(stall).is_err());
    assert!(matches!(events.recv_blocking().unwrap(), Event::Failed(_)));
    ready(&events);
    assert_eq!(starts(&fixture), 2);
    drop(session);
    while events.recv_blocking().is_ok() {}
}

#[test]
fn fake_server_synchronizes_unicode_cancels_requests_and_shuts_down_cleanly() {
    let fixture = Fixture::new();
    let (session, events) = Session::start(Host::Local, fixture.0.clone(), Family::TypeScript);
    ready(&events);
    let uri = position::uri(&fixture.0.join("file.ts")).unwrap();
    session
        .notify::<DidOpenTextDocument>(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "typescript".into(),
                version: 1,
                text: "const 😀 = '日本語';".into(),
            },
        })
        .unwrap();
    let cancelled = session.request::<Never>(Value::Null).unwrap();
    assert!(matches!(
        events.recv_blocking().unwrap(),
        Event::Diagnostics(_)
    ));
    drop(cancelled);
    let hover = receive(
        session
            .request::<HoverRequest>(HoverParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri },
                    position: Position::new(0, 0),
                },
                work_done_progress_params: Default::default(),
            })
            .unwrap(),
    )
    .unwrap()
    .unwrap();
    assert!(
        matches!(hover.contents,HoverContents::Markup(MarkupContent{value,..}) if value.contains("日本語"))
    );
    assert!(
        fs::read_to_string(fixture.0.join("messages"))
            .unwrap()
            .contains("$/cancelRequest")
    );
    drop(session);
    while events.recv_blocking().is_ok() {}
    let messages = fs::read_to_string(fixture.0.join("messages")).unwrap();
    assert!(messages.contains("shutdown\nexit\n"));
}

#[test]
fn a_request_past_its_deadline_fails_alone_and_the_server_keeps_running() {
    let fixture = Fixture::new();
    let (session, events) = Session::start(Host::Local, fixture.0.clone(), Family::TypeScript);
    ready(&events);
    // The fake peer skips methods it does not know, as a server still loading a project would.
    enum Silent {}
    impl lsp_types::request::Request for Silent {
        type Params = Value;
        type Result = Value;
        const METHOD: &'static str = "test/silent";
    }
    assert!(matches!(
        receive(session.request::<Silent>(Value::Null).unwrap()),
        Err(Error::TimedOut)
    ));
    let hover = session
        .request::<HoverRequest>(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: "file:///a.ts".parse().unwrap(),
                },
                position: Position::new(0, 0),
            },
            work_done_progress_params: Default::default(),
        })
        .unwrap();
    assert!(receive(hover).unwrap().is_some());
    assert_eq!(
        fs::read_to_string(fixture.0.join("starts"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn requests_from_the_server_get_the_answers_hive_gives_them() {
    let fixture = Fixture::new();
    let (session, events) = Session::start(Host::Local, fixture.0.clone(), Family::TypeScript);
    ready(&events);
    let ask = |method: &str, params: Value| {
        let relayed = serde_json::json!({"method": method, "params": params});
        receive(session.request::<Ask>(relayed).unwrap()).unwrap()
    };
    let section = serde_json::json!({"section": "typescript"});
    assert_eq!(
        ask(
            "workspace/configuration",
            serde_json::json!({"items": [section, section]})
        ),
        serde_json::json!({"result": [null, null]})
    );
    assert_eq!(
        ask(
            "workspace/configuration",
            serde_json::json!({"items": vec![section; 257]})
        ),
        serde_json::json!({"error": -32602})
    );
    let folders = ask("workspace/workspaceFolders", Value::Null);
    assert_eq!(folders["result"][0]["name"], "workspace", "{folders}");
    let folder = fixture.0.file_name().unwrap().to_str().unwrap();
    let uri = folders["result"][0]["uri"].as_str().unwrap();
    assert!(uri.trim_end_matches('/').ends_with(folder), "{folders}");
    assert_eq!(
        ask("workspace/applyEdit", serde_json::json!({"edit": {}}))["result"]["applied"],
        false
    );
    assert_eq!(
        ask(
            "window/showMessageRequest",
            serde_json::json!({"type": 1, "message": "Pick"})
        ),
        serde_json::json!({"result": null})
    );
    assert_eq!(
        ask("workspace/unknown", Value::Null),
        serde_json::json!({"error": -32601})
    );
    drop(session);
    while events.recv_blocking().is_ok() {}
}

#[test]
fn a_full_queue_is_busy_and_only_a_stopped_worker_is_unavailable() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let session = Session(Arc::new(Client {
        sender,
        next_id: AtomicI64::new(1),
        stopped: Arc::new(AtomicBool::new(false)),
    }));
    let closed = || DidCloseTextDocumentParams {
        text_document: TextDocumentIdentifier {
            uri: "file:///a.ts".parse().unwrap(),
        },
    };
    type Close = lsp_types::notification::DidCloseTextDocument;
    session.notify::<Close>(closed()).unwrap();
    assert!(matches!(
        session.notify::<Close>(closed()),
        Err(Error::Busy)
    ));
    assert!(matches!(
        session.request::<Never>(Value::Null),
        Err(Error::Busy)
    ));
    drop(receiver);
    assert!(matches!(
        session.notify::<Close>(closed()),
        Err(Error::Unavailable(_))
    ));
}

#[test]
fn a_crash_restarts_once_then_leaves_retry_to_the_caller() {
    let fixture = Fixture::new();
    let (session, events) = Session::start(Host::Local, fixture.0.clone(), Family::TypeScript);
    ready(&events);
    assert!(receive(session.request::<Crash>(Value::Null).unwrap()).is_err());
    assert!(matches!(events.recv_blocking().unwrap(), Event::Failed(_)));
    ready(&events);
    assert!(receive(session.request::<Crash>(Value::Null).unwrap()).is_err());
    assert!(matches!(events.recv_blocking().unwrap(), Event::Failed(_)));
    assert!(events.recv_blocking().is_err());
    assert_eq!(
        fs::read_to_string(fixture.0.join("starts"))
            .unwrap()
            .lines()
            .count(),
        2
    );
}

#[test]
fn a_server_that_cannot_start_reports_what_it_printed_and_parents_are_not_searched() {
    let outer = Fixture::new();
    let bin = outer.0.join("node_modules/.bin/typescript-language-server");
    // Stands in for a rustup proxy without its component, or a shell's "not found".
    let noise = "x".repeat(4096);
    fs::write(
        &bin,
        format!(
            "#!/bin/sh\necho \"error: Unknown binary\n'tool'\" >&2\necho {noise} >&2\nexit 1\n"
        ),
    )
    .unwrap();
    let (_session, events) = Session::start(Host::Local, outer.0.clone(), Family::TypeScript);
    let Event::Failed(error) = events.recv_blocking().unwrap() else {
        panic!("a server that exits at once cannot become ready");
    };
    let message = error.to_string();
    assert!(
        message.contains("error: Unknown binary 'tool' xxx"),
        "{message}"
    );
    assert!(message.len() < 2048, "stderr is bounded: {}", message.len());

    // The same executable one directory above the worktree is not this project's server.
    let nested = outer.0.join("project");
    fs::create_dir(&nested).unwrap();
    fs::write(&bin, "#!/bin/sh\n: > ran-from-parent\nexit 1\n").unwrap();
    let (session, events) = Session::start(Host::Local, nested.clone(), Family::TypeScript);
    // Failed without a server on PATH, Ready with one: either way the lookup has happened.
    let _ = events.recv_blocking();
    drop(session);
    while events.recv_blocking().is_ok() {}
    assert!(!nested.join("ran-from-parent").exists());
}

/// Set by the fixture's script for the test binary it runs as the fake server.
const FAKE_SERVER: &str = "HIVE_FAKE_LANGUAGE_SERVER";

/// The script the worker starts: this test binary with only [`fake_language_server`] selected.
/// The harness prints its own lines to standard output, so the fake writes the protocol to
/// descriptor 3, which the script makes the server's real output.
fn fake_script() -> String {
    let binary = std::env::current_exe().unwrap();
    let binary = shlex::try_quote(binary.to_str().unwrap())
        .unwrap()
        .into_owned();
    format!(
        "#!/bin/sh\n{FAKE_SERVER}=1 exec {binary} language_service::tests::fake_language_server --exact --ignored --test-threads=1 3>&1 1>/dev/null\n"
    )
}

/// Not a test of Hive: the stdio peer the other tests talk to. It records what it is sent in
/// its working directory, the fixture's temporary one.
#[test]
#[ignore = "the fake language server of these tests, run by their fixture"]
fn fake_language_server() {
    if std::env::var_os(FAKE_SERVER).is_some() {
        serve_as_fake();
    }
}

fn serve_as_fake() -> ! {
    use std::io::Write as _;
    use std::os::fd::FromRawFd as _;
    let append = |file: &str, line: &str| {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .unwrap();
        writeln!(file, "{line}").unwrap();
    };
    append("starts", "start");
    fs::write("pid", std::process::id().to_string()).unwrap();
    // SAFETY: the fixture's script opened descriptor 3 for this process alone.
    let mut output = unsafe { fs::File::from_raw_fd(3) };
    let mut send = move |value: Value| {
        output.write_all(&wire::encode(&value).unwrap()).unwrap();
    };
    let publish = |uri: &str, diagnostics: Value| serde_json::json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":uri,"diagnostics":diagnostics}});
    let mut input = std::io::BufReader::new(std::io::stdin().lock());
    let (mut source, mut uri) = (String::new(), String::new());
    // The `test/ask` request waiting for the client's answer to the request it relayed.
    let mut asking = None;
    loop {
        let (id, method, params) = match wire::read(&mut input) {
            Ok(Some(wire::Incoming::Call { id, method, params })) => (id, method, params),
            Ok(Some(wire::Incoming::Response(_, answer))) => {
                let answer = match answer {
                    Ok(result) => serde_json::json!({"result": result}),
                    Err(Error::Request { code, .. }) => serde_json::json!({"error": code}),
                    Err(error) => panic!("{error}"),
                };
                send(serde_json::json!({"jsonrpc":"2.0","id":asking.take(),"result":answer}));
                continue;
            }
            Ok(Some(wire::Incoming::Ignored(_))) => continue,
            Ok(None) | Err(_) => std::process::exit(0),
        };
        append("messages", &method);
        let result = match method.as_str() {
            "initialize" => {
                serde_json::json!({"capabilities":{"textDocumentSync":1,"hoverProvider":true}})
            }
            "textDocument/didOpen" => {
                source = params["textDocument"]["text"].as_str().unwrap().to_owned();
                uri = params["textDocument"]["uri"].as_str().unwrap().to_owned();
                continue;
            }
            "textDocument/hover" => {
                serde_json::json!({"contents":{"kind":"plaintext","value":source}})
            }
            "test/never" => {
                send(publish(&uri, serde_json::json!([])));
                continue;
            }
            "test/burst" => {
                for file in 0..BURST {
                    send(publish(
                        &format!("file:///file{file}.ts"),
                        serde_json::json!([]),
                    ));
                }
                fs::write("burst-sent", "").unwrap();
                Value::Bool(true)
            }
            "test/flood" => {
                let diagnostic = serde_json::json!([{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"message":"x".repeat(2000)}]);
                for file in 0..FLOOD {
                    send(publish(
                        &format!("file:///flood{file}.ts"),
                        diagnostic.clone(),
                    ));
                }
                Value::Bool(true)
            }
            "test/stall" => loop {
                std::thread::park();
            },
            "test/diagnostics" => {
                send(
                    serde_json::json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":params}),
                );
                Value::Bool(true)
            }
            "test/ask" => {
                asking = id;
                send(
                    serde_json::json!({"jsonrpc":"2.0","id":"ask","method":params["method"],"params":params["params"]}),
                );
                continue;
            }
            "test/crash" | "exit" => std::process::exit(0),
            "shutdown" => Value::Null,
            _ => continue,
        };
        if let Some(id) = id {
            send(serde_json::json!({"jsonrpc":"2.0","id":id,"result":result}));
        }
    }
}

#[test]
fn an_extension_in_capitals_names_the_same_language() {
    use std::path::Path;
    assert_eq!(
        Family::for_path(Path::new("src/App.TSX")),
        Some(Family::TypeScript)
    );
    assert_eq!(
        Family::for_path(Path::new("src/main.RS")),
        Some(Family::Rust)
    );
    assert_eq!(Family::language_id(Path::new("App.TSX")), "typescriptreact");
    assert_eq!(Family::language_id(Path::new("Index.Js")), "javascript");
    assert_eq!(Family::for_path(Path::new("README.MD")), None);
    assert!(crate::formatting::supported(Path::new("App.TSX")));
    assert!(crate::formatting::supported(Path::new("lib.Rs")));
}
