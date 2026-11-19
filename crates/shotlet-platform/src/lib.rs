use base64::Engine;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, RgbaImage};
use std::io::{BufWriter, Write};
use std::path::Path;

pub fn err(error: impl ToString) -> String {
    error.to_string()
}

pub fn decode_png(data: &str) -> Result<Vec<u8>, String> {
    let raw = data.split(',').next_back().unwrap_or(data).trim();
    base64::engine::general_purpose::STANDARD
        .decode(raw)
        .map_err(err)
}

pub fn load_rgba(bytes: &[u8]) -> Result<RgbaImage, String> {
    let image = image::load_from_memory(bytes).map_err(err)?;
    Ok(image.to_rgba8())
}

pub fn encode_png_fast(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    PngEncoder::new_with_quality(&mut bytes, CompressionType::Fast, FilterType::NoFilter)
        .write_image(image.as_raw(), image.width(), image.height(), ExtendedColorType::Rgba8)
        .map_err(err)?;
    Ok(bytes)
}

/// Width and height from a PNG header, without decoding the pixels.
pub fn png_size(bytes: &[u8]) -> Result<(u32, u32), String> {
    if bytes.len() < 24 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("不是 PNG".into());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().map_err(err)?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().map_err(err)?);
    if width == 0 || height == 0 {
        return Err("选区尺寸无效".into());
    }
    Ok((width, height))
}

pub fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    std::fs::write(path, bytes).map_err(err)
}

pub fn write_png(path: &Path, image: &RgbaImage) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    std::fs::write(path, encode_png_fast(image)?).map_err(err)
}

/// Uncompressed capture. One disk write, no encoder, so the overlay can open on the raw pixels.
pub fn write_frozen(path: &Path, image: &RgbaImage) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    let file = std::fs::File::create(path).map_err(err)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(b"SLT1").map_err(err)?;
    writer.write_all(&image.width().to_le_bytes()).map_err(err)?;
    writer.write_all(&image.height().to_le_bytes()).map_err(err)?;
    writer.write_all(image.as_raw()).map_err(err)?;
    writer.flush().map_err(err)
}

pub fn read_png_file(path: &Path) -> Result<RgbaImage, String> {
    let bytes = std::fs::read(path).map_err(err)?;
    if bytes.starts_with(b"SLT1") {
        return read_frozen_bytes(&bytes);
    }
    load_rgba(&bytes)
}

fn read_frozen_bytes(bytes: &[u8]) -> Result<RgbaImage, String> {
    if bytes.len() < 12 {
        return Err("截图数据不完整".into());
    }
    let width = u32::from_le_bytes(bytes[4..8].try_into().map_err(err)?);
    let height = u32::from_le_bytes(bytes[8..12].try_into().map_err(err)?);
    let raw = bytes[12..].to_vec();
    let expected = (width as usize).saturating_mul(height as usize).saturating_mul(4);
    if width == 0 || height == 0 || raw.len() != expected {
        return Err("截图数据不完整".into());
    }
    RgbaImage::from_raw(width, height, raw).ok_or_else(|| "截图数据不完整".into())
}

pub fn copy_text(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(err)?;
    clipboard.set_text(text).map_err(err)
}

pub fn copy_rgba(image: &RgbaImage) -> Result<(), String> {
    let png = encode_png_fast(image)?;
    copy_prepared(&png, image)
}

/// Put an already-encoded PNG on the clipboard.
/// Windows also writes a bitmap. macOS writes `public.png` and does not encode again.
pub fn copy_prepared(png: &[u8], image: &RgbaImage) -> Result<(), String> {
    clipboard::place(png, image)
}

pub fn clipboard_rgba() -> Result<RgbaImage, String> {
    #[cfg(target_os = "macos")]
    if let Some(bytes) = clipboard::read_png() {
        if let Ok(image) = load_rgba(&bytes) {
            return Ok(image);
        }
    }
    let mut clipboard = arboard::Clipboard::new().map_err(err)?;
    let image = clipboard.get_image().map_err(|_| "剪贴板里没有图像".to_string())?;
    let width = u32::try_from(image.width).map_err(err)?;
    let height = u32::try_from(image.height).map_err(err)?;
    RgbaImage::from_raw(width, height, image.bytes.into_owned()).ok_or_else(|| "剪贴板图像无法读取".into())
}

