use super::{
    Error, Event, Family, Message, position,
    process::Process,
    wire::{Id, Incoming},
};
use crate::host::Host;
use lsp_types::{
    ConfigurationParams, InitializeResult, PositionEncodingKind, PublishDiagnosticsParams,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError, SyncSender},
    },
    time::{Duration, Instant},
};

// Tests wait for a real deadline; the production value would make them minutes long.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(if cfg!(test) { 2 } else { 15 });
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(30);
type Pending = BTreeMap<i64, (Instant, async_channel::Sender<Result<Value, Error>>)>;

pub(super) fn run(
    host: Host,
    root: PathBuf,
    family: Family,
    sender: SyncSender<Message>,
    receiver: Receiver<Message>,
    events: async_channel::Sender<Event>,
    stopped: Arc<AtomicBool>,
) {
    let physical = super::paths::physical_root(&host, &root);
    for generation in 0..=1 {
        if stopped.load(Ordering::Acquire) {
            break;
        }
        let result = Process::start(
            &host,
            &root,
            family,
            generation,
            sender.clone(),
            stopped.clone(),
        )
        .map_err(|error| (error, None))
        .and_then(|mut process| {
            serve(
                &mut process,
                &root,
                &physical,
                generation,
                &receiver,
                &events,
                &stopped,
            )
            .map_err(|error| {
                let output = (!matches!(error, Error::Cancelled))
                    .then(|| process.startup_output())
                    .flatten();
                (error, output)
            })
        });
        match result {
            Ok(()) | Err((Error::Cancelled, _)) => break,
            Err((error, output)) => {
                let message = match output {
                    Some(output) => format!("{family:?}: {error}: {output}"),
                    None => format!("{family:?}: {error}"),
                };
                let error = Error::Session {
                    host: host.clone(),
                    root: root.clone(),
                    message,
                };
                tracing::warn!(%error, attempt=generation+1,"language server stopped");
                if super::events::send(&events, Event::Failed(error), &stopped).is_err() {
                    break;
                }
            }
        }
    }
}

