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
        .size(rems(size.0 * FOLDER_ICON_SCALE))
        .text_color(cx.theme().muted_foreground);
        return div()
            .size(size)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(icon)
            .into_any_element();
    }
    let (data, color) = file_icon(path, cx);
    let icon = if let Some(data) = data {
        Icon::default().data(data)
    } else {
        Icon::new(Lucide::File)
    };
    icon.size(size)
        .flex_none()
        .text_color(if mode == FileIcons::Colored {
            color
        } else {
            cx.theme().muted_foreground
        })
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
    if name == "dockerfile" || name.starts_with("docker-compose") {
        return icon!("docker", theme.blue);
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
