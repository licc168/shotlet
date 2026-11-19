use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::capture;
use crate::state::AppState;
use crate::util::err;

pub use crate::capture::{FreezeShot, FrozenCapture, LaterMonitor, ScreenImage};

pub struct StagedCapture {
    pub frozen: FrozenCapture,
    pub follow: bool,
    pub generation: u64,
}

pub fn begin(state: &AppState) -> u64 {
    state.overlay_follow.store(false, Ordering::Relaxed);
    state.overlay_epoch.fetch_add(1, Ordering::Relaxed) + 1
}

pub fn is_current(state: &AppState, epoch: u64) -> bool {
    state.overlay_epoch.load(Ordering::Relaxed) == epoch
}

/// Freeze the desktop after the shell has hidden the overlay.
/// `wait_for_hide` is set only when an overlay was actually on screen.
pub fn grab(mode: &str, wait_for_hide: bool) -> Result<FreezeShot, String> {
    if wait_for_hide {
        // The previous overlay must leave the screen before the pixels are frozen.
        std::thread::sleep(Duration::from_millis(60));
    }
    let mut shot = capture::freeze_under_cursor()?;
    match mode {
        "copy" => shot.frozen.mode = "copy".into(),
        "window" => {
            shot.frozen.preset = capture::window_under_cursor(shot.frozen.origin_x, shot.frozen.origin_y, &shot.frozen.windows);
            shot.frozen.mode = "edit".into();
        }
        _ => shot.frozen.mode = "edit".into(),
    }
    Ok(shot)
}

/// Grab the monitors left after the cursor screen. The packed origin continues
/// after that screen; the shell does not place these pixels.
pub fn capture_remaining(cursor: &ScreenImage, later: &[LaterMonitor]) -> Vec<ScreenImage> {
    let packed = cursor.x.saturating_add(cursor.width as i32);
    capture::capture_later(later, packed)
}

/// Add monitors that were grabbed after the cursor screen was shown.
/// A newer F1 drops them.
pub fn append_screens(state: &AppState, epoch: u64, generation: u64, extra: Vec<ScreenImage>) -> Option<(FrozenCapture, usize)> {
    if extra.is_empty() || !is_current(state, epoch) {
        return None;
    }
    let mut slot = state.capture.lock().map_err(err).ok()?;
    let frozen = slot.as_mut()?;
    if frozen.generation != generation {
        return None;
    }
    let from = frozen.screens.len();
    frozen.screens.extend(extra);
    Some((frozen.clone(), from))
}

/// Accept the shot when this launch is still the latest one.
/// Follow stays off until the shell has shown the overlay windows.
pub fn stage(state: &AppState, epoch: u64, mut frozen: FrozenCapture, mode: &str) -> Result<Option<StagedCapture>, String> {
    if !is_current(state, epoch) {
        return Ok(None);
    }
    let generation = state.overlay_generation.fetch_add(1, Ordering::Relaxed) + 1;
    frozen.generation = generation;
    let follow = mode != "window" || frozen.preset.is_none();
    state.capture.lock().map(|mut slot| *slot = Some(frozen.clone())).map_err(err)?;
    Ok(Some(StagedCapture { frozen, follow, generation }))
}

/// Hand out the signal the rest of the monitors wait on.
/// A newer launch replaces it, so a late signal from the previous one is ignored.
pub fn watch_shown(state: &AppState, generation: u64) -> std::sync::mpsc::Receiver<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    if let Ok(mut slot) = state.overlay_shown.lock() {
        *slot = Some((generation, tx));
    }
    rx
}

/// The cursor screen of this generation is on screen. Waiting monitors may start.
pub fn note_shown(state: &AppState, generation: u64) {
    let signal = state.overlay_shown.lock().ok().and_then(|mut slot| match slot.as_ref() {
        Some((shown, _)) if *shown == generation => slot.take().map(|(_, tx)| tx),
        _ => None,
    });
    if let Some(tx) = signal {
        let _ = tx.send(());
    }
}

pub fn arm_follow(state: &AppState, follow: bool) {
    state.overlay_follow.store(follow, Ordering::Relaxed);
}

pub fn flush_frozen() {
    capture::flush_frozen();
}

pub fn preview_bytes(path: &str) -> Result<Vec<u8>, String> {
    capture::preview_bytes(path)
}

pub fn current(state: &AppState) -> Result<Option<FrozenCapture>, String> {
    Ok(state.capture.lock().map_err(err)?.clone())
}

pub fn prepare_access() {
    #[cfg(target_os = "macos")]
    capture::prepare_capture_access();
}

/// Bitmap pixels of a selection, turned into the monitor's desktop rectangle.
/// `point_width / bitmap_width` is the scale. Pin, record, and scroll share this.
pub fn desktop_rect(
    screen_x: i32,
    screen_y: i32,
    local_x: i32,
    local_y: i32,
    local_w: u32,
    local_h: u32,
    bitmap_w: u32,
    bitmap_h: u32,
    point_w: u32,
    point_h: u32,
) -> (i32, i32, u32, u32) {
    let local_x = local_x.max(0) as u32;
    let local_y = local_y.max(0) as u32;
    let point_w = if point_w == 0 { bitmap_w } else { point_w };
    let point_h = if point_h == 0 { bitmap_h } else { point_h };
    (
        screen_x.saturating_add(scale_len(local_x, bitmap_w, point_w) as i32),
        screen_y.saturating_add(scale_len(local_y, bitmap_h, point_h) as i32),
        scale_len(local_w, bitmap_w, point_w).max(1),
        scale_len(local_h, bitmap_h, point_h).max(1),
    )
}

