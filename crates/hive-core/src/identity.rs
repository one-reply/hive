//! How a project presents itself in the sidebar and tabs: its display name, color and badge.
//! Everything is optional; the default is the folder name, a color by position and a folder icon.

use serde::{Deserialize, Serialize};

/// Project colors as `0xRRGGBB` (Orca's palette), in the order the picker shows them.
pub const PALETTE: [u32; 8] = [
    0x737373, 0xef4444, 0xf97316, 0xeab308, 0x22c55e, 0x14b8a6, 0x8b5cf6, 0xec4899,
];
/// The first project defaults to orange, as in Orca; later ones walk the palette from there.
const FIRST_DEFAULT: usize = 2;

/// Palette colors in the order projects get them: from orange on, gray last.
fn assignment_order() -> impl Iterator<Item = u32> {
    PALETTE
        .iter()
        .cycle()
        .skip(FIRST_DEFAULT)
        .take(PALETTE.len())
        .copied()
}

/// The palette color a project without a custom color shows at position `ix`.
pub fn default_color(ix: usize) -> u32 {
    PALETTE[(ix + FIRST_DEFAULT) % PALETTE.len()]
}

/// The palette color for a new project: the first one nobody uses, else the least used one,
/// so a new project stands apart from the ones already listed.
pub fn unused_color(used: &[u32]) -> u32 {
    let uses = |rgb: u32| used.iter().filter(|&&taken| taken == rgb).count();
    assignment_order()
        .min_by_key(|&rgb| uses(rgb))
        .expect("the palette is not empty")
}

/// User-chosen display details of one project. `Default` is what "Reset" restores.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Default)]
#[serde(default)]
pub struct ProjectIdentity {
    name: Option<String>,
    /// `0xRRGGBB`; `None` picks a palette color by the project's position.
    color: Option<u32>,
    badge: Badge,
}

impl ProjectIdentity {
    /// The custom display name, if one is set.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Sets the display name; blank text clears it back to the folder name.
    pub fn set_name(&mut self, name: &str) {
        self.name = non_blank(name);
    }

    /// The custom color as `0xRRGGBB`, if one is set.
    pub fn color(&self) -> Option<u32> {
        self.color
    }

    /// The custom color, else the palette color for position `ix`.
    pub fn color_or_default(&self, ix: usize) -> u32 {
        self.color.unwrap_or_else(|| default_color(ix))
    }

    /// Sets a custom color; bits above `0xRRGGBB` are dropped.
    pub fn set_color(&mut self, rgb: u32) {
        self.color = Some(rgb & 0xff_ff_ff);
    }

    /// What is drawn before the name.
    pub fn badge(&self) -> &Badge {
        &self.badge
    }

    /// Replaces the badge.
    pub fn set_badge(&mut self, badge: Badge) {
        self.badge = badge;
    }

    /// Shows `emoji` as the badge; blank text falls back to the default icon.
    pub fn set_emoji(&mut self, emoji: &str) {
        self.badge = non_blank(emoji).map_or_else(Badge::default, Badge::Emoji);
    }

    /// Nothing customized: what "Reset" leaves.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// What is drawn before the project's name.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum Badge {
    Icon(ProjectIcon),
    /// The first letter of the display name on the project's color.
    Avatar,
    Emoji(String),
}

impl Default for Badge {
    fn default() -> Self {
        Badge::Icon(ProjectIcon::Folder)
    }
}

/// Icons offered for a project badge (Lucide names in the UI).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectIcon {
    Code,
    Terminal,
    Bot,
    Package,
    Database,
    Globe,
    Server,
    Cpu,
    Layers,
    Braces,
    Rocket,
    Wrench,
    Briefcase,
    Building,
    Palette,
    Gauge,
    Sparkles,
    Shapes,
    Box,
    /// Also stands in for an icon name this version doesn't know (serde wants it last).
    #[serde(other)]
    Folder,
}

impl ProjectIcon {
    /// Every icon, in the order the picker shows them.
    pub const ALL: [ProjectIcon; 20] = [
        ProjectIcon::Folder,
        ProjectIcon::Code,
        ProjectIcon::Terminal,
        ProjectIcon::Bot,
        ProjectIcon::Package,
        ProjectIcon::Database,
        ProjectIcon::Globe,
        ProjectIcon::Server,
        ProjectIcon::Cpu,
        ProjectIcon::Layers,
        ProjectIcon::Braces,
        ProjectIcon::Rocket,
        ProjectIcon::Wrench,
        ProjectIcon::Briefcase,
        ProjectIcon::Building,
        ProjectIcon::Palette,
        ProjectIcon::Gauge,
        ProjectIcon::Sparkles,
        ProjectIcon::Shapes,
        ProjectIcon::Box,
    ];
}

fn non_blank(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_are_the_default() {
        let identity: ProjectIdentity = serde_json::from_str("{}").expect("valid json");
        assert!(identity.is_default());
    }

    #[test]
    fn unknown_icon_falls_back_to_folder() {
        let json = r#"{"badge":{"kind":"icon","value":"spaceship"}}"#;
        let identity: ProjectIdentity = serde_json::from_str(json).expect("valid json");
        assert_eq!(identity.badge(), &Badge::Icon(ProjectIcon::Folder));
    }

    #[test]
    fn badges_round_trip() {
        for badge in [
            Badge::Avatar,
            Badge::Emoji("🐝".into()),
            Badge::Icon(ProjectIcon::Rocket),
        ] {
            let mut identity = ProjectIdentity::default();
            identity.set_badge(badge.clone());
            let json = serde_json::to_string(&identity).expect("serializable");
            let back: ProjectIdentity = serde_json::from_str(&json).expect("valid json");
            assert_eq!(back.badge(), &badge);
        }
    }

    #[test]
    fn new_color_skips_taken_ones() {
        assert_eq!(unused_color(&[]), 0xf97316);
        assert_eq!(unused_color(&[0xf97316, 0xeab308]), 0x22c55e);
        let all_but_red: Vec<u32> = PALETTE.into_iter().filter(|&c| c != 0xef4444).collect();
        assert_eq!(unused_color(&all_but_red), 0xef4444);
    }

    #[test]
    fn full_palette_reuses_the_least_used() {
        let mut used = PALETTE.to_vec();
        used.extend([0xf97316, 0xeab308]);
        assert_eq!(unused_color(&used), 0x22c55e);
    }

    #[test]
    fn blank_text_clears() {
        let mut identity = ProjectIdentity::default();
        identity.set_name("  app  ");
        assert_eq!(identity.name(), Some("app"));
        identity.set_name("   ");
        assert_eq!(identity.name(), None);
        identity.set_emoji(" ");
        assert!(identity.is_default());
    }
}
