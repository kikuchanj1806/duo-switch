//! The app's own preferences, in `~/.pitstop/app.json` beside the state.
//! Nothing secret goes here either.

use std::path::PathBuf;

use pitstop_core::error::Result;
use pitstop_core::Host;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// `en` or `vi`; none follows the system language.
    pub language: Option<String>,
    /// `light` or `dark`; none follows the system theme.
    pub theme: Option<String>,
    /// Tools whose "Add a second account" tip was dismissed.
    pub dismissed_tips: Vec<String>,
    /// Notify when the Codex account in use runs out and another has room.
    pub notify_run_out: bool,
    /// Look for a newer Duo Switch now and then, and say when there is one.
    pub check_updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            language: None,
            theme: None,
            dismissed_tips: Vec::new(),
            notify_run_out: true,
            check_updates: false,
        }
    }
}

fn path(host: &dyn Host) -> PathBuf {
    host.home().join(".pitstop").join("app.json")
}

/// The saved preferences; a missing or unreadable file is the defaults.
pub fn load(host: &dyn Host) -> Settings {
    host.read(&path(host))
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save(host: &dyn Host, settings: &Settings) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(settings).unwrap_or_default();
    host.write_atomic(&path(host), &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_older_app_json_keeps_its_values_and_drops_what_is_gone() {
        // Written while Claude Code's warm-ups and automatic switch were here.
        let old = br#"{"language":"vi","dismissedTips":["codex"],"autoSwitch":true,"warmUp":["ben"],"notifyRunOut":false}"#;
        let settings: Settings = serde_json::from_slice(old).unwrap();
        assert_eq!(settings.language.as_deref(), Some("vi"));
        assert_eq!(settings.dismissed_tips, vec!["codex".to_string()]);
        assert!(!settings.notify_run_out);
        assert!(!settings.check_updates);
    }

    #[test]
    fn settings_survive_a_save_and_load() {
        let settings = Settings {
            notify_run_out: false,
            theme: Some("dark".into()),
            ..Settings::default()
        };
        let json = serde_json::to_vec(&settings).unwrap();
        assert_eq!(serde_json::from_slice::<Settings>(&json).unwrap(), settings);
    }
}
