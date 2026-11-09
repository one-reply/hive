mod app;
mod cli;
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

/// Launched from Finder, Hive inherits launchd's minimal `PATH`; git hooks and CLIs need the
/// terminal's. The lookup runs a login shell, so it stays off the UI thread, and commands that
/// start before it answers keep the process environment.
fn load_login_environment(cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor()
            .spawn(async {
                match hive_core::host::environment::load() {
                    Ok(environment) => {
                        tracing::info!(
                            variables = environment.len(),
                            "login shell environment loaded"
                        )
                    }
                    Err(error) => {
                        tracing::warn!(%error, "keeping the process environment for local commands")
                    }
                }
            })
            .await;
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
    let args: Vec<_> = std::env::args_os().collect();
    if let Some(code) = cli::run(&args) {
        return code;
    }
    init_logging();
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
            let (loaded, recovery, endpoint) = cx
                .background_executor()
                .spawn(async {
                    let loaded = hive_core::state::State::load();
                    // Recovery must be available before any restored buffer becomes editable.
                    let recovery = hive_core::files::recovery::load(
                        &hive_core::state::hive_dir().join("editor-recovery"),
                    );
                    // Restored panes start with the window; they need the sockets already.
                    let endpoint = state::agents::Endpoint::bind();
                    (loaded, recovery, endpoint)
                })
                .await;
            cx.update(|cx| {
                AppState::init(loaded, recovery, endpoint, cx);
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
