//! One record per pin window. The shell places the window. This module does not.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, MutexGuard};

use image::RgbaImage;

pub type PinImage = Arc<RgbaImage>;

use crate::state::AppState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinPhase {
    Warming,
    Standby,
    Assigned,
    Shown,
}

/// The cut on the desktop, in the same pixels the window position uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinCut {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Outer window rectangle the shell last asked for, including the glow margin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinFrame {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

pub struct PinSlot {
    pub phase: PinPhase,
    pub group: u8,
    pub image: Option<Arc<RgbaImage>>,
    pub file: Option<PathBuf>,
    pub cut: Option<PinCut>,
    pub frame: Option<PinFrame>,
}

#[derive(Default)]
pub struct PinBook {
    slots: HashMap<String, PinSlot>,
}

fn book(state: &AppState) -> MutexGuard<'_, PinBook> {
    state.pins.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn label_of(id: &str) -> String {
    if id.starts_with("pin-") {
        id.to_string()
    } else {
        format!("pin-{id}")
    }
}

fn id_of(label: &str) -> &str {
    label.strip_prefix("pin-").unwrap_or(label)
}

fn blank() -> PinSlot {
    PinSlot {
        phase: PinPhase::Warming,
        group: 0,
        image: None,
        file: None,
        cut: None,
        frame: None,
    }
}

pub fn warm(state: &AppState, label: &str) {
    book(state).slots.entry(label.to_string()).or_insert_with(blank);
}

/// Only a window that is still warming becomes standby. A second report does nothing.
pub fn report_standby(state: &AppState, label: &str) -> bool {
    let mut book = book(state);
    let Some(slot) = book.slots.get_mut(label) else {
        return false;
    };
    if slot.phase != PinPhase::Warming {
        return false;
    }
    slot.phase = PinPhase::Standby;
    true
}

/// Standby becomes assigned. A second take finds nothing.
pub fn take_standby(state: &AppState) -> Option<String> {
    let mut book = book(state);
    let label = book
        .slots
        .iter()
        .find(|(_, slot)| slot.phase == PinPhase::Standby)
        .map(|(label, _)| label.clone())?;
    if let Some(slot) = book.slots.get_mut(&label) {
        slot.phase = PinPhase::Assigned;
    }
    Some(label)
}

/// The first time an assigned window is shown. Later calls stay false.
pub fn mark_shown(state: &AppState, label: &str) -> bool {
    let mut book = book(state);
    let Some(slot) = book.slots.get_mut(label) else {
        return false;
    };
    if slot.phase != PinPhase::Assigned {
        return false;
    }
    slot.phase = PinPhase::Shown;
    true
}

pub fn forget(state: &AppState, label: &str) {
    book(state).slots.remove(label);
}

pub fn needs_spare(state: &AppState) -> bool {
    !book(state).slots.values().any(|slot| matches!(slot.phase, PinPhase::Warming | PinPhase::Standby))
}

pub fn assign(state: &AppState, label: &str, group: u8, image: Option<Arc<RgbaImage>>, file: Option<PathBuf>, cut: PinCut) {
    let mut slots = book(state);
    let slot = slots.slots.entry(label.to_string()).or_insert_with(blank);
    slot.phase = PinPhase::Assigned;
    slot.group = group;
    slot.image = image;
    slot.file = file;
    slot.cut = Some(cut);
}

pub fn note_frame(state: &AppState, label: &str, frame: PinFrame) {
    if let Some(slot) = book(state).slots.get_mut(label) {
        slot.frame = Some(frame);
    }
}

pub fn frame_of(state: &AppState, label: &str) -> Option<PinFrame> {
    book(state).slots.get(label).and_then(|slot| slot.frame)
}

pub fn content_size(state: &AppState, label: &str) -> Option<(u32, u32)> {
    book(state).slots.get(label).and_then(|slot| slot.cut.map(|cut| (cut.width.max(1), cut.height.max(1))))
}

pub fn file_path(state: &AppState, id: &str) -> Result<String, String> {
    book(state)
        .slots
        .get(&label_of(id))
        .and_then(|slot| slot.file.as_ref())
        .map(|path| path.to_string_lossy().to_string())
        .filter(|path| !path.is_empty())
        .ok_or_else(|| "贴图已经失效".into())
}

