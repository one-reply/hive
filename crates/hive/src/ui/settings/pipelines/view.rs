//! Rendering of the Pipelines page: the prompt library, and the editor of one prompt.
use super::*;
use crate::ui::{hover::SmoothHoverBg as _, text_field::TextField};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, h_flex,
    input::{Input, Textarea},
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;

/// Lines of a prompt's text shown in its library row.
const PREVIEW_LINES: usize = 2;

impl Render for PipelinesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.screen {
            Screen::List => self.render_library(window, cx),
            Screen::New | Screen::Edit(_) => self.render_editor(window, cx),
        }
    }
}

impl PipelinesPage {
    fn render_library(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let library = &AppState::get(cx).store.read(cx).state.pipelines;
        let prompts: Vec<_> = library
            .prompts()
            .iter()
            .map(|prompt| (prompt.clone(), library.prompt_uses(prompt.id)))
            .collect();
        let new_prompt = action(
            "new-prompt",
            "New prompt",
            Some(Lucide::Plus),
            ButtonLook::Primary,
            window,
            cx,
            |page, window, cx| page.start_new(window, cx),
        );
        let header = h_flex()
            .justify_between()
            .items_start()
            .gap_4()
            .child(card_title(
                "Prompts",
                "A prompt tells a stage's agent who it is and how to do its work. Hive adds the \
                 task, the lane assignments, the handoff and the report format itself.",
                cx,
            ))
            .child(div().flex_none().child(new_prompt));
        let body = if prompts.is_empty() {
            empty_library(cx)
        } else {
            v_flex().gap_0p5().children(
                prompts
                    .iter()
                    .map(|(prompt, uses)| self.render_row(prompt, *uses, window, cx)),
            )
        };
        card(cx)
            .child(header)
            .when_some(self.error.clone(), |card, error| {
                card.child(error_line(error, cx))
            })
            .child(body)
    }

    /// One library prompt: its name, the start of its text and how many stages follow it.
    fn render_row(
        &self,
        prompt: &Prompt,
        uses: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = prompt.id;
        let row = SharedString::from(format!("prompt-{id}"));
        let hover = Hover::new(row.clone(), window, cx);
        let (page, menu_prompt) = (cx.entity(), prompt.clone());
        let more = Button::new(SharedString::from(format!("prompt-actions-{id}")))
            .cursor_pointer()
            .ghost()
            .xsmall()
            .icon(Lucide::Ellipsis)
            .accessibility_label(format!("Actions for {}", prompt.name))
            .tooltip("Prompt actions")
            .dropdown_menu(move |menu, _, _| prompt_menu(menu, &page, &menu_prompt, uses));
        let theme = cx.theme();
        let (muted, surface) = (theme.muted_foreground, theme.muted);
        h_flex()
            .id(row)
            .role(Role::Button)
            .aria_label(prompt.name.clone())
            .tab_index(0)
            .gap_3()
            .px_3()
            .py_2p5()
            .rounded_md()
            .cursor_pointer()
            .hover_bg(&hover, theme.transparent, surface)
            .focus_visible(move |style| style.bg(surface))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(prompt.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .line_clamp(PREVIEW_LINES)
                            .child(prompt.text.clone()),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(muted)
                    .child(uses_label(uses)),
            )
            .child(more)
            .on_click(cx.listener(move |page, _, window, cx| page.edit(id, window, cx)))
            .on_key_down(cx.listener(move |page, event: &KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    page.edit(id, window, cx);
                }
            }))
    }

    fn render_editor(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let saved = match self.screen {
            Screen::Edit(id) => self.prompt(id, cx),
            Screen::List | Screen::New => None,
        };
        let library = &AppState::get(cx).store.read(cx).state.pipelines;
        let uses = saved.as_ref().map_or(0, |p| library.prompt_uses(p.id));
        let dirty = self.is_dirty(cx);
        let back = action(
            "back-to-prompts",
            "Prompts",
            Some(Lucide::ArrowLeft),
            ButtonLook::Ghost,
            window,
            cx,
            |page, window, cx| page.close(window, cx),
        );
        let cancel = action(
            "cancel-prompt",
            "Cancel",
            None,
            ButtonLook::Ghost,
            window,
            cx,
            |page, window, cx| page.close(window, cx),
        );
        // Nothing to save until the name or the text differs from the library's copy.
        let save_hover = Hover::new("save-prompt", window, cx).enabled(dirty);
        let save = Button::new("save-prompt")
            .cursor_pointer()
            .small()
            .label("Save")
            .disabled(!dirty)
            .on_click(cx.listener(|page, _, _, cx| page.save(cx)))
            .smooth_hover(ButtonLook::Primary, &save_hover, cx);
        let delete = saved.clone().map(|prompt| {
            action(
                "delete-prompt",
                "Delete…",
                Some(Lucide::Trash),
                ButtonLook::Danger,
                window,
                cx,
                move |_, window, cx| confirm_delete(&prompt, uses, window, cx),
            )
        });
        let muted = cx.theme().muted_foreground;
        let fields = card(cx)
            .child(card_title(
                if saved.is_some() {
                    "Edit prompt"
                } else {
                    "New prompt"
                },
                "Write to the agent directly: who it is, what to check and how to report.",
                cx,
            ))
            .child(labelled("Name", cx).child(TextField::input(
                Input::new(&self.name).aria_label("Prompt name"),
                &self.name,
                cx,
            )))
            .child(
                labelled("Instructions", cx)
                    .child(TextField::textarea(
                        Textarea::new(&self.text).aria_label("Prompt instructions"),
                        &self.text,
                        cx,
                    ))
                    .child(div().text_xs().text_color(muted).child(
                        "Hive adds the task, the lane assignments, the handoff and the report \
                         format after this text.",
                    )),
            )
            .when(uses > 0, |card| {
                card.child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(muted)
                        .child(Icon::new(Lucide::Workflow).size_3p5())
                        .child(format!(
                            "{}. Saving updates them; runs already started keep their text.",
                            uses_label(uses)
                        )),
                )
            })
            .when_some(self.error.clone(), |card, error| {
                card.child(error_line(error, cx))
            })
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .child(div().children(delete))
                    .child(h_flex().gap_2().child(cancel).child(save)),
            );
        v_flex().gap_3().child(h_flex().child(back)).child(fields)
    }
}

