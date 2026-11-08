//! The one global: registries of state entities. Views pull what they need from here
//! and `observe`/`subscribe` to the specific entity, so unrelated updates don't repaint them.

use gpui_kit::*;

use crate::state::agents::Agents;
use crate::state::ai_providers::AiProviders;
use crate::state::git::Git;
use crate::state::terminals::Terminals;
use crate::state::updates::Updates;
use crate::state::{projects::Projects, store::Store, usage::Usage};
use hive_core::state::{LoadedState, StateError};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct AppState {
    pub store: Entity<Store>,
    pub projects: Entity<Projects>,
    pub usage: Entity<Usage>,
    pub(crate) ai_providers: Entity<AiProviders>,
    pub(crate) agents: Entity<Agents>,
    pub(crate) git: Entity<Git>,
    pub(crate) documents: Entity<crate::state::documents::Documents>,
    pub(crate) pull_requests: Entity<crate::state::pull_requests::PullRequests>,
    pub(crate) terminals: Arc<Terminals>,
    pub(crate) updates: Entity<Updates>,
    _quit: Subscription,
    _quit_requests: Task<()>,
}

impl Global for AppState {}

impl AppState {
    pub fn init(
        loaded: Result<LoadedState, StateError>,
        recovery: (Vec<hive_core::files::recovery::Record>, Vec<String>),
        endpoint: Option<crate::state::agents::Endpoint>,
        cx: &mut App,
    ) {
        let store = cx.new(|cx| Store::new(loaded, cx));
        let projects = cx.new(|cx| Projects::new(&store, cx));
        let usage = cx.new(Usage::new);
        let ai_providers = cx.new(AiProviders::new);
        let agents = cx.new(|cx| Agents::new(endpoint, cx));
        let git = cx.new(|cx| Git::new(projects.clone(), cx));
        let documents =
            cx.new(|_| crate::state::documents::Documents::new(store.clone(), recovery));
        let updates = cx.new(Updates::new);
        let pull_requests = cx.new(|cx| {
            crate::state::pull_requests::PullRequests::new(projects.clone(), git.clone(), cx)
        });
        let quit = cx.on_app_quit(|cx| {
            let deadline = Instant::now() + Duration::from_secs(3);
            let app = Self::get(cx);
            let store = app.store.clone();
            let terminals = app.terminals.clone();
            app.git.read(cx).stop_generations();
            let documents = app.documents.clone();
            documents.update(cx, |documents, cx| documents.flush(cx));
            store.update(cx, |store, _| store.stop());
            terminals.stop();
            // GPUI allows its returned quit futures only 200 ms and stops foreground tasks.
            // This user-approved barrier waits solely for independent background work.
            let terminals_done = terminals.wait_until(deadline);
            let saved = store.read(cx).wait_until(deadline);
            if !terminals_done || !saved {
                tracing::error!(terminals_done, saved, "shutdown deadline exceeded");
            }
            async {}
        });
        let quit_requests = crate::quit::init(cx).unwrap_or_else(|error| {
            tracing::error!(%error, "native quit guard unavailable");
            store.update(cx, |store, _| {
                let message = format!(
                    "System quit protection unavailable: {error}. Unsaved edits will use recovery."
                );
                store.notice = Some(match store.notice.take() {
                    Some(previous) => format!("{previous}\n{message}"),
                    None => message,
                });
            });
            Task::ready(())
        });
        cx.set_global(AppState {
            store,
            projects,
            usage,
            ai_providers,
            agents,
            git,
            documents,
            terminals: Arc::default(),
            updates,
            pull_requests,
            _quit: quit,
            _quit_requests: quit_requests,
        });
    }

    pub fn get(cx: &App) -> &AppState {
        cx.global::<AppState>()
    }
}
