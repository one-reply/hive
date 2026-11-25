//! GPUI inserts only a completion's primary edit. A list containing deferred/compound items
//! goes through one acceptance path so a click and a keystroke perform the same transaction.
use super::{
    completion_items::{Candidate, primary},
    *,
};
use gpui_kit::component::{WindowExt as _, notification::Notification};

pub(crate) struct ImportMenu {
    snapshot: Snapshot,
    client: language_service::Session,
    candidates: Vec<Candidate>,
    pub(crate) selected: usize,
    pub(crate) cursor: usize,
    resolve: bool,
}

pub(super) fn needs_acceptance(item: &CompletionItem, resolve: bool) -> bool {
    resolve && item.data.is_some()
        || item
            .additional_text_edits
            .as_ref()
            .is_some_and(|edits| !edits.is_empty())
}

impl ImportMenu {
    pub(super) fn new(
        snapshot: Snapshot,
        client: language_service::Session,
        candidates: Vec<Candidate>,
        cursor: usize,
        resolve: bool,
    ) -> Self {
        Self {
            snapshot,
            client,
            candidates,
            selected: 0,
            cursor,
            resolve,
        }
    }
    pub(crate) fn items(&self) -> impl ExactSizeIterator<Item = &CompletionItem> {
        self.candidates.iter().map(|c| &c.shown)
    }
    pub(crate) fn step(&mut self, down: bool) {
        let count = self.candidates.len();
        self.selected = if down {
            (self.selected + 1) % count
        } else {
            (self.selected + count - 1) % count
        };
    }
    pub(crate) fn accept(mut self, index: usize, window: &mut Window, cx: &mut App) {
        if index >= self.candidates.len() || !self.snapshot.current(cx) {
            return;
        }
        let mut item = self.candidates.swap_remove(index).raw;
        let origin = self.cursor;
        window
            .spawn(cx, async move |cx| {
                let result: gpui_kit::Result<()> = async {
                    if self.resolve && item.data.is_some() {
                        item = self
                            .client
                            .request::<request::ResolveCompletionItem>(item)?
                            .receive()
                            .await?;
                    }
                    let source = self.snapshot.text.to_string();
                    let family = hive_core::completion::Family::for_path(&self.snapshot.path);
                    let changed = cx
                        .background_executor()
                        .spawn(async move { accepted(item, &source, origin, family) })
                        .await
                        .ok_or_else(|| {
                            std::io::Error::other(
                                "The server returned an unsupported completion edit.",
                            )
                        })?;
                    cx.update(|window, cx| {
                        let Some(doc) = self.snapshot.document.upgrade() else {
                            return Ok(());
                        };
                        let editor = doc.read(cx).editor.read(cx);
                        if !editor.focus_handle(cx).is_focused(window)
                            || editor.cursor() != origin
                            || !self.snapshot.current(cx)
                        {
                            return Ok(());
                        }
                        self.snapshot
                            .replace(changed.0, Some(changed.1), window, cx)
                    })?
                }
                .await;
                if let Err(error) = result {
                    tracing::warn!(%error,"completion not applied");
                    let _ = cx.update(|window, cx| {
                        window.push_notification(Notification::error(error.to_string()), cx)
                    });
                }
            })
            .detach();
    }
}

fn accepted(
    mut item: CompletionItem,
    source: &str,
    cursor: usize,
    family: hive_core::completion::Family,
) -> Option<(String, usize)> {
    let main = primary(&mut item, &position::Lines::new(source), cursor, family)?;
    language_service::edits::completion(
        source,
        main,
        item.additional_text_edits.unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::accepted;
    #[test]
    fn captured_server_autoimports_show_the_module_and_apply_the_resolved_edit() {
        use crate::state::documents::language_features::completion_items::candidates;
        use hive_core::completion::Family;
        use lsp_types::CompletionItem;
        #[derive(serde::Deserialize)]
        struct Capture {
            source: String,
            initial: CompletionItem,
            resolved: CompletionItem,
        }
        for (fixture, family, module, expected) in [
            (
                include_str!("fixtures/typescript-autoimport.json"),
                Family::Script,
                "./Button",
                "import { Button } from \"./Button\";\n\nexport const Demo = () => <Button",
            ),
            (
                include_str!("fixtures/rust-autoimport.json"),
                Family::Other,
                "crate::model::FancyWidget",
                "use crate::model::FancyWidget;\n\nmod model;\nfn main(){ let _ = FancyWidget",
            ),
        ] {
            let Capture {
                source,
                initial,
                resolved,
            } = serde_json::from_str(fixture).unwrap();
            let items = candidates(vec![initial], &source, source.len(), family);
            assert!(super::needs_acceptance(&items[0].raw, true));
            assert_eq!(items[0].shown.detail.as_deref(), Some(module));
            let (text, caret) = accepted(resolved, &source, source.len(), family).unwrap();
            assert_eq!(text, expected);
            assert_eq!(caret, expected.len());
        }
    }
    #[test]
    fn resolves_additional_edits_against_the_same_emoji_prefixed_buffer() {
        let item=serde_json::from_value(serde_json::json!({
            "label":"HashMap","textEdit":{"range":{"start":{"line":1,"character":2},"end":{"line":1,"character":4}},"newText":"HashMap"},
            "additionalTextEdits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"use std::collections::HashMap;\n"}]
        })).unwrap();
        let source = "// source\n😀Ha";
        let (text, caret) = accepted(
            item,
            source,
            source.len(),
            hive_core::completion::Family::Other,
        )
        .unwrap();
        assert_eq!(text, "use std::collections::HashMap;\n// source\n😀HashMap");
        assert_eq!(caret, text.len());
    }
}
