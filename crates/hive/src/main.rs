mod app;
mod state;
mod terminal;
mod ui;

use gpui_kit::*;
use gpui_kit::component::TitleBar;

use app::AppState;
use ui::workspace::Workspace;

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let dir = hive_core::state::hive_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let file = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("hive.log"));
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("hive=info,hive_core=info"));
    match file {
        Ok(file) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .init(),
        Err(_) => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }
}

fn main() {
    init_logging();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "hive starting");

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            AppState::init(cx);
            terminal::view::init(cx);
            ui::workspace::init(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1200.), px(760.)), cx))),
                ..TitleBar::window_options()
            };
            gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Workspace::new(window, cx)))
                .expect("failed to open window");
            cx.activate(true);
        });
}
