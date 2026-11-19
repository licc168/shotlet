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

pub fn ensure_dir(app_data: &Path, settings: &Settings) -> Result<PathBuf, String> {
    let root = settings::storage_root(app_data, settings);
    std::fs::create_dir_all(&root).map_err(err)?;
    Ok(root)
}

pub fn list(root: &Path, query: &str, filter: &str) -> Vec<HistoryItem> {
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
    let Some(path) = history::latest_file(root) else {
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

pub fn write_png_base64(data: &str, dest: &Path) -> Result<(), String> {
    let image = util::load_rgba(&util::decode_png(data)?)?;
    util::write_png(dest, &image)
}

pub fn copy_png_base64(data: &str) -> Result<(), String> {
    let bytes = util::decode_png(data)?;
    let image = util::load_rgba(&bytes)?;
    if bytes.starts_with(b"\x89PNG") {
        util::copy_prepared(&bytes, &image)
    } else {
        util::copy_rgba(&image)
    }
}

pub fn copy_annotated(root: &Path, png_b64: &str, base_b64: &str, document: &str) -> Result<(), String> {
    let png = util::decode_png(png_b64)?;
    let image = util::load_rgba(&png)?;
    if png.starts_with(b"\x89PNG") {
        util::copy_prepared(&png, &image)?;
    } else {
        util::copy_rgba(&image)?;
    }
    let root = root.to_path_buf();
    let base_b64 = base_b64.to_string();
    let document = document.to_string();
    let width = image.width();
    let height = image.height();
    std::thread::spawn(move || {
        let base = util::decode_png(&base_b64).unwrap_or_else(|_| png.clone());
        let base = if base.starts_with(b"\x89PNG") { base } else { png.clone() };
        if let Err(error) = history::commit_bytes(&root, &png, &base, &document, width, height, "", "") {
            eprintln!("history: {error}");
        }
    });
    Ok(())
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
    let image = util::read_png_file(path)?;
    Ok((image.width(), image.height()))
}

pub fn write_pin_png(data: &str) -> Result<(PathBuf, u32, u32), String> {
    let image = util::load_rgba(&util::decode_png(data)?)?;
    let path = capture::capture_dir().join(format!("pin-{}.png", util::new_id()));
    util::write_png(&path, &image)?;
    Ok((path, image.width(), image.height()))
}

pub fn pin_clipboard() -> Result<(PathBuf, i32, i32, u32, u32), String> {
    let image = util::clipboard_rgba()?;
    let path = capture::capture_dir().join(format!("pin-{}.png", util::new_id()));
    let width = image.width();
    let height = image.height();
    util::write_png(&path, &image)?;
    let (x, y) = capture::cursor_point().unwrap_or((120, 120));
    Ok((path, x, y, width, height))
}

pub fn pin_path(state: &AppState, id: &str) -> Result<String, String> {
    state
        .pins
        .lock()
        .map_err(err)?
        .get(id)
        .cloned()
        .ok_or_else(|| "贴图已经失效".into())
}

pub fn recognize_png(data: &str) -> Result<String, String> {
    let text = ocr::recognize_png_base64(data)?;
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

    use super::{copy_capture_region, copy_latest, ensure_dir, pin_path, read_image_bytes, save_settings, LatestCopy};
    use crate::settings::Settings;
    use crate::state::AppState;

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
