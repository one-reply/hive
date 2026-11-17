//! The toolbar button that says what language tools do for this file, or why they do not.
use super::*;
use crate::state::documents::LanguageStatus;

const FEATURES: &str = "Completion, hover, definitions and diagnostics";
const DISABLED: &str = "Language servers and formatters are off. They run code the worktree \
                        controls: node_modules binaries, build scripts, proc macros. Click to \
                        turn them on for all worktrees (Settings → Files).";
const PREVIEW: &str =
    "Preview tabs start no language server. Click to keep this tab open and start one.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Retry,
    Enable,
    Keep,
}

/// `None` when no language server handles the file type.
pub(super) fn button(
    document: &Entity<Document>,
    window: &mut Window,
    cx: &mut App,
) -> Option<impl IntoElement + use<>> {
    let status = document.read(cx).language_status(cx);
    let (label, tooltip, action) = describe(&status)?;
    let hover = Hover::new("language-service", window, cx);
    let document = document.clone();
    Some(
        Button::new("language-service")
            .small()
            .cursor_pointer()
            .label(label)
            .tooltip(tooltip)
            .disabled(action.is_none())
            .on_click(move |_, _, cx| {
                if let Some(action) = action {
                    run(action, &document, cx);
                }
            })
            .smooth_hover(ButtonLook::Ghost, &hover, cx),
    )
}

fn describe(status: &LanguageStatus) -> Option<(&'static str, SharedString, Option<Action>)> {
    Some(match status {
        LanguageStatus::Unsupported => return None,
        LanguageStatus::Disabled => ("Enable tools", DISABLED.into(), Some(Action::Enable)),
        LanguageStatus::Preview => ("Start tools", PREVIEW.into(), Some(Action::Keep)),
        LanguageStatus::Starting => ("Starting…", FEATURES.into(), None),
        LanguageStatus::Ready => ("Tools ready", FEATURES.into(), None),
        LanguageStatus::Failed(error) => ("Retry tools", error.clone().into(), Some(Action::Retry)),
    })
}

fn run(action: Action, document: &Entity<Document>, cx: &mut App) {
    match action {
        Action::Retry => {
            if let Some(language) = document.read(cx).language.clone() {
                language.update(cx, |language, cx| language.retry(cx));
            }
        }
        Action::Enable => {
            let store = AppState::get(cx).store.clone();
            let mut settings = store.read(cx).state.settings.files.clone();
            settings.language_tools = true;
            store.update(cx, |store, cx| store.set_files_settings(settings, cx));
        }
        Action::Keep => document.update(cx, |document, cx| document.pin(cx)),
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, LanguageStatus, describe};

    fn action(status: LanguageStatus) -> Option<Action> {
        describe(&status).and_then(|(_, _, action)| action)
    }

    #[test]
    fn only_states_the_user_can_change_offer_an_action() {
        assert!(describe(&LanguageStatus::Unsupported).is_none());
        assert_eq!(action(LanguageStatus::Disabled), Some(Action::Enable));
        assert_eq!(action(LanguageStatus::Preview), Some(Action::Keep));
        assert_eq!(
            action(LanguageStatus::Failed("stopped".into())),
            Some(Action::Retry)
        );
        assert_eq!(action(LanguageStatus::Starting), None);
        assert_eq!(action(LanguageStatus::Ready), None);
        let (_, tooltip, _) = describe(&LanguageStatus::Failed("no binary".into())).unwrap();
        assert_eq!(tooltip.as_ref(), "no binary");
    }
}
