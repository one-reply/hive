//! Version and update controls; Sparkle presents download and installation progress.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, v_flex};
use gpui_kit::*;

use super::{setting_row, setting_title};
use crate::app::AppState;
use crate::state::updates::Updates;
use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _};

pub(super) struct UpdatesPage {
    updates: Entity<Updates>,
    _subscription: Subscription,
}

impl UpdatesPage {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        let updates = AppState::get(cx).updates.clone();
        Self {
            _subscription: cx.observe(&updates, |_, _, cx| cx.notify()),
            updates,
        }
    }
}

impl Render for UpdatesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let updates = self.updates.read(cx);
        let loading = updates.is_loading();
        let available = updates.is_available();
        let error = updates.error().map(str::to_owned);
        let check_hover = Hover::new("check-updates-hover", window, cx).enabled(available);
        let releases_hover = Hover::new("release-notes-hover", window, cx);
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_3()
            .child(setting_row(
                setting_title("Hive", "macOS · Apple Silicon", cx),
                div().text_sm().child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
                cx,
            ))
            .child(setting_row(
                setting_title("Software updates", "Check for a newer version of Hive.", cx),
                Button::new("check-updates")
                    .label("Check for Updates")
                    .icon(Lucide::RefreshCw)
                    .loading(loading)
                    .disabled(!available)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.updates.read(cx).check()))
                    .smooth_hover(ButtonLook::Primary, &check_hover, cx),
                cx,
            ))
            .child(
                div().text_sm().text_color(muted).child(error.unwrap_or_else(|| {
                    "Hive checks daily. You choose when to install and restart. Finish active agent sessions before restarting.".into()
                })),
            )
            .child(setting_row(
                setting_title("Release notes", "What's new and previous releases.", cx),
                Button::new("open-releases")
                    .label("View Releases")
                    .icon(Lucide::ExternalLink)
                    .cursor_pointer()
                    .on_click(|_, _, cx| Updates::open_releases(cx))
                    .smooth_hover(ButtonLook::Outline, &releases_hover, cx),
                cx,
            ))
    }
}
