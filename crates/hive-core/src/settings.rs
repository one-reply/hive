use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "System",
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
        }
    }
}

/// Narrowest sidebar the user can drag to, in logical pixels.
pub const SIDEBAR_WIDTH_MIN: f32 = 180.;
/// Widest sidebar the user can drag to, in logical pixels.
pub const SIDEBAR_WIDTH_MAX: f32 = 600.;
const SIDEBAR_WIDTH_DEFAULT: f32 = 260.;

/// Minimum width of the source control sidebar.
pub const GIT_PANEL_WIDTH_MIN: f32 = 280.;
/// Maximum width of the source control sidebar.
pub const GIT_PANEL_WIDTH_MAX: f32 = 600.;
const GIT_PANEL_WIDTH_DEFAULT: f32 = 360.;

/// Smallest UI font size (the rem every UI size scales from), in logical pixels.
pub const UI_FONT_SIZE_MIN: f32 = 12.;
/// Largest UI font size, in logical pixels.
pub const UI_FONT_SIZE_MAX: f32 = 24.;
/// UI font size before the user changes it, in logical pixels.
pub const UI_FONT_SIZE_DEFAULT: f32 = 16.;

/// Smallest terminal font size, in logical pixels.
pub const TERMINAL_FONT_SIZE_MIN: f32 = 11.;
/// Largest terminal font size, in logical pixels.
pub const TERMINAL_FONT_SIZE_MAX: f32 = 28.;
/// Terminal font size before the user changes it, in logical pixels.
pub const TERMINAL_FONT_SIZE_DEFAULT: f32 = 13.;

/// User preferences. Persisted as part of [`crate::state::State`].
///
/// Numeric fields are private and kept within their ranges by the accessors, because
/// `state.json` may be edited by hand.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    /// CLI used when preparing a Git error draft in an agent terminal.
    #[serde(deserialize_with = "deserialize_default_cli_agent")]
    default_cli_agent: crate::agent::AgentKind,
    /// Global recipe for generating a staged commit message; never contains API keys.
    pub commit_message: crate::commit_message::Recipe,
    pub theme: ThemeChoice,
    sidebar_width: f32,
    git_panel_width: f32,
    /// Whether source control is visible on launch.
    pub git_panel_open: bool,
    /// Files is selected in the shared right sidebar; older snapshots default to Git.
    pub files_selected: bool,
    /// Side-by-side diff instead of the default unified presentation.
    pub diff_split: bool,
    ui_font_size: f32,
    terminal_font_size: f32,
    /// Shortcuts the user reassigned: action name (`"workspace::NewTerminal"`) → keystrokes
    /// (`"cmd-shift-t"`). Actions missing here use their default. The keystroke string is
    /// opaque to this crate; the app parses it and falls back to the default if it can't.
    keybindings: BTreeMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_cli_agent: crate::agent::AgentKind::Codex,
            commit_message: Default::default(),
            theme: ThemeChoice::default(),
            sidebar_width: SIDEBAR_WIDTH_DEFAULT,
            git_panel_width: GIT_PANEL_WIDTH_DEFAULT,
            git_panel_open: true,
            files_selected: false,
            diff_split: false,
            ui_font_size: UI_FONT_SIZE_DEFAULT,
            terminal_font_size: TERMINAL_FONT_SIZE_DEFAULT,
            keybindings: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// The interactive CLI that receives Git error drafts.
    pub fn default_cli_agent(&self) -> crate::agent::AgentKind {
        self.default_cli_agent
    }

    /// Select a supported interactive CLI without changing its permissions or model.
    pub fn set_default_cli_agent(
        &mut self,
        agent: crate::agent::AgentKind,
    ) -> Result<(), crate::agent::launch::LaunchError> {
        if !crate::agent::launch::AGENTS.contains(&agent) {
            return Err(crate::agent::launch::LaunchError::Unsupported);
        }
        self.default_cli_agent = agent;
        Ok(())
    }
    /// Source control width, clamped even for hand-edited state files.
    pub fn git_panel_width(&self) -> f32 {
        clamp_git_panel_width(self.git_panel_width)
    }

    /// Store a finite source control width within its supported range.
    pub fn set_git_panel_width(&mut self, width: f32) {
        self.git_panel_width = clamp_git_panel_width(width);
    }
    /// Sidebar width in logical pixels, always within the allowed range.
    pub fn sidebar_width(&self) -> f32 {
        clamp_sidebar_width(self.sidebar_width)
    }

    /// Stores `width` clamped to the allowed range.
    pub fn set_sidebar_width(&mut self, width: f32) {
        self.sidebar_width = clamp_sidebar_width(width);
    }

    /// UI font size in logical pixels, within `UI_FONT_SIZE_MIN..=UI_FONT_SIZE_MAX`.
    pub fn ui_font_size(&self) -> f32 {
        clamp_ui_font_size(self.ui_font_size)
    }

    /// Stores `size` clamped to the allowed range.
    pub fn set_ui_font_size(&mut self, size: f32) {
        self.ui_font_size = clamp_ui_font_size(size);
    }

    /// Terminal font size in logical pixels, within
    /// `TERMINAL_FONT_SIZE_MIN..=TERMINAL_FONT_SIZE_MAX`.
    pub fn terminal_font_size(&self) -> f32 {
        clamp_terminal_font_size(self.terminal_font_size)
    }

    /// Stores `size` clamped to the allowed range.
    pub fn set_terminal_font_size(&mut self, size: f32) {
        self.terminal_font_size = clamp_terminal_font_size(size);
    }

    /// Keystrokes the user assigned to `action`, if they reassigned it.
    pub fn keybinding(&self, action: &str) -> Option<&str> {
        self.keybindings.get(action).map(String::as_str)
    }

    /// Every reassigned shortcut, action name → keystrokes.
    pub fn keybindings(&self) -> &BTreeMap<String, String> {
        &self.keybindings
    }

    /// Assigns `keystrokes` to `action`, replacing its default.
    pub fn set_keybinding(&mut self, action: &str, keystrokes: &str) {
        self.keybindings
            .insert(action.to_owned(), keystrokes.to_owned());
    }

    /// Returns `action` to its default shortcut. Returns whether it was reassigned.
    pub fn reset_keybinding(&mut self, action: &str) -> bool {
        self.keybindings.remove(action).is_some()
    }
}

