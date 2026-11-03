use serde::{Deserialize, Serialize};

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

/// User preferences. Persisted as part of [`crate::state::State`].
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// Kept within `SIDEBAR_WIDTH_MIN..=SIDEBAR_WIDTH_MAX` by the accessors, because
    /// `state.json` may be edited by hand.
    sidebar_width: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::default(),
            sidebar_width: SIDEBAR_WIDTH_DEFAULT,
        }
    }
}

impl Settings {
    /// Sidebar width in logical pixels, always within the allowed range.
    pub fn sidebar_width(&self) -> f32 {
        clamp_sidebar_width(self.sidebar_width)
    }

    /// Stores `width` clamped to the allowed range.
    pub fn set_sidebar_width(&mut self, width: f32) {
        self.sidebar_width = clamp_sidebar_width(width);
    }
}

/// Clamps `width` to `SIDEBAR_WIDTH_MIN..=SIDEBAR_WIDTH_MAX`; a non-finite value falls back
/// to the default.
pub fn clamp_sidebar_width(width: f32) -> f32 {
    if !width.is_finite() {
        return SIDEBAR_WIDTH_DEFAULT;
    }
    width.clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_sidebar_width_uses_default() {
        let settings: Settings = serde_json::from_str(r#"{"theme":"dark"}"#).expect("valid json");
        assert_eq!(settings.sidebar_width(), SIDEBAR_WIDTH_DEFAULT);
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
    fn non_finite_width_falls_back_to_default() {
        assert_eq!(clamp_sidebar_width(f32::NAN), SIDEBAR_WIDTH_DEFAULT);
        assert_eq!(clamp_sidebar_width(f32::INFINITY), SIDEBAR_WIDTH_DEFAULT);
    }
}
