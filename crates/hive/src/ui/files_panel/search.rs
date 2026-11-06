use super::*;
use crate::ui::text_field::TextField;
use gpui_kit::component::{
    Icon,
    input::{Input, InputEvent, InputState},
    spinner::Spinner,
};

pub(super) fn input(
    window: &mut Window,
    cx: &mut Context<FilesPanel>,
) -> (Entity<InputState>, Subscription) {
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder("Find file…")
            .clean_on_escape()
    });
    let subscription = cx.subscribe_in(&input, window, |this, input, event, _, cx| match event {
        InputEvent::Change => {
            let query = input.read(cx).value().to_string();
            this.files
                .update(cx, |files, cx| files.set_search_query(query, cx));
        }
        InputEvent::PressEnter { .. } => {
            let path = this.files.read(cx).first_search_result();
            if let Some(path) = path {
                this.files
                    .update(cx, |files, cx| files.open(path, false, cx));
            }
        }
        _ => {}
    });
    (input, subscription)
}

impl FilesPanel {
    pub(crate) fn focus_search(&self, window: &mut Window, cx: &mut App) {
        if !self
            .files
            .read(cx)
            .target
            .as_ref()
            .is_some_and(|target| target.host == hive_core::host::Host::Local)
        {
            return;
        }
        self.search_input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.set_selected_range(0..input.value().len(), cx);
        });
    }

    pub(super) fn sync_search_input(&self, window: &mut Window, cx: &mut App) {
        let query = self.files.read(cx).search_query();
        if self.search_input.read(cx).value().as_str() != query {
            let query = query.to_owned();
            self.search_input
                .update(cx, |input, cx| input.set_value(query, window, cx));
        }
    }

    pub(super) fn focus_search_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.files.read(cx).first_search_result() else {
            return;
        };
        self.files.update(cx, |files, cx| files.reveal(path, cx));
        self.sync_tree(cx);
        self.focus_tree(window, cx);
    }

    pub(super) fn search_field(&self, available: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let loading = self.files.read(cx).search_loading();
        div()
            .key_context("FileSearch")
            .px_2()
            .pb_2()
            .on_action(cx.listener(|this, _: &FocusFileResults, window, cx| {
                this.focus_search_results(window, cx)
            }))
            .child(TextField::input(
                Input::new(&self.search_input)
                    .small()
                    .h(px(SEARCH_HEIGHT))
                    .aria_label("Find file")
                    .prefix(
                        Icon::new(Lucide::Search)
                            .small()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .cleanable(true)
                    .disabled(!available)
                    .when(loading, |input| input.suffix(Spinner::new().small())),
                &self.search_input,
                cx,
            ))
    }
}
