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
    _load: Task<()>,
}

impl Updates {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let load = cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async { updater::load_framework() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.status = match loaded.and_then(Updater::new) {
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
            _load: load,
        }
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

    pub(crate) fn check(&self) {
        if let Status::Ready(updater) = &self.status {
            updater.check();
        }
    }

    pub(crate) fn open_releases(cx: &mut App) {
        cx.open_url(updater::RELEASES_URL);
    }
}
