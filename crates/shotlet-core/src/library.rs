use std::path::{Path, PathBuf};

use crate::capture;
use crate::history;
use crate::ocr;
use crate::settings::{self, Settings};
use crate::state::AppState;
use crate::util::{self, err};

pub use crate::history::{CommitRequest, HistoryItem};

pub enum LatestCopy {
    Empty,
    Copied,
}

pub fn trace(stage: &str) {
    util::trace(stage);
}

pub fn trace_at(stage: &str, unix_micros: u128) {
    util::trace_at(stage, unix_micros);
}

pub fn set_trace_file(path: PathBuf) {
    util::set_trace_file(path);
}

pub fn ensure_dir(app_data: &Path, settings: &Settings) -> Result<PathBuf, String> {
    let root = settings::storage_root(app_data, settings);
    std::fs::create_dir_all(&root).map_err(err)?;
    Ok(root)
}

pub fn list(root: &Path, query: &str, filter: &str) -> Result<Vec<HistoryItem>, String> {
    history::list(root, query, filter)
}

pub fn commit(root: &Path, request: CommitRequest) -> Result<HistoryItem, String> {
    history::commit(root, request)
}

pub fn update_document(root: &Path, id: &str, request: CommitRequest) -> Result<HistoryItem, String> {
    history::update_document(root, id, request)
}

pub fn set_favorite(root: &Path, id: &str, favorite: bool) -> Result<(), String> {
    history::set_favorite(root, id, favorite)
}

pub fn set_tags(root: &Path, id: &str, tags: Vec<String>) -> Result<(), String> {
    history::set_tags(root, id, tags)
}

pub fn rename(root: &Path, id: &str, name: String) -> Result<(), String> {
    history::rename(root, id, name)
}

pub fn soft_delete(root: &Path, ids: &[String]) -> Result<(), String> {
    history::soft_delete(root, ids)
}

pub fn restore(root: &Path, ids: &[String]) -> Result<(), String> {
    history::restore(root, ids)
}

pub fn purge(root: &Path) -> Result<(), String> {
    history::purge(root)
}

pub fn export(root: &Path, ids: &[String], dest: &Path) -> Result<u32, String> {
    history::export(root, ids, dest)
}

pub fn save_settings(app_data: &Path, settings: &Settings) -> Result<(), String> {
    settings::save(app_data, settings)
}

pub fn copy_latest(root: &Path) -> Result<LatestCopy, String> {
    let Some(path) = history::latest_file(root)? else {
        return Ok(LatestCopy::Empty);
    };
    copy_png_file(Path::new(&path))?;
    Ok(LatestCopy::Copied)
}

pub fn copy_png_file(path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(err)?;
    if bytes.starts_with(b"\x89PNG") {
        let image = util::load_rgba(&bytes)?;
        return util::copy_prepared(&bytes, &image);
    }
    let image = util::read_png_file(path)?;
    util::copy_rgba(&image)
}

fn write_png_bytes_inner(bytes: &[u8], dest: &Path) -> Result<(), String> {
    if !dest.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("png")) {
        return Err("只能保存为 PNG".into());
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        util::write_bytes(dest, bytes)
    } else {
        let image = util::load_rgba(bytes)?;
        util::write_png(dest, &image)
    }
}

pub fn write_png_bytes(bytes: &[u8], dest: &Path) -> Result<(), String> {
    write_png_bytes_inner(bytes, dest)
}

pub fn write_png_base64(data: &str, dest: &Path) -> Result<(), String> {
    let bytes = util::decode_png(data)?;
    write_png_bytes_inner(&bytes, dest)
}

pub fn copy_png_base64(data: &str) -> Result<(), String> {
    let bytes = util::decode_png(data)?;
    copy_png_bytes(&bytes)
}

pub fn copy_png_bytes(bytes: &[u8]) -> Result<(), String> {
    let image = util::load_rgba(bytes)?;
    if bytes.starts_with(b"\x89PNG") {
        util::copy_prepared(bytes, &image)
    } else {
        util::copy_rgba(&image)
    }
}

