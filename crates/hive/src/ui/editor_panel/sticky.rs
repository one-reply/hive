use super::*;

pub(super) fn render(
    document: &Entity<Document>,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    if !AppState::get(cx)
        .store
        .read(cx)
        .state
        .settings
        .files
        .sticky_scroll
    {
        return None;
    }
    let d = document.read(cx);
    let editor = d.editor.read(cx);
    if d.symbols_text.as_ref() != Some(editor.text()) {
        return None;
    }
    let height = editor.line_height()?;
    let first_display = (-editor.scroll_offset().y / height).floor().max(0.) as usize;
    let first_line = editor.visible_row_range()?.start;
    let offset = editor.text().line_start_offset(first_line);
    let inset = (editor.range_to_bounds(&(offset..offset))?.left()
        - editor.input_bounds().left()
        - editor.scroll_offset().x)
        .max(px(0.));
    let scopes: Vec<_> = d
        .symbols
        .iter()
        .filter_map(|symbol| {
            if !symbol.range.contains(&offset) {
                return None;
            }
            let line = editor.text().offset_to_position(symbol.offset).line as usize;
            let end = editor
                .text()
                .offset_to_position(symbol.range.end.saturating_sub(1))
                .line as usize;
            if editor.display_map().is_buffer_line_hidden(line) {
                return None;
            }
            let start_row = editor
                .display_map()
                .buffer_line_to_display_row_range(line)?
                .start;
            let end_row = editor
                .display_map()
                .buffer_line_to_display_row_range(end)?
                .end;
            Some((symbol, line, start_row, end_row))
        })
        .collect();
    let chosen = visible_scopes(
        &scopes
            .iter()
            .map(|(_, _, start, end)| (*start, *end))
            .collect::<Vec<_>>(),
        first_display,
    );
    if chosen.is_empty() {
        return None;
    }
    let target = d.target.clone();
    let path = d.path.clone();
    let rows: Vec<_> = chosen
        .into_iter()
        .map(|index| {
            let (symbol, line, _, _) = scopes[index];
            let label = editor
                .text()
                .slice_line(line)
                .to_string()
                .trim_end()
                .to_owned();
            (line, label, editor.text().offset_to_position(symbol.offset))
        })
        .collect();
    let mut header = v_flex()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bg(cx.theme().background)
        .border_b_1()
        .border_color(cx.theme().border)
        .overflow_hidden();
    for (line, label, position) in rows {
        let request = OpenFile {
            target: target.clone(),
            path: path.clone(),
            position: Some(position),
            preview: false,
            placement: Default::default(),
        };
        let hover = Hover::new(("sticky-scope", line), window, cx);
        header = header.child(
            Button::new(("sticky-scope", line))
                .small()
                .cursor_pointer()
                .w_full()
                .h(height)
                .px_0()
                .justify_start()
                .tooltip(format!("{} {label}", line + 1))
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .child(
                            div()
                                .w(inset)
                                .flex_shrink_0()
                                .pr_2()
                                .text_right()
                                .text_color(cx.theme().muted_foreground)
                                .child((line + 1).to_string()),
                        )
                        .child(div().min_w_0().truncate().child(label)),
                )
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(cx.theme().mono_font_size)
                .on_click(move |_, _, cx| {
                    AppState::get(cx)
                        .documents
                        .clone()
                        .update(cx, |documents, cx| documents.request(request.clone(), cx))
                })
                .smooth_hover(ButtonLook::Ghost, &hover, cx),
        );
    }
    Some(header.into_any_element())
}

fn visible_scopes(scopes: &[(usize, usize)], first_row: usize) -> Vec<usize> {
    let mut selected: Vec<usize> = Vec::new();
    for (index, (start, end)) in scopes.iter().enumerate().rev() {
        if *start < first_row
            && *end > first_row + 1
            && !selected.iter().any(|other| scopes[*other].0 == *start)
        {
            selected.push(index);
            if selected.len() == 2 {
                break;
            }
        }
    }
    selected.reverse();
    selected
        .into_iter()
        .enumerate()
        .filter_map(|(slot, index)| (scopes[index].1 > first_row + slot + 1).then_some(index))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::visible_scopes;
    #[test]
    fn two_innermost_scopes_stick_only_after_their_display_rows_scroll_away() {
        let scopes = [(0, 100), (4, 90), (10, 50)];
        assert_eq!(visible_scopes(&scopes, 0), Vec::<usize>::new());
        assert_eq!(visible_scopes(&scopes, 4), [0]);
        assert_eq!(visible_scopes(&scopes, 11), [1, 2]);
        assert_eq!(visible_scopes(&scopes, 49), [0, 1]);
        assert!(visible_scopes(&scopes, 99).is_empty());
    }
    #[test]
    fn wrapped_or_folded_geometry_uses_display_rows_and_leaves_room_for_the_closing_line() {
        assert_eq!(visible_scopes(&[(0, 20), (3, 8)], 6), [0]);
        assert_eq!(visible_scopes(&[(0, 20), (3, 12)], 6), [0, 1]);
        assert_eq!(visible_scopes(&[(0, 50), (0, 40), (3, 20)], 6), [1, 2]);
    }
}
