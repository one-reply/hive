use gpui_kit::assets::IconName as Lucide;
use gpui_kit::{
    component::{ActiveTheme as _, Icon},
    *,
};
use hive_core::settings::FileIcons;
use std::path::Path;

/// Folder glyphs read heavier than the file-type icons next to them, so they are drawn
/// slightly smaller inside a box of the full icon size to keep labels aligned.
const FOLDER_ICON_SCALE: f32 = 0.85;

/// Seti glyphs fill about 60% of their viewBox while Lucide's `File` fills about 85%, so the
/// fallback for unsupported files is shrunk to match the height of the file-type icons.
const FALLBACK_FILE_ICON_SCALE: f32 = 0.7;

pub(super) fn render(
    path: &Path,
    folder: bool,
    expanded: bool,
    mode: FileIcons,
    size: Rems,
    cx: &App,
) -> AnyElement {
    if mode == FileIcons::Off {
        return div().into_any_element();
    }
    if folder {
        let icon = Icon::new(if expanded {
            Lucide::FolderOpen
        } else {
            Lucide::Folder
        })
        .text_color(cx.theme().muted_foreground);
        return scaled(icon, FOLDER_ICON_SCALE, size);
    }
    let (data, color) = file_icon(path, cx);
    let color = if mode == FileIcons::Colored {
        color
    } else {
        cx.theme().muted_foreground
    };
    match data {
        Some(data) => Icon::default()
            .data(data)
            .size(size)
            .flex_none()
            .text_color(color)
            .into_any_element(),
        None => scaled(
            Icon::new(Lucide::File).text_color(color),
            FALLBACK_FILE_ICON_SCALE,
            size,
        ),
    }
}

/// Draws `icon` at `scale` of `size`, centered in a box of the full `size` so labels stay aligned.
fn scaled(icon: Icon, scale: f32, size: Rems) -> AnyElement {
    div()
        .size(size)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(icon.size(rems(size.0 * scale)))
        .into_any_element()
}

fn file_icon(path: &Path, cx: &App) -> (Option<&'static [u8]>, Hsla) {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let theme = cx.theme();
    macro_rules! icon {
        ($name:literal, $color:expr) => {
            (
                Some(
                    include_bytes!(concat!("../../../assets/file-icons/", $name, ".svg"))
                        .as_slice(),
                ),
                $color,
            )
        };
    }
    if name.starts_with(".git") {
        return icon!("git", theme.red);
    }
    if hive_core::files::is_dockerfile(path)
        || name.starts_with("docker-compose")
        || name == ".dockerignore"
    {
        return icon!("docker", theme.blue);
    }
    if hive_core::files::editor_language(path) == "make" {
        return icon!("makefile", theme.yellow);
    }
    if name.ends_with(".lock") || name.ends_with("-lock.json") {
        return icon!("lock", theme.muted_foreground);
    }
    match extension.as_ref() {
        "rs" => icon!("rust", theme.yellow),
        "js" | "mjs" | "cjs" => icon!("javascript", theme.yellow),
        "ts" | "mts" | "cts" => icon!("typescript", theme.blue),
        "tsx" | "jsx" => icon!("react", theme.cyan),
        "py" | "pyi" => icon!("python", theme.yellow),
        "go" => icon!("go", theme.cyan),
        "java" => icon!("java", theme.red),
        "c" | "h" => icon!("c", theme.blue),
        "cpp" | "cc" | "hpp" => icon!("cpp", theme.blue),
        "swift" => icon!("swift", theme.red),
        "json" | "jsonc" => icon!("json", theme.yellow),
        "yaml" | "yml" => icon!("yml", theme.magenta),
        "toml" | "ini" | "conf" => icon!("config", theme.muted_foreground),
        "md" | "mdx" => icon!("markdown", theme.blue),
        "html" | "htm" => icon!("html", theme.red),
        "css" | "scss" | "sass" => icon!("css", theme.magenta),
        "sh" | "bash" | "zsh" => icon!("shell", theme.green),
        "png" | "jpg" | "jpeg" | "svg" | "webp" | "gif" | "ico" => icon!("image", theme.magenta),
        _ => (None, theme.muted_foreground),
    }
}
