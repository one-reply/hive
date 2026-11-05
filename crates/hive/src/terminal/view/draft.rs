//! A recoverable copy stays next to the recipient even after a paste into its TUI.
use super::*;
use crate::state::terminal_session::DraftStatus;
use gpui_kit::assets::IconName;
use gpui_kit::base::SelectableText;
use gpui_kit::component::{Disableable as _, h_flex};

const DRAFT_HEIGHT: Pixels = px(80.);

impl TerminalView {
    pub(super) fn render_agent_draft(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let draft = self.session.read(cx).agent_draft.clone()?;
        let [insert_hover, copy_hover, dismiss_hover] = [
            "insert-agent-draft",
            "copy-agent-draft",
            "dismiss-agent-draft",
        ]
        .map(|id| Hover::new(id, window, cx));
        let checking = matches!(draft.status, DraftStatus::Checking);
        let status: SharedString = match draft.status {
            DraftStatus::Checking => format!("Preparing draft for {}…", draft.agent.name()).into(),
            DraftStatus::Inserted => format!(
                "Draft inserted in {} · Press Enter in the CLI to send",
                draft.agent.name()
            )
            .into(),
            DraftStatus::Waiting(reason) => reason,
        };
        let copy = draft.text.clone();
        Some(
            v_flex()
                .flex_shrink_0()
                .p_2()
                .gap_1()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(status),
                )
                .child(
                    div()
                        .id("agent-draft-preview")
                        .max_h(DRAFT_HEIGHT)
                        .overflow_y_scroll()
                        .text_xs()
                        .cursor(CursorStyle::IBeam)
                        .child(SelectableText::new("agent-draft-text", draft.text)),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            Button::new("insert-agent-draft")
                                .cursor_pointer()
                                .small()
                                .label("Insert draft")
                                .loading(checking)
                                .disabled(self.session.read(cx).exited)
                                .tooltip(
                                    "Paste at the cursor without clearing text or pressing Enter",
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.focus_handle.focus(window, cx);
                                    this.session
                                        .update(cx, |session, cx| session.insert_agent_draft(cx));
                                }))
                                .smooth_hover(ButtonLook::Outline, &insert_hover, cx),
                        )
                        .child(
                            Button::new("copy-agent-draft")
                                .cursor_pointer()
                                .small()
                                .icon(IconName::Copy)
                                .accessibility_label("Copy agent draft")
                                .tooltip("Copy agent draft")
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copy.to_string(),
                                    ))
                                })
                                .smooth_hover(ButtonLook::Ghost, &copy_hover, cx),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("dismiss-agent-draft")
                                .cursor_pointer()
                                .small()
                                .icon(IconName::X)
                                .accessibility_label("Dismiss agent draft backup")
                                .tooltip("Dismiss backup")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.session
                                        .update(cx, |session, cx| session.dismiss_agent_draft(cx))
                                }))
                                .smooth_hover(ButtonLook::Ghost, &dismiss_hover, cx),
                        ),
                )
                .into_any_element(),
        )
    }
}
