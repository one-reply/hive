use super::*;
use gpui_kit::component::input::TabSize;
use hive_core::line_edit::{self, Command};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Indentation {
    Two,
    Four,
    Tabs,
}

impl Indentation {
    pub(crate) const ALL: [Self; 3] = [Self::Two, Self::Four, Self::Tabs];
    pub(super) fn for_path(path: &std::path::Path) -> Self {
        match files::editor_language(path).as_str() {
            "rs" => Self::Four,
            "make" => Self::Tabs,
            _ => Self::Two,
        }
    }
    fn detect(text: &str) -> Option<Self> {
        // Continuation alignment can use an arbitrary column. Infer a width only from
        // repeated, consistently nested lines, and bound the work on large/minified files.
        const SAMPLE_BYTES: usize = 32 * 1024;
        let sample = &text.as_bytes()[..text.len().min(SAMPLE_BYTES)];
        let mut minimum = usize::MAX;
        let mut four_aligned = true;
        let mut tab_lines = 0;
        let mut space_lines = 0;
        for line in sample.split(|b| *b == b'\n').take(256) {
            let Some(width) = line.iter().position(|b| !b.is_ascii_whitespace()) else {
                continue;
            };
            if width == 0 {
                continue;
            }
            let indent = &line[..width];
            if indent.iter().all(|b| *b == b'\t') {
                tab_lines += 1;
            } else if width.is_multiple_of(2) && indent.iter().all(|b| *b == b' ') {
                minimum = minimum.min(width);
                four_aligned &= width.is_multiple_of(4);
                space_lines += 1;
            } else {
                return None;
            }
        }
        match (tab_lines, space_lines, minimum) {
            (2.., 0, _) => Some(Self::Tabs),
            (0, 2.., 4) if four_aligned => Some(Self::Four),
            (0, 2.., 2) => Some(Self::Two),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Two => "Spaces: 2",
            Self::Four => "Spaces: 4",
            Self::Tabs => "Tabs",
        }
    }
    pub(super) fn tab_size(self) -> TabSize {
        TabSize {
            tab_size: if self == Self::Two { 2 } else { 4 },
            hard_tabs: self == Self::Tabs,
        }
    }
}

#[derive(Default)]
pub(super) struct SelectionCommands {
    /// gpui-base 0.9 exposes only the active selection. Remember the occurrences we add;
    /// never guess native secondary ranges after an unrelated input operation.
    occurrences: Vec<Range<usize>>,
    active_occurrence: Option<Range<usize>>,
    occurrence_count: usize,
    native_multiple: bool,
    count_after_edit: Option<usize>,
    vertical_rows: Option<Range<usize>>,
}

impl SelectionCommands {
    pub(super) fn edited(&mut self, event: &InputEditsEvent) {
        let multiple = self.occurrence_count > 1 || self.native_multiple;
        *self = Self {
            native_multiple: multiple,
            count_after_edit: (multiple && event.edits().len() > 1).then(|| event.edits().len()),
            ..Default::default()
        };
    }
}

impl Document {
    pub(super) fn infer_indentation(&mut self, text: &str, cx: &mut Context<Self>) {
        if !self.needs_indentation_detection {
            return;
        }
        self.needs_indentation_detection = false;
        if let Some(style) = Indentation::detect(text)
            && self.indentation != style
        {
            self.set_indentation(style, cx);
        }
    }

    pub(crate) fn set_indentation(&mut self, indentation: Indentation, cx: &mut Context<Self>) {
        self.needs_indentation_detection = false;
        self.indentation = indentation;
        self.editor
            .update(cx, |e, cx| e.set_tab_size(indentation.tab_size(), cx));
        cx.notify();
    }

    fn cursor_count(&self) -> Option<usize> {
        if self.selection_commands.native_multiple {
            self.selection_commands
                .vertical_rows
                .as_ref()
                .map(Range::len)
                .or(self.selection_commands.count_after_edit)
        } else {
            Some(self.selection_commands.occurrence_count.max(1))
        }
    }

    pub(crate) fn native_selections(&mut self, multiple: bool, cx: &mut Context<Self>) {
        self.selection_commands = SelectionCommands {
            native_multiple: multiple,
            ..Default::default()
        };
        cx.notify();
    }

    pub(crate) fn vertical_cursor(&mut self, above: bool, cx: &mut Context<Self>) {
        let editor = self.editor.read(cx);
        let map = editor.display_map();
        // In an unwrapped buffer the native add-cursor operation extends a contiguous
        // set of rows by precisely one, clipped at the file boundaries.
        let rows = if editor.cursor_layout().is_some()
            && map.wrap_row_count() == map.buffer_line_count()
        {
            self.selection_commands.vertical_rows.clone().or_else(|| {
                let row = map.buffer_line_to_display_row(editor.cursor_position().line as usize);
                (self.cursor_count() == Some(1) && editor.selected_range().is_empty())
                    .then_some(row..row + 1)
            })
        } else {
            None
        };
        let row_count = map.display_row_count();
        self.native_selections(true, cx);
        self.selection_commands.vertical_rows = rows.map(|rows| {
            if above {
                rows.start.saturating_sub(1)..rows.end
            } else {
                rows.start..(rows.end + 1).min(row_count)
            }
        });
        if self
            .selection_commands
            .vertical_rows
            .as_ref()
            .is_some_and(|rows| rows.len() == 1)
        {
            self.native_selections(false, cx);
        }
    }

