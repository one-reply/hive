//! Whether macOS lets Hive's agent notifications through. Re-read whenever the settings window
//! is activated: the user changes it in System Settings, outside Hive.

use gpui_kit::*;

use crate::notify::{self, Permission};

pub(crate) struct Notifications {
    /// `None` until the first answer.
    permission: Option<Permission>,
    _check: Task<()>,
}

impl Notifications {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            permission: None,
            _check: Task::ready(()),
        };
        this.refresh(cx);
        this
    }

    pub(crate) fn is_blocked(&self) -> bool {
        self.permission == Some(Permission::Blocked)
    }

    /// Re-reads the permission; a newer refresh replaces (and cancels) an older one.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self._check = cx.spawn(async move |this, cx| {
            let permission = notify::permission().await;
            let _ = this.update(cx, |this, cx| {
                if this.permission != Some(permission) {
                    tracing::info!(?permission, "system notification permission");
                    this.permission = Some(permission);
                    cx.notify();
                }
            });
        });
    }

    pub(crate) fn open_system_settings(cx: &mut App) {
        cx.open_url(&notify::settings_url());
    }
}
