use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::util::err;

fn default_capture() -> String { "F1".into() }
fn default_capture_copy() -> String { "Control+F1".into() }
fn default_window() -> String { "Shift+F1".into() }
fn default_pin() -> String { "F3".into() }
fn default_hide_pins() -> String { "Shift+F3".into() }
fn default_pin_group() -> String { "Control+F3".into() }
fn default_history() -> String { "Super+Shift+KeyH".into() }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub theme: String,
    pub language: String,
    #[serde(default = "default_capture")]
    pub capture_shortcut: String,
    #[serde(default = "default_capture_copy")]
    pub capture_copy_shortcut: String,
    #[serde(default = "default_window")]
    pub window_shortcut: String,
    #[serde(default = "default_pin")]
    pub pin_shortcut: String,
    #[serde(default = "default_hide_pins")]
    pub hide_pins_shortcut: String,
    #[serde(default = "default_pin_group")]
    pub pin_group_shortcut: String,
    #[serde(default = "default_history")]
    pub history_shortcut: String,
    #[serde(default)]
    pub copy_latest_shortcut: String,
    pub storage_dir: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "auto".into(),
            language: "auto".into(),
            capture_shortcut: default_capture(),
            capture_copy_shortcut: default_capture_copy(),
            window_shortcut: default_window(),
            pin_shortcut: default_pin(),
            hide_pins_shortcut: default_hide_pins(),
            pin_group_shortcut: default_pin_group(),
            history_shortcut: default_history(),
            copy_latest_shortcut: String::new(),
            storage_dir: String::new(),
        }
    }
}

pub fn settings_path(app_data: &Path) -> PathBuf {
    app_data.join("settings.json")
}

pub fn load(app_data: &Path) -> Settings {
    let path = settings_path(app_data);
    let Ok(text) = std::fs::read_to_string(path) else {
        return Settings::default();
    };
    let Ok(raw) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Settings::default();
    };
    let Ok(mut settings) = serde_json::from_value::<Settings>(raw.clone()) else {
        return Settings::default();
    };
    if raw.get("pinShortcut").is_none() && settings.capture_shortcut == "Super+Shift+KeyA" {
        settings.capture_shortcut = default_capture();
    }
    settings
}

pub fn save(app_data: &Path, settings: &Settings) -> Result<(), String> {
    std::fs::create_dir_all(app_data).map_err(err)?;
    let text = serde_json::to_string_pretty(settings).map_err(err)?;
    std::fs::write(settings_path(app_data), text).map_err(err)
}

pub fn storage_root(app_data: &Path, settings: &Settings) -> PathBuf {
    if settings.storage_dir.trim().is_empty() {
        app_data.join("history")
    } else {
        PathBuf::from(&settings.storage_dir)
    }
}

pub fn system_language() -> &'static str {
    #[cfg(windows)]
    {
        use windows::Win32::Globalization::GetUserDefaultLocaleName;
        let mut buf = [0u16; 85];
        let len = unsafe { GetUserDefaultLocaleName(&mut buf) };
        if len > 1 {
            let text = String::from_utf16_lossy(&buf[..(len as usize) - 1]).to_lowercase();
            if text.starts_with("zh") {
                return "zh";
            }
        }
        return "en";
    }
    #[cfg(not(windows))]
    {
        let lang = std::env::var("LC_ALL")
            .or_else(|_| std::env::var("LANG"))
            .unwrap_or_default()
            .to_lowercase();
        if lang.starts_with("zh") { "zh" } else { "en" }
    }
}

pub fn resolved_language(settings: &Settings) -> &'static str {
    match settings.language.as_str() {
        "zh" => "zh",
        "en" => "en",
        _ => system_language(),
    }
}
