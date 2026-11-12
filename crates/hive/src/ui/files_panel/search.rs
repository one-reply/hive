//! The panel's search field and its mode switch: file names filter the tree, file contents
//! replace it with results. The typed text follows the field across modes.
use super::*;
use crate::state::files::{ContentStatus, SearchMode, SearchOption};
use crate::ui::text_field::TextField;
use gpui_kit::component::{
    Icon, Selectable as _,
    input::{Input, InputEvent, InputState},
    spinner::Spinner,
    tab::{Tab, TabBar},
};

/// Tint behind matched text, in both modes. Strong enough to scan a long result list at a glance.
const HIGHLIGHT_OPACITY: f32 = 0.4;

/// How matched text is drawn in file names and in content results: the editors' yellow
/// find-match marker, because a primary-colored tint is barely visible where primary is
/// close to the foreground (dark themes).
pub(super) fn match_highlight(cx: &App) -> HighlightStyle {
    HighlightStyle {
        color: Some(cx.theme().foreground),
        background_color: Some(cx.theme().yellow.opacity(HIGHLIGHT_OPACITY)),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    }
}

pub(super) fn input(
    window: &mut Window,
    cx: &mut Context<FilesPanel>,
) -> (Entity<InputState>, Subscription) {
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder(SearchMode::Names))
            .clean_on_escape()
    });
    let subscription = cx.subscribe_in(&input, window, |this, input, event, _, cx| match event {
        InputEvent::Change => {
            let text = input.read(cx).value().to_string();
            this.files
                .update(cx, |files, cx| files.set_search_text(text, cx));
        }
        InputEvent::PressEnter { .. } => this.confirm_search(cx),
        _ => {}
    });
    (input, subscription)
}

fn placeholder(mode: SearchMode) -> &'static str {
    match mode {
        SearchMode::Names => "Find file…",
        SearchMode::Content => "Search in files…",
    }
}

impl FilesPanel {
    pub(crate) fn focus_search(&self, window: &mut Window, cx: &mut App) {
        if self.files.read(cx).target.is_none() {
            return;
        }
        self.search_input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.set_selected_range(0..input.value().len(), cx);
        });
    }

    /// Show the field in `mode` with its text selected, ready to type over.
    pub(crate) fn open_search(
        &mut self,
        mode: SearchMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.files
            .update(cx, |files, cx| files.set_search_mode(mode, cx));
        self.sync_search_input(window, cx);
        self.focus_search(window, cx);
    }

    /// Enter opens the first matching file, or searches the contents again at once.
    fn confirm_search(&mut self, cx: &mut Context<Self>) {
        if self.files.read(cx).search_mode() == SearchMode::Content {
            self.files
                .update(cx, |files, cx| files.rerun_content_search(cx));
            return;
        }
        if let Some(path) = self.files.read(cx).first_search_result() {
            self.files
                .update(cx, |files, cx| files.open(path, false, cx));
        }
    }

    pub(super) fn sync_search_input(&self, window: &mut Window, cx: &mut App) {
        let files = self.files.read(cx);
        let (text, mode) = (files.search_text().to_owned(), files.search_mode());
        self.search_input.update(cx, |input, cx| {
            input.set_placeholder(placeholder(mode), window, cx);
            if input.value().as_str() != text {
                input.set_value(text, window, cx);
            }
        });
    }

    pub(super) fn focus_search_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.files.read(cx).search_mode() == SearchMode::Content {
            self.focus_first_result(window, cx);
            return;
        }
        let Some(path) = self.files.read(cx).first_search_result() else {
            return;
        };
        self.files.update(cx, |files, cx| files.reveal(path, cx));
        self.sync_tree(cx);
        self.focus_tree(window, cx);
    }

    /// `Files | Content`: what the field searches.
    pub(super) fn mode_switch(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.files.read(cx).search_mode();
        let tab = |label: &'static str| Tab::new().label(label).flex_1().cursor_pointer();
        let panel = cx.entity();
        div().px_2().pb_2().child(
            TabBar::new("files-search-mode")
                .segmented()
                .small()
                .w_full()
                .selected_index(match mode {
                    SearchMode::Names => 0,
                    SearchMode::Content => 1,
                })
                .child(tab("Files"))
                .child(tab("Content"))
                .on_click(move |ix, window, cx| {
                    let mode = if *ix == 0 {
                        SearchMode::Names
                    } else {
                        SearchMode::Content
                    };
                    panel.update(cx, |panel, cx| panel.open_search(mode, window, cx));
                }),
        )
    }

    pub(super) fn search_field(
        &self,
        available: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let files = self.files.read(cx);
        let mode = files.search_mode();
        let loading = match mode {
            SearchMode::Names => files.search_loading(),
            SearchMode::Content => files.content_status() == ContentStatus::Searching,
        };
        let query = files.content_query().clone();
        let options = (mode == SearchMode::Content).then(|| {
            h_flex()
                .gap_0p5()
                .child(self.option(SearchOption::MatchCase, query.case_sensitive, window, cx))
                .child(self.option(SearchOption::WholeWord, query.whole_word, window, cx))
                .child(self.option(SearchOption::Regex, query.regex, window, cx))
        });
        let suffix = h_flex()
            .gap_1()
            .when(loading, |suffix| suffix.child(Spinner::new().small()))
            .children(options);
        div()
            .key_context("FileSearch")
            .px_2()
            .pb_2()
            .on_action(cx.listener(|this, _: &FocusFileResults, window, cx| {
                this.focus_search_results(window, cx)
            }))
            .child(TextField::input(
                Input::new(&self.search_input)
                    .aria_label(placeholder(mode))
                    .prefix(
                        Icon::new(Lucide::Search)
                            .small()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .cleanable(true)
                    .disabled(!available)
                    .suffix(suffix),
                &self.search_input,
                cx,
            ))
    }

    fn option(
        &self,
        option: SearchOption,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (id, icon, tooltip) = match option {
            SearchOption::MatchCase => ("search-match-case", Lucide::CaseSensitive, "Match Case"),
            SearchOption::WholeWord => ("search-whole-word", Lucide::WholeWord, "Match Whole Word"),
            SearchOption::Regex => ("search-regex", Lucide::Regex, "Use Regular Expression"),
        };
        let hover = Hover::new(id, window, cx);
        Button::new(id)
            .xsmall()
            .compact()
            .cursor_pointer()
            .icon(icon)
            .selected(on)
            .tooltip(tooltip)
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.files
                    .update(cx, |files, cx| files.toggle_search_option(option, cx));
                this.focus_search(window, cx);
            }))
            .smooth_hover(ButtonLook::Ghost, &hover, cx)
    }
}
