use hive_core::{
    completion::{self, Family},
    language_service::position,
};
use lsp_types::{CompletionItem, CompletionTextEdit, InsertTextFormat, Range, TextEdit};

pub(super) fn prepare(
    items: Vec<CompletionItem>,
    text: &str,
    cursor: usize,
    family: Family,
) -> Vec<CompletionItem> {
    let mut items: Vec<_> = items
        .into_iter()
        .filter_map(|item| adapt(item, text, cursor, family))
        .collect();
    items.sort_by(|a, b| {
        a.sort_text
            .as_ref()
            .unwrap_or(&a.label)
            .cmp(b.sort_text.as_ref().unwrap_or(&b.label))
            .then_with(|| a.label.cmp(&b.label))
    });
    items.truncate(completion::MAX_ITEMS);
    items
}

fn adapt(
    mut item: CompletionItem,
    text: &str,
    cursor: usize,
    family: Family,
) -> Option<CompletionItem> {
    if item.command.is_some()
        || item
            .additional_text_edits
            .as_ref()
            .is_some_and(|edits| !edits.is_empty())
        || item.insert_text_format == Some(InsertTextFormat::SNIPPET)
    {
        return None;
    }
    let (bytes, new_text) = match item.text_edit.take() {
        Some(CompletionTextEdit::Edit(edit)) => {
            if edit.range.start.line != edit.range.end.line {
                return None;
            }
            (
                position::offset(text, edit.range.start)?..position::offset(text, edit.range.end)?,
                edit.new_text,
            )
        }
        Some(CompletionTextEdit::InsertAndReplace(_)) => return None,
        None => (
            completion::word_range(text, cursor, family)?,
            item.insert_text
                .take()
                .unwrap_or_else(|| item.label.clone()),
        ),
    };
    if bytes.start > cursor || cursor > bytes.end {
        return None;
    }
    let prefix = text.get(bytes.start..cursor)?;
    if !completion::starts_with_ignoring_case(
        item.filter_text.as_deref().unwrap_or(&item.label),
        prefix,
    ) {
        return None;
    }
    // GPUI's implicit completion range can include '.', and insertText alone inserts at its end.
    item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
        range: position::editor_range(
            text,
            Range::new(
                position::at(text, bytes.start)?,
                position::at(text, bytes.end)?,
            ),
        )?,
        new_text,
    }));
    Some(item)
}

#[cfg(test)]
mod tests {
    use super::{Family, adapt, prepare};
    use gpui::component::input::{Editor, EditorState};
    use gpui::{
        AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, Window, div,
    };
    use gpui_kit as gpui;
    use lsp_types::*;

    fn item(label: &str) -> CompletionItem {
        CompletionItem {
            label: label.into(),
            ..Default::default()
        }
    }

    struct Harness {
        editor: Entity<EditorState>,
    }
    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(Editor::new(&self.editor))
        }
    }

    fn accepted(
        source: &str,
        cursor: usize,
        item: CompletionItem,
        cx: &mut TestAppContext,
    ) -> String {
        cx.update(gpui::init);
        let item = adapt(item, source, cursor, Family::Script).unwrap();
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let state = cx.new(|cx| EditorState::new(window, cx));
            state.update(cx, |state, cx| {
                state.set_value(source, window, cx);
                // Deliberately include punctuation, as the widget's fallback does.
                state.insert_completion(&item, 0..cursor, window, cx);
            });
            editor = Some(state.clone());
            Harness { editor: state }
        });
        cx.read(|cx| editor.as_ref().unwrap().read(cx).text().to_string())
    }

    #[gpui::test]
    fn accepting_label_only_preserves_member_punctuation(cx: &mut TestAppContext) {
        assert_eq!(accepted("object.fo", 9, item("foo"), cx), "object.foo");
    }

    #[gpui::test]
    fn accepting_insert_text_replaces_the_whole_unicode_word(cx: &mut TestAppContext) {
        let mut item = item("café");
        item.insert_text = Some("café()".into());
        assert_eq!(accepted("😀.cax", 7, item, cx), "😀.café()");
    }

    #[test]
    fn filters_before_sorting_and_honors_server_filter_and_sort_text() {
        let mut alias = item("display name");
        alias.filter_text = Some("totalCount".into());
        alias.sort_text = Some("0".into());
        let mut total = item("total");
        total.sort_text = Some("1".into());
        let items = prepare(
            vec![item("alpha"), total, alias, item("Totally")],
            "tot",
            3,
            Family::Other,
        );
        assert_eq!(
            items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["display name", "total", "Totally"]
        );
    }

    #[test]
    fn explicit_insert_does_not_guess_a_prefix_but_replace_edits_filter_their_range() {
        let mut insert = item("anything");
        insert.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range: Range::new(Position::new(0, 3), Position::new(0, 3)),
            new_text: "!".into(),
        }));
        assert!(adapt(insert, "tot", 3, Family::Other).is_some());
        let mut replace = item("alpha");
        replace.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range: Range::new(Position::new(0, 0), Position::new(0, 3)),
            new_text: "alpha".into(),
        }));
        assert!(adapt(replace.clone(), "tot", 3, Family::Other).is_none());
        replace.filter_text = Some("total".into());
        assert!(adapt(replace, "tot", 3, Family::Other).is_some());
    }

    #[test]
    fn rejects_unsafe_edits_and_converts_utf16_to_scalar_columns() {
        let mut item = item("name");
        item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range: Range::new(Position::new(0, 2), Position::new(0, 3)),
            new_text: "name".into(),
        }));
        let safe = adapt(item.clone(), "😀x", 4, Family::Other).unwrap();
        assert!(
            matches!(safe.text_edit, Some(CompletionTextEdit::Edit(edit)) if edit.range.start.character == 1 && edit.range.end.character == 2)
        );
        let mut snippet = item.clone();
        snippet.insert_text_format = Some(InsertTextFormat::SNIPPET);
        assert!(adapt(snippet, "😀x", 4, Family::Other).is_none());
        item.additional_text_edits = Some(vec![TextEdit {
            range: Range::default(),
            new_text: "import name".into(),
        }]);
        assert!(adapt(item, "😀x", 4, Family::Other).is_none());
        for range in [
            Range::new(Position::new(0, 3), Position::new(0, 2)),
            Range::new(Position::new(0, 1), Position::new(0, 3)),
            Range::new(Position::new(0, 0), Position::new(0, 0)),
        ] {
            let mut invalid = super::tests::item("name");
            invalid.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: "name".into(),
            }));
            assert!(adapt(invalid, "😀x", 4, Family::Other).is_none());
        }
    }
}
