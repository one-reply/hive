//! Owns SSH onboarding and directory requests; the dialog only renders and dispatches.
use crate::app::AppState;
use gpui_kit::*;
use hive_core::{
    command::Cancellation,
    host::Host,
    remote::{self, Directory},
};
use std::{path::PathBuf, sync::Arc};

enum Connection {
    Disconnected,
    Connecting,
    Connected(Host, Directory),
    Browsing(Host, Directory),
}

pub(crate) struct RemoteProject {
    connection: Connection,
    pub(crate) error: Option<String>,
    cancellation: Arc<Cancellation>,
    generation: u64,
    _task: Option<Task<()>>,
}

impl RemoteProject {
    pub(crate) fn new() -> Self {
        Self {
            connection: Connection::Disconnected,
            error: None,
            cancellation: Arc::default(),
            generation: 0,
            _task: None,
        }
    }

    pub(crate) fn busy(&self) -> bool {
        matches!(
            self.connection,
            Connection::Connecting | Connection::Browsing(..)
        )
    }

    pub(crate) fn directory(&self) -> Option<(&Host, &Directory)> {
        match &self.connection {
            Connection::Connected(host, directory) | Connection::Browsing(host, directory) => {
                Some((host, directory))
            }
            _ => None,
        }
    }

    pub(crate) fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.cancellation.cancel();
        self._task = None;
        self.generation += 1;
        self.connection = Connection::Disconnected;
        self.error = None;
        cx.notify();
    }

    pub(crate) fn connect(&mut self, target: String, cx: &mut Context<Self>) {
        self.start(
            Host::Ssh {
                target: target.trim().into(),
            },
            None,
            cx,
        );
    }

    pub(crate) fn browse(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some((host, _)) = self.directory() {
            self.start(host.clone(), Some(path), cx);
        }
    }

    fn start(&mut self, host: Host, path: Option<PathBuf>, cx: &mut Context<Self>) {
        self.cancellation.cancel();
        self.cancellation = Arc::default();
        self.generation += 1;
        let generation = self.generation;
        self.connection = match (path.as_ref(), self.directory()) {
            (Some(_), Some((host, directory))) => {
                Connection::Browsing(host.clone(), directory.clone())
            }
            _ => Connection::Connecting,
        };
        self.error = None;
        let cancellation = self.cancellation.clone();
        self._task = Some(cx.spawn(async move |this, cx| {
            let remote_host = host.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    match path {
                        Some(path) => remote::browse(&remote_host, &path, &cancellation),
                        None => remote::prepare(&remote_host, &cancellation),
                    }
                    .map_err(|e| e.to_string())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.complete(generation, host, result) {
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn complete(&mut self, generation: u64, host: Host, result: Result<Directory, String>) -> bool {
        if self.generation != generation {
            return false;
        }
        match result {
            Ok(directory) => self.connection = Connection::Connected(host, directory),
            Err(error) => {
                tracing::warn!(%error, "SSH onboarding failed");
                self.error = Some(error);
                self.connection =
                    match std::mem::replace(&mut self.connection, Connection::Disconnected) {
                        Connection::Browsing(host, directory) => {
                            Connection::Connected(host, directory)
                        }
                        _ => Connection::Disconnected,
                    };
            }
        }
        true
    }

    pub(crate) fn add(&self, cx: &mut Context<Self>) -> bool {
        let Connection::Connected(host, directory) = &self.connection else {
            return false;
        };
        let (store, projects) = {
            let app = AppState::get(cx);
            (app.store.clone(), app.projects.clone())
        };
        store.update(cx, |store, cx| {
            store.add_project(host.clone(), directory.path.clone(), cx)
        });
        projects.update(cx, |projects, cx| {
            projects.select(host, &directory.path, None, cx)
        });
        true
    }
}

impl Drop for RemoteProject {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::{Connection, Directory, Host, RemoteProject};
    #[test]
    fn late_connection_cannot_replace_current_host_and_failed_browse_retains_folder() {
        let mut state = RemoteProject::new();
        let host = Host::Ssh {
            target: "home".into(),
        };
        let directory = Directory {
            path: "/project".into(),
            children: vec![],
        };
        state.generation = 2;
        state.connection = Connection::Browsing(host.clone(), directory.clone());
        assert!(!state.complete(
            1,
            Host::Ssh {
                target: "old".into()
            },
            Ok(directory)
        ));
        assert_eq!(state.directory().unwrap().0, &host);
        assert!(state.busy());
        assert!(state.complete(2, host.clone(), Err("offline".into())));
        assert!(!state.busy());
        assert_eq!(
            state.directory().unwrap().1.path,
            std::path::Path::new("/project")
        );
        assert_eq!(state.error.as_deref(), Some("offline"));
        let cancellation = state.cancellation.clone();
        drop(state);
        assert!(cancellation.is_cancelled());
    }
}
