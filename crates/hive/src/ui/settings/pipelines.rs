//! The Pipelines page: the library of reusable prompts that template stages follow.
//! A prompt is written here once; every stage linked to it takes its text.

use crate::{
    app::AppState,
    ui::hover::{ButtonLook, Hover, SmoothButton as _},
};
use gpui_kit::component::button::{ButtonVariant, ButtonVariants as _};
use gpui_kit::{
    assets::IconName as Lucide,
    component::{
        Sizable as _, WindowExt as _,
        button::Button,
        input::{InputState, TextareaState},
    },
    *,
};
use hive_core::pipeline::Prompt;

mod view;

/// Enough of a prompt is visible to edit it without scrolling the field.
const PROMPT_ROWS: usize = 12;

/// What the page shows: the library, or one prompt being written.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    List,
    New,
    Edit(u64),
}

pub(super) struct PipelinesPage {
    screen: Screen,
    name: Entity<InputState>,
    text: Entity<TextareaState>,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl PipelinesPage {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = AppState::get(cx).store.clone();
        Self {
            screen: Screen::List,
            name: cx.new(|cx| InputState::new(window, cx).placeholder("Prompt name")),
            text: cx.new(|cx| TextareaState::new(window, cx).rows(PROMPT_ROWS)),
            error: None,
            _subscriptions: vec![cx.observe(&store, |this, _, cx| {
                this.leave_deleted_prompt(cx);
                cx.notify();
            })],
        }
    }

    /// Opens a prompt for editing, unless that would throw away a draft with changes.
    pub(super) fn edit(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Edit(id) || self.is_dirty(cx) {
            return;
        }
        let Some(prompt) = self.prompt(id, cx) else {
            return;
        };
        self.open(Screen::Edit(id), &prompt.name, &prompt.text, window, cx);
    }

    fn start_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open(Screen::New, "", "", window, cx);
        self.name.read(cx).focus_handle(cx).focus(window, cx);
    }

    fn open(
        &mut self,
        screen: Screen,
        name: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (name, text) = (name.to_owned(), text.to_owned());
        self.name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.text
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.screen = screen;
        self.error = None;
        cx.notify();
    }

    fn show_list(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::List;
        self.error = None;
        cx.notify();
    }

    /// Back to the library; a draft with changes is discarded only after confirmation.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_dirty(cx) {
            self.show_list(cx);
            return;
        }
        let page = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let page = page.clone();
            alert
                .confirm()
                .title("Discard changes?")
                .description("The changes to this prompt have not been saved.")
                .ok_text("Discard")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, _, cx| {
                    page.update(cx, |page, cx| page.show_list(cx));
                    true
                })
        });
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let id = match self.screen {
            Screen::List => return,
            Screen::New => None,
            Screen::Edit(id) => Some(id),
        };
        let name = self.name.read(cx).value();
        let text = self.text.read(cx).value();
        let pipelines = AppState::get(cx).pipelines.clone();
        match pipelines.update(cx, |p, cx| p.save_prompt(id, &name, &text, cx)) {
            Ok(_) => self.show_list(cx),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
            }
        }
    }

    fn duplicate(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let pipelines = AppState::get(cx).pipelines.clone();
        match pipelines.update(cx, |p, cx| p.duplicate_prompt(id, cx)) {
            Ok(copy) => self.edit(copy, window, cx),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
            }
        }
    }

    fn prompt(&self, id: u64, cx: &App) -> Option<Prompt> {
        let library = &AppState::get(cx).store.read(cx).state.pipelines;
        library.prompt(id).cloned()
    }

    fn is_dirty(&self, cx: &App) -> bool {
        let saved = match self.screen {
            Screen::List => return false,
            Screen::New => None,
            Screen::Edit(id) => self.prompt(id, cx),
        };
        is_dirty(
            saved.as_ref(),
            &self.name.read(cx).value(),
            &self.text.read(cx).value(),
        )
    }

    /// A prompt deleted elsewhere takes its editor with it.
    fn leave_deleted_prompt(&mut self, cx: &mut Context<Self>) {
        if let Screen::Edit(id) = self.screen
            && self.prompt(id, cx).is_none()
        {
            self.show_list(cx);
        }
    }
}

/// Asks before removing a prompt, saying what happens to the stages that follow it.
fn confirm_delete(prompt: &Prompt, uses: usize, window: &mut Window, cx: &mut App) {
    let (id, name) = (prompt.id, prompt.name.clone());
    window.open_alert_dialog(cx, move |alert, _, _| {
        alert
            .confirm()
            .title(format!("Delete “{name}”?"))
            .description(delete_consequence(uses))
            .ok_text("Delete")
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, _, cx| {
                let pipelines = AppState::get(cx).pipelines.clone();
                if let Err(error) = pipelines.update(cx, |p, cx| p.delete_prompt(id, cx)) {
                    tracing::warn!(%error, id, "prompt was already deleted");
                }
                true
            })
    });
}

fn delete_consequence(uses: usize) -> String {
    match uses {
        0 => "No stage follows this prompt.".into(),
        1 => "1 stage follows this prompt. It keeps the text as its own instructions.".into(),
        many => format!(
            "{many} stages follow this prompt. They keep the text as their own instructions."
        ),
    }
}

/// How many template stages follow a prompt, for its row in the library.
fn uses_label(uses: usize) -> String {
    match uses {
        0 => "Not used".into(),
        1 => "Used by 1 stage".into(),
        many => format!("Used by {many} stages"),
    }
}

/// Whether the editor holds something saving would change. A new prompt is dirty once
/// anything is typed; a library prompt once its name or text differs, ignoring outer spaces.
fn is_dirty(saved: Option<&Prompt>, name: &str, text: &str) -> bool {
    let (name, text) = (name.trim(), text.trim());
    match saved {
        Some(prompt) => prompt.name != name || prompt.text != text,
        None => !name.is_empty() || !text.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Prompt, delete_consequence, is_dirty, uses_label};

    #[test]
    fn uses_label_handles_zero_one_and_many() {
        assert_eq!(uses_label(0), "Not used");
        assert_eq!(uses_label(1), "Used by 1 stage");
        assert_eq!(uses_label(12), "Used by 12 stages");
        assert!(delete_consequence(0).starts_with("No stage"));
        assert!(delete_consequence(1).starts_with("1 stage follows"));
        assert!(delete_consequence(3).starts_with("3 stages follow"));
    }

    #[test]
    fn draft_is_dirty_only_when_name_or_text_differs() {
        let saved = Prompt {
            id: 1000,
            name: "Reviewer".into(),
            text: "Check the diff.".into(),
        };
        assert!(!is_dirty(Some(&saved), " Reviewer ", "Check the diff.\n"));
        assert!(is_dirty(Some(&saved), "Reviewer 2", "Check the diff."));
        assert!(is_dirty(Some(&saved), "Reviewer", "Check everything."));
        assert!(!is_dirty(None, "  ", "\n"));
        assert!(is_dirty(None, "", "Draft"));
    }
}
