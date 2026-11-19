pub use crate::settings::Settings;

use crate::state::AppState;
use crate::util::err;

pub fn values(settings: &Settings) -> [&str; 8] {
    [
        settings.capture_shortcut.as_str(),
        settings.capture_copy_shortcut.as_str(),
        settings.window_shortcut.as_str(),
        settings.pin_shortcut.as_str(),
        settings.hide_pins_shortcut.as_str(),
        settings.pin_group_shortcut.as_str(),
        settings.history_shortcut.as_str(),
        settings.copy_latest_shortcut.as_str(),
    ]
}

pub fn normalize(shortcut: &str) -> String {
    let mut modifiers = Vec::new();
    let mut key = String::new();
    for token in shortcut.split('+').map(str::trim).filter(|token| !token.is_empty()) {
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "cmdorcontrol" | "commandorcontrol" => modifiers.push("control"),
            "alt" | "option" => modifiers.push("alt"),
            "shift" => modifiers.push("shift"),
            "super" | "meta" | "win" | "command" | "cmd" | "windows" => modifiers.push("super"),
            _ => key = token.to_ascii_lowercase(),
        }
    }
    const ORDER: [&str; 4] = ["shift", "control", "alt", "super"];
    modifiers.sort_by_key(|name| ORDER.iter().position(|item| item == name).unwrap_or(ORDER.len()));
    modifiers.dedup();
    if key.is_empty() {
        modifiers.join("+")
    } else if modifiers.is_empty() {
        key
    } else {
        format!("{}+{key}", modifiers.join("+"))
    }
}

pub fn same(normalized: &str, configured: &str) -> bool {
    !configured.trim().is_empty() && normalized == normalize(configured)
}

pub fn reject_duplicates(settings: &Settings) -> Result<(), String> {
    let enabled = values(settings);
    for (index, left) in enabled.iter().enumerate() {
        if left.trim().is_empty() {
            continue;
        }
        for right in enabled.iter().skip(index + 1) {
            if !right.trim().is_empty() && normalize(left) == normalize(right) {
                return Err("快捷键不能重复".into());
            }
        }
    }
    Ok(())
}

pub fn remember(state: &AppState, settings: Settings) -> Result<(), String> {
    *state.settings.lock().map_err(err)? = settings;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn shortcut_normalize_ignores_modifier_order_and_case() {
        assert_eq!(normalize("Control+F1"), normalize("control+f1"));
        assert_eq!(normalize("Control+Shift+F1"), normalize("shift+control+F1"));
        assert_eq!(normalize("F3"), normalize("f3"));
        assert_eq!(normalize("Super+Shift+KeyH"), normalize("shift+super+KeyH"));
        assert_ne!(normalize("F1"), normalize("Control+F1"));
    }

    #[test]
    fn duplicate_shortcuts_are_rejected() {
        let mut settings = crate::settings::Settings::default();
        settings.capture_copy_shortcut = "f1".into();
        assert_eq!(super::reject_duplicates(&settings).unwrap_err(), "快捷键不能重复");
        settings.capture_copy_shortcut = "Control+F1".into();
        assert!(super::reject_duplicates(&settings).is_ok());
    }
}
