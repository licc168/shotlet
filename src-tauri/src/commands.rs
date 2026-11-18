use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tauri_plugin_opener::OpenerExt;

use crate::capture::{self, FrozenCapture};
use crate::history::{self, CommitRequest, HistoryItem};
use crate::ocr;
use crate::record;
use crate::scroll;
use crate::settings::{self, Settings};
use crate::state::{AppState, EditorSession};
use crate::ui;
use crate::util::{self, err};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorView {
    pub record_id: Option<String>,
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub document: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordStatus {
    pub running: bool,
    pub elapsed_ms: u64,
    pub frames: u32,
    pub message: String,
}

pub async fn launch_mode(app: AppHandle, mode: &str) -> Result<(), String> {
    let mode = mode.to_string();
    let state = app.state::<AppState>();
    state.overlay_follow.store(false, Ordering::Relaxed);
    let epoch = state.overlay_epoch.fetch_add(1, Ordering::Relaxed) + 1;
    drop(state);
    ui::on_main_wait(&app, |app| {
        ui::conceal_overlays(app);
    })?;
    let mode_for_capture = mode.clone();
    let mut frozen = tauri::async_runtime::spawn_blocking(move || -> Result<FrozenCapture, String> {
        // Let the previous overlay leave the screen so it is not frozen into the shot.
        std::thread::sleep(std::time::Duration::from_millis(60));
        let mut frozen = capture::freeze_under_cursor()?;
        match mode_for_capture.as_str() {
            "copy" => frozen.mode = "copy".into(),
            "window" => {
                frozen.preset = capture::window_under_cursor(frozen.origin_x, frozen.origin_y, &frozen.windows);
                frozen.mode = "edit".into();
            }
            _ => frozen.mode = "edit".into(),
        }
        Ok(frozen)
    })
    .await
    .map_err(err)??;
    let state = app.state::<AppState>();
    if state.overlay_epoch.load(Ordering::Relaxed) != epoch {
        return Ok(());
    }
    let generation = state.overlay_generation.fetch_add(1, Ordering::Relaxed) + 1;
    drop(state);
    frozen.generation = generation;
    let follow = mode != "window" || frozen.preset.is_none();
    let app_for_window = app.clone();
    app.run_on_main_thread(move || {
        let state = app_for_window.state::<AppState>();
        if state.overlay_epoch.load(Ordering::Relaxed) != epoch {
            return;
        }
        if let Err(error) = state.capture.lock().map(|mut slot| *slot = Some(frozen.clone())) {
            let _ = app_for_window.emit("capture-error", error.to_string());
            return;
        }
        if let Err(error) = ui::present_overlays(&app_for_window, &frozen, generation) {
            let _ = app_for_window.emit("capture-error", error);
            return;
        }
        state.overlay_follow.store(follow, Ordering::Relaxed);
    })
    .map_err(err)
}

#[tauri::command]
pub fn overlay_ready(window: tauri::WebviewWindow, generation: u64) -> Result<(), String> {
    ui::reveal_overlay(&window, generation)
}

#[tauri::command]
pub fn set_overlay_follow(app: AppHandle, follow: bool) {
    ui::set_overlay_follow(&app, follow);
}

#[tauri::command]
pub fn aim_overlay(app: AppHandle, x: i32, y: i32) {
    ui::aim_overlay_at(&app, x, y);
}

pub(crate) fn spawn_launch(app: &AppHandle, mode: &'static str) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = launch_mode(app.clone(), mode).await {
            let _ = app.emit("capture-error", error);
        }
    });
}

