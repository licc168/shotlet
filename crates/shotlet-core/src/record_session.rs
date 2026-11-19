use std::path::PathBuf;

use serde::Serialize;

use crate::record::{self, Recorder};
use crate::state::AppState;
use crate::util::err;

#[derive(Clone, Copy, Debug)]
pub struct RecordBox {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordStatus {
    pub running: bool,
    pub paused: bool,
    pub done: bool,
    pub ready: bool,
    pub elapsed_ms: u64,
    pub limit_ms: u64,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub message: String,
}

/// Checks that must pass before the shell hides the overlay.
pub fn plan(state: &AppState, x: f64, y: f64, width: f64, height: f64, format: &str) -> Result<RecordBox, String> {
    if state.recorder.lock().map_err(err)?.is_some() {
        return Err("已经有一段录制在进行".into());
    }
    let area = box_of(x, y, width, height)?;
    if format == "mp4" && !record::ffmpeg_available() {
        return Err("没有找到 ffmpeg，请改用 GIF 或先安装 ffmpeg".into());
    }
    Ok(area)
}

pub fn place(state: &AppState, x: f64, y: f64, width: f64, height: f64) -> Result<RecordBox, String> {
    let area = box_of(x, y, width, height)?;
    if state.recorder.lock().map_err(err)?.is_some() {
        return Err("已经有一段录制在进行".into());
    }
    *state.pending_record.lock().map_err(err)? = Some((area.x, area.y, area.width, area.height));
    Ok(area)
}

pub fn clear_pending(state: &AppState) {
    if let Ok(mut slot) = state.pending_record.lock() {
        *slot = None;
    }
}

pub fn begin(state: &AppState, format: &str, audio: &str, cursor: bool) -> Result<Recorder, String> {
    let area = state.pending_record.lock().map_err(err)?.take().ok_or("没有待开始的录制")?;
    let area = RecordBox { x: area.0, y: area.1, width: area.2, height: area.3 };
    if format == "mp4" && !record::ffmpeg_available() {
        *state.pending_record.lock().map_err(err)? = Some((area.x, area.y, area.width, area.height));
        return Err("没有找到 ffmpeg，请改用 GIF 或先安装 ffmpeg".into());
    }
    match record::start(area.x, area.y, area.width, area.height, format, audio, cursor) {
        Ok(recorder) => Ok(recorder),
        Err(error) => {
            if let Ok(mut slot) = state.pending_record.lock() {
                *slot = Some((area.x, area.y, area.width, area.height));
            }
            Err(error)
        }
    }
}

pub fn set_paused(state: &AppState, paused: bool) -> Result<(), String> {
    let recorder = state.recorder.lock().map_err(err)?;
    let recorder = recorder.as_ref().ok_or("没有正在进行的录制")?;
    record::set_paused(recorder, paused);
    Ok(())
}

fn box_of(x: f64, y: f64, width: f64, height: f64) -> Result<RecordBox, String> {
    let x = x.round() as i32;
    let y = y.round() as i32;
    let width = width.round().max(1.0) as u32;
    let height = height.round().max(1.0) as u32;
    if width < 8 || height < 8 {
        return Err("录制选区太小".into());
    }
    Ok(RecordBox { x, y, width, height })
}

pub fn store(state: &AppState, recorder: Recorder) -> Result<(), String> {
    *state.recorder.lock().map_err(err)? = Some(recorder);
    Ok(())
}

pub fn take(state: &AppState) -> Result<Option<Recorder>, String> {
    Ok(state.recorder.lock().map_err(err)?.take())
}

pub fn cancel_running(state: &AppState) {
    if let Ok(mut slot) = state.recorder.lock() {
        if let Some(mut recorder) = slot.take() {
            record::cancel(&mut recorder);
        }
    }
}

pub fn finish(recorder: &mut Recorder) -> Result<PathBuf, String> {
    record::finish(recorder)
}

pub fn status(state: &AppState) -> Result<RecordStatus, String> {
    let recorder = state.recorder.lock().map_err(err)?;
    if let Some(recorder) = recorder.as_ref() {
        let done = recorder.is_done();
        return Ok(RecordStatus {
            running: !done,
            paused: recorder.is_paused(),
            done,
            ready: false,
            elapsed_ms: recorder.elapsed_ms(),
            limit_ms: record::limit_ms(),
            width: recorder.width(),
            height: recorder.height(),
            frames: recorder.frame_count(),
            message: recorder.message(),
        });
    }
    drop(recorder);
    let pending = state.pending_record.lock().map_err(err)?;
    Ok(match pending.as_ref() {
        Some((_, _, width, height)) => RecordStatus {
            running: false,
            paused: false,
            done: false,
            ready: true,
            elapsed_ms: 0,
            limit_ms: record::limit_ms(),
            width: *width,
            height: *height,
            frames: 0,
            message: String::new(),
        },
        None => RecordStatus {
            running: false,
            paused: false,
            done: false,
            ready: false,
            elapsed_ms: 0,
            limit_ms: record::limit_ms(),
            width: 0,
            height: 0,
            frames: 0,
            message: String::new(),
        },
    })
}

pub fn ffmpeg_available() -> bool {
    record::ffmpeg_available()
}

#[cfg(test)]
mod tests {
    use super::{clear_pending, place, plan};
    use crate::record;
    use crate::settings::Settings;
    use crate::state::AppState;

    fn state() -> AppState {
        AppState::new(Settings::default())
    }

    #[test]
    fn place_keeps_the_box_ready_without_starting() {
        let state = state();
        let area = place(&state, 1.2, 3.8, 20.2, 30.4).unwrap();
        assert_eq!((area.x, area.y, area.width, area.height), (1, 4, 20, 30));
        let status = super::status(&state).unwrap();
        assert!(status.ready);
        assert!(!status.running);
        assert_eq!((status.width, status.height), (20, 30));
        clear_pending(&state);
        assert!(!super::status(&state).unwrap().ready);
    }

    #[test]
    fn plan_rounds_the_box_and_rejects_a_tiny_one() {
        assert_eq!(plan(&state(), 1.6, -0.2, 7.4, 100.0, "gif").unwrap_err(), "录制选区太小");
        let area = plan(&state(), 1.6, -0.2, 8.2, 9.0, "gif").unwrap();
        assert_eq!((area.x, area.y, area.width, area.height), (2, 0, 8, 9));
    }

    #[test]
    fn mp4_requires_ffmpeg_before_anything_is_hidden() {
        let result = plan(&state(), 0.0, 0.0, 32.0, 32.0, "mp4");
        if record::ffmpeg_available() {
            assert!(result.is_ok());
        } else {
            assert_eq!(result.unwrap_err(), "没有找到 ffmpeg，请改用 GIF 或先安装 ffmpeg");
        }
    }
}
