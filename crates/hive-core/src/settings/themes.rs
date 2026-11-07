//! Independent built-in palettes for the light and dark appearance modes.

use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// The palette used when the application is in light mode.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum LightTheme {
    /// Atom's One Light palette.
    One,
    /// Ethan Schoonover's Solarized Light palette.
    Solarized,
    /// Hive's original light palette; also the fallback for an unknown name.
    #[default]
    #[serde(other)]
    Hive,
}

impl LightTheme {
    /// Palettes in the order shown in Appearance.
    pub const ALL: [Self; 3] = [Self::Hive, Self::One, Self::Solarized];

    /// The name shown in the theme picker.
    pub fn label(self) -> &'static str {
        match self {
            Self::Hive => "Hive Light",
            Self::One => "One Light",
            Self::Solarized => "Solarized Light",
        }
    }
}

/// The palette used when the application is in dark mode.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum DarkTheme {
    /// Atom's One Dark palette.
    One,
    /// The Dracula palette.
    Dracula,
    /// Hive's original dark palette; also the fallback for an unknown name.
    #[default]
    #[serde(other)]
    Hive,
}

impl DarkTheme {
    /// Palettes in the order shown in Appearance.
    pub const ALL: [Self; 3] = [Self::Hive, Self::One, Self::Dracula];

    /// The name shown in the theme picker.
    pub fn label(self) -> &'static str {
        match self {
            Self::Hive => "Hive Dark",
            Self::One => "One Dark",
            Self::Dracula => "Dracula",
        }
    }
}

pub(super) fn deserialize_theme<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_else(|error| {
        tracing::warn!(%error, "invalid theme preference; using the default palette");
        T::default()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Settings, ThemeChoice};

    #[test]
    fn old_settings_keep_the_mode_and_original_palettes() {
        for mode in ["system", "light", "dark"] {
            let settings: Settings = serde_json::from_value(serde_json::json!({
                "theme": mode, "ui_font_size": 19, "future_setting": true
            }))
            .unwrap();
            assert_eq!(serde_json::to_value(settings.theme).unwrap(), mode);
            assert_eq!(settings.light_theme(), LightTheme::Hive);
            assert_eq!(settings.dark_theme(), DarkTheme::Hive);
            assert_eq!(settings.ui_font_size(), 19.);
        }
    }

    #[test]
    fn invalid_palette_resets_only_its_own_preference() {
        for invalid in serde_json::json!([null, 42, false, [], {}, "future-theme"])
            .as_array()
            .unwrap()
        {
            let settings: Settings = serde_json::from_value(serde_json::json!({
                "theme": "dark", "light_theme": invalid, "dark_theme": "dracula",
                "ui_font_size": 19
            }))
            .unwrap();
            assert_eq!(settings.light_theme(), LightTheme::Hive);
            assert_eq!(settings.dark_theme(), DarkTheme::Dracula);
            assert_eq!(settings.theme, ThemeChoice::Dark);
            assert_eq!(settings.ui_font_size(), 19.);

            let settings: Settings = serde_json::from_value(serde_json::json!({
                "light_theme": "solarized", "dark_theme": invalid
            }))
            .unwrap();
            assert_eq!(settings.light_theme(), LightTheme::Solarized);
            assert_eq!(settings.dark_theme(), DarkTheme::Hive);
        }
    }

    #[test]
    fn palette_choices_are_independent_and_round_trip_without_changing_system() {
        let mut settings = Settings::default();
        for light in LightTheme::ALL {
            settings.set_light_theme(light);
            for dark in DarkTheme::ALL {
                settings.set_dark_theme(dark);
                let saved = serde_json::to_vec(&settings).unwrap();
                let loaded: Settings = serde_json::from_slice(&saved).unwrap();
                assert_eq!(loaded.theme, ThemeChoice::System);
                assert_eq!(loaded.light_theme(), light);
                assert_eq!(loaded.dark_theme(), dark);
            }
        }
    }
}
