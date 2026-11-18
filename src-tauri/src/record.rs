use gif::{Encoder, Frame, Repeat};
use image::imageops::FilterType;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::capture::capture_rect;
use crate::record_audio::AudioCapture;
use crate::record_cursor::paint_cursor;
use crate::util::{err, new_id};

pub struct Recorder {
    stop: Arc<AtomicBool>,
    frames: Arc<AtomicU32>,
    started: Instant,
    thread: Option<JoinHandle<Result<PathBuf, String>>>,
    message: Arc<Mutex<String>>,
}

impl Recorder {
    pub fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    pub fn frame_count(&self) -> u32 {
        self.frames.load(Ordering::Relaxed)
    }

    pub fn message(&self) -> String {
        self.message.lock().map(|slot| slot.clone()).unwrap_or_default()
    }
}

pub fn cancel(recorder: &mut Recorder) {
    recorder.stop.store(true, Ordering::Relaxed);
    if let Some(thread) = recorder.thread.take() {
        let _ = thread.join();
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
    if audio != "none" && audio != "mic" && audio != "speaker" {
        return Err("未知的声音来源".into());
    }
    let audio = if format == "gif" { "none".to_string() } else { audio.to_string() };
    Ok(Request { format: format.to_string(), audio, cursor })
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
    if width < 8 || height < 8 {
        return Err("录制选区太小".into());
    }
    let request = normalize_request(format, audio, cursor)?;
    if request.format == "mp4" && !ffmpeg_available() {
        return Err("没有找到 ffmpeg，请改用 GIF 或先安装 ffmpeg".into());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let frames = Arc::new(AtomicU32::new(0));
    let message = Arc::new(Mutex::new(String::new()));
    let stop_thread = stop.clone();
    let frames_thread = frames.clone();
    let message_thread = message.clone();
    let thread = thread::spawn(move || {
        let result = record_loop(x, y, width, height, &request, &stop_thread, &frames_thread, &message_thread);
        if let Err(error) = &result {
            if let Ok(mut slot) = message_thread.lock() {
                *slot = error.clone();
            }
        }
        result
    });
    Ok(Recorder {
        stop,
        frames,
        started: Instant::now(),
        thread: Some(thread),
        message,
    })
}

pub fn finish(recorder: &mut Recorder) -> Result<PathBuf, String> {
    recorder.stop.store(true, Ordering::Relaxed);
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
    frames: &Arc<AtomicU32>,
    message: &Mutex<String>,
) -> Result<PathBuf, String> {
    let dir = output_dir()?;
    let id = new_id();
    let audio = if request.audio == "none" {
        None
    } else {
        let wav = dir.join(format!("shotlet-{id}.wav"));
        match AudioCapture::start(&request.audio, wav) {
            Ok(capture) => Some(capture),
            Err(error) => {
                note(message, &error);
                None
            }
        }
    };
    let mut grabbed = Vec::new();
    let started = Instant::now();
    while !stop.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(12) && grabbed.len() < 80 {
        let mut shot = capture_rect(x, y, width, height)?;
        if request.cursor {
            paint_cursor(&mut shot, x, y);
        }
        grabbed.push(downscale(shot));
        frames.store(grabbed.len() as u32, Ordering::Relaxed);
        thread::sleep(Duration::from_millis(120));
    }
    let elapsed = started.elapsed();
    let wav = match audio {
        Some(capture) => match capture.finish() {
            Ok(path) => Some(path),
            Err(error) => {
                note(message, &error);
                None
            }
        },
        None => None,
    };
    if grabbed.is_empty() {
        if let Some(path) = &wav {
            let _ = std::fs::remove_file(path);
        }
        return Err("没有采集到画面".into());
    }
    if request.format == "mp4" {
        encode_mp4(&dir, &id, &grabbed, elapsed, wav.as_deref())
    } else {
        if let Some(path) = &wav {
            let _ = std::fs::remove_file(path);
        }
        encode_gif(&dir, &id, &grabbed, elapsed)
    }
}

fn downscale(image: image::RgbaImage) -> image::RgbaImage {
    let max_edge = 960u32;
    let width = image.width();
    let height = image.height();
    let edge = width.max(height);
    if edge <= max_edge {
        return image;
    }
    let scale = max_edge as f32 / edge as f32;
    let nw = ((width as f32) * scale).round().max(1.0) as u32;
    let nh = ((height as f32) * scale).round().max(1.0) as u32;
    image::imageops::resize(&image, nw, nh, FilterType::Triangle)
}

fn gif_delay(frame_count: usize, elapsed: Duration) -> u16 {
    let frames = frame_count.max(1) as u128;
    let centiseconds = elapsed.as_millis() / frames / 10;
    centiseconds.clamp(2, 100) as u16
}

fn encode_gif(dir: &Path, id: &str, frames: &[image::RgbaImage], elapsed: Duration) -> Result<PathBuf, String> {
    let width = frames[0].width() as u16;
    let height = frames[0].height() as u16;
    let path = dir.join(format!("shotlet-{id}.gif"));
    let delay = gif_delay(frames.len(), elapsed);
    let mut bytes = Vec::new();
    {
        let mut encoder = Encoder::new(&mut bytes, width, height, &[]).map_err(err)?;
        encoder.set_repeat(Repeat::Infinite).map_err(err)?;
        for image in frames {
            let mut pixels = image.clone().into_raw();
            let mut frame = Frame::from_rgba_speed(width, height, &mut pixels, 10);
            frame.delay = delay;
            encoder.write_frame(&frame).map_err(err)?;
        }
    }
    std::fs::write(&path, bytes).map_err(err)?;
    Ok(path)
}

fn even_size(frame: &image::RgbaImage) -> (u32, u32) {
    let width = (frame.width() & !1).max(2).min(frame.width());
    let height = (frame.height() & !1).max(2).min(frame.height());
    (width, height)
}

fn even_bytes(frame: &image::RgbaImage, width: u32, height: u32) -> Vec<u8> {
    if frame.width() == width && frame.height() == height {
        return frame.as_raw().to_vec();
    }
    image::imageops::crop_imm(frame, 0, 0, width, height).to_image().into_raw()
}

fn frame_rate(frame_count: usize, elapsed: Duration) -> String {
    let seconds = elapsed.as_secs_f64().max(0.05);
    format!("{:.3}", frame_count as f64 / seconds)
}

fn encode_mp4(
    dir: &Path,
    id: &str,
    frames: &[image::RgbaImage],
    elapsed: Duration,
    wav: Option<&Path>,
) -> Result<PathBuf, String> {
    let (width, height) = even_size(&frames[0]);
    let final_path = dir.join(format!("shotlet-{id}.mp4"));
    let silent_path = if wav.is_some() {
        dir.join(format!("shotlet-{id}.silent.mp4"))
    } else {
        final_path.clone()
    };
    write_silent_mp4(&silent_path, frames, width, height, &frame_rate(frames.len(), elapsed))?;
    let Some(wav) = wav else {
        return Ok(final_path);
    };
    let muxed = mux_audio(&silent_path, wav, &final_path);
    let _ = std::fs::remove_file(wav);
    match muxed {
        Ok(()) => {
            let _ = std::fs::remove_file(&silent_path);
            Ok(final_path)
        }
        Err(error) => {
            let _ = std::fs::remove_file(&final_path);
            if silent_path != final_path {
                let _ = std::fs::rename(&silent_path, &final_path);
            }
            Err(format!("{error}。无声视频已保存：{}", final_path.display()))
        }
    }
}

fn write_silent_mp4(path: &Path, frames: &[image::RgbaImage], width: u32, height: u32, rate: &str) -> Result<(), String> {
    let size = format!("{width}x{height}");
    let output = path.to_string_lossy().to_string();
    let mut child = Command::new("ffmpeg")
        .args([
            "-y",
            "-f", "rawvideo",
            "-pix_fmt", "rgba",
            "-s", &size,
            "-r", rate,
            "-i", "-",
            "-an",
            "-c:v", "libx264",
            "-pix_fmt", "yuv420p",
            &output,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(err)?;
    {
        let stdin = child.stdin.as_mut().ok_or("无法写入 ffmpeg")?;
        use std::io::Write;
        for frame in frames {
            stdin.write_all(&even_bytes(frame, width, height)).map_err(err)?;
        }
    }
    wait_ffmpeg(child, "ffmpeg 编码失败")
}

fn mux_audio(video: &Path, wav: &Path, output: &Path) -> Result<(), String> {
    let video_s = video.to_string_lossy().to_string();
    let wav_s = wav.to_string_lossy().to_string();
    let output_s = output.to_string_lossy().to_string();
    let child = Command::new("ffmpeg")
        .args([
            "-y",
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
        .map_err(err)?;
    wait_ffmpeg(child, "ffmpeg 合成声音失败")
}

fn wait_ffmpeg(child: std::process::Child, label: &str) -> Result<(), String> {
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
    use super::{gif_delay, normalize_request};
    use std::time::Duration;

    #[test]
    fn gif_drops_audio_and_keeps_the_cursor_choice() {
        let request = normalize_request("gif", "mic", true).unwrap();
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
    fn unknown_audio_is_rejected() {
        assert!(normalize_request("mp4", "line", true).is_err());
    }

    #[test]
    fn gif_delay_matches_the_capture_interval() {
        assert_eq!(gif_delay(80, Duration::from_millis(9600)), 12);
        assert_eq!(gif_delay(10, Duration::from_millis(100)), 2);
    }
}