pub fn copy_annotated(root: &Path, png_b64: &str, base_b64: &str, document: &str) -> Result<(), String> {
    let png = util::decode_png(png_b64)?;
    let base = util::decode_png(base_b64).unwrap_or_default();
    copy_annotated_bytes(root, png, base, document)
}

/// Copy an annotated shot from raw PNG bytes. Clipboard work stays on the
/// caller; the history write keeps its original background thread.
pub fn copy_annotated_bytes(root: &Path, png: Vec<u8>, base: Vec<u8>, document: &str) -> Result<(), String> {
    let image = util::load_rgba(&png)?;
    if png.starts_with(b"\x89PNG") {
        util::copy_prepared(&png, &image)?;
    } else {
        util::copy_rgba(&image)?;
    }
    let root = root.to_path_buf();
    let document = document.to_string();
    let width = image.width();
    let height = image.height();
    std::thread::spawn(move || {
        let base = if base.starts_with(b"\x89PNG") { base } else { png.clone() };
        if let Err(error) = history::commit_bytes(&root, &png, &base, &document, width, height, "", "") {
            eprintln!("history: {error}");
        }
    });
    Ok(())
}

/// Commit straight from raw PNG bytes, skipping a Base64 decode on both images.
#[allow(clippy::too_many_arguments)]
pub fn commit_raw(
    root: &Path,
    png: &[u8],
    base: &[u8],
    document: &str,
    width: u32,
    height: u32,
    name: &str,
    ocr_text: &str,
) -> Result<HistoryItem, String> {
    history::commit_bytes(root, png, base, document, width, height, name, ocr_text)
}

#[allow(clippy::too_many_arguments)]
pub fn update_document_raw(
    root: &Path,
    id: &str,
    png: &[u8],
    base: &[u8],
    document: &str,
    width: u32,
    height: u32,
    name: &str,
    ocr_text: &str,
) -> Result<HistoryItem, String> {
    history::update_document_bytes(root, id, png, base, document, width, height, name, ocr_text)
}

pub fn copy_capture_region(state: &AppState, root: &Path, x: i32, y: i32, width: u32, height: u32) -> Result<(), String> {
    let (screens, path) = {
        let capture = state.capture.lock().map_err(err)?;
        let item = capture.as_ref().ok_or_else(|| "没有可复制的截图".to_string())?;
        (item.screens.clone(), item.path.clone())
    };
    let cropped = if screens.is_empty() {
        let image = util::read_png_file(Path::new(&path))?;
        capture::crop_frozen(&image, x, y, width, height)?
    } else {
        capture::crop_screens(&screens, x, y, width, height)?
    };
    let png = util::encode_png_fast(&cropped)?;
    util::copy_prepared(&png, &cropped)?;
    let root = root.to_path_buf();
    let width = cropped.width();
    let height = cropped.height();
    std::thread::spawn(move || {
        if let Err(error) = history::commit_bytes(&root, &png, &png, "[]", width, height, "自动复制", "") {
            eprintln!("history: {error}");
        }
    });
    Ok(())
}

pub fn image_size(path: &Path) -> Result<(u32, u32), String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(err)?;
    let mut header = [0u8; 24];
    file.read_exact(&mut header).map_err(err)?;
    util::png_size(&header)
}

pub fn write_pin_png(data: &str) -> Result<(PathBuf, u32, u32), String> {
    let bytes = util::decode_png(data)?;
    let (width, height) = util::png_size(&bytes)?;
    let path = capture::capture_dir().join(format!("pin-{}.png", util::new_id()));
    util::write_bytes(&path, &bytes)?;
    Ok((path, width, height))
}

/// Pin the selection straight from the frozen pixels. No canvas export, no re-encode.
/// `x`/`y` are the selection's origin in the packed crop, the same space `copy_capture_region` uses.
/// The history entry is written after the sticker file, on a background thread.
pub fn pin_capture_region(state: &AppState, root: &Path, x: i32, y: i32, width: u32, height: u32) -> Result<(PathBuf, u32, u32), String> {
    pin_taken(root, taken_capture(state)?, x, y, width, height)
}

