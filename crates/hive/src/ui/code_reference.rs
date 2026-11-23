//! Copying selected lines of a file tab as a reference for an agent's prompt.
use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::*;
use hive_core::agent::reference::CodeReference;

actions!(code_reference, [CopyCodeReference]);

/// Copies the reference with metadata for worktree-relative pasting, or shows `missing`
/// when there is no reference to copy.
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
}

#[cfg(test)]
mod tests {
    use gpui_kit as gpui;
    use gpui_kit::{
        AppContext as _, EmptyView, TestAppContext, base::Root, component::WindowExt as _,
    };
    use hive_core::{
        agent::reference::{CodeReference, Version},
        host::Host,
    };
    use std::path::Path;

    #[gpui::test]
    fn only_a_missing_reference_shows_a_notification(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| EmptyView);
            Root::new(view, window, cx)
        });
        let reference = CodeReference::new(
            Host::Local,
            Path::new("/repo"),
            Path::new("file.rs"),
            7..=16,
            Version::Worktree,
        )
        .unwrap();
        cx.update(|window, cx| {
            let missing = "This file has no lines to copy.";
            let text = Some(reference.to_text());
            super::copy(Some(reference), missing, window, cx);
            assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), text);
            assert!(window.notifications(cx).is_empty());

            super::copy(None, missing, window, cx);
            assert_eq!(window.notifications(cx).len(), 1);
            assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), text);
        });
    }
}
