//! macOS permission for Hive's system notifications. GPUI posts them but cannot tell that the
//! user blocked Hive, in which case macOS accepts every post and silently never shows it.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2_foundation::NSBundle;
use objc2_user_notifications::{
    UNAuthorizationStatus, UNNotificationSettings, UNUserNotificationCenter,
};

/// Notifications pane of System Settings, scrolled to one app by its bundle identifier.
const SETTINGS_URL: &str = "x-apple.systempreferences:com.apple.Notifications-Settings.extension";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Permission {
    Allowed,
    Blocked,
    /// macOS asks the first time Hive posts a notification.
    NotAsked,
    /// Not running from an app bundle (`cargo run`): there is nothing to ask.
    Unavailable,
}

impl From<UNAuthorizationStatus> for Permission {
    fn from(status: UNAuthorizationStatus) -> Self {
        match status {
            UNAuthorizationStatus::Denied => Self::Blocked,
            UNAuthorizationStatus::NotDetermined => Self::NotAsked,
            // Authorized, provisional and ephemeral all deliver; a future status should not
            // raise a warning it cannot explain.
            _ => Self::Allowed,
        }
    }
}

/// Asks the notification center for Hive's permission. The answer arrives on a notification
/// center thread, hence the channel.
pub(crate) async fn permission() -> Permission {
    // The notification center aborts the process outside an app bundle.
    if NSBundle::mainBundle().bundleIdentifier().is_none() {
        return Permission::Unavailable;
    }
    let (sender, receiver) = async_channel::bounded(1);
    let completion = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        // SAFETY: the notification center passes valid settings for the duration of the call.
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        let _ = sender.try_send(Permission::from(status));
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .getNotificationSettingsWithCompletionHandler(&completion);
    drop(completion);
    receiver.recv().await.unwrap_or(Permission::Unavailable)
}

/// The System Settings page where the user allows Hive's notifications.
pub(crate) fn settings_url() -> String {
    match NSBundle::mainBundle().bundleIdentifier() {
        Some(identifier) => format!("{SETTINGS_URL}?id={identifier}"),
        None => SETTINGS_URL.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Permission, UNAuthorizationStatus};

    #[test]
    fn only_a_denial_blocks_and_unknown_statuses_do_not_warn() {
        assert_eq!(
            Permission::from(UNAuthorizationStatus::Denied),
            Permission::Blocked
        );
        assert_eq!(
            Permission::from(UNAuthorizationStatus::NotDetermined),
            Permission::NotAsked
        );
        for status in [
            UNAuthorizationStatus::Authorized,
            UNAuthorizationStatus::Provisional,
            UNAuthorizationStatus::Ephemeral,
            UNAuthorizationStatus(99),
        ] {
            assert_eq!(Permission::from(status), Permission::Allowed);
        }
    }
}
