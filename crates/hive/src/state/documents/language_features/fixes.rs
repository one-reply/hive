use super::*;
use gpui_kit::component::{
    WindowExt as _,
    input::{CodeActionProvider, EditorState, RopeExt as _},
    notification::Notification,
};
use std::{cell::RefCell, ops::Range as ByteRange};

pub(super) struct QuickFixes {
    document: WeakEntity<Document>,
    offered: Rc<RefCell<Option<Offer>>>,
}

struct Offer {
    snapshot: Snapshot,
    client: language_service::Session,
    selection: ByteRange<usize>,
}

impl QuickFixes {
    pub(super) fn new(document: WeakEntity<Document>) -> Self {
        Self {
            document,
            offered: Default::default(),
        }
    }
}

impl CodeActionProvider for QuickFixes {
    fn id(&self) -> SharedString {
        "language-server".into()
    }

    fn code_actions(
        &self,
        editor: Entity<EditorState>,
        selection: ByteRange<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<gpui_kit::Result<Vec<CodeAction>>> {
        self.offered.borrow_mut().take();
        // The widget leaves its previous menu open while a new request is pending.
        editor.update(cx, |editor, cx| editor.dismiss_code_action_overlay(cx));
        if editor.read(cx).selected_range() != selection {
            return Task::ready(Ok(Vec::new()));
        }
        let text = editor.read(cx).text().clone();
        let Some((mut snapshot, client, pos)) = Snapshot::capture(
            &self.document,
            &text,
            selection.start,
            |caps| {
                caps.code_action_provider
                    .as_ref()
                    .is_some_and(|cap| !matches!(cap, CodeActionProviderCapability::Simple(false)))
            },
            cx,
        ) else {
            window.push_notification(
                Notification::info("Quick fixes are unavailable for this document."),
                cx,
            );
            return Task::ready(Ok(Vec::new()));
        };
        let Some(end) = position::at(&text.to_string(), selection.end) else {
            return Task::ready(Ok(Vec::new()));
        };
        let range = Range::new(pos.position, end);
        let Some(document) = self.document.upgrade() else {
            return Task::ready(Ok(Vec::new()));
        };
        let d = document.read(cx);
        snapshot.revision = Some(d.revision);
        let resolve = d.language.as_ref()
            .and_then(|session| session.read(cx).capabilities.as_ref())
            .and_then(|caps| caps.code_action_provider.as_ref())
            .is_some_and(|cap| matches!(cap, CodeActionProviderCapability::Options(options) if options.resolve_provider == Some(true)));
        let diagnostics = d
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.range.start <= range.end && range.start <= diagnostic.range.end
            })
            .cloned()
            .collect();
        let req = client.request::<request::CodeActionRequest>(CodeActionParams {
            text_document: pos.text_document,
            range,
            context: CodeActionContext {
                diagnostics,
                only: Some(vec![CodeActionKind::QUICKFIX]),
                trigger_kind: Some(CodeActionTriggerKind::INVOKED),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        });
        let offered = self.offered.clone();
        window.spawn(cx, async move |cx| {
            let response = match req {
                Ok(ticket) => ticket.receive().await,
                Err(error) => Err(error),
            };
            let actions = match response {
                Ok(response) => response
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|item| eligible_action(item, &snapshot, resolve))
                    .collect::<Vec<_>>(),
                Err(error) => {
                    tracing::warn!(%error, "quick fixes unavailable");
                    return cx.update(|window, cx| {
                        window.push_notification(Notification::error(error.to_string()), cx);
                        Vec::new()
                    });
                }
            };
            cx.update(|window, cx| {
                if !snapshot.current(cx) || editor.read(cx).selected_range() != selection {
                    return Vec::new();
                }
                if actions.is_empty() {
                    window.push_notification(
                        Notification::info("No quick fixes for this location."),
                        cx,
                    );
                }
                *offered.borrow_mut() = Some(Offer {
                    snapshot,
                    client,
                    selection,
                });
                actions
            })
        })
    }

    fn perform_code_action(
        &self,
        editor: Entity<EditorState>,
        action: CodeAction,
        _: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<gpui_kit::Result<()>> {
        let Some(offer) = self.offered.borrow_mut().take() else {
            return Task::ready(Ok(()));
        };
        window.spawn(cx, async move |cx| {
            if let Err(error) = apply_quick_fix(offer, editor, action, cx).await {
                tracing::warn!(%error, "quick fix not applied");
                cx.update(|window, cx| {
                    window.push_notification(Notification::error(error.to_string()), cx)
                })?;
            }
            Ok(())
        })
    }
}

