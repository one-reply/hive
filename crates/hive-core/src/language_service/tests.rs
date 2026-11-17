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
        fs::write(&bin, FAKE).unwrap();
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

enum Burst {}
impl lsp_types::request::Request for Burst {
    type Params = Value;
    type Result = Value;
    const METHOD: &'static str = "test/burst";
}

#[test]
fn a_diagnostics_burst_preserves_the_session_and_all_file_updates() {
    let fixture = Fixture::new();
    let (session, events) = Session::start(Host::Local, fixture.0.clone(), Family::TypeScript);
    ready(&events);
    let burst = session.request::<Burst>(Value::Null).unwrap();
    for line in 0..100 {
        let Event::Diagnostics(params) = events.recv_blocking().unwrap() else {
            panic!("a healthy session must not restart during a diagnostics burst");
        };
        assert_eq!(params.uri.as_str(), format!("file:///file{line}.ts"));
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

// Python is only the fake stdio peer; it reads/writes this test's temporary directory.
const FAKE: &str = r#"#!/usr/bin/python3
import sys, json
with open('starts','a') as f: f.write('start\n')
source = ''
def send(value):
    body=json.dumps(value,ensure_ascii=False).encode()
    sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(body)).encode()+body)
    sys.stdout.buffer.flush()
while True:
    length=None
    while True:
        line=sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if line==b'\r\n': break
        if line.lower().startswith(b'content-length:'): length=int(line.split(b':')[1])
    request=json.loads(sys.stdin.buffer.read(length))
    method=request.get('method','')
    with open('messages','a') as f: f.write(method+'\n')
    if method=='initialize': result={'capabilities':{'textDocumentSync':1,'hoverProvider':True}}
    elif method=='textDocument/didOpen': source=request['params']['textDocument']['text']; uri=request['params']['textDocument']['uri']; continue
    elif method=='textDocument/hover': result={'contents':{'kind':'plaintext','value':source}}
    elif method=='test/never': send({'jsonrpc':'2.0','method':'textDocument/publishDiagnostics','params':{'uri':uri,'version':1,'diagnostics':[]}}); continue
    elif method=='test/burst':
        for i in range(100): send({'jsonrpc':'2.0','method':'textDocument/publishDiagnostics','params':{'uri':'file:///file%d.ts'%i,'diagnostics':[]}})
        result=True
    elif method=='test/crash': sys.exit(0)
    elif method=='shutdown': result=None
    elif method=='exit': sys.exit(0)
    else: continue
    send({'jsonrpc':'2.0','id':request['id'],'result':result})
"#;
