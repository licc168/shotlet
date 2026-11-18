use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Mutex;

#[derive(Clone)]
pub struct OverlayPlace {
    pub label: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub point_width: u32,
    pub point_height: u32,
    pub generation: u64,
}

use crate::capture::FrozenCapture;
use crate::record::Recorder;
use crate::settings::Settings;

pub struct EditorSession {
    pub record_id: Option<String>,
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub document: String,
}

pub struct AppState {
    pub settings: Mutex<Settings>,
    pub capture: Mutex<Option<FrozenCapture>>,
    pub editor: Mutex<Option<EditorSession>>,
    pub pins: Mutex<HashMap<String, String>>,
    pub pin_groups: Mutex<HashMap<String, u8>>,
    pub pin_group: Mutex<u8>,
    pub pins_hidden: Mutex<bool>,
    pub recorder: Mutex<Option<Recorder>>,
    pub overlay_follow: AtomicBool,
    pub overlay_epoch: AtomicU64,
    pub overlay_generation: AtomicU64,
    pub overlay_aim: Mutex<Option<(i32, i32)>>,
    pub overlay_pending: Mutex<Vec<OverlayPlace>>,
}

impl AppState {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings: Mutex::new(settings),
            capture: Mutex::new(None),
            editor: Mutex::new(None),
            pins: Mutex::new(HashMap::new()),
            pin_groups: Mutex::new(HashMap::new()),
            pin_group: Mutex::new(0),
            pins_hidden: Mutex::new(false),
            recorder: Mutex::new(None),
            overlay_follow: AtomicBool::new(false),
            overlay_epoch: AtomicU64::new(1),
            overlay_generation: AtomicU64::new(1),
            overlay_aim: Mutex::new(None),
            overlay_pending: Mutex::new(Vec::new()),
        }
    }
}