/// `SHOTLET_TRACE=1` appends one line per stage to `shotlet.log`.
/// Off unless that variable is set, so a normal launch pays nothing.
/// The shell points the file at its own data directory once that directory is known.
pub fn trace(stage: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_micros())
        .unwrap_or(0);
    trace_at(stage, now);
}

/// Same line as [`trace`], with a caller-supplied unix timestamp in microseconds.
/// The page sends its own clock so the gap is not the IPC trip.
pub fn trace_at(stage: &str, unix_micros: u128) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !ON.get_or_init(trace_enabled) {
        return;
    }
    let line = format!("[trace {unix_micros}] {stage}\n");
    eprint!("{line}");
    let path = trace_file().lock().ok().and_then(|slot| slot.clone());
    if let Some(path) = path {
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            use std::io::Write;
            let _ = file.write_all(line.as_bytes());
        }
    }
}

fn trace_file() -> &'static std::sync::Mutex<Option<std::path::PathBuf>> {
    static FILE: std::sync::OnceLock<std::sync::Mutex<Option<std::path::PathBuf>>> = std::sync::OnceLock::new();
    FILE.get_or_init(|| std::sync::Mutex::new(None))
}

/// Where stage lines are appended. Ignored unless tracing is on.
pub fn set_trace_file(path: std::path::PathBuf) {
    if let Ok(mut slot) = trace_file().lock() {
        *slot = Some(path);
    }
}

fn trace_enabled() -> bool {
    match std::env::var("SHOTLET_TRACE") {
        Ok(flag) => flag == "1" || flag.eq_ignore_ascii_case("true"),
        Err(_) => false,
    }
}

pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static TICK: AtomicU64 = AtomicU64::new(0);
    let tick = TICK.fetch_add(1, Ordering::Relaxed);
    format!("{}-{tick}", chrono::Utc::now().timestamp_millis())
}

mod clipboard;

#[cfg(test)]
mod tests {
    use super::{encode_png_fast, load_rgba, png_size, read_png_file, write_bytes, write_frozen, write_png};
    use image::{Rgba, RgbaImage};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn png_size_reads_the_header_without_the_pixels() {
        let image = RgbaImage::from_pixel(5, 7, Rgba([1, 2, 3, 255]));
        let bytes = encode_png_fast(&image).unwrap();
        assert_eq!(png_size(&bytes).unwrap(), (5, 7));
        assert!(png_size(b"not a png").is_err());
        let dir = std::env::temp_dir().join(format!("shotlet-png-size-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pin.png");
        write_bytes(&path, &bytes).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_frozen_frame_roundtrips_without_changing_a_pixel() {
        let mut image = RgbaImage::from_pixel(2, 1, Rgba([9, 8, 7, 255]));
        image.put_pixel(1, 0, Rgba([1, 2, 3, 255]));
        let path = std::env::temp_dir().join(format!(
            "shotlet-frozen-{}.shot",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()
        ));
        write_frozen(&path, &image).unwrap();
        let loaded = read_png_file(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(loaded.as_raw(), image.as_raw());
    }

    #[test]
    fn png_files_still_decode() {
        let image = RgbaImage::from_pixel(1, 1, Rgba([4, 5, 6, 255]));
        let path = std::env::temp_dir().join(format!(
            "shotlet-png-{}.png",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()
        ));
        write_png(&path, &image).unwrap();
        let loaded = read_png_file(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(loaded.as_raw(), image.as_raw());
    }

    #[test]
    fn fast_png_roundtrips_without_a_second_encode() {
        let mut image = RgbaImage::from_pixel(3, 2, Rgba([10, 20, 30, 255]));
        image.put_pixel(2, 1, Rgba([1, 2, 3, 255]));
        let bytes = encode_png_fast(&image).unwrap();
        assert!(bytes.starts_with(b"\x89PNG"));
        assert_eq!(load_rgba(&bytes).unwrap().as_raw(), image.as_raw());
    }
}