/// The frozen pixels, copied out so the crop can run off the UI thread.
pub fn taken_capture(state: &AppState) -> Result<(Vec<capture::ScreenImage>, String), String> {
    let capture = state.capture.lock().map_err(err)?;
    let item = capture.as_ref().ok_or_else(|| "没有可复制的截图".to_string())?;
    Ok((item.screens.clone(), item.path.clone()))
}

pub fn pin_taken(root: &Path, taken: (Vec<capture::ScreenImage>, String), x: i32, y: i32, width: u32, height: u32) -> Result<(PathBuf, u32, u32), String> {
    let (screens, path) = taken;
    pin_region(root, &screens, &path, x, y, width, height)
}

fn pin_region(root: &Path, screens: &[capture::ScreenImage], path: &str, x: i32, y: i32, width: u32, height: u32) -> Result<(PathBuf, u32, u32), String> {
    let cropped = if screens.is_empty() {
        let image = util::read_png_file(Path::new(&path))?;
        capture::crop_frozen(&image, x, y, width, height)?
    } else {
        capture::crop_screens(&screens, x, y, width, height)?
    };
    let png = util::encode_png_fast(&cropped)?;
    let pin_path = capture::capture_dir().join(format!("pin-{}.png", util::new_id()));
    util::write_bytes(&pin_path, &png)?;
    let root = root.to_path_buf();
    let width = cropped.width();
    let height = cropped.height();
    std::thread::spawn(move || {
        if let Err(error) = history::commit_bytes(&root, &png, &png, "[]", width, height, "", "") {
            eprintln!("history: {error}");
        }
    });
    Ok((pin_path, width, height))
}

/// Pin an annotated PNG and record it. The bytes are written as they are.
/// History runs after the file is on disk, so the window does not wait for it.
pub fn pin_annotated(root: &Path, png_b64: &str, base_b64: &str, document: &str) -> Result<(PathBuf, u32, u32), String> {
    let png = util::decode_png(png_b64)?;
    let (width, height) = util::png_size(&png)?;
    let path = capture::capture_dir().join(format!("pin-{}.png", util::new_id()));
    util::write_bytes(&path, &png)?;
    let root = root.to_path_buf();
    let base_b64 = base_b64.to_string();
    let document = document.to_string();
    std::thread::spawn(move || {
        let base = util::decode_png(&base_b64).unwrap_or_else(|_| png.clone());
        let base = if base.starts_with(b"\x89PNG") { base } else { png.clone() };
        if let Err(error) = history::commit_bytes(&root, &png, &base, &document, width, height, "", "") {
            eprintln!("history: {error}");
        }
    });
    Ok((path, width, height))
}

pub fn pin_clipboard() -> Result<(crate::pin_session::PinImage, i32, i32), String> {
    let image = util::clipboard_rgba()?;
    let (x, y) = capture::cursor_point().unwrap_or((120, 120));
    Ok((std::sync::Arc::new(image), x, y))
}

/// Crop the frozen selection and keep the pixels. The file is written later.
pub fn crop_pin_image(taken: (Vec<capture::ScreenImage>, String), x: i32, y: i32, width: u32, height: u32) -> Result<crate::pin_session::PinImage, String> {
    let (screens, path) = taken;
    let cropped = if screens.is_empty() {
        let image = util::read_png_file(Path::new(&path))?;
        capture::crop_frozen(&image, x, y, width, height)?
    } else {
        capture::crop_screens(&screens, x, y, width, height)?
    };
    Ok(std::sync::Arc::new(cropped))
}

/// Encode once and write the sticker file. History is a separate step.
pub fn write_pin_file(image: &image::RgbaImage) -> Result<(PathBuf, Vec<u8>), String> {
    let png = util::encode_png_fast(image)?;
    let path = capture::capture_dir().join(format!("pin-{}.png", util::new_id()));
    util::write_bytes(&path, &png)?;
    Ok((path, png))
}

