use gif::{Encoder, Frame, Repeat};
use image::RgbaImage;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::record_audio::AudioCapture;
use crate::record_cursor::paint_cursor;
use shotlet_capture::capture_rect;
use shotlet_platform::{err, new_id};

const LIMIT: Duration = Duration::from_secs(60 * 60);
const GIF_INTERVAL: Duration = Duration::from_millis(100);
const MP4_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 30);

pub fn limit_ms() -> u64 {
    LIMIT.as_millis() as u64
}

pub struct Recorder {
    stop: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    frames: Arc<AtomicU32>,
    elapsed: Arc<AtomicU64>,
    width: u32,
    height: u32,
    thread: Option<JoinHandle<Result<PathBuf, String>>>,
    message: Arc<Mutex<String>>,
}

impl Recorder {
    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed.load(Ordering::Relaxed)
    }

    pub fn frame_count(&self) -> u32 {
        self.frames.load(Ordering::Relaxed)
    }

    pub fn message(&self) -> String {
        self.message.lock().map(|slot| slot.clone()).unwrap_or_default()
    }

    pub fn is_paused(&self) -> bool {
        self.pause.load(Ordering::Relaxed)
    }

    pub fn is_done(&self) -> bool {
        self.thread.as_ref().is_none_or(|thread| thread.is_finished())
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}

pub fn set_paused(recorder: &Recorder, paused: bool) {
    recorder.pause.store(paused, Ordering::Relaxed);
}

pub fn cancel(recorder: &mut Recorder) {
    // Release pairs with the capture thread's Acquire loads. On Apple Silicon a
    // relaxed store of `cancelled` can stay invisible after `stop` is already seen,
    // so a cancel would still mix and save the file.
    recorder.cancelled.store(true, Ordering::Release);
    recorder.stop.store(true, Ordering::Release);
    if let Some(thread) = recorder.thread.take() {
        let _ = thread.join();
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        cancel(self);
    }
}

pub fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

struct Request {
    format: String,
    audio: String,
    cursor: bool,
}

fn normalize_request(format: &str, audio: &str, cursor: bool) -> Result<Request, String> {
    if format != "gif" && format != "mp4" {
        return Err("未知的录制格式".into());
    }
    if audio != "none" && audio != "mic" && audio != "speaker" && audio != "both" {
        return Err("未知的声音来源".into());
    }
    let audio = if format == "gif" { "none".to_string() } else { audio.to_string() };
    Ok(Request { format: format.to_string(), audio, cursor })
}

pub fn fit_size(width: u32, height: u32, even: bool) -> Result<(u32, u32), String> {
    let width = if even { width & !1 } else { width };
    let height = if even { height & !1 } else { height };
    if width < 8 || height < 8 {
        return Err("录制选区太小".into());
    }
    if width > 7680 || height > 4320 {
        return Err("录制选区太大".into());
    }
    Ok((width, height))
}

pub fn start(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    format: &str,
    audio: &str,
    cursor: bool,
) -> Result<Recorder, String> {
    let request = normalize_request(format, audio, cursor)?;
    if request.format == "mp4" && !ffmpeg_available() {
        return Err("没有找到 ffmpeg，请改用 GIF 或先安装 ffmpeg".into());
    }
    let (width, height) = fit_size(width, height, request.format == "mp4")?;
    let stop = Arc::new(AtomicBool::new(false));
    let cancelled = Arc::new(AtomicBool::new(false));
    let pause = Arc::new(AtomicBool::new(false));
    let frames = Arc::new(AtomicU32::new(0));
    let elapsed = Arc::new(AtomicU64::new(0));
    let message = Arc::new(Mutex::new(String::new()));
    let stop_thread = stop.clone();
    let cancelled_thread = cancelled.clone();
    let pause_thread = pause.clone();
    let frames_thread = frames.clone();
    let elapsed_thread = elapsed.clone();
    let message_thread = message.clone();
    let thread = thread::spawn(move || {
        let result = record_loop(
            x,
            y,
            width,
            height,
            &request,
            &stop_thread,
            &cancelled_thread,
            &pause_thread,
            &frames_thread,
            &elapsed_thread,
            &message_thread,
        );
        if let Err(error) = &result {
            if let Ok(mut slot) = message_thread.lock() {
                *slot = error.clone();
            }
        }
        result
    });
    Ok(Recorder {
        stop,
        cancelled,
        pause,
        frames,
        elapsed,
        width,
        height,
        thread: Some(thread),
        message,
    })
}

