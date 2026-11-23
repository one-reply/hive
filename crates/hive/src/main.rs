mod app;
mod cli;
mod mcp;
mod notify;
mod quit;
mod state;
mod terminal;
mod ui;
mod updater;

use gpui_kit::base::Root;
use gpui_kit::component::TitleBar;
use gpui_kit::*;

use app::AppState;
use ui::workspace::Workspace;

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let dir = hive_core::state::hive_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("hive.log"));
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("hive=info,hive_core=info"));
    match file {
        Ok(file) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .init(),
        Err(_) => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }
}

/// A panic inside a GPUI task aborts the process, and stderr leads nowhere when Hive is started
/// from Finder: without this the log would not say why Hive disappeared.
fn log_panics() {
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(%panic, %backtrace, "hive panicked");
        report(panic);
    }));
}

/// Launched from Finder, Hive inherits launchd's minimal `PATH`; git hooks and CLIs need the
/// terminal's. The lookup runs a login shell, so it stays off the UI thread, and commands that
/// start before it answers keep the process environment. A shell that fails is asked again;
/// one that keeps failing is reported, since hooks and CLIs it puts on `PATH` go missing.
fn load_login_environment(cx: &mut App) {
    use hive_core::host::environment;
    cx.spawn(async move |cx| {
        let load = || environment::load().map(|loaded| loaded.len());
        match keep_loading(load, &environment::RETRY_AFTER, cx).await {
            Ok(variables) => tracing::info!(variables, "login shell environment loaded"),
            Err(error) => {
                tracing::warn!(%error, "keeping the process environment for local commands");
                cx.update(|cx| report_login_environment(&error, cx));
                return;
            }
        }
        // A scan made before the login PATH arrived misses CLIs installed through it (nvm,
        // ~/.opencode/bin). Before `AppState` exists, its own first scan comes later anyway.
        cx.update(|cx| {
            if let Some(app) = cx.try_global::<AppState>() {
                app.agents
                    .clone()
                    .update(cx, |agents, cx| agents.refresh_installed(cx));
            }
        });
    })
    .detach();
}

/// Calls `load` off the UI thread until it succeeds, waiting each of `delays` before the next
/// try. Returns the last failure, or the first one no try can change.
async fn keep_loading<T: Send + 'static>(
    load: impl Fn() -> Result<T, hive_core::host::environment::CaptureError> + Clone + Send + 'static,
    delays: &[std::time::Duration],
    cx: &AsyncApp,
) -> Result<T, hive_core::host::environment::CaptureError> {
    let mut delays = delays.iter();
    loop {
        let attempt = load.clone();
        let error = match cx
            .background_executor()
            .spawn(async move { attempt() })
            .await
        {
            Ok(loaded) => return Ok(loaded),
            Err(error) => error,
        };
        let Some(delay) = delays.next().filter(|_| error.is_retryable()) else {
            return Err(error);
        };
        tracing::warn!(%error, retry_in = ?delay, "login shell environment not loaded yet");
        cx.background_executor().timer(*delay).await;
    }
}

/// Tells the user in the main window, the one place a failure at launch can be seen.
fn report_login_environment(error: &hive_core::host::environment::CaptureError, cx: &mut App) {
    let Some(window) = workspace_window(cx) else {
        return;
    };
    let message = format!(
        "Hive could not read your login shell's environment, so Git hooks and CLIs installed \
         through it may not be found. {error}"
    );
    let _ = window.update(cx, |_, window, cx| {
        use gpui_kit::component::{WindowExt as _, notification::Notification};
        window.push_notification(Notification::warning(message), cx);
    });
}

fn open_main_window(cx: &mut App) {
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Hive".into()),
            ..TitleBar::title_bar_options()
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1200.), px(760.)),
            cx,
        ))),
        ..TitleBar::window_options()
    };
    if let Err(error) = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| Workspace::new(window, cx))
    }) {
        tracing::error!(%error, "failed to open the main window");
    }
}

/// The open main window, found among the app's windows (Settings is another one) by its root view.
fn workspace_window(cx: &App) -> Option<WindowHandle<Root>> {
    cx.windows()
        .into_iter()
        .filter_map(|window| window.downcast::<Root>())
        .find(|root| {
            root.read(cx)
                .is_ok_and(|root| root.view().clone().downcast::<Workspace>().is_ok())
        })
}

