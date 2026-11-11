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

/// Files font size before the user changes it, in logical pixels.
pub const FILES_FONT_SIZE_DEFAULT: f32 = 14.;
/// Font size the row heights are designed at; rows scale relative to it.
const ROW_REFERENCE_FONT_SIZE: f32 = 13.;

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
    /// Save a dirty editor buffer when keyboard focus leaves it, and on close
    /// instead of asking.
    pub save_on_focus_change: bool,
    /// Suggest names from the open file and import paths while typing in the editor.
    pub code_completion: bool,
    /// Show who last changed the line under the editor caret.
    pub inline_blame: bool,
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
            save_on_focus_change: true,
            code_completion: true,
            inline_blame: true,
            font_size: FILES_FONT_SIZE_DEFAULT,
        }
    }
}

impl FilesSettings {
    /// Font size clamped to 11–20 logical pixels at the default UI scale.
    pub fn font_size(&self) -> f32 {
        super::clamp_or_default(self.font_size, 11., 20., FILES_FONT_SIZE_DEFAULT)
    }

    /// Store a finite, bounded font size.
    pub fn set_font_size(&mut self, size: f32) {
        self.font_size = super::clamp_or_default(size, 11., 20., FILES_FONT_SIZE_DEFAULT);
    }

    /// Row height scaled proportionally to the Files font size.
    pub fn row_height(&self) -> f32 {
        let base = match self.density {
            FileDensity::Compact => 22.,
            FileDensity::Comfortable => 26.,
        };
        base * self.font_size() / ROW_REFERENCE_FONT_SIZE
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
        assert!(future.save_on_focus_change);
        assert!(
            future.code_completion,
            "settings saved before completion turn it on"
        );
        assert!(
            future.inline_blame,
            "settings saved before blame turn it on"
        );
        let opted_out: FilesSettings =
            serde_json::from_str(r#"{"save_on_focus_change":false,"code_completion":false}"#)
                .unwrap();
        assert!(!opted_out.save_on_focus_change);
        assert!(!opted_out.code_completion);
        let no_blame: FilesSettings = serde_json::from_str(r#"{"inline_blame":false}"#).unwrap();
        assert!(!no_blame.inline_blame);
        let mut settings = FilesSettings::default();
        settings.set_font_size(f32::NAN);
        assert_eq!(settings.font_size(), FILES_FONT_SIZE_DEFAULT);
        settings.set_font_size(0.);
        assert_eq!(settings.font_size(), 11.);
    }
}
