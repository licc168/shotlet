use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::capture;
use crate::state::AppState;
use crate::util::err;

pub use crate::capture::FrozenCapture;

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
pub fn grab(mode: &str) -> Result<FrozenCapture, String> {
    // The previous overlay must leave the screen before the pixels are frozen.
    std::thread::sleep(Duration::from_millis(60));
    let mut frozen = capture::freeze_under_cursor()?;
    match mode {
        "copy" => frozen.mode = "copy".into(),
        "window" => {
            frozen.preset = capture::window_under_cursor(frozen.origin_x, frozen.origin_y, &frozen.windows);
            frozen.mode = "edit".into();
        }
        _ => frozen.mode = "edit".into(),
    }
    Ok(frozen)
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

pub fn arm_follow(state: &AppState, follow: bool) {
    state.overlay_follow.store(follow, Ordering::Relaxed);
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::{arm_follow, begin, grab, is_current, stage};
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
    #[ignore = "captures the desktop"]
    fn grab_freezes_the_monitor_under_the_cursor() {
        for mode in ["edit", "copy", "window"] {
            let frozen = grab(mode).unwrap_or_else(|error| panic!("{mode}: {error}"));
            assert!(frozen.width >= 8 && frozen.height >= 8, "{mode} {}x{}", frozen.width, frozen.height);
            assert_eq!(frozen.mode, if mode == "copy" { "copy" } else { "edit" });
            assert!(std::path::Path::new(&frozen.path).is_file(), "{mode} missing {}", frozen.path);
            if mode == "window" {
                let found = frozen.preset.is_some();
                eprintln!("window preset found: {found}");
            }
            for screen in &frozen.screens {
                let _ = std::fs::remove_file(&screen.path);
            }
        }
    }
}
