//! The one global: registries of state entities. Views pull what they need from here
//! and `observe`/`subscribe` to the specific entity, so unrelated updates don't repaint them.

use gpui_kit::*;

use crate::state::{projects::Projects, store::Store, usage::Usage};

pub struct AppState {
    pub store: Entity<Store>,
    pub projects: Entity<Projects>,
    pub usage: Entity<Usage>,
}

impl Global for AppState {}

impl AppState {
    pub fn init(cx: &mut App) {
        let store = cx.new(|_| Store::load());
        let projects = cx.new(|cx| Projects::new(&store, cx));
        let usage = cx.new(Usage::new);
        cx.set_global(AppState { store, projects, usage });
    }

    pub fn get(cx: &App) -> &AppState {
        cx.global::<AppState>()
    }
}
