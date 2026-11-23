//! Rendering of the Pipelines page: the prompt library, and the editor of one prompt.
//! A prompt carries the icon and color of the stage role its name speaks of, the same ones
//! the recipe canvas uses, so the library reads at a glance.
use super::*;
use crate::ui::stage_role::{self, RoleKind, TINT_OPACITY};
use crate::ui::text_field::TextField;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, h_flex,
    input::{Input, Textarea},
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;

/// Lines of a prompt's text shown in its library row.
const PREVIEW_LINES: usize = 2;
/// How far a hovered row's border moves toward the color of its role.
const HOVER_BORDER_OPACITY: f32 = 0.6;

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
        let theme = cx.theme();
        let (muted, surface) = (theme.muted_foreground, theme.muted);
        let count = div()
            .px_1p5()
            .rounded_full()
            .bg(surface)
            .text_xs()
            .text_color(muted)
            .child(prompts.len().to_string());
        let header = h_flex()
            .justify_between()
            .items_start()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("Prompts"))
                            .child(count),
                    )
                    .child(div().text_sm().text_color(muted).child(
                        "A prompt tells a stage's agent who it is and how to do its work. Hive \
                         adds the task, the lane assignments, the handoff and the report format \
                         itself.",
                    )),
            )
            .child(div().flex_none().child(new_prompt));
        let body = if prompts.is_empty() {
            empty_library(cx)
        } else {
            v_flex().gap_2().children(
                prompts
                    .iter()
                    .map(|(prompt, uses)| self.render_row(prompt, *uses, window, cx)),
            )
        };
        v_flex()
            .gap_4()
            .child(self.dictation.clone())
            .child(header)
            .when_some(self.error.clone(), |page, error| {
                page.child(error_line(error, cx))
            })
            .child(body)
    }

    /// One library prompt: its role, name, the start of its text and how many stages follow it.
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
        let role = RoleKind::of_prompt(&prompt.name);
        let color = role_color(role, cx);
        let theme = cx.theme();
        h_flex()
            .id(row)
            .role(Role::Button)
            .aria_label(prompt.name.clone())
            .tab_index(0)
            .items_start()
            .gap_3()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(hover.mix(theme.border, color.opacity(HOVER_BORDER_OPACITY)))
            .bg(theme.sidebar)
            .cursor_pointer()
            .on_hover(hover.listener())
            .focus_visible(move |style| style.border_color(color))
            .child(role_tile(role, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(prompt.name.clone()),
                            )
                            .child(usage_badge(uses, color, cx))
                            .child(more),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .whitespace_normal()
                            .line_clamp(PREVIEW_LINES)
                            .text_ellipsis()
                            .child(prompt.text.clone()),
                    ),
            )
            // Enter and Space on the focused row arrive as this click too.
            .on_click(cx.listener(move |page, _, window, cx| page.edit(id, window, cx)))
    }

    fn render_editor(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let saved = match self.screen {
            Screen::Edit(id) => self.prompt(id, cx),
            Screen::List | Screen::New => None,
        };
        let library = &AppState::get(cx).store.read(cx).state.pipelines;
        let uses = saved.as_ref().map_or(0, |p| library.prompt_uses(p.id));
        let dirty = self.is_dirty(cx);
        // The role follows the name as it is typed, so the icon shows what the row will get.
        let role = RoleKind::of_prompt(&self.name.read(cx).value());
        let color = role_color(role, cx);
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
        let theme = cx.theme();
        let (muted, foreground) = (theme.muted_foreground, theme.foreground);
        let title = h_flex().gap_3().child(role_tile(role, cx)).child(
            v_flex()
                .gap_0p5()
                .min_w_0()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(if saved.is_some() {
                            "Edit prompt"
                        } else {
                            "New prompt"
                        }),
                )
                .child(div().text_sm().text_color(muted).child(
                    "Write to the agent directly: who it is, what to check and how to report.",
                )),
        );
        let fields = v_flex()
            .gap_4()
            .p_4()
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
            .bg(theme.sidebar)
            .child(title)
            .child(labelled("Name").child(TextField::input(
                Input::new(&self.name).aria_label("Prompt name"),
                &self.name,
                cx,
            )))
            .child(
                labelled("Instructions")
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
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(color.opacity(TINT_OPACITY))
                        .text_xs()
                        .text_color(foreground)
                        .child(Icon::new(Lucide::Workflow).size_3p5().text_color(color))
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

/// The color a prompt is drawn in: its role's, or a neutral one when its name names no role.
fn role_color(role: Option<RoleKind>, cx: &App) -> Hsla {
    role.map_or(cx.theme().muted_foreground, |role| role.color(cx))
}

fn role_tile(role: Option<RoleKind>, cx: &App) -> Div {
    match role {
        Some(role) => role.tile(cx),
        None => stage_role::tile(Lucide::MessageSquare, cx.theme().muted_foreground),
    }
}

/// How many stages follow a prompt: tinted with its color when used, plain text when not.
fn usage_badge(uses: usize, color: Hsla, cx: &App) -> Div {
    let badge = div()
        .flex_none()
        .px_2()
        .py_0p5()
        .rounded_full()
        .text_xs()
        .child(uses_label(uses));
    if uses == 0 {
        badge.text_color(cx.theme().muted_foreground)
    } else {
        badge.bg(color.opacity(TINT_OPACITY)).text_color(color)
    }
}

/// A field's label above its control.
fn labelled(label: &'static str) -> Div {
    v_flex()
        .gap_1p5()
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label))
}

fn error_line(error: SharedString, cx: &App) -> Div {
    div().text_sm().text_color(cx.theme().danger).child(error)
}

fn empty_library(cx: &App) -> Div {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    v_flex()
        .items_center()
        .gap_1()
        .py_8()
        .border_1()
        .border_color(theme.border)
        .rounded_lg()
        .child(stage_role::tile(Lucide::MessageSquare, muted))
        .child(div().pt_2().text_sm().child("No prompts yet"))
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child("Create one to reuse the same instructions across pipeline stages."),
        )
}
