// No glob import of `super`: it re-exports `gpui_kit::test`, which `#[gpui::test]` expands to.
use super::Store;
use super::startup::open;
use gpui_kit as gpui;
use gpui_kit::{AppContext as _, Entity, TestAppContext};
use hive_core::state::{Access, LoadedState, Recovery, RecoveryCause, State};
use std::sync::{Arc, Mutex};

/// A store whose saves are recorded instead of written, opened on `recovery`. It keeps no
/// editor drafts, so nothing here names the data folder.
fn store(
    recovery: Option<RecoveryCause>,
    access: Access,
    cx: &mut TestAppContext,
) -> (Entity<Store>, Arc<Mutex<Vec<State>>>) {
    let mut state = State::default();
    state.settings.set_git_panel_width(480.);
    let loaded = LoadedState {
        state,
        recovery: recovery.map(|cause| Recovery {
            backup: "/backup/state.original.json".into(),
            cause,
        }),
    };
    let mut opened = open(Ok(loaded), access);
    opened.keeps_drafts = false;
    let saved = Arc::new(Mutex::new(Vec::new()));
    let written = saved.clone();
    let store = cx.new(|cx| {
        Store::start(
            opened,
            move |state| {
                written.lock().unwrap().push(state.clone());
                Ok(())
            },
            cx,
        )
    });
    cx.run_until_parked();
    (store, saved)
}

#[gpui::test]
fn a_file_with_unreadable_parts_is_rewritten_before_anything_changes(cx: &mut TestAppContext) {
    for cause in [
        RecoveryCause::Corrupt,
        RecoveryCause::Partial(vec!["settings.sidebar_width".into()]),
    ] {
        let (_store, saved) = store(Some(cause), Access::Unguarded, cx);
        let saved = saved.lock().unwrap();
        assert_eq!(saved.len(), 1, "the next launch must find a readable file");
        assert_eq!(saved[0].settings.git_panel_width(), 480.);
    }
}

#[gpui::test]
fn a_file_that_read_cleanly_or_is_not_this_sessions_to_write_is_left_alone(
    cx: &mut TestAppContext,
) {
    let partial = || Some(RecoveryCause::Partial(vec!["settings".into()]));
    for (recovery, access) in [
        (None, Access::Unguarded),
        (partial(), Access::Held),
        (
            Some(RecoveryCause::Newer { version: 99 }),
            Access::Unguarded,
        ),
    ] {
        let (store, saved) = store(recovery, access, cx);
        assert!(saved.lock().unwrap().is_empty());
        // And a session without a writer stays without one.
        store.update(cx, |store, cx| store.set_git_panel_width(500., cx));
        cx.run_until_parked();
        let writes = store.read_with(cx, |store, _| store.can_save());
        assert_eq!(saved.lock().unwrap().len(), usize::from(writes));
    }
}

#[gpui::test]
fn a_dictation_edit_is_saved_only_when_it_changes_the_recipe(cx: &mut TestAppContext) {
    use hive_core::llm::Provider;
    let (store, saved) = store(None, Access::Unguarded, cx);
    let edit = |cx: &mut TestAppContext, provider| {
        let result = store.update(cx, |store, cx| {
            store.edit_dictation(cx, |recipe| recipe.set_provider(Some(provider)))
        });
        cx.run_until_parked();
        result
    };
    assert!(edit(cx, Provider::OpenRouter).is_ok());
    assert_eq!(saved.lock().unwrap().len(), 1);
    assert_eq!(
        saved.lock().unwrap()[0].settings.dictation.provider(),
        Some(Provider::OpenRouter)
    );
    // The same choice again, and one the recipe refuses, leave the file alone.
    assert!(edit(cx, Provider::OpenRouter).is_ok());
    assert!(edit(cx, Provider::Anthropic).is_err());
    assert_eq!(saved.lock().unwrap().len(), 1);
    let provider = store.read_with(cx, |store, _| store.state.settings.dictation.provider());
    assert_eq!(provider, Some(Provider::OpenRouter));
}
