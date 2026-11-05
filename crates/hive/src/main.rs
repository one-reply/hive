mod app;
mod quit;
mod state;
mod terminal;
mod ui;
mod updater;

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
        .detach();
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "--agent-hook") {
        if let (Some(endpoint), Some(terminal)) = (
            args.get(2),
            args.get(3)
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse().ok()),
        ) && let Err(error) = hive_core::agent::hooks::forward(
            std::path::Path::new(endpoint),
            terminal,
            std::io::stdin().lock(),
        ) {
            eprintln!("Hive hook: {error}");
        }
        return;
    }
    init_logging();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "hive starting");

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(|cx| {
            gpui_kit::init(cx);
            load_login_environment(cx);
            cx.spawn(async move |cx| {
                let (loaded, recovery) = cx
                    .background_executor()
                    .spawn(async {
                        let loaded = hive_core::state::State::load();
                        // Recovery must be available before any restored buffer becomes editable.
                        let recovery = hive_core::files::recovery::load(
                            &hive_core::state::hive_dir().join("editor-recovery"),
                        );
                        (loaded, recovery)
                    })
                    .await;
                cx.update(|cx| {
                    AppState::init(loaded, recovery, cx);
                    ui::app_menu::init(cx);
                    ui::theme::init(cx);
                    ui::syntax::init();
                    // After every layer has bound its keys: it rebuilds the keymap around them.
                    ui::keymap::init(cx);
                    ui::settings::init(cx);
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
                    gpui_kit::open_window(options, cx, |window, cx| {
                        cx.new(|cx| Workspace::new(window, cx))
                    })
                    .expect("failed to open window");
                    cx.activate(true);
                });
            })
            .detach();
        });
}
