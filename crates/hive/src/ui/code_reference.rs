//! Copying selected lines of a file tab as a reference for an agent's prompt.
use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::*;
use hive_core::agent::reference::CodeReference;

actions!(code_reference, [CopyCodeReference]);

/// Puts `reference` on the clipboard, carrying itself as metadata so a terminal of the same
/// worktree pastes it relative to its directory, and says what was copied; `missing` says
/// why there is nothing to copy.
pub(crate) fn copy(
    reference: Option<CodeReference>,
    missing: &'static str,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(reference) = reference else {
        window.push_notification(Notification::info(missing), cx);
        return;
    };
    let text = reference.to_text();
    let item = match serde_json::to_string(&reference) {
        Ok(json) => ClipboardItem::new_string_with_metadata(text, json),
        Err(error) => {
            tracing::warn!(%error, "code reference copied without its metadata");
            ClipboardItem::new_string(text)
        }
    };
    cx.write_to_clipboard(item);
    window.push_notification(
        Notification::info(format!("Copied {}", reference.label())),
        cx,
    );
}
