//! Connection-free data the shell and the engines share.
//! This crate must not grow a screen grabber, a file store, or a window handle.

use serde::{Deserialize, Serialize};

pub const DEFAULT_CAPTURE_SHORTCUT: &str = "F1";
pub const DEFAULT_CAPTURE_COPY_SHORTCUT: &str = "Control+F1";
pub const DEFAULT_WINDOW_SHORTCUT: &str = "Shift+F1";
pub const DEFAULT_PIN_SHORTCUT: &str = "F3";
pub const DEFAULT_HIDE_PINS_SHORTCUT: &str = "Shift+F3";
pub const DEFAULT_PIN_GROUP_SHORTCUT: &str = "Control+F3";
pub const DEFAULT_HISTORY_SHORTCUT: &str = "Super+Shift+KeyH";

fn default_capture() -> String { DEFAULT_CAPTURE_SHORTCUT.into() }
fn default_capture_copy() -> String { DEFAULT_CAPTURE_COPY_SHORTCUT.into() }
fn default_window() -> String { DEFAULT_WINDOW_SHORTCUT.into() }
fn default_pin() -> String { DEFAULT_PIN_SHORTCUT.into() }
fn default_hide_pins() -> String { DEFAULT_HIDE_PINS_SHORTCUT.into() }
fn default_pin_group() -> String { DEFAULT_PIN_GROUP_SHORTCUT.into() }
fn default_history() -> String { DEFAULT_HISTORY_SHORTCUT.into() }

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowInfo {
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub z: u32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

pub fn default_mode() -> String {
    "edit".into()
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenImage {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Logical points on macOS, physical pixels on Windows. Window placement uses this, not `width`.
    #[serde(default)]
    pub point_width: u32,
    #[serde(default)]
    pub point_height: u32,
    pub path: String,
    pub screen_x: i32,
    pub screen_y: i32,
    /// Window rects in this screen's local coordinates. `x` above is the packed crop origin, not a window origin.
    #[serde(default)]
    pub windows: Vec<WindowInfo>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrozenCapture {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub origin_x: i32,
    pub origin_y: i32,
    pub windows: Vec<WindowInfo>,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub preset: Option<PresetRect>,
    #[serde(default)]
    pub screens: Vec<ScreenImage>,
    #[serde(default)]
    pub generation: u64,
}

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

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: String,
    pub created_at: i64,
    pub name: String,
    pub tags: Vec<String>,
    pub favorite: bool,
    pub width: u32,
    pub height: u32,
    pub ocr_text: String,
    pub file: String,
    pub base_file: String,
    pub document: String,
    #[serde(default)]
    pub deleted_at: Option<i64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub id: String,
    pub created_at: i64,
    pub name: String,
    pub tags: Vec<String>,
    pub favorite: bool,
    pub width: u32,
    pub height: u32,
    pub ocr_text: String,
    pub file: String,
    pub deleted: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRequest {
    pub png_base64: String,
    pub base_png_base64: String,
    pub document: String,
    pub ocr_text: String,
    pub width: u32,
    pub height: u32,
    pub name: String,
}