async fn apply_quick_fix(
    offer: Offer,
    editor: Entity<EditorState>,
    mut action: CodeAction,
    cx: &mut AsyncWindowContext,
) -> gpui_kit::Result<()> {
    let Offer {
        snapshot,
        client,
        selection,
    } = offer;
    if !cx.update(|_, cx| snapshot.current(cx) && editor.read(cx).selected_range() == selection)? {
        return Err(stale_action().into());
    }
    if action.edit.is_none() {
        action = client
            .request::<request::CodeActionResolveRequest>(action)?
            .receive()
            .await?;
    }
    let edits = edits_are_sufficient(&action)
        .then_some(action.edit)
        .flatten()
        .and_then(|edit| current_file_edits(&snapshot.uri, snapshot.version, edit))
        .ok_or_else(|| {
            std::io::Error::other(
                "This quick fix requires changes outside the current file or another command.",
            )
        })?;
    let source = snapshot.text.to_string();
    let updated = cx
        .background_executor()
        .spawn(async move { language_service::edits::apply(&source, &edits) })
        .await
        .ok_or_else(|| {
            std::io::Error::other("The server returned invalid or overlapping edits.")
        })?;
    cx.update(|window, cx| {
        if editor.read(cx).selected_range() != selection {
            return Err(stale_action().into());
        }
        snapshot.replace(updated, None, window, cx)
    })?
}

fn eligible_action(
    item: CodeActionOrCommand,
    snapshot: &Snapshot,
    resolve: bool,
) -> Option<CodeAction> {
    let CodeActionOrCommand::CodeAction(action) = item else {
        return None;
    };
    if !edits_are_sufficient(&action)
        || action.kind.as_ref().is_some_and(|kind| {
            kind.as_str() != "quickfix" && !kind.as_str().starts_with("quickfix.")
        })
    {
        return None;
    }
    let supported = match &action.edit {
        Some(edit) => current_file_edits(&snapshot.uri, snapshot.version, edit.clone()).is_some(),
        None => resolve && action.data.is_some(),
    };
    supported.then_some(action)
}

// typescript-language-server attaches this wrapper even when tsserver returned only edits.
// Its handler executes only entries of action.commands. Never run arbitrary server commands.
fn edits_are_sufficient(action: &CodeAction) -> bool {
    if action.disabled.is_some() {
        return false;
    }
    let Some(command) = &action.command else {
        return true;
    };
    if command.command != "_typescript.applyCodeActionCommand" {
        return false;
    }
    let Some([argument]) = command.arguments.as_deref() else {
        return false;
    };
    let Some(payload) = argument.get("action").and_then(|value| value.as_object()) else {
        return false;
    };
    payload
        .get("commands")
        .is_none_or(|value| value.as_array().is_some_and(Vec::is_empty))
}

fn current_file_edits(uri: &Uri, version: i32, edit: WorkspaceEdit) -> Option<Vec<TextEdit>> {
    let [document] =
        <[_; 1]>::try_from(language_service::workspace_edits::documents(edit).ok()?).ok()?;
    if position::path(&document.uri)? != position::path(uri)?
        || document.version.is_some_and(|v| v != version)
    {
        return None;
    }
    Some(document.edits)
}

impl Snapshot {
    pub(super) fn replace(
        &self,
        text: String,
        caret: Option<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui_kit::Result<()> {
        if !self.current(cx) {
            return Err(stale_action().into());
        }
        let document = self
            .document
            .upgrade()
            .ok_or_else(|| std::io::Error::other("The document was closed."))?;
        let editor = document.read(cx).editor.clone();
        editor.update(cx, |editor, cx| {
            let pos = editor.cursor_position();
            let scroll = editor.scroll_offset();
            editor.replace_all(text, window, cx);
            let caret = caret.map_or(pos, |offset| editor.text().offset_to_position(offset));
            editor.set_cursor_position(caret, window, cx);
            editor.set_scroll_offset(scroll, cx);
        });
        Ok(())
    }
}

fn stale_action() -> std::io::Error {
    std::io::Error::other("The language tools or document changed. Request the action again.")
}

#[cfg(test)]
mod tests {
    use super::{current_file_edits, edits_are_sufficient};
    use hive_core::language_service::position;
    use lsp_types::CodeAction;
    use std::path::Path;
    #[test]
    fn rust_quick_fix_targets_only_the_offered_file_and_version() {
        let action: CodeAction =
            serde_json::from_str(include_str!("fixtures/rust-fix.json")).unwrap();
        assert!(edits_are_sufficient(&action));
        let edit = action.edit.unwrap();
        let uri = position::uri(Path::new("/project/rs/src/main.rs")).unwrap();
        let wrong = position::uri(Path::new("/project/other/src/main.rs")).unwrap();
        let edits = current_file_edits(&uri, 2, edit.clone()).unwrap();
        assert_eq!(edits[0].new_text, "use crate::model::FancyWidget;\n\n");
        assert!(current_file_edits(&uri, 3, edit.clone()).is_none());
        assert!(current_file_edits(&wrong, 2, edit).is_none());
    }
    #[test]
    fn typescript_import_fix_needs_only_its_workspace_edit() {
        let mut action: CodeAction =
            serde_json::from_str(include_str!("fixtures/typescript-fix.json")).unwrap();
        assert!(edits_are_sufficient(&action));
        let command = action.command.as_mut().unwrap();
        command.arguments.as_mut().unwrap()[0]["action"]["commands"] =
            serde_json::json!([{"type":"install package"}]);
        assert!(!edits_are_sufficient(&action));
        action.command.as_mut().unwrap().command = "arbitrary".into();
        assert!(!edits_are_sufficient(&action));
    }
}
