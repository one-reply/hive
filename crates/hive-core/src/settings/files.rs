use serde::{Deserialize, Serialize};

/// File-type icon treatment; unknown future values use colored icons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileIcons {
    /// Theme foreground for every icon.
    Monochrome,
    /// Hide file and folder icons, retaining disclosure controls.
    Off,
    /// Language colors with neutral folders.
    #[default]
    #[serde(other)]
    Colored,
}

/// Vertical spacing of file rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileDensity {
    /// More space between adjacent rows.
    Comfortable,
    /// Compact desktop rows.
    #[default]
    #[serde(other)]
    Compact,
}

/// Global Files preferences. Sizes are at the default 16px UI scale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilesSettings {
    /// File-type icon visibility and coloring.
    pub icons: FileIcons,
    /// Vertical row spacing.
    pub density: FileDensity,
    /// Show index/worktree status and changed-folder indicators.
    pub git_decorations: bool,
    /// Reveal the active editor without moving keyboard focus.
    pub auto_reveal: bool,
    /// Include dotfiles and dot-directories, excluding Git internals.
    pub show_hidden: bool,
    /// Include ignored paths, rendered with subdued labels.
    pub show_ignored: bool,
    font_size: f32,
}

impl Default for FilesSettings {
    fn default() -> Self {
        Self {
            icons: FileIcons::Colored,
            density: FileDensity::Compact,
            git_decorations: true,
            auto_reveal: true,
            show_hidden: true,
            show_ignored: true,
            font_size: 13.,
        }
    }
}

impl FilesSettings {
    /// Font size clamped to 11–20 logical pixels at the default UI scale.
    pub fn font_size(&self) -> f32 {
        super::clamp_or_default(self.font_size, 11., 20., 13.)
    }

    /// Store a finite, bounded font size.
    pub fn set_font_size(&mut self, size: f32) {
        self.font_size = super::clamp_or_default(size, 11., 20., 13.);
    }

    /// Row height scaled proportionally to the Files font size.
    pub fn row_height(&self) -> f32 {
        let base = match self.density {
            FileDensity::Compact => 22.,
            FileDensity::Comfortable => 26.,
        };
        base * self.font_size() / 13.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_settings_and_future_enum_values_keep_defaults() {
        let old: crate::settings::Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.files, FilesSettings::default());
        let future: FilesSettings = serde_json::from_str(
            r#"{"icons":"future","density":"future","font_size":100,"show_hidden":false}"#,
        )
        .unwrap();
        assert_eq!(future.icons, FileIcons::Colored);
        assert_eq!(future.density, FileDensity::Compact);
        assert_eq!(future.font_size(), 20.);
        assert!(!future.show_hidden);
        let mut settings = FilesSettings::default();
        settings.set_font_size(f32::NAN);
        assert_eq!(settings.font_size(), 13.);
        settings.set_font_size(0.);
        assert_eq!(settings.font_size(), 11.);
    }
}
