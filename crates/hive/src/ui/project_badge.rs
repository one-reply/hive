//! A project's badge: the folder icon, a chosen icon, its initial or an emoji, in the project's
//! color. Drawn in the sidebar header and in the project's settings.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::Icon;
use gpui_kit::*;
use hive_core::identity::{Badge, ProjectIcon, ProjectIdentity};

/// Project colors (Orca's palette). A project without a custom color takes one by position.
pub(crate) const PALETTE: [u32; 8] = [
    0x737373, 0xef4444, 0xf97316, 0xeab308, 0x22c55e, 0x14b8a6, 0x8b5cf6, 0xec4899,
];
/// The first project defaults to orange, as in Orca; later ones walk the palette from there.
const FIRST_DEFAULT: usize = 2;
/// Initial letter relative to the badge size.
const AVATAR_TEXT_SCALE: f32 = 0.62;
const AVATAR_TEXT_COLOR: u32 = 0xffffff;

/// The project's custom color, else a palette color by its position `ix`.
pub(crate) fn color(identity: &ProjectIdentity, ix: usize) -> Hsla {
    rgb(identity
        .color()
        .unwrap_or(PALETTE[(ix + FIRST_DEFAULT) % PALETTE.len()]))
    .into()
}

/// `0xRRGGBB` of a picked color, as stored in [`ProjectIdentity`].
pub(crate) fn to_rgb_hex(color: Hsla) -> u32 {
    let Rgba { r, g, b, .. } = color.into();
    let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u32;
    channel(r) << 16 | channel(g) << 8 | channel(b)
}

pub(crate) fn lucide(icon: ProjectIcon) -> Lucide {
    match icon {
        ProjectIcon::Folder => Lucide::Folder,
        ProjectIcon::Code => Lucide::CodeXml,
        ProjectIcon::Terminal => Lucide::SquareTerminal,
        ProjectIcon::Bot => Lucide::Bot,
        ProjectIcon::Package => Lucide::Package,
        ProjectIcon::Database => Lucide::Database,
        ProjectIcon::Globe => Lucide::Globe,
        ProjectIcon::Server => Lucide::Server,
        ProjectIcon::Cpu => Lucide::Cpu,
        ProjectIcon::Layers => Lucide::Layers,
        ProjectIcon::Braces => Lucide::Braces,
        ProjectIcon::Rocket => Lucide::Rocket,
        ProjectIcon::Wrench => Lucide::Wrench,
        ProjectIcon::Briefcase => Lucide::Briefcase,
        ProjectIcon::Building => Lucide::Building2,
        ProjectIcon::Palette => Lucide::Palette,
        ProjectIcon::Gauge => Lucide::Gauge,
        ProjectIcon::Sparkles => Lucide::Sparkles,
        ProjectIcon::Shapes => Lucide::Shapes,
        ProjectIcon::Box => Lucide::Box,
    }
}

/// The badge of a project named `name`, `size` square.
pub(crate) fn render(
    identity: &ProjectIdentity,
    name: &str,
    color: Hsla,
    size: Pixels,
) -> AnyElement {
    match identity.badge() {
        Badge::Icon(icon) => Icon::new(lucide(*icon))
            .size(size)
            .text_color(color)
            .into_any_element(),
        Badge::Avatar => avatar(name, color, size).into_any_element(),
        Badge::Emoji(emoji) => div()
            .size(size)
            .flex()
            .items_center()
            .justify_center()
            .text_size(size)
            .line_height(size)
            .child(emoji.clone())
            .into_any_element(),
    }
}

/// The first letter of `name` on `color`.
pub(crate) fn avatar(name: &str, color: Hsla, size: Pixels) -> Div {
    let initial: String = name
        .chars()
        .next()
        .into_iter()
        .flat_map(char::to_uppercase)
        .collect();
    div()
        .size(size)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(size * 0.25)
        .bg(color)
        .text_color(rgb(AVATAR_TEXT_COLOR))
        .text_size(size * AVATAR_TEXT_SCALE)
        .font_weight(FontWeight::BOLD)
        .child(initial)
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the `gpui_kit::*` glob brings in GPUI's own `test` macro.
    use super::{PALETTE, to_rgb_hex};
    use gpui_kit::rgb;

    #[test]
    fn hex_round_trips() {
        for hex in PALETTE {
            assert_eq!(to_rgb_hex(rgb(hex).into()), hex);
        }
    }
}
