//! Commit composer and its staged-change controls.
use super::*;

mod actions;
mod generating;
mod menu;
mod resize;

pub(super) use menu::ActionsMenu;
pub(super) use resize::{MessageResize, MessageSize};

/// Keeps the AI button clear of the message's scrollbar, which `TextField` draws in a 16px
/// track at the field's right edge (gpui-kit's default track width).
const AI_BUTTON_RIGHT: Pixels = px(14.);
/// Room the message text keeps free on the right for the AI button.
const MESSAGE_TRAILING_ROOM: Pixels = px(42.);

impl GitPanel {
    fn generate_message(&self, cx: &mut Context<Self>) {
        let git = self.git.read(cx);
        if git.is_busy()
            || git
                .status
                .as_ref()
                .is_none_or(|s| s.has_conflicts() || staged_count(s) == 0)
        {
            return;
        }
        let recipe = AppState::get(cx)
            .store
            .read(cx)
            .state
            .settings
            .commit_message
            .clone();
        if !recipe.is_configured() {
            crate::ui::settings::open_git(cx);
            return;
        }
        self.git
            .update(cx, |git, cx| git.generate_message(recipe, cx));
    }

    pub(super) fn footer(
        &self,
        status: Option<&Status>,
        busy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let staged = status.map(staged_count).unwrap_or(0);
        let generating = self.git.read(cx).is_generating();
        let conflicts = status.is_some_and(Status::has_conflicts);
        let generate_hover = Hover::new("generate-commit-message", window, cx)
            .enabled(!busy && !conflicts && staged > 0);
        v_flex()
            .relative()
            .p_3()
            .gap_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(self.message_resize_handle(cx))
            .child(
                div()
                    .relative()
                    .child(TextField::textarea(
                        Textarea::new(&self.message)
                            .pr(MESSAGE_TRAILING_ROOM)
                            .when_some(self.message_size.height(), |area, height| area.h(height))
                            .disabled(generating),
                        &self.message,
                        cx,
                    ))
                    .child(self.message_size.measure())
                    .when(generating, |field| field.child(generating::sweep(cx)))
                    .child(
                        div().absolute().top_1().right(AI_BUTTON_RIGHT).child(
                            Button::new("generate-commit-message")
                                .cursor_pointer()
                                .small()
                                .icon(Lucide::Sparkles)
                                .loading(generating)
                                .accessibility_label("Generate commit message with AI")
                                .disabled(busy || conflicts || staged == 0)
                                .tooltip(if conflicts {
                                    "Resolve conflicts first"
                                } else if staged == 0 {
                                    "Stage changes to enable AI generation"
                                } else {
                                    "Generate a new commit message · configure in Git settings"
                                })
                                .on_click(cx.listener(|this, _, _, cx| this.generate_message(cx)))
                                .smooth_hover(ButtonLook::Ghost, &generate_hover, cx),
                        ),
                    ),
            )
            .child(self.commit_actions(status, busy, window, cx))
    }
}

impl GitPanel {
    /// Drag the footer's top edge to make the message taller; double-click restores it.
    fn message_resize_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("commit-message-resize")
            .absolute()
            .top(-resize::HANDLE_HEIGHT / 2.)
            .left_0()
            .right_0()
            .h(resize::HANDLE_HEIGHT)
            .occlude()
            .cursor_row_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    this.message_size.begin(event.position.y)
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.message_size.end()),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.message_size.end()),
            )
            .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                if event.click_count() == 2 && this.message_size.reset() {
                    cx.notify();
                }
            }))
            .on_drag(MessageResize, |_, _, _, cx| cx.new(|_| MessageResize))
    }
}
