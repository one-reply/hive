//! Native Sparkle UI, loaded only from the installed app's embedded framework.

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadMarker, msg_send};
use objc2_foundation::{NSBundle, NSError, NSString};

pub(crate) const RELEASES_URL: &str = "https://github.com/vitaliits-stack/hive/releases";

/// Loading a framework touches disk, so the state entity runs this in the background.
pub(crate) fn load_framework() -> Result<Retained<NSBundle>, String> {
    let directory = NSBundle::mainBundle()
        .privateFrameworksPath()
        .ok_or("Install the packaged Hive.app to enable updates.")?;
    let path = NSString::from_str(&format!("{directory}/Sparkle.framework"));
    let framework = NSBundle::bundleWithPath(&path)
        .ok_or("Install the packaged Hive.app to enable updates.")?;
    // The packager embeds the pinned Sparkle distribution; never load a downloaded plugin.
    unsafe { framework.loadAndReturnError() }.map_err(|error| {
        format!(
            "Could not load the updater: {}",
            error.localizedDescription()
        )
    })?;
    Ok(framework)
}

pub(crate) struct Updater {
    controller: Retained<AnyObject>,
    _framework: Retained<NSBundle>,
    _main_thread: MainThreadMarker,
}

impl Updater {
    pub(crate) fn new(framework: Retained<NSBundle>) -> Result<Self, String> {
        let main_thread = MainThreadMarker::new().ok_or("The updater requires the main thread.")?;
        let class = AnyClass::get(c"SPUStandardUpdaterController")
            .ok_or("The embedded updater is missing SPUStandardUpdaterController.")?;
        // Signatures and ownership follow Sparkle's SPUStandardUpdaterController/SPUUpdater headers.
        // The marker confines all Cocoa UI calls and the retained controller to the main thread.
        unsafe {
            let allocated: Allocated<AnyObject> = msg_send![class, alloc];
            let controller: Retained<AnyObject> = msg_send![allocated,
                initWithStartingUpdater: false,
                updaterDelegate: Option::<&AnyObject>::None,
                userDriverDelegate: Option::<&AnyObject>::None
            ];
            let updater: Retained<AnyObject> = msg_send![&*controller, updater];
            let mut error: Option<Retained<NSError>> = None;
            let started: bool = msg_send![&*updater, startUpdater: &mut error];
            if !started {
                return Err(error.map_or_else(
                    || "The updater could not start.".into(),
                    |error| {
                        format!(
                            "Could not start the updater: {}",
                            error.localizedDescription()
                        )
                    },
                ));
            }
            Ok(Self {
                controller,
                _framework: framework,
                _main_thread: main_thread,
            })
        }
    }

    pub(crate) fn check(&self) {
        // Sparkle owns the progress, result and error windows, and prevents concurrent checks.
        unsafe {
            let updater: Retained<AnyObject> = msg_send![&*self.controller, updater];
            let can_check: bool = msg_send![&*updater, canCheckForUpdates];
            if can_check {
                let _: () =
                    msg_send![&*self.controller, checkForUpdates: Option::<&AnyObject>::None];
            }
        }
    }
}
