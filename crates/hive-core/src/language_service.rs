//! Installed language servers, with bounded JSON-RPC stdio and no UI/runtime dependency.
use crate::host::Host;
use std::path::PathBuf;

mod events;
mod paths;
pub mod position;
mod process;
#[cfg(test)]
mod tests;
mod wire;
mod worker;

use lsp_types::{
    PublishDiagnosticsParams, ServerCapabilities, notification::Notification,
    request::Request as LspRequest,
};
use serde_json::Value;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI64, Ordering},
    mpsc,
};

/// Notifications relevant to document state; all payloads have been parsed at the boundary.
pub enum Event {
    /// The server has completed initialization and its capabilities are available.
    Ready(Box<ServerCapabilities>),
    /// A versioned diagnostics snapshot for a server-host file URI.
    Diagnostics(PublishDiagnosticsParams),
    /// Process failure; one automatic restart may follow.
    Failed(Error),
}

enum Message {
    Request {
        id: i64,
        method: &'static str,
        params: Value,
        reply: async_channel::Sender<Result<Value, Error>>,
    },
    Notify {
        method: &'static str,
        params: Value,
    },
    Cancel(i64),
    Incoming(u8, Result<Option<wire::Incoming>, Error>),
}

struct Client {
    sender: mpsc::SyncSender<Message>,
    next_id: AtomicI64,
    stopped: Arc<AtomicBool>,
}
impl Drop for Client {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

/// A cloneable connection; dropping its last owner shuts down the owned process group.
#[derive(Clone)]
pub struct Session(Arc<Client>);

impl Session {
    /// Starts an installed server off-thread; no process work happens on the caller thread.
    pub fn start(
        host: Host,
        root: PathBuf,
        family: Family,
    ) -> (Self, async_channel::Receiver<Event>) {
        let (sender, receiver) = mpsc::sync_channel(32);
        let (events, incoming) = async_channel::bounded(16);
        let stopped = Arc::new(AtomicBool::new(false));
        let thread_sender = sender.clone();
        let thread_stopped = stopped.clone();
        let thread_events = events.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("hive-language-server".into())
            .spawn(move || {
                worker::run(
                    host,
                    root,
                    family,
                    thread_sender,
                    receiver,
                    thread_events,
                    thread_stopped,
                )
            })
        {
            let _ = events.try_send(Event::Failed(Error::Protocol(error.to_string())));
        }
        (
            Self(Arc::new(Client {
                sender,
                next_id: AtomicI64::new(1),
                stopped,
            })),
            incoming,
        )
    }

    /// Sends a typed LSP request with a bounded queue and cancellation on ticket drop.
    pub fn request<R: LspRequest>(&self, params: R::Params) -> Result<Request<R>, Error> {
        let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
        if id <= 0 {
            return Err(Error::Unavailable("request id space exhausted"));
        }
        let params = serde_json::to_value(params).map_err(|e| Error::Protocol(e.to_string()))?;
        let (reply, receiver) = async_channel::bounded(1);
        self.0
            .sender
            .try_send(Message::Request {
                id,
                method: R::METHOD,
                params,
                reply,
            })
            .map_err(|_| Error::Unavailable("request queue full or process stopped"))?;
        Ok(Request {
            id,
            session: self.clone(),
            receiver,
            complete: false,
            marker: std::marker::PhantomData,
        })
    }

    /// Sends a typed synchronization notification. A full queue is an explicit error.
    pub fn notify<N: Notification>(&self, params: N::Params) -> Result<(), Error> {
        let params = serde_json::to_value(params).map_err(|e| Error::Protocol(e.to_string()))?;
        self.0
            .sender
            .try_send(Message::Notify {
                method: N::METHOD,
                params,
            })
            .map_err(|_| Error::Unavailable("notification queue full or process stopped"))
    }
}

/// One response future; abandoning it asks the server to cancel the associated request.
pub struct Request<R: LspRequest> {
    id: i64,
    session: Session,
    receiver: async_channel::Receiver<Result<Value, Error>>,
    complete: bool,
    marker: std::marker::PhantomData<R>,
}
impl<R: LspRequest> Request<R> {
    /// Awaits and parses the response without blocking a runtime thread.
    pub async fn receive(mut self) -> Result<R::Result, Error> {
        let result = self
            .receiver
            .recv()
            .await
            .map_err(|_| Error::Unavailable("session ended"))?;
        self.complete = true;
        serde_json::from_value(result?).map_err(|e| Error::Protocol(e.to_string()))
    }
}
impl<R: LspRequest> Drop for Request<R> {
    fn drop(&mut self) {
        if !self.complete {
            let _ = self.session.0.sender.try_send(Message::Cancel(self.id));
        }
    }
}

/// Supported server families share a process across documents of one host/worktree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    /// typescript-language-server (JavaScript, JSX, TypeScript and TSX).
    TypeScript,
    /// rust-analyzer.
    Rust,
}

impl Family {
    /// Chooses a supported language family from the source extension.
    pub fn for_path(path: &std::path::Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" => Some(Self::TypeScript),
            "rs" => Some(Self::Rust),
            _ => None,
        }
    }

    /// The LSP document language identifier.
    pub fn language_id(path: &std::path::Path) -> &'static str {
        match path.extension().and_then(|s| s.to_str()) {
            Some("jsx") => "javascriptreact",
            Some("tsx") => "typescriptreact",
            Some("js" | "mjs" | "cjs") => "javascript",
            Some("rs") => "rust",
            _ => "typescript",
        }
    }
}

/// A bounded protocol or server failure, suitable for an unavailable/retry UI.
#[derive(Clone, Debug, thiserror::Error)]
pub enum Error {
    /// The subprocess failed or was disconnected.
    #[error("Language server on {host:?} in {root}: {message}")]
    Session {
        /// Host running the process.
        host: Host,
        /// Its worktree.
        root: PathBuf,
        /// Transport or process failure.
        message: String,
    },
    /// Malformed or oversized input from the server.
    #[error("Invalid language-server message: {0}")]
    Protocol(String),
    /// Request was declined by the server.
    #[error("Language-server request failed ({code}): {message}")]
    Request {
        /// JSON-RPC error code.
        code: i64,
        /// Server explanation.
        message: String,
    },
    /// No ready process, bounded queue full, or closed session.
    #[error("Language server unavailable: {0}")]
    Unavailable(&'static str),
    /// A request exceeded its deadline.
    #[error("Language-server request timed out")]
    TimedOut,
    /// A superseded request.
    #[error("Language-server request cancelled")]
    Cancelled,
}
