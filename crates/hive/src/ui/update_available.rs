//! Gentle update reminders; Sparkle still owns download, installation and restart.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::Button;
use gpui_kit::component::{Sizable as _, WindowExt as _, notification::Notification};
use gpui_kit::*;

use crate::app::AppState;
use crate::state::updates::Updates;
use crate::ui::app_menu::CheckForUpdates;
use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _};

const NOTIFICATION_ID: &str = "update-available";

pub(crate) fn sync(window: &mut Window, cx: &mut App) {
    let updates = AppState::get(cx).updates.clone();
    let Some(version) = updates.read(cx).reminder().cloned() else {
        window.remove_notification1::<Updates>(NOTIFICATION_ID, cx);
        return;
    };
    window.push_notification(
        Notification::new()
            .id1::<Updates>(NOTIFICATION_ID)
            .title(format!("Hive {version} is available"))
            .message("Update when you're ready. You choose when to install and restart.")
            .placement(Anchor::BottomRight)
            .autohide(false)
            .content(|_, window, cx| {
                let hover = Hover::new("install-update-hover", window, cx);
                div()
                    .flex()
                    .justify_end()
                    .pt_3()
                    .child(
                        Button::new("install-update")
                            .label("Update")
                            .icon(Lucide::Download)
                            .small()
                            .cursor_pointer()
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(CheckForUpdates), cx)
                            })
                            .smooth_hover(ButtonLook::Primary, &hover, cx),
                    )
                    .into_any_element()
            }),
        cx,
    );
}