pub fn image_of(state: &AppState, id: &str) -> Option<Arc<RgbaImage>> {
    book(state).slots.get(&label_of(id)).and_then(|slot| slot.image.clone())
}

pub fn set_file(state: &AppState, label: &str, file: PathBuf) {
    if let Some(slot) = book(state).slots.get_mut(label) {
        slot.file = Some(file);
    }
}

pub fn release_image(state: &AppState, label: &str) {
    if let Some(slot) = book(state).slots.get_mut(label) {
        slot.image = None;
    }
}

/// Ids of windows that already have a picture, without the `pin-` prefix.
pub fn groups(state: &AppState) -> HashMap<String, u8> {
    book(state)
        .slots
        .iter()
        .filter(|(_, slot)| matches!(slot.phase, PinPhase::Assigned | PinPhase::Shown))
        .map(|(label, slot)| (id_of(label).to_string(), slot.group))
        .collect()
}

/// Window rectangle centered on `center`, grown from the scale-1 cut.
pub fn zoomed_frame(content_w: u32, content_h: u32, scale: f64, center_x: i32, center_y: i32, glow: i32) -> PinFrame {
    let scale = if scale.is_finite() { scale.clamp(0.2, 4.0) } else { 1.0 };
    let width = ((content_w.max(1) as f64) * scale).round().max(1.0) as i32 + glow.saturating_mul(2);
    let height = ((content_h.max(1) as f64) * scale).round().max(1.0) as i32 + glow.saturating_mul(2);
    PinFrame {
        x: center_x - width / 2,
        y: center_y - height / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use image::{Rgba, RgbaImage};

    use super::{assign, forget, file_path, image_of, mark_shown, report_standby, take_standby, warm, zoomed_frame, PinCut};
    use crate::settings::Settings;
    use crate::state::AppState;

    fn state() -> AppState {
        AppState::new(Settings::default())
    }

    #[test]
    fn three_reports_leave_one_standby_and_the_next_take_is_empty() {
        let state = state();
        warm(&state, "pin-a");
        assert!(report_standby(&state, "pin-a"));
        assert!(!report_standby(&state, "pin-a"));
        assert!(!report_standby(&state, "pin-a"));
        assert_eq!(take_standby(&state).as_deref(), Some("pin-a"));
        assert_eq!(take_standby(&state), None);
    }

    #[test]
    fn an_assigned_window_does_not_go_back_to_standby() {
        let state = state();
        warm(&state, "pin-a");
        assert!(report_standby(&state, "pin-a"));
        assert!(take_standby(&state).is_some());
        assert!(!report_standby(&state, "pin-a"));
        assert_eq!(take_standby(&state), None);
    }

    #[test]
    fn mark_shown_is_only_true_the_first_time() {
        let state = state();
        warm(&state, "pin-a");
        report_standby(&state, "pin-a");
        take_standby(&state);
        assert!(mark_shown(&state, "pin-a"));
        assert!(!mark_shown(&state, "pin-a"));
    }

    #[test]
    fn forget_drops_the_pixels_and_the_file() {
        let state = state();
        let image = Arc::new(RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 255])));
        assign(
            &state,
            "pin-a",
            1,
            Some(image),
            Some(PathBuf::from("pin.png")),
            PinCut { x: 4, y: 5, width: 6, height: 7 },
        );
        assert_eq!(file_path(&state, "a").unwrap(), "pin.png");
        assert!(image_of(&state, "a").is_some());
        forget(&state, "pin-a");
        assert!(file_path(&state, "a").is_err());
        assert!(image_of(&state, "pin-a").is_none());
    }

    #[test]
    fn zoom_grows_around_the_current_center() {
        let frame = zoomed_frame(100, 50, 2.0, 200, 100, 22);
        assert_eq!((frame.width, frame.height), (244, 144));
        assert_eq!(frame.x + frame.width / 2, 200);
        assert_eq!(frame.y + frame.height / 2, 100);
    }
}