fn serve(
    process: &mut Process,
    root: &Path,
    physical: &Path,
    generation: u8,
    receiver: &Receiver<Message>,
    events: &async_channel::Sender<Event>,
    stopped: &AtomicBool,
) -> Result<(), Error> {
    let uri = position::uri(physical).ok_or_else(|| Error::Protocol("Invalid root URI".into()))?;
    process.write(&json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "processId":null,"clientInfo":{"name":"Hive"},"rootUri":uri,
        "workspaceFolders":[{"uri":uri,"name":root.file_name().unwrap_or_default().to_string_lossy()}],
        "capabilities":{
            "general":{"positionEncodings":["utf-16"]},
            "workspace":{"configuration":true,"workspaceFolders":true,"applyEdit":false},
            "textDocument":{
                "synchronization":{"dynamicRegistration":false,"didSave":true},
                "completion":{"completionItem":{"snippetSupport":false,"insertReplaceSupport":false}},
                "hover":{"contentFormat":["markdown","plaintext"]},
                "definition":{"linkSupport":true},"references":{},
                "publishDiagnostics":{"versionSupport":true,"relatedInformation":true}
            }
        }
    }}))?;
    let mut ready = false;
    let initialize_deadline = Instant::now() + INITIALIZE_TIMEOUT;
    let mut pending = Pending::new();
    let result = (|| loop {
        if stopped.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let now = Instant::now();
        let abandoned: Vec<_> = pending
            .iter()
            .filter_map(|(id, (_, reply))| reply.is_closed().then_some(*id))
            .collect();
        for id in abandoned {
            pending.remove(&id);
            cancel(process, id)?;
        }
        if !ready && now >= initialize_deadline {
            return Err(Error::TimedOut);
        }
        // A slow answer is not a dead server: restarting one that is still loading a large
        // project only makes it start over. Exit, a closed pipe or a stuck write end a session.
        for id in expired(&pending, now) {
            if let Some((_, reply)) = pending.remove(&id) {
                let _ = reply.try_send(Err(Error::TimedOut));
            }
            cancel(process, id)?;
        }
        let wait = pending
            .values()
            .map(|(deadline, _)| deadline.saturating_duration_since(now))
            .min()
            .unwrap_or(Duration::from_secs(1))
            .min(Duration::from_secs(1));
        let message = match receiver.recv_timeout(wait) {
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return shutdown(process, receiver, generation),
        };
        match message {
            Message::Request {
                id,
                method,
                mut params,
                reply,
            } => {
                if !ready {
                    let _ = reply.try_send(Err(Error::Unavailable("initializing")));
                    continue;
                }
                if pending.len() >= 64 {
                    let _ = reply.try_send(Err(Error::Unavailable("too many requests")));
                    continue;
                }
                if reply.is_closed() {
                    continue;
                }
                super::paths::rebase(&mut params, root, physical);
                process.write(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
                pending.insert(id, (Instant::now() + REQUEST_TIMEOUT, reply));
            }
            Message::Notify { method, mut params } if ready => {
                super::paths::rebase(&mut params, root, physical);
                process.write(&json!({"jsonrpc":"2.0","method":method,"params":params}))?
            }
            Message::Notify { .. } => {}
            Message::Cancel(id) => {
                if let Some((_, reply)) = pending.remove(&id) {
                    let _ = reply.try_send(Err(Error::Cancelled));
                    cancel(process, id)?;
                }
            }
            Message::Incoming(from, _) if from != generation => {}
            Message::Incoming(_, incoming) => match incoming?.ok_or(Error::Unavailable(
                "server closed stdout; check that the language server is installed",
            ))? {
                Incoming::Response(Id::Number(0), result) if !ready => {
                    let initialized: InitializeResult = serde_json::from_value(result?)
                        .map_err(|e| Error::Protocol(e.to_string()))?;
                    if initialized
                        .capabilities
                        .position_encoding
                        .as_ref()
                        .is_some_and(|encoding| *encoding != PositionEncodingKind::UTF16)
                    {
                        return Err(Error::Protocol(
                            "Server selected unsupported position encoding".into(),
                        ));
                    }
                    process.write(&json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
                    ready = true;
                    super::events::send(
                        events,
                        Event::Ready(Box::new(initialized.capabilities)),
                        stopped,
                    )?;
                }
                Incoming::Response(Id::Number(id), result) => {
                    if let Some((_, reply)) = pending.remove(&id) {
                        let _ = reply.try_send(result.map(|mut value| {
                            super::paths::inbound(&mut value, physical, root);
                            value
                        }));
                    }
                }
                Incoming::Response(_, _) => {}
                Incoming::Call {
                    id: Some(id),
                    method,
                    params,
                } => server_request(process, id, &method, params, &uri)?,
                Incoming::Call {
                    id: None,
                    method,
                    mut params,
                } if method == "textDocument/publishDiagnostics" => {
                    super::paths::inbound(&mut params, physical, root);
                    match serde_json::from_value::<PublishDiagnosticsParams>(params) {
                        Ok(diagnostics) => {
                            super::events::send(events, Event::Diagnostics(diagnostics), stopped)?
                        }
                        // One publication the client cannot type is not a dead server.
                        Err(error) => tracing::debug!(%error, "skipped unreadable diagnostics"),
                    }
                }
                Incoming::Call { .. } => {}
            },
        }
    })();
    let result = if matches!(result, Err(Error::Cancelled)) {
        shutdown(process, receiver, generation)
    } else {
        result
    };
    for (_, (_, reply)) in pending {
        let _ = reply.try_send(Err(result
            .as_ref()
            .err()
            .cloned()
            .unwrap_or(Error::Cancelled)));
    }
    result
}

fn expired(pending: &Pending, now: Instant) -> Vec<i64> {
    pending
        .iter()
        .filter_map(|(id, (deadline, _))| (now >= *deadline).then_some(*id))
        .collect()
}
fn cancel(process: &mut Process, id: i64) -> Result<(), Error> {
    process.write(&json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":id}}))
}

fn server_request(
    process: &mut Process,
    id: Id,
    method: &str,
    params: Value,
    root: &lsp_types::Uri,
) -> Result<(), Error> {
    let result = match method {
        "workspace/configuration" => {
            let configuration: ConfigurationParams=serde_json::from_value(params).map_err(|e|Error::Protocol(e.to_string()))?;
            if configuration.items.len() > 256 {
                return process.write(&json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Too many configuration sections"}}));
            }
            Value::Array(vec![Value::Null;configuration.items.len()])
        },
        "workspace/workspaceFolders"=>json!([{"uri":root,"name":"workspace"}]),
        "workspace/applyEdit"=>json!({"applied":false,"failureReason":"Hive does not apply server-initiated workspace edits"}),
        "window/showMessageRequest"=>Value::Null,
        _=>return process.write(&json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not supported"}})),
    };
    process.write(&json!({"jsonrpc":"2.0","id":id,"result":result}))
}

fn shutdown(
    process: &mut Process,
    receiver: &Receiver<Message>,
    generation: u8,
) -> Result<(), Error> {
    process.closing(&json!({"jsonrpc":"2.0","id":-1,"method":"shutdown","params":null}))?;
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Message::Incoming(from, Ok(Some(Incoming::Response(Id::Number(-1), _)))))
                if from == generation =>
            {
                break;
            }
            Ok(Message::Request { reply, .. }) => {
                let _ = reply.try_send(Err(Error::Cancelled));
            }
            Ok(_) => {}
            Err(_) => break,
        }
        if Instant::now() >= deadline {
            break;
        }
    }
    process.closing(&json!({"jsonrpc":"2.0","method":"exit"}))?;
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Message::Incoming(from, Ok(None))) if from == generation => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deadlines_only_expire_requests_that_have_reached_their_limit() {
        let now = Instant::now();
        let (sender, _) = async_channel::bounded(1);
        let pending = Pending::from([
            (1, (now, sender.clone())),
            (2, (now + Duration::from_secs(1), sender)),
        ]);
        assert_eq!(expired(&pending, now), [1]);
    }
}
