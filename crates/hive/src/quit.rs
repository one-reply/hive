//! AppKit's cancellable pre-quit stage, before GPUI's irreversible shutdown callback.
use gpui_kit::{App, Task};
use objc2::{
    MainThreadMarker, ffi, msg_send,
    runtime::{AnyClass, AnyObject, Imp, Sel},
    sel,
};
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};

static REQUESTS: OnceLock<async_channel::Sender<()>> = OnceLock::new();
static PENDING: AtomicBool = AtomicBool::new(false);
static APPROVED: AtomicBool = AtomicBool::new(false);

pub(crate) fn init(cx: &mut App) -> Result<Task<()>, &'static str> {
    let _main_thread = MainThreadMarker::new().ok_or("Quit guard requires the main thread")?;
    let class = AnyClass::get(c"GPUIApplicationDelegate").ok_or("Missing GPUI app delegate")?;
    if class
        .instance_method(sel!(applicationShouldTerminate:))
        .is_some()
    {
        return Err("GPUI already handles pre-quit; review the native adapter");
    }
    let (sender, receiver) = async_channel::bounded(1);
    REQUESTS
        .set(sender)
        .map_err(|_| "Quit guard already installed")?;
    // GPUI 0.3.11 has only applicationWillTerminate:. Add the earlier AppKit hook;
    // never replace an upstream implementation. NSUInteger is 64-bit on supported arm64 macOS.
    unsafe {
        let implementation = std::mem::transmute::<
            extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize,
            Imp,
        >(should_terminate);
        if !ffi::class_addMethod(
            class as *const AnyClass as *mut AnyClass,
            sel!(applicationShouldTerminate:),
            implementation,
            c"Q@:@".as_ptr(),
        )
        .as_bool()
        {
            return Err(
                "GPUI already implements applicationShouldTerminate; review the quit adapter",
            );
        }
        let app_class = AnyClass::get(c"NSApplication").ok_or("Missing NSApplication")?;
        let app: *mut AnyObject = msg_send![app_class, sharedApplication];
        let delegate: *mut AnyObject = msg_send![app, delegate];
        // AppKit caches optional delegate selectors when the delegate is installed.
        let _: () = msg_send![app, setDelegate: delegate];
    }
    Ok(cx.spawn(async move |cx| {
        while receiver.recv().await.is_ok() {
            cx.update(|cx| {
                if let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied()) {
                    if window
                        .update(cx, |_, window, cx| {
                            crate::ui::editor_panel::close::quit(window, cx)
                        })
                        .is_err()
                    {
                        reply(false, cx);
                    }
                } else {
                    reply(true, cx);
                }
            });
        }
    }))
}

extern "C-unwind" fn should_terminate(_: *mut AnyObject, _: Sel, _: *mut AnyObject) -> usize {
    if APPROVED.swap(false, Ordering::SeqCst) {
        return 1; // NSTerminateNow
    }
    if !PENDING.swap(true, Ordering::SeqCst)
        && !REQUESTS
            .get()
            .is_some_and(|sender| sender.try_send(()).is_ok())
    {
        PENDING.store(false, Ordering::SeqCst);
        return 0; // NSTerminateCancel
    }
    // AppKit's NSTerminateLater stops this GPUI version's foreground executor.
    // Cancel first so dialogs/save tasks run, then retry once after explicit approval.
    0 // NSTerminateCancel
}

pub(crate) fn reply(approved: bool, cx: &mut App) {
    PENDING.store(false, Ordering::SeqCst);
    if approved {
        APPROVED.store(true, Ordering::SeqCst);
        // GPUI's platform.quit schedules termination after App/Window leases are released.
        cx.quit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_quit_waits_and_coalesces_repeated_requests() {
        let (sender, receiver) = async_channel::bounded(1);
        REQUESTS.set(sender).unwrap();
        for _ in 0..3 {
            assert_eq!(
                should_terminate(
                    std::ptr::null_mut(),
                    sel!(applicationShouldTerminate:),
                    std::ptr::null_mut()
                ),
                0
            );
        }
        assert_eq!(receiver.len(), 1);
        receiver.try_recv().unwrap();
        assert_eq!(
            should_terminate(
                std::ptr::null_mut(),
                sel!(applicationShouldTerminate:),
                std::ptr::null_mut()
            ),
            0
        );
        assert!(receiver.is_empty());
        APPROVED.store(true, Ordering::SeqCst);
        assert_eq!(
            should_terminate(
                std::ptr::null_mut(),
                sel!(applicationShouldTerminate:),
                std::ptr::null_mut()
            ),
            1
        );
        assert!(!APPROVED.load(Ordering::SeqCst));
        receiver.close();
        PENDING.store(false, Ordering::SeqCst);
        assert_eq!(
            should_terminate(
                std::ptr::null_mut(),
                sel!(applicationShouldTerminate:),
                std::ptr::null_mut()
            ),
            0
        );
    }
}