fn deserialize_default_cli_agent<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<crate::agent::AgentKind, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    match serde_json::from_value(value) {
        Ok(agent) if crate::agent::launch::AGENTS.contains(&agent) => Ok(agent),
        _ => {
            tracing::warn!("invalid default CLI agent; using Codex");
            Ok(crate::agent::AgentKind::Codex)
        }
    }
}

/// Clamps `width` to `SIDEBAR_WIDTH_MIN..=SIDEBAR_WIDTH_MAX`; a non-finite value falls back
/// to the default.
pub fn clamp_sidebar_width(width: f32) -> f32 {
    clamp_or_default(
        width,
        SIDEBAR_WIDTH_MIN,
        SIDEBAR_WIDTH_MAX,
        SIDEBAR_WIDTH_DEFAULT,
    )
}

/// Clamp the right sidebar width, falling back for non-finite values.
pub fn clamp_git_panel_width(width: f32) -> f32 {
    clamp_or_default(
        width,
        GIT_PANEL_WIDTH_MIN,
        GIT_PANEL_WIDTH_MAX,
        GIT_PANEL_WIDTH_DEFAULT,
    )
}

fn clamp_ui_font_size(size: f32) -> f32 {
    clamp_or_default(
        size,
        UI_FONT_SIZE_MIN,
        UI_FONT_SIZE_MAX,
        UI_FONT_SIZE_DEFAULT,
    )
}

fn clamp_terminal_font_size(size: f32) -> f32 {
    clamp_or_default(
        size,
        TERMINAL_FONT_SIZE_MIN,
        TERMINAL_FONT_SIZE_MAX,
        TERMINAL_FONT_SIZE_DEFAULT,
    )
}

