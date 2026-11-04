//! The one global: registries of state entities. Views pull what they need from here
//! and `observe`/`subscribe` to the specific entity, so unrelated updates don't repaint them.

use gpui_kit::*;

use crate::state::git::Git;
use crate::state::terminals::Terminals;
use crate::state::{projects::Projects, store::Store, usage::Usage};
use hive_core::state::{LoadedState, StateError};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct AppState {
    pub store: Entity<Store>,
    pub projects: Entity<Projects>,
    pub usage: Entity<Usage>,
    pub(crate) git: Entity<Git>,
    pub(crate) terminals: Arc<Terminals>,
    _quit: Subscription,
}

impl Global for AppState {}

impl AppState {
    pub fn init(loaded: Result<LoadedState, StateError>, cx: &mut App) {
        let store = cx.new(|cx| Store::new(loaded, cx));
        let projects = cx.new(|cx| Projects::new(&store, cx));
        let usage = cx.new(Usage::new);
        let git = cx.new(|cx| Git::new(projects.clone(), cx));
        let quit = cx.on_app_quit(|cx| {
            let deadline = Instant::now() + Duration::from_secs(3);
            let app = Self::get(cx);
            let store = app.store.clone();
            let terminals = app.terminals.clone();
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
        cx.set_global(AppState {
            store,
            projects,
            usage,
            git,
            terminals: Arc::default(),
            _quit: quit,
        });
    }

    pub fn get(cx: &App) -> &AppState {
        cx.global::<AppState>()
    }
}
