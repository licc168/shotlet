//! No shared-buffer handoff. macOS and other platforms keep the existing
//! `preview_rgba` reply. This file is `preview_buffer` outside Windows.

use tauri::WebviewWindow;

pub fn post(_window: &WebviewWindow, _path: &str, _slot: usize, _generation: u64) {}
