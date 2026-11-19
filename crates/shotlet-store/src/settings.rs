use std::path::{Path, PathBuf};

pub use shotlet_types::Settings;
use shotlet_platform::err;
use shotlet_types::DEFAULT_CAPTURE_SHORTCUT;

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
        settings.capture_shortcut = DEFAULT_CAPTURE_SHORTCUT.into();
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