    pub(crate) fn native_selection_motion(&mut self, cx: &mut Context<Self>) {
        if self.selection_commands.native_multiple || self.selection_commands.occurrence_count > 1 {
            self.native_selections(true, cx);
        } else {
            self.selection_commands = Default::default();
        }
    }

    pub(crate) fn select_next_occurrence(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.selection_commands;
        self.editor.update(cx, |editor, cx| {
            let selection = editor.selected_range();
            let text = editor.text().to_string();
            if selection.is_empty() {
                let Some(word) = hive_core::completion::word_range(
                    &text,
                    selection.start,
                    hive_core::completion::Family::for_path(&self.path),
                )
                .filter(|r| !r.is_empty()) else {
                    return;
                };
                if state.native_multiple {
                    // An Alt-click or block selection may have added other carets. Expanding
                    // the active word through add_selection keeps those native ranges.
                    editor.add_selection(word.clone(), cx);
                } else {
                    editor.set_selected_range(word.clone(), cx);
                }
                *state = SelectionCommands {
                    occurrences: vec![word.clone()],
                    active_occurrence: Some(editor.selected_range()),
                    native_multiple: state.native_multiple,
                    occurrence_count: 1,
                    ..Default::default()
                };
                return;
            }
            if state.active_occurrence.as_ref() != Some(&selection) {
                state.occurrences = vec![selection.clone()];
                state.occurrence_count = 1;
                state.active_occurrence = Some(selection);
            }
            let Some(next) = next_occurrence(&text, &state.occurrences) else {
                return;
            };
            editor.add_selection(next.clone(), cx);
            state.vertical_rows = None;
            state.count_after_edit = None;
            // The native selection set merges touching ranges, including after wrapping.
            let joined = state
                .occurrences
                .iter()
                .filter(|s| s.start == next.end || s.end == next.start)
                .count();
            state.occurrence_count = state.occurrence_count + 1 - joined;
            state.active_occurrence = Some(editor.selected_range());
            state.occurrences.push(next);
        });
        cx.notify();
    }

    pub(crate) fn edit_lines(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.loading || self.path_busy || self.base.is_none() || self.content.shows_image() {
            return;
        }
        if self.selection_commands.native_multiple || self.selection_commands.occurrence_count > 1 {
            use gpui_kit::component::{WindowExt as _, notification::Notification};
            window.push_notification(
                Notification::info("Select one block of lines for this command."),
                cx,
            );
            return;
        }
        let editor = self.editor.read(cx);
        let selection = editor.selected_range();
        let reversed = editor.is_selection_reversed();
        let text = editor.text().clone();
        let path = self.path.clone();
        if command != Command::Comment || !line_edit::uses_jsx_comments(&path) {
            // A held shortcut must operate on the previous result. These commands only
            // need the selection and one neighboring line in either direction.
            let first = text.offset_to_point(selection.start).row.saturating_sub(1);
            let limit = text.offset_to_point(selection.end).row + 2;
            let start = text.line_start_offset(first);
            let end = text.line_start_offset(limit);
            if let Some(mut edit) = line_edit::edit(
                command,
                &path,
                &text.slice(start..end).to_string(),
                selection.start - start..selection.end - start,
            ) {
                edit.range = edit.range.start + start..edit.range.end + start;
                edit.selection = edit.selection.start + start..edit.selection.end + start;
                self.editor
                    .update(cx, |e, cx| apply_edit(e, edit, reversed, window, cx));
            }
            return;
        }
        let revision = self.revision;
        let task = cx.background_executor().spawn({
            let text = text.clone();
            let selection = selection.clone();
            let path = path.clone();
            async move { line_edit::edit(command, &path, &text.to_string(), selection) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(edit) = task.await else {
                return;
            };
            let _ = this.update_in(cx, |d, window, cx| {
                if d.revision != revision
                    || d.path != path
                    || d.path_busy
                    || d.selection_commands.native_multiple
                    || d.selection_commands.occurrence_count > 1
                {
                    return;
                }
                d.editor.update(cx, |e, cx| {
                    if e.text() != &text
                        || e.selected_range() != selection
                        || !e.focus_handle(cx).is_focused(window)
                    {
                        return;
                    }
                    apply_edit(e, edit, reversed, window, cx);
                });
            });
        })
        .detach();
    }
}

fn apply_edit(
    editor: &mut EditorState,
    edit: line_edit::Edit,
    reversed: bool,
    window: &mut Window,
    cx: &mut Context<EditorState>,
) {
    editor.set_selected_range(edit.range, cx);
    editor.replace(edit.text, window, cx);
    editor.set_selected_range_with_direction(edit.selection, reversed, cx);
}

fn next_occurrence(text: &str, selected: &[Range<usize>]) -> Option<Range<usize>> {
    let primary = selected.first()?;
    let needle = text.get(primary.clone()).filter(|s| !s.is_empty())?;
    let after = selected.last()?.end;
    text.get(after..)?
        .match_indices(needle)
        .map(|(i, _)| i + after)
        .chain(text.get(..after)?.match_indices(needle).map(|(i, _)| i))
        .map(|i| i..i + needle.len())
        .find(|r| {
            selected
                .iter()
                .all(|other| r.end <= other.start || other.end <= r.start)
        })
}

#[cfg(test)]
mod tests;
