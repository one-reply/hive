//! The private directory local panes reach this app through: Claude hook reports, `hive agent`
//! calls and the `hive` launcher on their `PATH`. Bound before the first pane starts, so
//! restored panes get hooks and the agent API too.
use hive_core::agent::{api, hooks};
use std::path::PathBuf;
use std::sync::Arc;

/// Everything bound at startup, handed to [`super::Agents::new`].
pub(crate) struct Endpoint {
    pub(super) hooks: Arc<hooks::Server>,
    pub(super) calls: Option<async_channel::Receiver<api::Incoming>>,
    pub(super) pane: PaneEndpoint,
}

/// What a local terminal is given so its agent can report to and call this app.
#[derive(Clone, Debug)]
pub(crate) struct PaneEndpoint {
    /// Claude hook datagram socket.
    pub(crate) hooks: PathBuf,
    /// `hive agent` socket, when it could be bound.
    pub(crate) api: Option<PathBuf>,
    /// Directory with the `hive` launcher, when it could be linked.
    pub(crate) launcher: Option<PathBuf>,
}

impl Endpoint {
    /// Blocking: bind both sockets in one owner-only directory and link the launcher. Without
    /// the directory agents fall back to estimated status; without the API they only lose
    /// `hive agent`.
    pub(crate) fn bind() -> Option<Self> {
        let hooks = hooks::Server::bind()
            .inspect_err(
                |error| tracing::error!(%error, "agent hooks unavailable; using estimated status"),
            )
            .ok()?;
        let directory = hooks.directory().to_owned();
        let socket = directory.join("a");
        let calls = api::listen(&socket)
            .inspect_err(|error| tracing::error!(%error, "hive agent API unavailable"))
            .ok();
        let launcher = std::env::current_exe()
            .and_then(|executable| api::link_launcher(&directory.join("bin"), &executable))
            .inspect_err(
                |error| tracing::warn!(%error, "hive launcher unavailable; panes use HIVE_BIN"),
            )
            .ok();
        Some(Self {
            pane: PaneEndpoint {
                hooks: hooks.path().to_owned(),
                api: calls.is_some().then_some(socket),
                launcher,
            },
            hooks: Arc::new(hooks),
            calls,
        })
    }
}