fn main() -> std::process::ExitCode {
    // Before anything names the data folder: a debug build told to stay out of `~/.hive` with
    // a path it cannot use must not start there, or anywhere else, instead.
    if let Err(error) = hive_core::state::dev_data_dir() {
        eprintln!("hive: {error}. Nothing was started and no data folder was touched.");
        return std::process::ExitCode::from(2);
    }
    let args: Vec<_> = std::env::args_os().collect();
    if let Some(code) = cli::run(&args, cli::Origin::detect(&args)) {
        return code;
    }
    init_logging();
    log_panics();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "hive starting");

    let application = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);
    // Closing the last window keeps the process alive (macOS convention), so a Dock click
    // must bring a window back. Before `AppState` loads, the launch path opens it instead.
    application.on_reopen(|cx| {
        if !cx.has_global::<AppState>() {
            return;
        }
        match workspace_window(cx) {
            Some(window) => {
                if let Err(error) = window.update(cx, |_, window, _| window.activate_window()) {
                    tracing::warn!(%error, "failed to activate the main window");
                }
            }
            None => open_main_window(cx),
        }
        cx.activate(true);
    });
    application.run(|cx| {
        gpui_kit::init(cx);
        load_login_environment(cx);
        cx.spawn(async move |cx| {
            let (loaded, access, recovery, endpoint) = cx
                .background_executor()
                .spawn(async {
                    // Taken before reading, so a second Hive never saves over the first.
                    let access = hive_core::state::Access::acquire();
                    let loaded = hive_core::state::State::load();
                    // After the read, so the copy a recovery reports is among those kept.
                    if loaded.is_ok() {
                        hive_core::state::tidy(&access);
                    }
                    // Recovery must be available before any restored buffer becomes editable.
                    let recovery = state::store::load_drafts(&access);
                    // Restored panes start with the window; they need the sockets already.
                    let endpoint = state::agents::Endpoint::bind();
                    (loaded, access, recovery, endpoint)
                })
                .await;
            cx.update(|cx| {
                AppState::init(loaded, access, recovery, endpoint, cx);
                ui::app_menu::init(cx);
                ui::theme::init(cx);
                ui::syntax::init();
                // After every layer has bound its keys: it rebuilds the keymap around them.
                ui::keymap::init(cx);
                ui::settings::init(cx);
                open_main_window(cx);
                cx.activate(true);
            });
        })
        .detach();
    });
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::keep_loading;
    use gpui_kit as gpui;
    use gpui_kit::TestAppContext;
    use hive_core::host::environment::CaptureError;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn slow_shell() -> CaptureError {
        CaptureError::Unparsed {
            shell: "/bin/zsh".into(),
            code: Some(1),
            stderr: "compinit: insecure directories".into(),
        }
    }

    #[gpui::test]
    fn a_login_shell_that_failed_is_asked_again_until_it_answers(cx: &mut TestAppContext) {
        let tries = Arc::new(AtomicUsize::new(0));
        let counted = tries.clone();
        let load = move || match counted.fetch_add(1, Ordering::SeqCst) {
            0 | 1 => Err(slow_shell()),
            _ => Ok(42),
        };
        let delays = [Duration::from_secs(10), Duration::from_secs(60)];
        let task = cx.spawn(async move |cx| keep_loading(load, &delays, &cx).await);
        cx.run_until_parked();
        assert_eq!(tries.load(Ordering::SeqCst), 1);
        cx.executor().advance_clock(Duration::from_secs(10));
        cx.run_until_parked();
        assert_eq!(tries.load(Ordering::SeqCst), 2);
        cx.executor().advance_clock(Duration::from_secs(60));
        cx.run_until_parked();
        assert_eq!(tries.load(Ordering::SeqCst), 3);
        assert_eq!(cx.foreground_executor().block_test(task).unwrap(), 42);
    }

    #[gpui::test]
    fn a_failure_no_retry_can_change_or_the_last_one_is_returned(cx: &mut TestAppContext) {
        let tries = Arc::new(AtomicUsize::new(0));
        let counted = tries.clone();
        let no_shell = move || -> Result<(), CaptureError> {
            counted.fetch_add(1, Ordering::SeqCst);
            Err(CaptureError::NoShell)
        };
        let task =
            cx.spawn(async move |cx| keep_loading(no_shell, &[Duration::from_secs(10)], &cx).await);
        cx.run_until_parked();
        assert!(matches!(
            cx.foreground_executor().block_test(task),
            Err(CaptureError::NoShell)
        ));
        assert_eq!(tries.load(Ordering::SeqCst), 1);

        let always = || -> Result<(), CaptureError> { Err(slow_shell()) };
        let task =
            cx.spawn(async move |cx| keep_loading(always, &[Duration::from_secs(10)], &cx).await);
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(10));
        cx.run_until_parked();
        assert!(matches!(
            cx.foreground_executor().block_test(task),
            Err(CaptureError::Unparsed { .. })
        ));
    }
}
