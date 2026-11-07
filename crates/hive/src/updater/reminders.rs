//! Sparkle's gentle-reminder delegate; Cocoa callbacks never borrow GPUI entities.

use async_channel::Sender;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass as _, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{NSError, NSObject, NSObjectProtocol, NSString};

const NO_UPDATE_ERROR: isize = 1001;
const INSTALLATION_CANCELED_ERROR: isize = 4007;

define_class!(
    // NSObject imposes no subclassing requirements. Sparkle weakly references this delegate.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Sender<Option<String>>]
    #[name = "HiveUpdateReminders"]
    pub(super) struct Reminders;

    unsafe impl NSObjectProtocol for Reminders {}

    impl Reminders {
        // Selectors and argument types follow the embedded Sparkle 2.10.1 headers.
        #[unsafe(method(supportsGentleScheduledUpdateReminders))]
        fn supports_gentle_reminders(&self) -> bool {
            true
        }

        #[unsafe(method(standardUserDriverShouldHandleShowingScheduledUpdate:andInImmediateFocus:))]
        fn should_show_native_alert(&self, _update: &AnyObject, _immediate: bool) -> bool {
            false
        }

        #[unsafe(method(standardUserDriverWillHandleShowingUpdate:forUpdate:state:))]
        fn will_show_update(&self, native: bool, update: &AnyObject, _state: &AnyObject) {
            let version = if native {
                None
            } else {
                // Sparkle has already verified the feed, compared versions and checked OS support.
                let version: Retained<NSString> = unsafe { msg_send![update, displayVersionString] };
                Some(version.to_string())
            };
            self.publish(version);
        }

        #[unsafe(method(standardUserDriverDidReceiveUserAttentionForUpdate:))]
        fn received_attention(&self, _update: &AnyObject) {
            self.publish(None);
        }

        #[unsafe(method(standardUserDriverWillFinishUpdateSession))]
        fn finished_session(&self) {
            self.publish(None);
        }

        #[unsafe(method(updater:didAbortWithError:))]
        fn aborted(&self, _updater: &AnyObject, error: &NSError) {
            if !matches!(error.code(), NO_UPDATE_ERROR | INSTALLATION_CANCELED_ERROR) {
                tracing::warn!(code = error.code(), error = %error.localizedDescription(), "update check failed");
            }
            self.publish(None);
        }
    }
);

impl Reminders {
    pub(super) fn new(
        sender: Sender<Option<String>>,
        main_thread: MainThreadMarker,
    ) -> Retained<Self> {
        let this = Self::alloc(main_thread).set_ivars(sender);
        // NSObject's initializer returns the retained instance with our Rust ivars intact.
        unsafe { msg_send![super(this), init] }
    }

    fn publish(&self, version: Option<String>) {
        // Only the latest reminder matters; a closed receiver means the application is exiting.
        let _ = self.ivars().force_send(version);
    }
}
