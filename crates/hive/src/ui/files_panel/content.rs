//! The content search results: a virtualized list over `Files`' results, driven by keys.
//! Which files are collapsed and what is selected is view state; the model holds results.
use super::*;
use crate::ui::keymap::CONTENT_RESULTS_CONTEXT;
use gpui_kit::base::actions::{Cancel, Confirm, SelectDown, SelectLeft, SelectRight, SelectUp};
use gpui_kit::component::scroll::ScrollableElement as _;
use std::{collections::HashSet, ops::Range};

mod row;
mod rows;
use rows::{Effect, Key, Row};

pub(super) struct Results {
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    rows: Vec<Row>,
    selected: Option<usize>,
    collapsed: HashSet<PathBuf>,
    /// The model's `(epoch, revision)` the rows were built from.
    synced: Option<(u64, u64)>,
}

impl Results {
    pub(super) fn new(cx: &mut App) -> Self {
        Self {
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            rows: Vec::new(),
            selected: None,
            collapsed: HashSet::new(),
            synced: None,
        }
    }
}

impl FilesPanel {
    /// Rebuild the rows when the results changed; a new result set starts at the top.
    pub(super) fn sync_content(&mut self, cx: &mut Context<Self>) {
        let files = self.files.read(cx);
        let version = (files.content_epoch(), files.content_revision());
        let results = &mut self.content;
        if results.synced == Some(version) {
            return;
        }
        if results.synced.map(|(epoch, _)| epoch) != Some(version.0) {
            results.selected = None;
            results.collapsed.clear();
            results.scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
        results.rows = rows::rows(files.content_results(), &results.collapsed);
        results.synced = Some(version);
    }

    /// Focus what the field hands off to: the results in Content mode, the tree otherwise.
    pub(crate) fn focus_list(&self, window: &mut Window, cx: &mut App) {
        match self.files.read(cx).search_mode() {
            SearchMode::Content => self.content.focus.focus(window, cx),
            SearchMode::Names => self.focus_tree(window, cx),
        }
    }

    /// Down from the field: select and focus the first result.
    pub(super) fn focus_first_result(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.content.rows.is_empty() {
            return;
        }
        self.select_result(0, cx);
        self.content.focus.focus(window, cx);
    }

    fn select_result(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.content.selected = Some(ix);
        self.content
            .scroll
            .scroll_to_item(ix, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn set_collapsed(&mut self, file: usize, collapsed: bool, cx: &mut Context<Self>) {
        let Some(path) = self
            .files
            .read(cx)
            .content_results()
            .get(file)
            .map(|file| file.path.clone())
        else {
            return;
        };
        if collapsed {
            self.content.collapsed.insert(path);
        } else {
            self.content.collapsed.remove(&path);
        }
        self.content.rows = rows::rows(
            self.files.read(cx).content_results(),
            &self.content.collapsed,
        );
        if let Some(header) = self
            .content
            .rows
            .iter()
            .position(|row| *row == Row::File { file })
        {
            self.select_result(header, cx);
        }
    }

    fn navigate_results(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        let results = self.files.read(cx).content_results();
        let collapsed = &self.content.collapsed;
        let is_collapsed = |file: usize| {
            results
                .get(file)
                .is_some_and(|file| collapsed.contains(&file.path))
        };
        match rows::navigate(&self.content.rows, is_collapsed, self.content.selected, key) {
            Effect::Select(ix) => self.select_result(ix, cx),
            Effect::Collapse { file } => self.set_collapsed(file, true, cx),
            Effect::Expand { file } => self.set_collapsed(file, false, cx),
            Effect::FocusInput => self.focus_search(window, cx),
            Effect::None => {}
        }
    }

    /// Open a line's match, or fold a file header.
    fn activate_result(&mut self, ix: usize, preview: bool, cx: &mut Context<Self>) {
        let Some(row) = self.content.rows.get(ix).copied() else {
            return;
        };
        self.select_result(ix, cx);
        let results = self.files.read(cx).content_results();
        match row {
            Row::File { file } => {
                let collapsed = results
                    .get(file)
                    .is_some_and(|file| self.content.collapsed.contains(&file.path));
                self.set_collapsed(file, !collapsed, cx);
            }
            Row::Line { file, line } => {
                let Some((path, line, column)) = results.get(file).and_then(|file| {
                    let found = file.lines.get(line)?;
                    Some((file.path.clone(), found.line, found.column))
                }) else {
                    return;
                };
                self.files.update(cx, |files, cx| {
                    files.open_at(path, line, column, preview, cx)
                });
            }
        }
    }

    pub(super) fn render_content_status(&self, cx: &App) -> impl IntoElement {
        let files = self.files.read(cx);
        let (text, tone) = rows::status_text(
            &files.content_status(),
            &files.content_query().text,
            files.content_matched_lines(),
            files.content_results().len(),
        );
        div()
            .id("content-search-status")
            .role(Role::Status)
            .aria_label(text.clone())
            .px_2()
            .pb_1()
            .text_size(rems(META_SIZE / 16.))
            .text_color(match tone {
                rows::Tone::Muted => cx.theme().muted_foreground,
                rows::Tone::Danger => cx.theme().danger,
            })
            .child(text)
    }

    pub(super) fn render_results(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("content-results")
            .key_context(CONTENT_RESULTS_CONTEXT)
            .track_focus(&self.content.focus)
            .flex_1()
            .min_h_0()
            .min_w_0()
            .px_1()
            .on_action(cx.listener(|this, _: &SelectUp, window, cx| {
                this.navigate_results(Key::Up, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectDown, window, cx| {
                this.navigate_results(Key::Down, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectLeft, window, cx| {
                this.navigate_results(Key::Left, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectRight, window, cx| {
                this.navigate_results(Key::Right, window, cx)
            }))
            .on_action(cx.listener(|this, _: &Confirm, _, cx| {
                if let Some(ix) = this.content.selected {
                    this.activate_result(ix, false, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Cancel, window, cx| this.focus_search(window, cx)))
            .child(
                uniform_list(
                    "content-results-list",
                    self.content.rows.len(),
                    cx.processor(|this, range: Range<usize>, window, cx| {
                        range
                            .filter_map(|ix| this.render_result(ix, window, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.content.scroll)
                .size_full(),
            )
            .vertical_scrollbar(&self.content.scroll)
    }

    fn render_result(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let row = *self.content.rows.get(ix)?;
        let selected = self.content.selected == Some(ix);
        let files = self.files.read(cx);
        let settings = files.settings.clone();
        let file = files.content_results().get(row.file())?;
        let element = match row {
            Row::File { .. } => {
                let header = row::Header {
                    path: file.path.clone(),
                    count: file.lines.len(),
                    collapsed: self.content.collapsed.contains(&file.path),
                };
                row::file(header, selected, &settings, window, cx)
            }
            Row::Line { line, .. } => {
                let digits = file.lines.last().map_or(1, |last| digits(last.line + 1));
                let key = format!("content-line:{}:{line}", file.path.display());
                let found = file.lines.get(line)?.clone();
                row::line(key, found, digits, selected, &settings, window, cx)
            }
        };
        Some(
            element.on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.content.focus.focus(window, cx);
                this.activate_result(ix, event.click_count() < 2, cx);
            })),
        )
    }
}

fn digits(number: u32) -> usize {
    number.checked_ilog10().map_or(1, |log| log as usize + 1)
}

#[cfg(test)]
mod tests {
    #[test]
    fn line_number_width_counts_digits() {
        assert_eq!(super::digits(1), 1);
        assert_eq!(super::digits(10), 2);
        assert_eq!(super::digits(12_345), 5);
    }
}