fn clamp_or_default(value: f32, min: f32, max: f32, default: f32) -> f32 {
    if !value.is_finite() {
        return default;
    }
    value.clamp(min, max)
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_cli_agent_survives_roundtrip_and_invalid_saved_choices() {
        use crate::agent::AgentKind;
        let mut settings: super::Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.default_cli_agent(), AgentKind::Codex);
        for agent in crate::agent::launch::AGENTS {
            settings.set_default_cli_agent(agent).unwrap();
            let json = serde_json::to_string(&settings).unwrap();
            assert_eq!(
                serde_json::from_str::<super::Settings>(&json)
                    .unwrap()
                    .default_cli_agent(),
                agent
            );
        }
        for value in ["\"FutureAgent\"", "\"Aider\"", "null", "42"] {
            let json = format!("{{\"default_cli_agent\":{value},\"git_panel_open\":false}}");
            let settings: super::Settings = serde_json::from_str(&json).unwrap();
            assert_eq!(settings.default_cli_agent(), AgentKind::Codex);
            assert!(!settings.git_panel_open);
        }
        assert!(settings.set_default_cli_agent(AgentKind::Aider).is_err());
    }
    use super::*;

    #[test]
    fn missing_fields_use_defaults() {
        let settings: Settings = serde_json::from_str(r#"{"theme":"dark"}"#).expect("valid json");
        assert_eq!(settings.sidebar_width(), SIDEBAR_WIDTH_DEFAULT);
        assert_eq!(settings.ui_font_size(), UI_FONT_SIZE_DEFAULT);
        assert_eq!(settings.terminal_font_size(), TERMINAL_FONT_SIZE_DEFAULT);
        assert!(settings.keybindings().is_empty());
        assert_eq!(settings.git_panel_width(), 360.);
        assert!(settings.git_panel_open);
        assert!(!settings.diff_split);
    }

    #[test]
    fn git_preferences_round_trip_and_clamp() {
        let mut settings: Settings = serde_json::from_str(
            r#"{"git_panel_width":9000,"git_panel_open":false,"diff_split":true}"#,
        )
        .unwrap();
        assert_eq!(settings.git_panel_width(), 600.);
        settings.set_git_panel_width(1.);
        assert_eq!(settings.git_panel_width(), 280.);
        settings.set_git_panel_width(f32::NAN);
        let loaded: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(loaded.git_panel_width(), 360.);
        assert!(!loaded.git_panel_open);
        assert!(loaded.diff_split);
    }

    #[test]
    fn hand_edited_sidebar_width_is_clamped() {
        let settings: Settings =
            serde_json::from_str(r#"{"sidebar_width":5}"#).expect("valid json");
        assert_eq!(settings.sidebar_width(), SIDEBAR_WIDTH_MIN);
        let settings: Settings =
            serde_json::from_str(r#"{"sidebar_width":9000}"#).expect("valid json");
        assert_eq!(settings.sidebar_width(), SIDEBAR_WIDTH_MAX);
    }

    #[test]
    fn hand_edited_font_sizes_are_clamped() {
        let settings: Settings =
            serde_json::from_str(r#"{"ui_font_size":2,"terminal_font_size":200}"#)
                .expect("valid json");
        assert_eq!(settings.ui_font_size(), UI_FONT_SIZE_MIN);
        assert_eq!(settings.terminal_font_size(), TERMINAL_FONT_SIZE_MAX);
    }

    #[test]
    fn non_finite_values_fall_back_to_defaults() {
        assert_eq!(clamp_sidebar_width(f32::NAN), SIDEBAR_WIDTH_DEFAULT);
        assert_eq!(clamp_sidebar_width(f32::INFINITY), SIDEBAR_WIDTH_DEFAULT);
        let mut settings = Settings::default();
        settings.set_ui_font_size(f32::NAN);
        settings.set_terminal_font_size(f32::NEG_INFINITY);
        assert_eq!(settings.ui_font_size(), UI_FONT_SIZE_DEFAULT);
        assert_eq!(settings.terminal_font_size(), TERMINAL_FONT_SIZE_DEFAULT);
    }

    #[test]
    fn keybindings_round_trip_and_reset() {
        let mut settings = Settings::default();
        settings.set_keybinding("workspace::NewTerminal", "cmd-shift-t");
        let json = serde_json::to_string(&settings).expect("serializable");
        let mut loaded: Settings = serde_json::from_str(&json).expect("valid json");
        assert_eq!(
            loaded.keybinding("workspace::NewTerminal"),
            Some("cmd-shift-t")
        );
        assert!(loaded.reset_keybinding("workspace::NewTerminal"));
        assert!(!loaded.reset_keybinding("workspace::NewTerminal"));
        assert_eq!(loaded.keybinding("workspace::NewTerminal"), None);
    }
}