pub fn finish(recorder: &mut Recorder) -> Result<PathBuf, String> {
    recorder.stop.store(true, Ordering::Release);
    let thread = recorder.thread.take().ok_or("录制线程已经结束")?;
    thread.join().map_err(|_| "录制线程异常退出".to_string())?
}

fn note(message: &Mutex<String>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Ok(mut slot) = message.lock() {
        if slot.is_empty() {
            *slot = text.to_string();
        }
    }
}

fn output_dir() -> Result<PathBuf, String> {
    let dir = dirs::video_dir()
        .or_else(dirs::document_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("Shotlet");
    std::fs::create_dir_all(&dir).map_err(err)?;
    Ok(dir)
}

fn record_loop(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    request: &Request,
    stop: &Arc<AtomicBool>,
    cancelled: &Arc<AtomicBool>,
    pause: &Arc<AtomicBool>,
    frames: &Arc<AtomicU32>,
    elapsed: &Arc<AtomicU64>,
    message: &Mutex<String>,
) -> Result<PathBuf, String> {
    let dir = output_dir()?;
    let id = new_id();
    let captures = start_audio(request, &dir, &id, pause, message);
    let expect_audio = !captures.is_empty();
    let video = if request.format == "mp4" {
        record_mp4(&dir, &id, x, y, width, height, request.cursor, expect_audio, stop, pause, frames, elapsed, message, cancelled)
    } else {
        record_gif(&dir, &id, x, y, width, height, request.cursor, stop, pause, frames, elapsed, message)
    };
    if cancelled.load(Ordering::Acquire) {
        if let Ok(path) = &video {
            let _ = std::fs::remove_file(path);
        }
        for wav in finish_audio(captures, message) {
            let _ = std::fs::remove_file(wav);
        }
        return Err("已取消".into());
    }
    let wavs = finish_audio(captures, message);
    let path = match video {
        Ok(path) => path,
        Err(error) => {
            for wav in &wavs {
                let _ = std::fs::remove_file(wav);
            }
            return Err(error);
        }
    };
    if wavs.is_empty() {
        return Ok(path);
    }
    let final_path = dir.join(format!("shotlet-{id}.mp4"));
    let muxed = mux_audio(&path, &wavs, &final_path);
    for wav in &wavs {
        let _ = std::fs::remove_file(wav);
    }
    match muxed {
        Ok(()) => {
            if path != final_path {
                let _ = std::fs::remove_file(&path);
            }
            Ok(final_path)
        }
        Err(error) => {
            let _ = std::fs::remove_file(&final_path);
            let kept = if path != final_path {
                let _ = std::fs::rename(&path, &final_path);
                final_path
            } else {
                path
            };
            Err(format!("{error}。无声视频已保存：{}", kept.display()))
        }
    }
}

fn start_audio(request: &Request, dir: &Path, id: &str, pause: &Arc<AtomicBool>, message: &Mutex<String>) -> Vec<AudioCapture> {
    let kinds: &[&str] = match request.audio.as_str() {
        "mic" => &["mic"],
        "speaker" => &["speaker"],
        "both" => &["mic", "speaker"],
        _ => &[],
    };
    let mut captures = Vec::new();
    for kind in kinds {
        let wav = dir.join(format!("shotlet-{id}-{kind}.wav"));
        match AudioCapture::start(kind, wav, pause.clone()) {
            Ok(capture) => captures.push(capture),
            Err(error) => note(message, &error),
        }
    }
    captures
}

fn finish_audio(captures: Vec<AudioCapture>, message: &Mutex<String>) -> Vec<PathBuf> {
    let mut wavs = Vec::new();
    for capture in captures {
        match capture.finish() {
            Ok(path) => wavs.push(path),
            Err(error) => note(message, &error),
        }
    }
    wavs
}

struct Pace<'a> {
    interval: Duration,
    stop: &'a AtomicBool,
    pause: &'a AtomicBool,
    frames: &'a AtomicU32,
    elapsed: &'a AtomicU64,
    active: Duration,
    misses: u32,
}