pub fn on_shortcut(app: &AppHandle, shortcut: &Shortcut, event: ShortcutState) {
    if event != ShortcutState::Pressed {
        return;
    }
    let Ok(settings) = ui::read_settings(app) else {
        return;
    };
    let incoming = normalize(&shortcut.to_string());
    let matched = |value: &str| !value.trim().is_empty() && incoming == normalize(value);
    if matched(&settings.capture_shortcut) {
        spawn_launch(app, "edit");
    } else if matched(&settings.capture_copy_shortcut) {
        spawn_launch(app, "copy");
    } else if matched(&settings.window_shortcut) {
        spawn_launch(app, "window");
    } else if matched(&settings.pin_shortcut) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = pin_clipboard_now(&app).await {
                let _ = app.emit("capture-error", error);
            }
        });
    } else if matched(&settings.hide_pins_shortcut) {
        ui::toggle_pins(app);
    } else if matched(&settings.pin_group_shortcut) {
        ui::switch_pin_group(app);
    } else if matched(&settings.history_shortcut) {
        ui::open_history(app);
    } else if matched(&settings.copy_latest_shortcut) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = copy_latest_now(&app).await;
        });
    }
}

fn normalize(shortcut: &str) -> String {
    let mut modifiers = Vec::new();
    let mut key = String::new();
    for token in shortcut.split('+').map(str::trim).filter(|token| !token.is_empty()) {
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "cmdorcontrol" | "commandorcontrol" => modifiers.push("control"),
            "alt" | "option" => modifiers.push("alt"),
            "shift" => modifiers.push("shift"),
            "super" | "meta" | "win" | "command" | "cmd" | "windows" => modifiers.push("super"),
            _ => key = token.to_ascii_lowercase(),
        }
    }
    const ORDER: [&str; 4] = ["shift", "control", "alt", "super"];
    modifiers.sort_by_key(|name| ORDER.iter().position(|item| item == name).unwrap_or(ORDER.len()));
    modifiers.dedup();
    if key.is_empty() {
        modifiers.join("+")
    } else if modifiers.is_empty() {
        key
    } else {
        format!("{}+{key}", modifiers.join("+"))
    }
}

pub async fn copy_latest_now(app: &AppHandle) -> Result<(), String> {
    let root = storage(app)?;
    let Some(path) = history::latest_file(&root) else {
        let _ = app.emit("capture-error", "还没有可复制的截图");
        return Ok(());
    };
    let image = util::read_png_file(std::path::Path::new(&path))?;
    util::copy_rgba(&image)?;
    let _ = app.emit("capture-error", "已复制最近一张截图");
    Ok(())
}

fn storage(app: &AppHandle) -> Result<PathBuf, String> {
    let data = ui::app_data_dir(app)?;
    let settings = ui::read_settings(app)?;
    let root = settings::storage_root(&data, &settings);
    std::fs::create_dir_all(&root).map_err(err)?;
    Ok(root)
}

pub(crate) fn shortcut_log(app: &AppHandle, message: &str) {
    let stamp = chrono::Local::now().format("%H:%M:%S");
    let line = format!("[{stamp}] {message}\n");
    eprint!("shortcuts: {line}");
    if let Ok(dir) = ui::app_data_dir(app) {
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("shotlet.log"))
        {
            use std::io::Write;
            let _ = file.write_all(line.as_bytes());
        }
    }
}

pub(crate) fn register_shortcuts_when_ready(app: &AppHandle) {
    shortcut_log(app, "event loop ready, registering shortcuts");
    match ui::read_settings(app) {
        Ok(settings) => {
            if let Err(error) = apply_shortcuts(app, &settings) {
                shortcut_log(app, &format!("register failed: {error}"));
            }
        }
        Err(error) => shortcut_log(app, &format!("read settings failed: {error}")),
    }
}