fn scale_len(value: u32, from: u32, to: u32) -> u32 {
    if from == 0 || to == 0 || from == to {
        return value;
    }
    ((u64::from(value) * u64::from(to) + u64::from(from) / 2) / u64::from(from)) as u32
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::{append_screens, arm_follow, begin, desktop_rect, grab, is_current, stage};
    use crate::capture::ScreenImage;
    use crate::capture::{FrozenCapture, PresetRect};
    use crate::settings::Settings;
    use crate::state::AppState;

    fn state() -> AppState {
        AppState::new(Settings::default())
    }

    fn shot(preset: Option<PresetRect>) -> FrozenCapture {
        FrozenCapture {
            path: "shot.png".into(),
            width: 10,
            height: 10,
            origin_x: 0,
            origin_y: 0,
            windows: Vec::new(),
            mode: "edit".into(),
            preset,
            screens: Vec::new(),
            generation: 0,
        }
    }

    #[test]
    fn a_cancelled_launch_drops_the_shot_and_leaves_follow_off() {
        let state = state();
        state.overlay_follow.store(true, Ordering::Relaxed);
        let cancelled = begin(&state);
        assert!(!state.overlay_follow.load(Ordering::Relaxed));
        let current = begin(&state);
        assert!(!is_current(&state, cancelled));
        assert!(stage(&state, cancelled, shot(None), "edit").unwrap().is_none());
        assert!(state.capture.lock().unwrap().is_none());
        assert_eq!(state.overlay_generation.load(Ordering::Relaxed), 1);
        assert!(!state.overlay_follow.load(Ordering::Relaxed));

        let Some(staged) = stage(&state, current, shot(None), "copy").unwrap() else {
            panic!("stage dropped the current launch");
        };
        assert_eq!(staged.frozen.path, "shot.png");
        assert_eq!(staged.generation, 2);
        assert!(staged.follow);
        assert!(state.capture.lock().unwrap().is_some());
        assert!(!state.overlay_follow.load(Ordering::Relaxed));
        arm_follow(&state, staged.follow);
        assert!(state.overlay_follow.load(Ordering::Relaxed));
    }

    #[test]
    fn window_mode_follows_only_when_no_window_was_found() {
        let state = state();
        let epoch = begin(&state);
        let Some(open) = stage(&state, epoch, shot(None), "window").unwrap() else {
            panic!("stage dropped the current launch");
        };
        assert!(open.follow);

        let epoch = begin(&state);
        let preset = Some(PresetRect { x: 1, y: 2, w: 3, h: 4 });
        let Some(framed) = stage(&state, epoch, shot(preset), "window").unwrap() else {
            panic!("stage dropped the current launch");
        };
        assert!(!framed.follow);
    }

    #[test]
    fn a_newer_launch_drops_screens_from_the_previous_one() {
        let state = state();
        let epoch = begin(&state);
        let Some(staged) = stage(&state, epoch, shot(None), "edit").unwrap() else {
            panic!("stage dropped the current launch");
        };
        let newer = begin(&state);
        let extra = vec![ScreenImage {
            x: 10,
            y: 0,
            width: 2,
            height: 2,
            point_width: 2,
            point_height: 2,
            path: "other".into(),
            screen_x: 10,
            screen_y: 0,
            windows: Vec::new(),
        }];
        assert!(append_screens(&state, epoch, staged.generation, extra.clone()).is_none());
        assert!(is_current(&state, newer));
        assert!(state.capture.lock().unwrap().as_ref().unwrap().screens.is_empty());

        let Some(open) = stage(&state, newer, shot(None), "edit").unwrap() else {
            panic!("stage dropped the current launch");
        };
        let Some((frozen, from)) = append_screens(&state, newer, open.generation, extra) else {
            panic!("append dropped the current launch");
        };
        assert_eq!(from, 0);
        assert_eq!(frozen.screens.len(), 1);
        assert_eq!(frozen.screens[0].path, "other");
    }

    #[test]
    fn desktop_rect_scales_bitmap_pixels_into_the_monitor_slot() {
        assert_eq!(desktop_rect(1920, 0, 100, 50, 200, 100, 2000, 1000, 1000, 500), (1970, 25, 100, 50));
        assert_eq!(desktop_rect(-1920, 10, 0, 0, 40, 40, 800, 600, 0, 0), (-1920, 10, 40, 40));
    }

    #[test]
    #[ignore = "captures the desktop"]
    fn grab_freezes_the_monitor_under_the_cursor() {
        for mode in ["edit", "copy", "window"] {
            let shot = grab(mode, false).unwrap_or_else(|error| panic!("{mode}: {error}"));
            let frozen = &shot.frozen;
            assert!(frozen.width >= 8 && frozen.height >= 8, "{mode} {}x{}", frozen.width, frozen.height);
            assert_eq!(frozen.mode, if mode == "copy" { "copy" } else { "edit" });
            assert_eq!(frozen.screens.len(), 1, "{mode} should show the cursor screen first");
            let cursor = &frozen.screens[0];
            let extra = super::capture_remaining(cursor, &shot.later);
            if mode == "window" {
                let found = frozen.preset.is_some();
                eprintln!("window preset found: {found}");
            }
            for screen in frozen.screens.iter().chain(&extra) {
                wait_frozen_file(&screen.path);
                let _ = std::fs::remove_file(&screen.path);
            }
        }
    }

    fn wait_frozen_file(path: &str) {
        let file = std::path::Path::new(path);
        for _ in 0..100 {
            if file.is_file() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("missing {path}");
    }
}