fn capture_until(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    cursor: bool,
    pace: &mut Pace<'_>,
    message: &Mutex<String>,
    mut write: impl FnMut(&RgbaImage) -> Result<(), String>,
) -> Result<(), String> {
    loop {
        if pace.stop.load(Ordering::Acquire) || pace.active >= LIMIT {
            return Ok(());
        }
        if pace.pause.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(30));
            continue;
        }
        let started = Instant::now();
        match grab_frame(x, y, width, height, cursor) {
            Ok(frame) => {
                pace.misses = 0;
                write(&frame)?;
                pace.frames.fetch_add(1, Ordering::Relaxed);
                pace.active += pace.interval;
                pace.elapsed.store(pace.active.as_millis() as u64, Ordering::Relaxed);
            }
            Err(error) => {
                pace.misses += 1;
                note(message, &error);
                if pace.misses >= 8 {
                    if pace.frames.load(Ordering::Relaxed) == 0 {
                        return Err(error);
                    }
                    return Ok(());
                }
            }
        }
        let spent = started.elapsed();
        if spent < pace.interval {
            thread::sleep(pace.interval - spent);
        }
    }
}

fn grab_frame(x: i32, y: i32, width: u32, height: u32, cursor: bool) -> Result<RgbaImage, String> {
    let mut shot = capture_rect(x, y, width, height)?;
    if cursor {
        paint_cursor(&mut shot, x, y);
    }
    Ok(shot)
}

fn record_gif(
    dir: &Path,
    id: &str,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    cursor: bool,
    stop: &Arc<AtomicBool>,
    pause: &Arc<AtomicBool>,
    frames: &Arc<AtomicU32>,
    elapsed: &Arc<AtomicU64>,
    message: &Mutex<String>,
) -> Result<PathBuf, String> {
    let path = dir.join(format!("shotlet-{id}.gif"));
    let file = std::fs::File::create(&path).map_err(err)?;
    let mut encoder = Encoder::new(BufWriter::new(file), width as u16, height as u16, &[]).map_err(err)?;
    encoder.set_repeat(Repeat::Infinite).map_err(err)?;
    let mut pace = Pace { interval: GIF_INTERVAL, stop, pause, frames, elapsed, active: Duration::ZERO, misses: 0 };
    let written = capture_until(x, y, width, height, cursor, &mut pace, message, |frame| {
        let mut pixels = frame.clone().into_raw();
        let mut gif_frame = Frame::from_rgba_speed(width as u16, height as u16, &mut pixels, 10);
        gif_frame.delay = 10;
        encoder.write_frame(&gif_frame).map_err(err)
    });
    drop(encoder);
    written?;
    if frames.load(Ordering::Relaxed) == 0 {
        let _ = std::fs::remove_file(&path);
        return Err("没有采集到画面".into());
    }
    Ok(path)
}

fn record_mp4(
    dir: &Path,
    id: &str,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    cursor: bool,
    silent_name: bool,
    stop: &Arc<AtomicBool>,
    pause: &Arc<AtomicBool>,
    frames: &Arc<AtomicU32>,
    elapsed: &Arc<AtomicU64>,
    message: &Mutex<String>,
    cancelled: &AtomicBool,
) -> Result<PathBuf, String> {
    let path = if silent_name {
        dir.join(format!("shotlet-{id}.silent.mp4"))
    } else {
        dir.join(format!("shotlet-{id}.mp4"))
    };
    let (mut child, mut stdin, stderr) = spawn_ffmpeg(&path, width, height)?;
    let mut pace = Pace { interval: MP4_INTERVAL, stop, pause, frames, elapsed, active: Duration::ZERO, misses: 0 };
    let written = capture_until(x, y, width, height, cursor, &mut pace, message, |frame| {
        stdin.write_all(frame.as_raw()).map_err(err)
    });
    drop(stdin);
    if cancelled.load(Ordering::Acquire) {
        let _ = child.kill();
        let _ = child.wait();
        let _ = stderr.join();
        let _ = std::fs::remove_file(&path);
        let _ = written;
        return Err("已取消".into());
    }
    let encoded = finish_ffmpeg(child, stderr, "ffmpeg 编码失败");
    if frames.load(Ordering::Relaxed) == 0 {
        let _ = std::fs::remove_file(&path);
        return Err(written.err().unwrap_or_else(|| "没有采集到画面".into()));
    }
    written?;
    encoded?;
    Ok(path)
}

fn spawn_ffmpeg(path: &Path, width: u32, height: u32) -> Result<(Child, std::process::ChildStdin, JoinHandle<Vec<u8>>), String> {
    let size = format!("{width}x{height}");
    let output = path.to_string_lossy().to_string();
    let mut child = Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel", "error",
            "-f", "rawvideo",
            "-pix_fmt", "rgba",
            "-s", &size,
            "-r", "30",
            "-i", "-",
            "-an",
            "-c:v", "libx264",
            "-preset", "ultrafast",
            "-crf", "18",
            "-pix_fmt", "yuv420p",
            &output,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(err)?;
    let stderr = child.stderr.take().ok_or("无法读取 ffmpeg 日志")?;
    let stderr_thread = thread::spawn(move || drain_stderr(stderr));
    let stdin = child.stdin.take().ok_or("无法写入 ffmpeg")?;
    Ok((child, stdin, stderr_thread))
}