/// Encode once. The same bytes go to the pin file and to history.
pub fn store_pin(root: &Path, image: &image::RgbaImage) -> Result<PathBuf, String> {
    let (path, png) = write_pin_file(image)?;
    let root = root.to_path_buf();
    let width = image.width();
    let height = image.height();
    std::thread::spawn(move || {
        if let Err(error) = history::commit_bytes(&root, &png, &png, "[]", width, height, "", "") {
            eprintln!("history: {error}");
        }
    });
    Ok(path)
}

pub fn rgba_pin(bytes: Vec<u8>, width: u32, height: u32) -> Result<crate::pin_session::PinImage, String> {
    if width == 0 || height == 0 || bytes.len() != width as usize * height as usize * 4 {
        return Err("标注像素尺寸不对".into());
    }
    let image = image::RgbaImage::from_raw(width, height, bytes).ok_or_else(|| "标注像素尺寸不对".to_string())?;
    Ok(std::sync::Arc::new(image))
}

pub fn pin_image_or_file(state: &AppState, id: &str) -> Result<crate::pin_session::PinImage, String> {
    if let Some(image) = crate::pin_session::image_of(state, id) {
        return Ok(image);
    }
    let path = crate::pin_session::file_path(state, id)?;
    let image = util::read_png_file(Path::new(&path))?;
    Ok(std::sync::Arc::new(image))
}

/// History for an annotated pin. The window is already up; this does not block it.
pub fn queue_pin_history(root: &Path, annotated: &crate::pin_session::PinImage, base: &crate::pin_session::PinImage, document: &str) {
    let root = root.to_path_buf();
    let annotated = annotated.clone();
    let base = base.clone();
    let document = document.to_string();
    let width = annotated.width();
    let height = annotated.height();
    std::thread::spawn(move || {
        let png = match util::encode_png_fast(&annotated) {
            Ok(png) => png,
            Err(error) => {
                eprintln!("history: {error}");
                return;
            }
        };
        let base_png = match util::encode_png_fast(&base) {
            Ok(png) => png,
            Err(error) => {
                eprintln!("history: {error}");
                return;
            }
        };
        if let Err(error) = history::commit_bytes(&root, &png, &base_png, &document, width, height, "", "") {
            eprintln!("history: {error}");
        }
    });
}

pub fn pin_rgba(state: &AppState, id: &str) -> Result<Vec<u8>, String> {
    if let Some(image) = crate::pin_session::image_of(state, id) {
        return Ok(image.as_raw().to_vec());
    }
    let path = crate::pin_session::file_path(state, id)?;
    let image = util::read_png_file(Path::new(&path))?;
    Ok(image.into_raw())
}

pub fn copy_pin(state: &AppState, id: &str) -> Result<(), String> {
    if let Some(image) = crate::pin_session::image_of(state, id) {
        return util::copy_rgba(&image);
    }
    copy_png_file(Path::new(&crate::pin_session::file_path(state, id)?))
}

pub fn pin_path(state: &AppState, id: &str) -> Result<String, String> {
    crate::pin_session::file_path(state, id)
}

pub fn recognize_png(data: &str) -> Result<String, String> {
    let text = ocr::recognize_png_base64(data)?;
    if !text.is_empty() {
        let _ = util::copy_text(&text);
    }
    Ok(text)
}

pub fn recognize_png_bytes(bytes: &[u8]) -> Result<String, String> {
    let text = ocr::recognize_png_bytes(bytes)?;
    if !text.is_empty() {
        let _ = util::copy_text(&text);
    }
    Ok(text)
}

