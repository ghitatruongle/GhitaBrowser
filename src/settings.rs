//! Settings and Profile Customization for GhitaBrowser (Phase 24).
//! Implements dark/light themes, search engine configuration, and JSON profile settings persistence.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThemeMode {
    Light,
    Dark,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StartupBehavior {
    OpenNewTab,
    ContinueWhereLeftOff,
    OpenSpecificPages(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserSettings {
    pub theme: ThemeMode,
    pub default_search_engine: String,
    pub startup_behavior: StartupBehavior,
    pub clear_on_exit: bool,
    pub do_not_track: bool,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            default_search_engine: "Google".to_string(),
            startup_behavior: StartupBehavior::OpenNewTab,
            clear_on_exit: false,
            do_not_track: true,
        }
    }
}

impl BrowserSettings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| format!("Failed to serialize settings: {e}"))
    }

    pub fn from_json(json_str: &str) -> Result<Self, String> {
        serde_json::from_str(json_str).map_err(|e| format!("Failed to parse settings: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_serialization_round_trip() {
        let mut s = BrowserSettings::new();
        s.theme = ThemeMode::Dark;
        s.default_search_engine = "DuckDuckGo".to_string();

        let json = s.to_json().unwrap();
        assert!(json.contains("Dark"));
        assert!(json.contains("DuckDuckGo"));

        let loaded = BrowserSettings::from_json(&json).unwrap();
        assert_eq!(s, loaded);
    }
}