pub fn apply_shortcuts(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = settings;
        if let Err(error) = app.global_shortcut().unregister_all() {
            shortcut_log(app, &format!("plugin unregister failed: {error}"));
        }
        // Plugin registration during setup reports success, then macOS drops the hotkey.
        crate::hotkey_mac::schedule_apply(app, true);
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
    {
    app.global_shortcut().unregister_all().map_err(err)?;
    let failed = register_missing(app, settings);
    if !failed.is_empty() {
        // Another app often still holds these keys while Shotlet is restarting.
        // Retry without dropping the shortcuts that already registered.
        let app = app.clone();
        std::thread::spawn(move || retry_shortcuts(app, failed));
    }
    Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
fn register_missing(app: &AppHandle, settings: &Settings) -> Vec<String> {
    let mut failed = Vec::new();
    for shortcut in shortcut_values(settings) {
        if shortcut.trim().is_empty() || app.global_shortcut().is_registered(shortcut) {
            continue;
        }
        if let Err(error) = register(app, shortcut) {
            eprintln!("shortcuts: {shortcut} failed: {error}");
            failed.push(shortcut.to_string());
        }
    }
    failed
}

#[cfg(not(target_os = "macos"))]
fn retry_shortcuts(app: AppHandle, mut failed: Vec<String>) {
    for delay_ms in [800, 2000, 4000, 8000] {
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        let Ok(settings) = ui::read_settings(&app) else {
            return;
        };
        failed = register_missing(&app, &settings);
        if failed.is_empty() {
            eprintln!("shortcuts: registered after retry");
            let _ = app.emit("capture-error", "");
            return;
        }
    }
    let message = format!("这些快捷键被其他程序占用：{}", failed.join("、"));
    eprintln!("shortcuts: {message}");
    let _ = app.emit("capture-error", message);
}

pub(crate) fn shortcut_values(settings: &Settings) -> [&str; 8] {
    [
        settings.capture_shortcut.as_str(),
        settings.capture_copy_shortcut.as_str(),
        settings.window_shortcut.as_str(),
        settings.pin_shortcut.as_str(),
        settings.hide_pins_shortcut.as_str(),
        settings.pin_group_shortcut.as_str(),
        settings.history_shortcut.as_str(),
        settings.copy_latest_shortcut.as_str(),
    ]
}

#[cfg(not(target_os = "macos"))]
fn register(app: &AppHandle, text: &str) -> Result<(), String> {
    let parsed: Shortcut = text.parse().map_err(err)?;
    app.global_shortcut().register(parsed).map_err(err)
}

#[tauri::command]
pub async fn start_capture(app: AppHandle, mode: Option<String>) -> Result<(), String> {
    launch_mode(app, mode.as_deref().unwrap_or("edit")).await
}

#[tauri::command]
pub fn current_capture(state: State<AppState>) -> Result<Option<FrozenCapture>, String> {
    Ok(state.capture.lock().map_err(err)?.clone())
}

#[tauri::command]
pub fn cancel_capture(app: AppHandle) {
    ui::close_overlay(&app);
}

#[tauri::command]
pub fn open_history(app: AppHandle) {
    ui::open_history(&app);
}

#[tauri::command]
pub fn open_settings(app: AppHandle) {
    ui::open_settings(&app);
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Result<Settings, String> {
    ui::read_settings(&app)
}

#[tauri::command]
pub fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let enabled = shortcut_values(&settings);
    for shortcut in enabled {
        if !shortcut.trim().is_empty() {
            validate_shortcut(shortcut)?;
        }
    }
    for (index, left) in enabled.iter().enumerate() {
        if left.trim().is_empty() {
            continue;
        }
        for right in enabled.iter().skip(index + 1) {
            if !right.trim().is_empty() && normalize(left) == normalize(right) {
                return Err("快捷键不能重复".into());
            }
        }
    }
    let data = ui::app_data_dir(&app)?;
    settings::save(&data, &settings)?;
    *app.state::<AppState>().settings.lock().map_err(err)? = settings.clone();
    apply_shortcuts(&app, &settings)?;
    ui::rebuild_tray_menu(&app)?;
    Ok(())
}

fn validate_shortcut(text: &str) -> Result<(), String> {
    let parsed: Shortcut = text.parse().map_err(|_| format!("无法识别快捷键 {text}"))?;
    if parsed.to_string().is_empty() {
        return Err("快捷键为空".into());
    }
    Ok(())
}

#[tauri::command]
pub fn list_history(app: AppHandle, query: String, filter: String) -> Result<Vec<HistoryItem>, String> {
    Ok(history::list(&storage(&app)?, &query, &filter))
}

#[tauri::command]
pub fn commit_history(app: AppHandle, request: CommitRequest) -> Result<HistoryItem, String> {
    history::commit(&storage(&app)?, request)
}

#[tauri::command]
pub fn update_history(app: AppHandle, id: String, request: CommitRequest) -> Result<HistoryItem, String> {
    history::update_document(&storage(&app)?, &id, request)
}

#[tauri::command]
pub fn set_favorite(app: AppHandle, id: String, favorite: bool) -> Result<(), String> {
    history::set_favorite(&storage(&app)?, &id, favorite)
}

#[tauri::command]
pub fn set_tags(app: AppHandle, id: String, tags: Vec<String>) -> Result<(), String> {
    history::set_tags(&storage(&app)?, &id, tags)
}

#[tauri::command]
pub fn rename_capture(app: AppHandle, id: String, name: String) -> Result<(), String> {
    history::rename(&storage(&app)?, &id, name)
}

#[tauri::command]
pub fn delete_captures(app: AppHandle, ids: Vec<String>) -> Result<(), String> {
    history::soft_delete(&storage(&app)?, &ids)
}

#[tauri::command]
pub fn restore_captures(app: AppHandle, ids: Vec<String>) -> Result<(), String> {
    history::restore(&storage(&app)?, &ids)
}

#[tauri::command]
pub fn purge_deleted(app: AppHandle) -> Result<(), String> {
    history::purge(&storage(&app)?)
}

#[tauri::command]
pub fn export_captures(app: AppHandle, ids: Vec<String>, dest_dir: String) -> Result<u32, String> {
    history::export(&storage(&app)?, &ids, std::path::Path::new(&dest_dir))
}

#[tauri::command]
pub fn copy_file(path: String) -> Result<(), String> {
    let image = util::read_png_file(std::path::Path::new(&path))?;
    util::copy_rgba(&image)
}

#[tauri::command]
pub fn write_png_base64(data: String, dest: String) -> Result<(), String> {
    let image = util::load_rgba(&util::decode_png(&data)?)?;
    util::write_png(std::path::Path::new(&dest), &image)
}

#[tauri::command]
pub fn copy_png_base64(data: String) -> Result<(), String> {
    let image = util::load_rgba(&util::decode_png(&data)?)?;
    util::copy_rgba(&image)
}

fn round_i32(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    value.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

#[tauri::command]
pub fn pin_file(app: AppHandle, path: String, x: f64, y: f64) -> Result<(), String> {
    let x = round_i32(x);
    let y = round_i32(y);
    let image = util::read_png_file(std::path::Path::new(&path))?;
    eprintln!("pin: file {path} at {x},{y} ({}x{})", image.width(), image.height());
    ui::open_pin(&app, path, x, y, image.width(), image.height())
}

#[tauri::command]
pub async fn pin_png_base64(app: AppHandle, data: String, x: f64, y: f64) -> Result<(), String> {
    // Selection math is fractional. serde rejects a JSON float for i32, so the
    // command never ran and no pin file was written. Round here.
    let x = round_i32(x);
    let y = round_i32(y);
    eprintln!("pin: png bytes={} at {x},{y}", data.len());
    let (path, width, height) = tauri::async_runtime::spawn_blocking(move || -> Result<(std::path::PathBuf, u32, u32), String> {
        let image = util::load_rgba(&util::decode_png(&data)?)?;
        let path = capture::capture_dir().join(format!("pin-{}.png", util::new_id()));
        util::write_png(&path, &image)?;
        eprintln!("pin: wrote {} ({}x{})", path.display(), image.width(), image.height());
        Ok((path, image.width(), image.height()))
    })
    .await
    .map_err(err)??;
    ui::open_pin(&app, path.to_string_lossy().to_string(), x, y, width, height)?;
    eprintln!("pin: window queued");
    Ok(())
}

#[tauri::command]
pub fn copy_capture_region(app: AppHandle, x: i32, y: i32, width: u32, height: u32) -> Result<(), String> {
    let (screens, path) = {
        let state = app.state::<AppState>();
        let capture = state.capture.lock().map_err(err)?;
        let item = capture.as_ref().ok_or_else(|| "没有可复制的截图".to_string())?;
        (item.screens.clone(), item.path.clone())
    };
    let cropped = if screens.is_empty() {
        let image = util::read_png_file(std::path::Path::new(&path))?;
        capture::crop_frozen(&image, x, y, width, height)?
    } else {
        capture::crop_screens(&screens, x, y, width, height)?
    };
    util::copy_rgba(&cropped)?;
    history::commit_image(&storage(&app)?, &cropped, "自动复制")?;
    let _ = app.emit("capture-error", "已复制截图");
    Ok(())
}

pub async fn pin_clipboard_now(app: &AppHandle) -> Result<(), String> {
    let image = tauri::async_runtime::spawn_blocking(util::clipboard_rgba)
        .await
        .map_err(err)??;
    let path = capture::capture_dir().join(format!("pin-{}.png", util::new_id()));
    let width = image.width();
    let height = image.height();
    util::write_png(&path, &image)?;
    let (x, y) = capture::cursor_point().unwrap_or((120, 120));
    eprintln!("pin: clipboard {} at {x},{y} ({width}x{height})", path.display());
    ui::open_pin(app, path.to_string_lossy().to_string(), x, y, width, height)?;
    let _ = app.emit("capture-error", "已贴图");
    Ok(())
}

#[tauri::command]
pub fn pin_path(state: State<AppState>, id: String) -> Result<String, String> {
    state
        .pins
        .lock()
        .map_err(err)?
        .get(&id)
        .cloned()
        .ok_or_else(|| "贴图已经失效".into())
}


#[tauri::command]
pub fn read_image_bytes(path: String) -> Result<tauri::ipc::Response, String> {
    let path = std::path::Path::new(&path);
    let meta = std::fs::metadata(path).map_err(err)?;
    if !meta.is_file() {
        return Err("找不到截图".into());
    }
    // Full-screen retina captures stay well under this. Reject anything else
    // before it is copied into the webview.
    if meta.len() > 80 * 1024 * 1024 {
        return Err("截图太大，无法显示".into());
    }
    let bytes = std::fs::read(path).map_err(err)?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub async fn ocr_png_base64(data: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let text = ocr::recognize_png_base64(&data)?;
        if !text.is_empty() {
            let _ = util::copy_text(&text);
        }
        Ok(text)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub fn open_editor_record(app: AppHandle, id: String) -> Result<(), String> {
    let (path, width, height, document) = history::editor_source(&storage(&app)?, &id)?;
    *app.state::<AppState>().editor.lock().map_err(err)? = Some(EditorSession {
        record_id: Some(id),
        path,
        width,
        height,
        document,
    });
    ui::open_editor(&app)
}

#[tauri::command]
pub fn current_editor(state: State<AppState>) -> Result<Option<EditorView>, String> {
    let session = state.editor.lock().map_err(err)?;
    Ok(session.as_ref().map(|session| EditorView {
        record_id: session.record_id.clone(),
        path: session.path.clone(),
        width: session.width,
        height: session.height,
        document: session.document.clone(),
    }))
}

#[tauri::command]
pub async fn scroll_capture(app: AppHandle, x: i32, y: i32, width: u32, height: u32) -> Result<EditorView, String> {
    if let Err(error) = scroll::ensure_scroll_access() {
        let _ = app.emit("capture-error", error.clone());
        return Err(error);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let app_hide = app.clone();
    app.run_on_main_thread(move || {
        ui::hide_overlay(&app_hide);
        let _ = tx.send(());
    })
    .map_err(err)?;
    let _ = rx.recv();
    std::thread::sleep(std::time::Duration::from_millis(190));
    let captured = tauri::async_runtime::spawn_blocking(move || scroll::capture_scrolled(x, y, width, height)).await;
    let result = match captured.map_err(err).and_then(|inner| inner) {
        Ok(result) => result,
        Err(error) => {
            ui::close_overlay(&app);
            let _ = app.emit("capture-error", error.clone());
            ui::open_home(&app);
            return Err(error);
        }
    };
    let view = EditorView {
        record_id: None,
        path: result.path.clone(),
        width: result.width,
        height: result.height,
        document: "[]".into(),
    };
    *app.state::<AppState>().editor.lock().map_err(err)? = Some(EditorSession {
        record_id: None,
        path: result.path,
        width: result.width,
        height: result.height,
        document: "[]".into(),
    });
    ui::close_overlay(&app);
    ui::open_editor(&app)?;
    Ok(view)
}

#[tauri::command]
pub fn ffmpeg_available() -> bool {
    record::ffmpeg_available()
}

#[tauri::command(async)]
pub fn start_recording(
    app: AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    format: String,
    audio: String,
    cursor: bool,
) -> Result<(), String> {
    let x = x.round() as i32;
    let y = y.round() as i32;
    let width = width.round().max(1.0) as u32;
    let height = height.round().max(1.0) as u32;
    if app.state::<AppState>().recorder.lock().map_err(err)?.is_some() {
        return Err("已经有一段录制在进行".into());
    }
    if width < 8 || height < 8 {
        return Err("录制选区太小".into());
    }
    if format == "mp4" && !record::ffmpeg_available() {
        return Err("没有找到 ffmpeg，请改用 GIF 或先安装 ffmpeg".into());
    }
    ui::on_main_wait(&app, |app| {
        ui::hide_overlay(app);
    })?;
    std::thread::sleep(std::time::Duration::from_millis(120));
    let recorder = record::start(x, y, width, height, &format, &audio, cursor)?;
    *app.state::<AppState>().recorder.lock().map_err(err)? = Some(recorder);
    if let Err(error) = ui::open_recorder(&app, x, y, height) {
        if let Ok(mut slot) = app.state::<AppState>().recorder.lock() {
            if let Some(mut recorder) = slot.take() {
                record::cancel(&mut recorder);
            }
        }
        let _ = app.emit("capture-error", error.clone());
        return Err(error);
    }
    Ok(())
}

#[tauri::command]
pub fn recording_status(state: State<AppState>) -> Result<RecordStatus, String> {
    let recorder = state.recorder.lock().map_err(err)?;
    Ok(match recorder.as_ref() {
        Some(recorder) => RecordStatus {
            running: true,
            elapsed_ms: recorder.elapsed_ms(),
            frames: recorder.frame_count(),
            message: recorder.message(),
        },
        None => RecordStatus { running: false, elapsed_ms: 0, frames: 0, message: String::new() },
    })
}

#[tauri::command]
pub async fn stop_recording(app: AppHandle) -> Result<String, String> {
    let mut recorder = app
        .state::<AppState>()
        .recorder
        .lock()
        .map_err(err)?
        .take()
        .ok_or("没有正在进行的录制")?;
    let path = tauri::async_runtime::spawn_blocking(move || record::finish(&mut recorder))
        .await
        .map_err(err)??;
    ui::close_recorder(&app);
    let text = path.to_string_lossy().to_string();
    let _ = app.opener().open_path(text.clone(), None::<&str>);
    Ok(text)
}

#[tauri::command]
pub fn storage_dir(app: AppHandle) -> Result<String, String> {
    Ok(storage(&app)?.to_string_lossy().to_string())
}

#[tauri::command]
pub fn reveal_path(app: AppHandle, path: String) -> Result<(), String> {
    app.opener().open_path(path, None::<&str>).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn shortcut_normalize_ignores_modifier_order_and_case() {
        assert_eq!(normalize("Control+F1"), normalize("control+f1"));
        assert_eq!(normalize("Control+Shift+F1"), normalize("shift+control+F1"));
        assert_eq!(normalize("F3"), normalize("f3"));
        assert_eq!(normalize("Super+Shift+KeyH"), normalize("shift+super+KeyH"));
        assert_ne!(normalize("F1"), normalize("Control+F1"));
    }
}

