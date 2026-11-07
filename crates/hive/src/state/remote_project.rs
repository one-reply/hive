//! Owns SSH onboarding and directory requests; the dialog only renders and dispatches.
use crate::app::AppState;
use gpui_kit::*;
use hive_core::{
    command::Cancellation,
    host::{Host, ssh_config::SshHost},
    remote::{self, Directory},
    state::home_dir,
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

/// SSH hosts Add Project offers: targets of existing SSH projects first, as the likeliest
/// next pick, then the remaining `~/.ssh/config` aliases. Reading the config is disk I/O,
/// so it runs on the background executor.
pub(crate) fn known_hosts(cx: &App) -> Task<Vec<SshHost>> {
    let used: Vec<String> = AppState::get(cx)
        .store
        .read(cx)
        .state
        .projects
        .iter()
        .filter_map(|project| match &project.host {
            Host::Ssh { target } => Some(target.clone()),
            Host::Local => None,
        })
        .collect();
    cx.background_executor().spawn(async move {
        let config = home_dir()
            .map(|home| hive_core::host::ssh_config::hosts(&home))
            .unwrap_or_default();
        merge_hosts(config, used)
    })
}

fn merge_hosts(config: Vec<SshHost>, used: Vec<String>) -> Vec<SshHost> {
    let mut hosts: Vec<SshHost> = Vec::with_capacity(config.len() + used.len());
    for target in used {
        if hosts.iter().any(|host| host.alias == target) {
            continue;
        }
        let known = config.iter().find(|host| host.alias == target).cloned();
        hosts.push(known.unwrap_or(SshHost {
            alias: target,
            hostname: None,
            user: None,
        }));
    }
    for host in config {
        if !hosts.iter().any(|known| known.alias == host.alias) {
            hosts.push(host);
        }
    }
    hosts
}

impl Drop for RemoteProject {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::{Connection, Directory, Host, RemoteProject, SshHost, merge_hosts};

    fn host(alias: &str, hostname: Option<&str>) -> SshHost {
        SshHost {
            alias: alias.into(),
            hostname: hostname.map(Into::into),
            user: None,
        }
    }

    #[test]
    fn used_targets_come_first_once_and_keep_config_details() {
        let config = vec![host("home", Some("10.0.0.2")), host("build", None)];
        let used = vec!["build".into(), "me@box".into(), "build".into()];
        assert_eq!(
            merge_hosts(config, used),
            [
                host("build", None),
                host("me@box", None),
                host("home", Some("10.0.0.2"))
            ]
        );
    }

    #[test]
    fn no_config_and_no_projects_offer_nothing() {
        assert!(merge_hosts(Vec::new(), Vec::new()).is_empty());
    }
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