fn drain_stderr(mut pipe: impl Read) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > 8192 {
                    let extra = buf.len() - 8192;
                    buf.drain(..extra);
                }
            }
            Err(_) => break,
        }
    }
    buf
}

fn finish_ffmpeg(mut child: Child, stderr_thread: JoinHandle<Vec<u8>>, label: &str) -> Result<(), String> {
    let status = child.wait().map_err(err)?;
    let stderr = stderr_thread.join().unwrap_or_default();
    if status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&stderr);
    let trimmed = text.trim();
    let tail: String = trimmed.chars().rev().take(180).collect::<String>().chars().rev().collect();
    if tail.is_empty() {
        Err(label.into())
    } else {
        Err(format!("{label}：{tail}"))
    }
}

fn mux_audio(video: &Path, wavs: &[PathBuf], output: &Path) -> Result<(), String> {
    if wavs.is_empty() {
        return Ok(());
    }
    let video_s = video.to_string_lossy().to_string();
    let output_s = output.to_string_lossy().to_string();
    let child = if wavs.len() == 1 {
        let wav_s = wavs[0].to_string_lossy().to_string();
        Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel", "error",
                "-i", &video_s,
                "-i", &wav_s,
                "-map", "0:v:0",
                "-map", "1:a:0",
                "-c:v", "copy",
                "-c:a", "aac",
                "-b:a", "160k",
                "-shortest",
                &output_s,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(err)?
    } else {
        let mic = wavs[0].to_string_lossy().to_string();
        let speaker = wavs[1].to_string_lossy().to_string();
        Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel", "error",
                "-i", &video_s,
                "-i", &mic,
                "-i", &speaker,
                "-filter_complex", "[1:a][2:a]amix=inputs=2:duration=shortest:dropout_transition=0[a]",
                "-map", "0:v:0",
                "-map", "[a]",
                "-c:v", "copy",
                "-c:a", "aac",
                "-b:a", "160k",
                "-shortest",
                &output_s,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(err)?
    };
    wait_ffmpeg(child, "ffmpeg 合成声音失败")
}

fn wait_ffmpeg(child: Child, label: &str) -> Result<(), String> {
    let output = child.wait_with_output().map_err(err)?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let trimmed = stderr.trim();
    let tail: String = trimmed.chars().rev().take(180).collect::<String>().chars().rev().collect();
    if tail.is_empty() {
        Err(label.into())
    } else {
        Err(format!("{label}：{tail}"))
    }
}

#[cfg(test)]
mod tests {
    use super::{fit_size, normalize_request, GIF_INTERVAL, LIMIT, MP4_INTERVAL};

    #[test]
    fn gif_drops_audio_and_keeps_the_cursor_choice() {
        let request = normalize_request("gif", "both", true).unwrap();
        assert_eq!(request.format, "gif");
        assert_eq!(request.audio, "none");
        assert!(request.cursor);
    }

    #[test]
    fn mp4_keeps_speaker_and_a_hidden_cursor() {
        let request = normalize_request("mp4", "speaker", false).unwrap();
        assert_eq!(request.audio, "speaker");
        assert!(!request.cursor);
    }

    #[test]
    fn mp4_can_record_the_microphone_and_speaker_together() {
        let request = normalize_request("mp4", "both", true).unwrap();
        assert_eq!(request.audio, "both");
    }

    #[test]
    fn unknown_audio_is_rejected() {
        assert!(normalize_request("mp4", "line", true).is_err());
    }

    #[test]
    fn gif_is_ten_frames_and_mp4_is_thirty_for_up_to_an_hour() {
        assert_eq!(GIF_INTERVAL.as_millis(), 100);
        assert_eq!(MP4_INTERVAL.as_millis(), 33);
        assert_eq!(LIMIT.as_secs(), 3600);
    }

    #[test]
    fn mp4_drops_one_odd_pixel_and_gif_keeps_it() {
        assert_eq!(fit_size(1919, 1080, true).unwrap(), (1918, 1080));
        assert_eq!(fit_size(1919, 1080, false).unwrap(), (1919, 1080));
        assert!(fit_size(7, 40, true).is_err());
    }
}