pub fn read_image_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(err)?;
    if !meta.is_file() {
        return Err("找不到截图".into());
    }
    let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("").to_ascii_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "shot") {
        return Err("不支持的图片".into());
    }
    // Full-screen retina captures stay well under this. Reject anything else
    // before it is copied into the webview.
    if meta.len() > 80 * 1024 * 1024 {
        return Err("截图太大，无法显示".into());
    }
    std::fs::read(path).map_err(err)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use base64::Engine;

    use super::{copy_capture_region, copy_latest, ensure_dir, pin_annotated, pin_capture_region, pin_path, read_image_bytes, save_settings, write_pin_png, LatestCopy};
    use crate::settings::Settings;
    use crate::state::AppState;

    #[test]
    fn pin_annotated_writes_the_png_unchanged() {
        use image::{Rgba, RgbaImage};
        let image = RgbaImage::from_pixel(3, 2, Rgba([4, 5, 6, 255]));
        let png = crate::util::encode_png_fast(&image).unwrap();
        let data = base64::engine::general_purpose::STANDARD.encode(&png);
        let dir = std::env::temp_dir().join(format!("shotlet-pin-ann-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (path, width, height) = pin_annotated(&dir, &data, &data, "[]").unwrap();
        assert_eq!((width, height), (3, 2));
        assert_eq!(std::fs::read(&path).unwrap(), png);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pin_from_the_cache_crops_the_selection() {
        use image::{Rgba, RgbaImage};
        let image = RgbaImage::from_fn(4, 3, |x, y| Rgba([x as u8, y as u8, 9, 255]));
        crate::capture::remember_preview("pin-crop", std::sync::Arc::new(image));
        let state = AppState::new(Settings::default());
        *state.capture.lock().unwrap() = Some(crate::capture::FrozenCapture {
            path: "pin-crop".into(),
            width: 4,
            height: 3,
            origin_x: 0,
            origin_y: 0,
            windows: Vec::new(),
            mode: "edit".into(),
            preset: None,
            screens: vec![crate::capture::ScreenImage {
                x: 0,
                y: 0,
                width: 4,
                height: 3,
                point_width: 4,
                point_height: 3,
                path: "pin-crop".into(),
                screen_x: 100,
                screen_y: 50,
                windows: Vec::new(),
            }],
            generation: 1,
        });
        let dir = std::env::temp_dir().join(format!("shotlet-pin-crop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (path, width, height) = pin_capture_region(&state, &dir, 1, 1, 2, 2).unwrap();
        assert_eq!((width, height), (2, 2));
        let saved = crate::util::read_png_file(&path).unwrap();
        assert_eq!(saved.get_pixel(0, 0).0, [1, 1, 9, 255]);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_pin_png_keeps_the_encoded_bytes() {
        use image::{Rgba, RgbaImage};
        let image = RgbaImage::from_pixel(2, 2, Rgba([8, 8, 8, 255]));
        let png = crate::util::encode_png_fast(&image).unwrap();
        let data = base64::engine::general_purpose::STANDARD.encode(&png);
        let (path, width, height) = write_pin_png(&data).unwrap();
        assert_eq!((width, height), (2, 2));
        assert_eq!(std::fs::read(&path).unwrap(), png);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_empty_capture_cannot_be_copied() {
        let state = AppState::new(Settings::default());
        let error = copy_capture_region(&state, Path::new("."), 0, 0, 8, 8).unwrap_err();
        assert_eq!(error, "没有可复制的截图");
        assert_eq!(pin_path(&state, "missing").unwrap_err(), "贴图已经失效");
    }

    #[test]
    fn storage_and_image_limits_stay_in_the_library() {
        let dir = std::env::temp_dir().join(format!("shotlet-selftest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let settings = Settings::default();
        let root = ensure_dir(&dir, &settings).unwrap();
        assert!(root.ends_with("history") && root.is_dir());
        assert!(matches!(copy_latest(&root).unwrap(), LatestCopy::Empty));
        save_settings(&dir, &settings).unwrap();
        assert!(dir.join("settings.json").is_file());
        assert_eq!(read_image_bytes(&dir).unwrap_err(), "找不到截图");
        let big = dir.join("big.png");
        std::fs::File::create(&big).unwrap().set_len(80 * 1024 * 1024 + 1).unwrap();
        assert_eq!(read_image_bytes(&big).unwrap_err(), "截图太大，无法显示");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
