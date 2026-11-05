//! Selectable diagnostics with actions kept outside the scrolling text.
use super::*;
use gpui_kit::base::SelectableText;
use gpui_kit::component::{WindowExt as _, notification::Notification};

const ERROR_HEIGHT: Pixels = px(120.);
const ERROR_BACKGROUND_OPACITY: f32 = 0.04;
const ERROR_BORDER_OPACITY: f32 = 0.3;

impl GitPanel {
    pub(super) fn error(
        &self,
        error: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let [copy_hover, close_hover, agent_hover, verify_hover] = [
            "copy-git-error",
            "close-git-error",
            "error-agent",
            "error-verified",
        ]
        .map(|id| Hover::new(id, window, cx));
        let state = self.git.read(cx);
        let target = state.target.clone();
        let needs_verification = state.needs_verification();
        let inspected = state.result_inspected();
        let agent = AppState::get(cx)
            .store
            .read(cx)
            .state
            .settings
            .default_cli_agent();
        let danger = cx.theme().danger;
        let copy = error.clone();
        let prompt = error.clone();
        v_flex()
            .px_3()
            .pb_2()
            .gap_1()
            .flex_shrink_0()
            .child(
                v_flex()
                    .id("git-error-alert")
                    .role(Role::Alert)
                    .rounded_md()
                    .border_1()
                    .border_color(danger.opacity(ERROR_BORDER_OPACITY))
                    .bg(danger.opacity(ERROR_BACKGROUND_OPACITY))
                    .child(
                        h_flex()
                            .px_2()
                            .pt_1()
                            .gap_1()
                            .items_center()
                            .child(Icon::new(Lucide::CircleX).small().text_color(danger))
                            .child(div().flex_1())
                            .child(
                                Button::new("copy-git-error")
                                    .cursor_pointer()
                                    .small()
                                    .icon(Lucide::Copy)
                                    .accessibility_label("Copy error")
                                    .tooltip("Copy full error")
                                    .on_click(move |_, window, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.to_string(),
                                        ));
                                        window.push_notification(
                                            Notification::success("Error copied"),
                                            cx,
                                        );
                                    })
                                    .smooth_hover(ButtonLook::Ghost, &copy_hover, cx),
                            )
                            .child(
                                Button::new("close-git-error")
                                    .cursor_pointer()
                                    .small()
                                    .icon(Lucide::X)
                                    .accessibility_label("Dismiss error")
                                    .tooltip("Dismiss error")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.git.update(cx, |git, cx| git.clear_error(cx))
                                    }))
                                    .smooth_hover(ButtonLook::Ghost, &close_hover, cx),
                            ),
                    )
                    .child(
                        div()
                            .id("git-error")
                            .max_h(ERROR_HEIGHT)
                            .overflow_y_scroll()
                            .px_3()
                            .pb_2()
                            .text_sm()
                            .text_color(danger)
                            .cursor(CursorStyle::IBeam)
                            .child(SelectableText::new("git-error-text", error)),
                    ),
            )
            .child(
                h_flex()
                    .justify_end()
                    .flex_wrap()
                    .gap_2()
                    .when(needs_verification, |row| {
                        row.child(
                            Button::new("error-verified")
                                .cursor_pointer()
                                .xsmall()
                                .label(if inspected {
                                    "I reviewed this result"
                                } else {
                                    "Verify actual result"
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.git.update(cx, |git, cx| {
                                        if inspected {
                                            git.acknowledge_outcome(cx)
                                        } else {
                                            git.verify_result(cx)
                                        }
                                    })
                                }))
                                .smooth_hover(ButtonLook::Outline, &verify_hover, cx),
                        )
                    })
                    .child(
                        Button::new("error-agent")
                            .cursor_pointer()
                            .xsmall()
                            .icon(Lucide::MessageSquare)
                            .label("Send to agent")
                            .tooltip(format!(
                                "Prepare a draft in {} without pressing Enter",
                                agent.name()
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(target) = target.clone() {
                                    this.git.update(cx, |git, cx| {
                                        git.prepare_error_draft(target, &prompt, cx)
                                    });
                                }
                            }))
                            .smooth_hover(ButtonLook::Outline, &agent_hover, cx),
                    ),
            )
    }
}
