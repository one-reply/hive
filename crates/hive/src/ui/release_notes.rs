//! A nonblocking, persistent card using the existing notification stack.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::Button;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _, notification::Notification, text::TextView,
    v_flex,
};
use gpui_kit::*;
use hive_core::release_notes;

use crate::state::updates::Updates;
use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _};

const HIGHLIGHTS_MAX_HEIGHT: Pixels = px(200.);

pub(crate) fn show(window: &mut Window, cx: &mut App) {
    window.push_notification(
        Notification::new()
            .id::<Updates>()
            .title(format!("What's new in Hive {}", release_notes::VERSION))
            .placement(Anchor::BottomRight)
            .autohide(false)
            .content(|_, window, cx| {
                let hover = Hover::new("full-release-notes-hover", window, cx);
                v_flex()
                    .gap_3()
                    .pt_2()
                    .child(
                        div()
                            .id("release-highlights")
                            .max_h(HIGHLIGHTS_MAX_HEIGHT)
                            .overflow_y_scroll()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(TextView::markdown(
                                "release-highlights-text",
                                release_notes::highlights(),
                            )),
                    )
                    .child(
                        Button::new("full-release-notes")
                            .label("Full release notes")
                            .icon(Lucide::ExternalLink)
                            .small()
                            .cursor_pointer()
                            .on_click(|_, _, cx| Updates::open_current_release(cx))
                            .smooth_hover(ButtonLook::Outline, &hover, cx),
                    )
                    .into_any_element()
            }),
        cx,
    );
}
