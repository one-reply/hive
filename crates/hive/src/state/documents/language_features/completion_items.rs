use hive_core::{
    completion::{self, Family},
    language_service::{markdown, position},
};
use lsp_types::{CompletionItem, CompletionTextEdit, InsertTextFormat, Range, TextEdit};

pub(super) struct Candidate {
    pub raw: CompletionItem,
    pub shown: CompletionItem,
}

pub(super) fn candidates(
    items: Vec<CompletionItem>,
    text: &str,
    cursor: usize,
    family: Family,
) -> Vec<Candidate> {
    // Every item carries a range in the same buffer; a list of a thousand is ordinary.
    let lines = position::Lines::new(text);
    let mut items: Vec<_> = items
        .into_iter()
        .filter_map(|item| {
            let shown = adapt(item.clone(), &lines, cursor, family)?;
            Some(Candidate { raw: item, shown })
        })
        .collect();
    items.sort_by(|a, b| {
        let (a, b) = (&a.shown, &b.shown);
        a.sort_text
            .as_ref()
            .unwrap_or(&a.label)
            .cmp(b.sort_text.as_ref().unwrap_or(&b.label))
            .then_with(|| a.label.cmp(&b.label))
    });
    items.truncate(completion::MAX_ITEMS);
    // The menu renders documentation as Markdown and opens any link in it. `detail` is drawn
    // as a plain label, which has no links.
    for item in &mut items {
        let shown = &mut item.shown;
        shown.documentation = shown.documentation.take().map(markdown::documentation);
        if let Some(source) = origin(&item.raw) {
            shown.detail = Some(source.into());
        }
    }
    items
}

fn origin(item: &CompletionItem) -> Option<&str> {
    let details = item.label_details.as_ref();
    item.data
        .as_ref()
        .and_then(|data| {
            data.pointer("/imports/0/full_import_path")
                .or_else(|| data.pointer("/entryNames/0/source"))
                .and_then(|source| source.as_str())
        })
        .or_else(|| {
            details
                .and_then(|d| d.description.as_deref())
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            details
                .and_then(|d| d.detail.as_deref())
                .filter(|s| !s.is_empty())
        })
}

#[cfg(test)]
fn prepare(
    items: Vec<CompletionItem>,
    text: &str,
    cursor: usize,
    family: Family,
) -> Vec<CompletionItem> {
    candidates(items, text, cursor, family)
        .into_iter()
        .map(|item| item.shown)
        .collect()
}

pub(super) fn adapt(
    mut item: CompletionItem,
    lines: &position::Lines,
    cursor: usize,
    family: Family,
) -> Option<CompletionItem> {
    let mut edit = primary(&mut item, lines, cursor, family)?;
    edit.range = lines.editor_range(edit.range)?;
    item.text_edit = Some(CompletionTextEdit::Edit(edit));
    Some(item)
}

pub(super) fn primary(
    item: &mut CompletionItem,
    lines: &position::Lines,
    cursor: usize,
    family: Family,
) -> Option<TextEdit> {
    let text = lines.text();
    if item.command.is_some() || item.insert_text_format == Some(InsertTextFormat::SNIPPET) {
        return None;
    }
    let (bytes, new_text) = match item.text_edit.take() {
        Some(CompletionTextEdit::Edit(edit)) => {
            if edit.range.start.line != edit.range.end.line {
                return None;
            }
            (
                lines.offset(edit.range.start)?..lines.offset(edit.range.end)?,
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
    // The menu paints `filter_text.len()` leading bytes of the label as the match; a server's
    // filter text is the whole lookup string.
    item.filter_text = Some(
        if completion::starts_with_ignoring_case(&item.label, prefix) {
            prefix.to_owned()
        } else {
            String::new()
        },
    );
    // GPUI's implicit completion range can include '.', and insertText alone inserts at its end.
    Some(TextEdit {
        range: Range::new(lines.at(bytes.start)?, lines.at(bytes.end)?),
        new_text,
    })
}

#[cfg(test)]
mod tests {
    use super::{Family, adapt, position, prepare};
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
        let item = adapt(item, &position::Lines::new(source), cursor, Family::Script).unwrap();
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
    fn the_menu_highlights_what_was_typed_not_the_servers_filter_text() {
        let item = |label: &str| CompletionItem {
            label: label.into(),
            filter_text: Some("totalCount".into()),
            ..CompletionItem::default()
        };
        let lines = position::Lines::new("tot");
        let named = adapt(item("totalCount"), &lines, 3, Family::Other).unwrap();
        assert_eq!(named.filter_text.as_deref(), Some("tot"));
        // A label the typed text does not begin has no leading bytes to paint.
        let aliased = adapt(item("display name"), &lines, 3, Family::Other).unwrap();
        assert_eq!(aliased.filter_text.as_deref(), Some(""));
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
    fn documentation_links_only_to_the_web_and_detail_stays_plain_text() {
        let mut total = item("total");
        total.detail = Some("[x](file:///x)".into());
        total.documentation = Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: "[notes](file:///x.command) and [docs](https://docs.rs)".into(),
        }));
        let [prepared] =
            <[CompletionItem; 1]>::try_from(prepare(vec![total], "tot", 3, Family::Other)).unwrap();
        assert_eq!(
            prepared.documentation,
            Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: r"\[notes](file:///x.command) and [docs](https://docs.rs)".into(),
            }))
        );
        assert_eq!(prepared.detail.as_deref(), Some("[x](file:///x)"));
    }

    #[test]
    fn explicit_insert_does_not_guess_a_prefix_but_replace_edits_filter_their_range() {
        let mut insert = item("anything");
        insert.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range: Range::new(Position::new(0, 3), Position::new(0, 3)),
            new_text: "!".into(),
        }));
        assert!(adapt(insert, &position::Lines::new("tot"), 3, Family::Other).is_some());
        let mut replace = item("alpha");
        replace.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range: Range::new(Position::new(0, 0), Position::new(0, 3)),
            new_text: "alpha".into(),
        }));
        assert!(
            adapt(
                replace.clone(),
                &position::Lines::new("tot"),
                3,
                Family::Other
            )
            .is_none()
        );
        replace.filter_text = Some("total".into());
        assert!(adapt(replace, &position::Lines::new("tot"), 3, Family::Other).is_some());
    }

    #[test]
    fn rejects_unsafe_edits_and_converts_utf16_to_scalar_columns() {
        let mut item = item("name");
        item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range: Range::new(Position::new(0, 2), Position::new(0, 3)),
            new_text: "name".into(),
        }));
        let safe = adapt(item.clone(), &position::Lines::new("😀x"), 4, Family::Other).unwrap();
        assert!(
            matches!(safe.text_edit, Some(CompletionTextEdit::Edit(edit)) if edit.range.start.character == 1 && edit.range.end.character == 2)
        );
        let mut snippet = item.clone();
        snippet.insert_text_format = Some(InsertTextFormat::SNIPPET);
        assert!(adapt(snippet, &position::Lines::new("😀x"), 4, Family::Other).is_none());
        item.additional_text_edits = Some(vec![TextEdit {
            range: Range::default(),
            new_text: "import name".into(),
        }]);
        assert!(adapt(item, &position::Lines::new("😀x"), 4, Family::Other).is_some());
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
            assert!(adapt(invalid, &position::Lines::new("😀x"), 4, Family::Other).is_none());
        }
    }
}