fn prompt_menu(
    menu: PopupMenu,
    page: &Entity<PipelinesPage>,
    prompt: &Prompt,
    uses: usize,
) -> PopupMenu {
    let id = prompt.id;
    let (edit, duplicate, delete) = (page.clone(), page.clone(), prompt.clone());
    menu.item(
        PopupMenuItem::new("Edit prompt")
            .icon(Lucide::Pencil)
            .on_click(move |_, window, cx| edit.update(cx, |page, cx| page.edit(id, window, cx))),
    )
    .item(
        PopupMenuItem::new("Duplicate")
            .icon(Lucide::Copy)
            .on_click(move |_, window, cx| {
                duplicate.update(cx, |page, cx| page.duplicate(id, window, cx));
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("Delete prompt…")
            .icon(Lucide::Trash)
            .on_click(move |_, window, cx| confirm_delete(&delete, uses, window, cx)),
    )
}

/// A small button of this page with its eased hover.
fn action(
    id: &'static str,
    label: &'static str,
    icon: Option<Lucide>,
    look: ButtonLook,
    window: &mut Window,
    cx: &mut Context<PipelinesPage>,
    click: impl Fn(&mut PipelinesPage, &mut Window, &mut Context<PipelinesPage>) + 'static,
) -> Stateful<Div> {
    let hover = Hover::new(id, window, cx);
    let button = Button::new(id)
        .cursor_pointer()
        .small()
        .label(label)
        .on_click(cx.listener(move |page, _, window, cx| click(page, window, cx)));
    match icon {
        Some(icon) => button.icon(icon),
        None => button,
    }
    .smooth_hover(look, &hover, cx)
}

fn card(cx: &App) -> Div {
    v_flex()
        .gap_4()
        .p_4()
        .border_1()
        .border_color(cx.theme().border)
        .rounded_lg()
}

fn card_title(title: &'static str, description: &'static str, cx: &App) -> Div {
    v_flex()
        .gap_1()
        .min_w_0()
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(description),
        )
}

/// A field's label above its control.
fn labelled(label: &'static str, cx: &App) -> Div {
    v_flex().gap_1p5().child(
        div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(label),
    )
}

fn error_line(error: SharedString, cx: &App) -> Div {
    div().text_sm().text_color(cx.theme().danger).child(error)
}

fn empty_library(cx: &App) -> Div {
    let muted = cx.theme().muted_foreground;
    v_flex()
        .items_center()
        .gap_1()
        .py_8()
        .child(Icon::new(Lucide::MessageSquare).size_6().text_color(muted))
        .child(div().pt_2().text_sm().child("No prompts yet"))
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child("Create one to reuse the same instructions across pipeline stages."),
        )
}
