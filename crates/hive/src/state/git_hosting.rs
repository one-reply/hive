//! Owns local hosting setup; browser/CLI operations survive settings navigation and cancel on quit.
use gpui_kit::*;
use hive_core::{
    command::Cancellation,
    git::hosting::{self, Connection, DevicePrompt, Provider, Server},
};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Checking,
    Ready(Connection),
    Installing,
    SigningIn(Option<DevicePrompt>),
    Failed(String),
}
impl Status {
    pub(crate) fn busy(&self) -> bool {
        matches!(self, Self::Checking | Self::Installing | Self::SigningIn(_))
    }
}
struct Operation {
    cancellation: Arc<Cancellation>,
    _task: Task<()>,
}
impl Drop for Operation {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}
pub(crate) struct Account {
    pub(crate) server: Server,
    pub(crate) status: Status,
    generation: u64,
    operation: Option<Operation>,
}
impl Account {
    fn accepts(&self, generation: u64) -> bool {
        self.generation == generation
    }
}
pub(crate) struct GitHosting {
    accounts: [Account; 2],
}
fn index(provider: Provider) -> usize {
    match provider {
        Provider::GitHub => 0,
        Provider::GitLab => 1,
    }
}
impl GitHosting {
    pub(crate) fn new(servers: hive_core::settings::HostingServers, _: &mut Context<Self>) -> Self {
        Self {
            accounts: Provider::ALL.map(|provider| Account {
                server: servers.server(provider),
                status: Status::Ready(Connection::SignedOut),
                generation: 0,
                operation: None,
            }),
        }
    }
    pub(crate) fn account(&self, provider: Provider) -> &Account {
        &self.accounts[index(provider)]
    }
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        for provider in Provider::ALL {
            if !self.account(provider).status.busy() {
                self.start(provider, false, None, cx);
            }
        }
    }
    pub(crate) fn check(&mut self, provider: Provider, input: &str, cx: &mut Context<Self>) {
        if self.set_server(provider, input, cx) {
            self.remember(provider, cx);
            self.start(provider, false, None, cx);
        }
    }
    pub(crate) fn connect(
        &mut self,
        provider: Provider,
        input: &str,
        client_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.set_server(provider, input, cx) {
            self.remember(provider, cx);
            self.start(provider, true, client_id, cx);
        }
    }
    fn remember(&self, provider: Provider, cx: &mut Context<Self>) {
        let server = self.account(provider).server.clone();
        crate::app::AppState::get(cx)
            .store
            .clone()
            .update(cx, |store, cx| {
                store.set_hosting_server(provider, server, cx)
            });
    }
    fn set_server(&mut self, provider: Provider, input: &str, cx: &mut Context<Self>) -> bool {
        let account = &mut self.accounts[index(provider)];
        if account.status.busy() {
            return false;
        }
        match Server::parse(input) {
            Ok(server) => {
                account.server = server;
                true
            }
            Err(error) => {
                account.status = Status::Failed(error.to_string());
                cx.notify();
                false
            }
        }
    }
    pub(crate) fn cancel(&mut self, provider: Provider, cx: &mut Context<Self>) {
        let account = &mut self.accounts[index(provider)];
        account.generation = account.generation.wrapping_add(1);
        account.operation = None;
        account.status = Status::Failed("Connection cancelled. You can try again.".into());
        cx.notify();
    }
    pub(crate) fn stop(&mut self) {
        for account in &mut self.accounts {
            account.operation = None;
        }
    }
    fn start(
        &mut self,
        provider: Provider,
        connect: bool,
        client_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let account = &mut self.accounts[index(provider)];
        account.operation = None;
        account.generation = account.generation.wrapping_add(1);
        let generation = account.generation;
        let server = account.server.clone();
        account.status = if connect {
            Status::Installing
        } else {
            Status::Checking
        };
        let cancellation = Arc::new(Cancellation::default());
        let cancel = cancellation.clone();
        let task = cx.spawn(async move |this, cx| {
            let (send, receive) = async_channel::unbounded();
            let worker = cx.background_executor().spawn(async move {
                let result = if connect {
                    let installed = if provider.executable().is_some() {
                        Ok(())
                    } else {
                        hosting::install(provider, &cancel)
                    };
                    installed.and_then(|()| {
                        let _ = send.send_blocking(Status::SigningIn(None));
                        hosting::sign_in(
                            provider,
                            &server,
                            &cancel,
                            client_id.as_deref(),
                            |prompt| {
                                let _ = send.send_blocking(Status::SigningIn(Some(prompt)));
                            },
                        )
                    })
                } else {
                    hosting::check(provider, &server, &cancel)
                };
                let _ = send.send_blocking(match result {
                    Ok(connection) => Status::Ready(connection),
                    Err(error) => {
                        tracing::warn!(provider = provider.name(), %error, "hosting setup failed");
                        Status::Failed(error.to_string())
                    }
                });
            });
            while let Ok(status) = receive.recv().await {
                let connected =
                    connect && matches!(status, Status::Ready(Connection::Connected(_)));
                let applied = this
                    .update(cx, |this, cx| {
                        let account = &mut this.accounts[index(provider)];
                        if !account.accepts(generation) {
                            return false;
                        }
                        if account.status != status {
                            account.status = status;
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !applied {
                    break;
                }
                if connected {
                    cx.update(|cx| {
                        let requests = crate::app::AppState::get(cx).pull_requests.clone();
                        requests.update(cx, |requests, cx| requests.refresh(cx));
                    });
                }
            }
            worker.await;
        });
        self.accounts[index(provider)].operation = Some(Operation {
            cancellation,
            _task: task,
        });
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{Connection, GitHosting, Provider, Status};
    use gpui_kit::{AppContext as _, TestAppContext};
    #[gpui::test]
    fn cancelling_invalidates_callbacks_and_keeps_other_provider_independent(
        cx: &mut TestAppContext,
    ) {
        let state = cx.new(|cx| GitHosting::new(Default::default(), cx));
        state.update(cx, |state, cx| {
            state.accounts[0].status = Status::SigningIn(None);
            let generation = state.accounts[0].generation;
            state.cancel(Provider::GitHub, cx);
            assert!(!state.accounts[0].accepts(generation));
            assert!(matches!(state.accounts[0].status, Status::Failed(_)));
            assert!(matches!(
                state.accounts[1].status,
                Status::Ready(Connection::SignedOut)
            ));
        });
    }
    #[gpui::test]
    fn invalid_or_busy_server_changes_cannot_start_authentication(cx: &mut TestAppContext) {
        let state = cx.new(|cx| GitHosting::new(Default::default(), cx));
        state.update(cx, |state, cx| {
            assert!(!state.set_server(Provider::GitLab, "https://evil.test/x", cx));
            assert_eq!(state.accounts[1].server.as_str(), "gitlab.com");
            state.accounts[1].status = Status::SigningIn(None);
            assert!(!state.set_server(Provider::GitLab, "other.test", cx));
            assert_eq!(state.accounts[1].server.as_str(), "gitlab.com");
        });
    }
}
