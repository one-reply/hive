//! Owns the native updater for the lifetime of the application.

use gpui_kit::*;

use crate::updater::{self, Updater};

enum Status {
    Loading,
    Ready(Updater),
    Unavailable(String),
}

pub(crate) struct Updates {
    status: Status,
    reminder: Option<SharedString>,
    _load: Task<()>,
    _reminders: Task<()>,
}

impl Updates {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let (sender, receiver) = async_channel::bounded(1);
        let reminders = cx.spawn(async move |this, cx| {
            while let Ok(version) = receiver.recv().await {
                if this
                    .update(cx, |this, cx| {
                        if this.set_reminder(version) {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let load = cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async { updater::load_framework() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.status = match loaded.and_then(|framework| Updater::new(framework, sender)) {
                    Ok(updater) => Status::Ready(updater),
                    Err(error) => {
                        tracing::warn!(%error, "updater unavailable");
                        Status::Unavailable(error)
                    }
                };
                cx.notify();
            });
        });
        Self {
            status: Status::Loading,
            reminder: None,
            _load: load,
            _reminders: reminders,
        }
    }

    fn set_reminder(&mut self, version: Option<String>) -> bool {
        let version = version.map(SharedString::from);
        if self.reminder == version {
            return false;
        }
        self.reminder = version;
        true
    }

    pub(crate) fn reminder(&self) -> Option<&SharedString> {
        self.reminder.as_ref()
    }

    pub(crate) fn is_loading(&self) -> bool {
        matches!(self.status, Status::Loading)
    }

    pub(crate) fn is_available(&self) -> bool {
        matches!(self.status, Status::Ready(_))
    }

    pub(crate) fn error(&self) -> Option<&str> {
        match &self.status {
            Status::Unavailable(error) => Some(error),
            _ => None,
        }
    }

    pub(crate) fn check(&self) -> Result<(), &str> {
        match &self.status {
            Status::Ready(updater) => {
                updater.check();
                Ok(())
            }
            Status::Loading => Err("The updater is starting. Please try again in a moment."),
            Status::Unavailable(error) => Err(error),
        }
    }

    pub(crate) fn open_releases(cx: &mut App) {
        cx.open_url(updater::RELEASES_URL);
    }

    pub(crate) fn open_current_release(cx: &mut App) {
        cx.open_url(hive_core::release_notes::URL);
    }
}

#[cfg(test)]
mod tests {
    use super::{Status, Updates};
    use gpui_kit::Task;

    #[test]
    fn manual_checks_report_loading_and_unavailable_updater() {
        let mut updates = Updates {
            status: Status::Loading,
            reminder: None,
            _load: Task::ready(()),
            _reminders: Task::ready(()),
        };
        assert_eq!(
            updates.check(),
            Err("The updater is starting. Please try again in a moment.")
        );
        updates.status =
            Status::Unavailable("Install the packaged Hive.app to enable updates.".into());
        assert_eq!(
            updates.check(),
            Err("Install the packaged Hive.app to enable updates.")
        );
    }

    #[test]
    fn reminders_follow_verified_availability_and_clear_on_attention_or_failure() {
        let mut updates = Updates {
            status: Status::Loading,
            reminder: None,
            _load: Task::ready(()),
            _reminders: Task::ready(()),
        };
        assert!(updates.reminder().is_none());
        for (version, changed) in [
            (Some("0.1.5"), true),
            (Some("0.1.5"), false),
            (Some("0.1.6"), true),
            (None, true),
            (None, false),
        ] {
            assert_eq!(updates.set_reminder(version.map(str::to_owned)), changed);
            assert_eq!(updates.reminder().map(|v| v.as_ref()), version);
        }
    }
}
