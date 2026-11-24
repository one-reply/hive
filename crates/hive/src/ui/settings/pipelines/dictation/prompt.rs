//! The dictation prompt: read as a preview of its first lines, edited in its field on demand.
//! The prompt is long and rarely changed, so the card stays short until the user asks to edit.
use super::*;

/// Lines of the prompt the preview shows before it trails off.
const PREVIEW_LINES: usize = 6;
/// Rows of the field while editing; a longer prompt scrolls inside it.
pub(super) const EDIT_ROWS: usize = 9;

impl DictationCard {
    fn edit_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing_prompt = true;
        self.prompt.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn finish_prompt(&mut self, cx: &mut Context<Self>) {
        self.save_prompt(cx);
        self.editing_prompt = false;
        cx.notify();
    }

    fn reset_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt
            .update(cx, |input, cx| input.set_value(DEFAULT_PROMPT, window, cx));
        self.save_prompt(cx);
        cx.notify();
    }

    /// The Prompt heading with its actions, then the preview or the field.
    pub(super) fn prompt_section(
        &self,
        saved: &Recipe,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let reset_hover = Hover::new("reset-dictation-prompt", window, cx);
        let toggle_hover = Hover::new("edit-dictation-prompt", window, cx);
        let reset = Button::new("reset-dictation-prompt")
            .cursor_pointer()
            .small()
            .icon(Lucide::RotateCcw)
            .tooltip("Reset prompt")
            .accessibility_label("Reset prompt")
            .disabled(saved.prompt() == DEFAULT_PROMPT)
            .on_click(cx.listener(|card, _, window, cx| card.reset_prompt(window, cx)))
            .smooth_hover(ButtonLook::Ghost, &reset_hover, cx);
        let toggle = if self.editing_prompt {
            Button::new("edit-dictation-prompt")
                .cursor_pointer()
                .small()
                .icon(Lucide::Check)
                .label("Done")
                .on_click(cx.listener(|card, _, _, cx| card.finish_prompt(cx)))
                .smooth_hover(ButtonLook::Outline, &toggle_hover, cx)
        } else {
            Button::new("edit-dictation-prompt")
                .cursor_pointer()
                .small()
                .icon(Lucide::Pencil)
                .tooltip("Edit prompt")
                .accessibility_label("Edit prompt")
                .on_click(cx.listener(|card, _, window, cx| card.edit_prompt(window, cx)))
                .smooth_hover(ButtonLook::Ghost, &toggle_hover, cx)
        };
        let file_transcription = saved
            .provider()
            .zip(saved.model())
            .is_some_and(|(provider, model)| provider.uses_file_transcription(model));
        let body = if self.editing_prompt {
            TextField::textarea(
                Textarea::new(&self.prompt).aria_label("Dictation prompt"),
                &self.prompt,
                cx,
            )
            .into_any_element()
        } else {
            self.prompt_preview(saved.prompt(), cx).into_any_element()
        };
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(label("Prompt"))
                    .child(h_flex().gap_1().child(reset).child(toggle)),
            )
            .child(body)
            .when(file_transcription, |section| {
                section.child(div().text_xs().text_color(muted).child(
                    "Use the prompt to give context, such as project names and technical terms \
                     you expect to say.",
                ))
            })
            .when(self.editing_prompt, |section| {
                section.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("Saves when you click Done or leave the field."),
                )
            })
    }

    /// The first lines of the prompt as text; a click opens it for editing too.
    fn prompt_preview(&self, prompt: &str, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        div()
            .id("dictation-prompt-preview")
            .cursor_pointer()
            .px_3()
            .py_2()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .text_sm()
            .text_color(theme.muted_foreground)
            .whitespace_normal()
            .line_clamp(PREVIEW_LINES)
            .text_ellipsis()
            .child(prompt.to_owned())
            .on_click(cx.listener(|card, _, window, cx| card.edit_prompt(window, cx)))
    }
}
