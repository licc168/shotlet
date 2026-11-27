use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tauri_plugin_opener::OpenerExt;

use shotlet_core::capture_session::{self, FrozenCapture};
use shotlet_core::pin_session;
use shotlet_core::editor::{self, EditorView};
use shotlet_core::library::{self, CommitRequest, HistoryItem, LatestCopy};
use shotlet_core::record_session::{self, RecordStatus};
use shotlet_core::scroll_session::{self, ScrollRequest, ScrollStatus};
use shotlet_core::shortcut::{self, Settings};
use shotlet_core::state::AppState;

use crate::ui;

#[tauri::command]
pub fn trace_mark(stage: String, at: Option<f64>) {
    // The page sends `performance.timeOrigin + performance.now()`, which is milliseconds.
    // Shell lines are microseconds. Convert here so one log can be subtracted.
    match at {
        Some(ms) if ms.is_finite() && ms > 0.0 => library::trace_at(&stage, (ms * 1000.0).round() as u128),
        _ => library::trace(&stage),
    }
}

fn err(error: impl ToString) -> String {
    error.to_string()
}

pub async fn launch_mode(app: AppHandle, mode: &str) -> Result<(), String> {
    library::trace("f1: key");
    let mode = mode.to_string();
    let epoch = capture_session::begin(&app.state::<AppState>());
    library::trace("f1: conceal begin");
    let covered = ui::on_main_wait(&app, |app| ui::conceal_overlays(app))?;
    library::trace(&format!("f1: conceal done covered={covered}"));
    let wait_for_hide = covered && !ui::overlay_uses_cloak();
    let mode_for_capture = mode.clone();
    let shot = tauri::async_runtime::spawn_blocking(move || capture_session::grab(&mode_for_capture, wait_for_hide))
        .await
        .map_err(err)?;
    let shot = match shot {
        Ok(shot) => shot,
        Err(error) => {
            if error.contains("屏幕录制") {
                prompt_screen_recording(&app);
            }
            return Err(error);
        }
    };
    if !capture_session::is_current(&app.state::<AppState>(), epoch) {
        return Ok(());
    }
    let later = shot.later;
    let mode_for_stage = mode;
    let app_for_window = app.clone();
    app.run_on_main_thread(move || {
        let state = app_for_window.state::<AppState>();
        let staged = match capture_session::stage(&state, epoch, shot.frozen, &mode_for_stage) {
            Ok(Some(staged)) => staged,
            Ok(None) => return,
            Err(error) => {
                let _ = app_for_window.emit("capture-error", error);
                return;
            }
        };
        library::trace("f1: present begin");
        if let Err(error) = ui::present_overlays(&app_for_window, &staged.frozen, staged.generation) {
            let _ = app_for_window.emit("capture-error", error);
            return;
        }
        library::trace("f1: present done");
        capture_session::flush_frozen();
        capture_session::arm_follow(&state, staged.follow);
        let Some(cursor) = staged.frozen.screens.first().cloned() else {
            return;
        };
        if later.is_empty() {
            return;
        }
        let generation = staged.generation;
        let app_later = app_for_window.clone();
        library::trace(&format!("f1: remaining queued count={}", later.len()));
        tauri::async_runtime::spawn(async move {
            library::trace("f1: remaining start");
            let added = tauri::async_runtime::spawn_blocking(move || capture_session::capture_remaining(&cursor, &later))
                .await
                .unwrap_or_default();
            if added.is_empty() {
                return;
            }
            let _ = app_later.clone().run_on_main_thread(move || {
                let Some((frozen, from)) = capture_session::append_screens(&app_later.state::<AppState>(), epoch, generation, added) else {
                    return;
                };
                library::trace("f1: present more begin");
                if let Err(error) = ui::present_more_overlays(&app_later, &frozen, generation, from) {
                    let _ = app_later.emit("capture-error", error);
                }
                capture_session::flush_frozen();
            });
        });
    })
    .map_err(err)
}

#[tauri::command]
pub fn overlay_ready(window: tauri::WebviewWindow, generation: u64, cursor: Option<bool>) -> Result<(), String> {
    library::trace(&format!("f1: overlay ready generation={generation}"));
    // Only the cursor screen releases the other monitors. A later screen reporting
    // ready must not be what they were waiting for.
    if cursor.unwrap_or(true) {
        capture_session::note_shown(window.state::<AppState>().inner(), generation);
    }
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

fn prompt_screen_recording(app: &AppHandle) {
    static SHOWN: AtomicBool = AtomicBool::new(false);
    if SHOWN.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        app.dialog()
            .message("没有屏幕录制权限，所以选区不会出现。请到系统设置中允许 Shotlet。用开发模式运行时，列表里的名字是 shotlet；如果开关已经打开，先关掉再打开，然后重新启动应用。")
            .title("Shotlet 截图")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCustom("打开系统设置".into()))
            .show(move |ok| {
                if ok {
                    let _ = app.opener().open_url(
                        "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_ScreenCapture",
                        None::<&str>,
                    );
                }
            });
    });
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
    let incoming = shortcut::normalize(&shortcut.to_string());
    if shortcut::same(&incoming, &settings.capture_shortcut) {
        spawn_launch(app, "edit");
    } else if shortcut::same(&incoming, &settings.capture_copy_shortcut) {
        spawn_launch(app, "copy");
    } else if shortcut::same(&incoming, &settings.window_shortcut) {
        spawn_launch(app, "window");
    } else if shortcut::same(&incoming, &settings.pin_shortcut) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = pin_clipboard_now(&app).await {
                let _ = app.emit("capture-error", error);
            }
        });
    } else if shortcut::same(&incoming, &settings.hide_pins_shortcut) {
        ui::toggle_pins(app);
    } else if shortcut::same(&incoming, &settings.pin_group_shortcut) {
        ui::switch_pin_group(app);
    } else if shortcut::same(&incoming, &settings.history_shortcut) {
        ui::open_history(app);
    } else if shortcut::same(&incoming, &settings.copy_latest_shortcut) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = copy_latest_now(&app).await;
        });
    }
}

pub async fn copy_latest_now(app: &AppHandle) -> Result<(), String> {
    match library::copy_latest(&storage(app)?)? {
        LatestCopy::Empty => {
            let _ = app.emit("capture-error", "还没有可复制的截图");
        }
        LatestCopy::Copied => {
            let _ = app.emit("capture-error", "已复制最近一张截图");
        }
    }
    Ok(())
}

fn storage(app: &AppHandle) -> Result<PathBuf, String> {
    let data = ui::app_data_dir(app)?;
    let settings = ui::read_settings(app)?;
    library::ensure_dir(&data, &settings)
}

fn shortcut_log_sender() -> &'static std::sync::mpsc::Sender<(PathBuf, String)> {
    static SENDER: std::sync::OnceLock<std::sync::mpsc::Sender<(PathBuf, String)>> = std::sync::OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<(PathBuf, String)>();
        std::thread::Builder::new()
            .name("shotlet-shortcut-log".into())
            .spawn(move || {
                use std::io::Write;
                while let Ok((dir, line)) = rx.recv() {
                    eprint!("shortcuts: {line}");
                    let _ = std::fs::create_dir_all(&dir);
                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(dir.join("shotlet.log"))
                    {
                        let _ = file.write_all(line.as_bytes());
                    }
                }
            })
            .expect("spawn shortcut log worker");
        tx
    })
}

pub(crate) fn shortcut_log(app: &AppHandle, message: &str) {
    let stamp = chrono::Local::now().format("%H:%M:%S");
    let line = format!("[{stamp}] {message}\n");
    // The CGEventTap callback calls this on macOS. Keep it non-blocking: only
    // format and hand off; the worker owns the synchronous filesystem write.
    if let Ok(dir) = ui::app_data_dir(app) {
        let _ = shortcut_log_sender().send((dir, line));
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
    shortcut::values(settings)
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

#[tauri::command(async)]
pub fn current_capture(state: State<AppState>) -> Result<Option<FrozenCapture>, String> {
    capture_session::current(&state)
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
    for shortcut in shortcut::values(&settings) {
        if !shortcut.trim().is_empty() {
            validate_shortcut(shortcut)?;
        }
    }
    shortcut::reject_duplicates(&settings)?;
    let data = ui::app_data_dir(&app)?;
    library::save_settings(&data, &settings)?;
    shortcut::remember(&app.state::<AppState>(), settings.clone())?;
    // Authorize after the in-memory settings are updated: `storage` reads them
    // back, so granting the scope any earlier would whitelist the old directory
    // and leave a newly chosen storage_dir unreadable over asset://.
    if let Ok(root) = storage(&app) {
        let _ = app.asset_protocol_scope().allow_directory(root, true);
    }
    apply_shortcuts(&app, &settings)?;
    ui::rebuild_tray_menu(&app)?;
    let _ = app.emit("settings-changed", &settings);
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
    library::list(&storage(&app)?, &query, &filter)
}

// ---- Binary commit protocol ------------------------------------------------
// A raw IPC body can carry only one blob, so a commit is packed into one frame:
//   [u32 LE png len][u32 LE base len][u32 LE meta len]
//   [png bytes][base bytes][meta JSON: document / ocrText / width / height / name / id]
// Both images and the annotation document stay out of JSON Base64.

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitMeta {
    #[serde(default)]
    document: String,
    #[serde(default)]
    ocr_text: String,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    id: String,
}

fn commit_frame(request: &tauri::ipc::Request<'_>) -> Result<(Vec<u8>, Vec<u8>, CommitMeta), String> {
    match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => parse_frame(bytes),
        _ => Err("expected a raw commit frame".into()),
    }
}

fn parse_frame(bytes: &[u8]) -> Result<(Vec<u8>, Vec<u8>, CommitMeta), String> {
    const HEADER: usize = 12;
    if bytes.len() < HEADER {
        return Err("commit frame too short".into());
    }
    let png_len = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    let base_len = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let meta_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let total = HEADER
        .checked_add(png_len)
        .and_then(|value| value.checked_add(base_len))
        .and_then(|value| value.checked_add(meta_len))
        .ok_or_else(|| "commit frame overflow".to_string())?;
    if bytes.len() != total {
        return Err("commit frame length mismatch".into());
    }
    let png_end = HEADER + png_len;
    let base_end = png_end + base_len;
    let png = bytes[HEADER..png_end].to_vec();
    // Guard against a caller sending an empty comparison image: an empty
    // `.base.png` would silently corrupt the history compare view.
    let base = if base_len == 0 {
        png.clone()
    } else {
        bytes[png_end..base_end].to_vec()
    };
    let meta: CommitMeta = serde_json::from_slice(&bytes[base_end..]).map_err(err)?;
    Ok((png, base, meta))
}

#[cfg(test)]
mod commit_frame_tests {
    use super::parse_frame;

    fn frame(png: &[u8], base: &[u8], meta: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + png.len() + base.len() + meta.len());
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&(base.len() as u32).to_le_bytes());
        out.extend_from_slice(&(meta.len() as u32).to_le_bytes());
        out.extend_from_slice(png);
        out.extend_from_slice(base);
        out.extend_from_slice(meta.as_bytes());
        out
    }

    const META: &str = r#"{"document":"[]","ocrText":"hi","width":10,"height":20,"name":"n","id":"abc"}"#;

    #[test]
    fn parses_a_valid_frame() {
        let (png, base, meta) = parse_frame(&frame(b"PNG-DATA", b"BASE-DATA", META)).unwrap();
        assert_eq!(png, b"PNG-DATA");
        assert_eq!(base, b"BASE-DATA");
        assert_eq!(meta.id, "abc");
        assert_eq!(meta.width, 10);
        assert_eq!(meta.ocr_text, "hi");
    }

    #[test]
    fn an_empty_base_falls_back_to_the_shot() {
        let (png, base, _) = parse_frame(&frame(b"PNG-DATA", b"", META)).unwrap();
        assert_eq!(png, b"PNG-DATA");
        assert_eq!(base, png);
    }

    #[test]
    fn rejects_short_and_mismatched_frames() {
        assert!(parse_frame(&[0, 1, 2]).is_err());
        let mut broken = frame(b"PNG-DATA", b"BASE-DATA", META);
        broken.pop();
        assert!(parse_frame(&broken).is_err());
    }

    #[test]
    fn rejects_oversized_length_fields() {
        let (png, base, _) = parse_frame(&frame(b"PNG-DATA", b"", META)).unwrap();
        assert_eq!(png.len(), 8);
        assert_eq!(base.len(), 8);
        let mut huge = Vec::new();
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        huge.extend_from_slice(&0u32.to_le_bytes());
        assert!(parse_frame(&huge).is_err());
    }
}

#[tauri::command]
pub async fn commit_history_raw(app: AppHandle, request: tauri::ipc::Request<'_>) -> Result<HistoryItem, String> {
    let (png, base, meta) = commit_frame(&request)?;
    let root = storage(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        library::commit_raw(&root, &png, &base, &meta.document, meta.width, meta.height, &meta.name, &meta.ocr_text)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub async fn update_history_raw(app: AppHandle, request: tauri::ipc::Request<'_>) -> Result<HistoryItem, String> {
    let (png, base, meta) = commit_frame(&request)?;
    if meta.id.trim().is_empty() {
        return Err("missing history id".into());
    }
    let root = storage(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        library::update_document_raw(
            &root, &meta.id, &png, &base, &meta.document, meta.width, meta.height, &meta.name, &meta.ocr_text,
        )
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub fn copy_annotated_raw(app: AppHandle, request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let (png, base, meta) = commit_frame(&request)?;
    library::copy_annotated_bytes(&storage(&app)?, png, base, &meta.document)?;
    let _ = app.emit("capture-error", "已复制截图");
    Ok(())
}

#[tauri::command]
pub fn copy_png_raw(request: tauri::ipc::Request<'_>) -> Result<(), String> {
    match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => library::copy_png_bytes(bytes),
        _ => Err("expected raw PNG bytes".into()),
    }
}

#[tauri::command]
pub fn commit_history(app: AppHandle, request: CommitRequest) -> Result<HistoryItem, String> {
    library::commit(&storage(&app)?, request)
}

#[tauri::command]
pub fn update_history(app: AppHandle, id: String, request: CommitRequest) -> Result<HistoryItem, String> {
    library::update_document(&storage(&app)?, &id, request)
}

#[tauri::command]
pub fn set_favorite(app: AppHandle, id: String, favorite: bool) -> Result<(), String> {
    library::set_favorite(&storage(&app)?, &id, favorite)
}

#[tauri::command]
pub fn set_tags(app: AppHandle, id: String, tags: Vec<String>) -> Result<(), String> {
    library::set_tags(&storage(&app)?, &id, tags)
}

#[tauri::command]
pub fn rename_capture(app: AppHandle, id: String, name: String) -> Result<(), String> {
    library::rename(&storage(&app)?, &id, name)
}

#[tauri::command]
pub fn delete_captures(app: AppHandle, ids: Vec<String>) -> Result<(), String> {
    library::soft_delete(&storage(&app)?, &ids)
}

#[tauri::command]
pub fn restore_captures(app: AppHandle, ids: Vec<String>) -> Result<(), String> {
    library::restore(&storage(&app)?, &ids)
}

#[tauri::command]
pub fn purge_deleted(app: AppHandle) -> Result<(), String> {
    library::purge(&storage(&app)?)
}

#[tauri::command]
pub fn export_captures(app: AppHandle, ids: Vec<String>, dest_dir: String) -> Result<u32, String> {
    library::export(&storage(&app)?, &ids, Path::new(&dest_dir))
}

#[tauri::command]
pub fn copy_file(path: String) -> Result<(), String> {
    library::copy_png_file(Path::new(&path))
}

#[tauri::command]
pub async fn write_png_base64(data: String, dest: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || library::write_png_base64(&data, Path::new(&dest)))
        .await
        .map_err(err)?
}

/// Save a PNG sent as a raw binary body. The destination path travels in the
/// `x-write-dest` header as Base64(UTF-8) so non-ASCII paths survive the header.
#[tauri::command]
pub async fn write_png_raw(request: tauri::ipc::Request<'_>) -> Result<(), String> {
    use base64::Engine;
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.to_vec(),
        _ => return Err("expected raw PNG bytes".into()),
    };
    let encoded = request
        .headers()
        .get("x-write-dest")
        .ok_or_else(|| "missing x-write-dest".to_string())?
        .to_str()
        .map_err(err)?
        .to_string();
    let dest = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(err)?;
    let dest = String::from_utf8(dest).map_err(err)?;
    tauri::async_runtime::spawn_blocking(move || library::write_png_bytes(&bytes, Path::new(&dest)))
        .await
        .map_err(err)?
}

#[tauri::command]
pub fn copy_png_base64(data: String) -> Result<(), String> {
    library::copy_png_base64(&data)
}

#[tauri::command]
pub fn copy_annotated(app: AppHandle, png: String, base: String, document: String) -> Result<(), String> {
    library::copy_annotated(&storage(&app)?, &png, &base, &document)?;
    let _ = app.emit("capture-error", "已复制截图");
    Ok(())
}

fn round_i32(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    value.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

#[tauri::command]
pub async fn pin_file(app: AppHandle, path: String, x: f64, y: f64) -> Result<(), String> {
    let x = round_i32(x);
    let y = round_i32(y);
    let sized = path.clone();
    let (width, height) = tauri::async_runtime::spawn_blocking(move || library::image_size(Path::new(&sized)))
        .await
        .map_err(err)??;
    eprintln!("pin: file {path} at {x},{y} ({width}x{height})");
    let _ = ui::open_pin(&app, path, x, y, width, height)?;
    Ok(())
}

/// Let the pin reveal itself without holding the caller's future open.
/// `PinWait::wait` still runs in the background so its 3s reveal fallback
/// (and the 8s one armed with the window) keep protecting a stalled webview.
fn reveal_pin_in_background(wait: ui::PinWait) {
    tauri::async_runtime::spawn_blocking(move || {
        let _ = wait.wait();
    });
}

#[tauri::command]
pub async fn pin_png_base64(app: AppHandle, data: String, x: f64, y: f64) -> Result<(), String> {
    library::trace("pin: command");
    // Selection math is fractional. serde rejects a JSON float for i32, so the
    // command never ran and no pin file was written. Round here.
    let x = round_i32(x);
    let y = round_i32(y);
    eprintln!("pin: png bytes={} at {x},{y}", data.len());
    let (path, width, height) = tauri::async_runtime::spawn_blocking(move || library::write_pin_png(&data))
        .await
        .map_err(err)??;
    eprintln!("pin: wrote {} ({width}x{height})", path.display());
    library::trace("pin: file written");
    let wait = ui::open_pin(&app, path.to_string_lossy().to_string(), x, y, width, height)?;
    reveal_pin_in_background(wait);
    Ok(())
}

#[tauri::command]
pub async fn pin_capture_region(
    app: AppHandle,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    screen_x: i32,
    screen_y: i32,
    local_x: i32,
    local_y: i32,
    bitmap_w: u32,
    bitmap_h: u32,
    point_w: u32,
    point_h: u32,
) -> Result<(), String> {
    library::trace("pin: plain command");
    // `x`/`y` address the packed crop. The desktop cut is derived here, not on the page.
    let (place_x, place_y, place_w, place_h) = capture_session::desktop_rect(screen_x, screen_y, local_x, local_y, width, height, bitmap_w, bitmap_h, point_w, point_h);
    let taken = library::taken_capture(&app.state::<AppState>())?;
    let image = tauri::async_runtime::spawn_blocking(move || library::crop_pin_image(taken, x, y, width, height))
        .await
        .map_err(err)??;
    library::trace("pin: plain cropped");
    let root = storage(&app)?;
    let wait = ui::open_memory(&app, root, image, place_x, place_y, place_w, place_h)?;
    reveal_pin_in_background(wait);
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnnotatedJson {
    png: String,
    base: String,
    document: String,
    screen_x: f64,
    screen_y: f64,
    local_x: f64,
    local_y: f64,
    width: f64,
    height: f64,
    bitmap_w: f64,
    bitmap_h: f64,
    point_w: f64,
    point_h: f64,
}

fn header_f64(headers: &tauri::http::HeaderMap, name: &str) -> f64 {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .filter(|value: &f64| value.is_finite())
        .unwrap_or(0.0)
}

fn place_from_parts(screen_x: f64, screen_y: f64, local_x: f64, local_y: f64, width: f64, height: f64, bitmap_w: f64, bitmap_h: f64, point_w: f64, point_h: f64) -> (i32, i32, u32, u32) {
    capture_session::desktop_rect(
        round_i32(screen_x),
        round_i32(screen_y),
        round_i32(local_x),
        round_i32(local_y),
        width.round().max(1.0) as u32,
        height.round().max(1.0) as u32,
        bitmap_w.round().max(1.0) as u32,
        bitmap_h.round().max(1.0) as u32,
        point_w.round().max(0.0) as u32,
        point_h.round().max(0.0) as u32,
    )
}

#[tauri::command(async)]
pub fn pin_annotated(app: AppHandle, request: tauri::ipc::Request<'_>) -> Result<String, String> {
    library::trace("pin: annotated command");
    match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => pin_annotated_raw(&app, bytes, request.headers()),
        tauri::ipc::InvokeBody::Json(value) => {
            pin_annotated_json(&app, value)?;
            Ok(String::new())
        }
    }
}

fn pin_annotated_raw(app: &AppHandle, bytes: &[u8], headers: &tauri::http::HeaderMap) -> Result<String, String> {
    let pixel_w = header_f64(headers, "x-pin-pixel-w").round().max(1.0) as u32;
    let pixel_h = header_f64(headers, "x-pin-pixel-h").round().max(1.0) as u32;
    let image = library::rgba_pin(bytes.to_vec(), pixel_w, pixel_h)?;
    let (place_x, place_y, place_w, place_h) = place_from_parts(
        header_f64(headers, "x-pin-screen-x"),
        header_f64(headers, "x-pin-screen-y"),
        header_f64(headers, "x-pin-local-x"),
        header_f64(headers, "x-pin-local-y"),
        header_f64(headers, "x-pin-w"),
        header_f64(headers, "x-pin-h"),
        header_f64(headers, "x-pin-bitmap-w"),
        header_f64(headers, "x-pin-bitmap-h"),
        header_f64(headers, "x-pin-point-w"),
        header_f64(headers, "x-pin-point-h"),
    );
    let crop_x = header_f64(headers, "x-pin-x").round() as i32;
    let crop_y = header_f64(headers, "x-pin-y").round() as i32;
    let crop_w = header_f64(headers, "x-pin-w").round().max(1.0) as u32;
    let crop_h = header_f64(headers, "x-pin-h").round().max(1.0) as u32;
    let base = match library::taken_capture(app.state::<AppState>().inner()) {
        Ok(taken) => library::crop_pin_image(taken, crop_x, crop_y, crop_w, crop_h).unwrap_or_else(|error| {
            eprintln!("pin: base crop failed: {error}");
            image.clone()
        }),
        Err(error) => {
            eprintln!("pin: base crop failed: {error}");
            image.clone()
        }
    };
    library::trace("pin: annotated cropped");
    let wait = ui::open_annotated(app, image, base, place_x, place_y, place_w, place_h)?;
    let id = wait.id().to_string();
    wait.wait()?;
    Ok(id)
}

fn pin_annotated_json(app: &AppHandle, value: &serde_json::Value) -> Result<(), String> {
    let body: AnnotatedJson = serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
    let (place_x, place_y, place_w, place_h) = place_from_parts(
        body.screen_x,
        body.screen_y,
        body.local_x,
        body.local_y,
        body.width,
        body.height,
        body.bitmap_w,
        body.bitmap_h,
        body.point_w,
        body.point_h,
    );
    let root = storage(app)?;
    let (path, pixels_w, pixels_h) = library::pin_annotated(&root, &body.png, &body.base, &body.document)?;
    library::trace("pin: annotated file written");
    ui::open_file(app, path.to_string_lossy().to_string(), place_x, place_y, place_w, place_h, pixels_w, pixels_h)?.wait()?;
    Ok(())
}

#[tauri::command(async)]
pub fn pin_history(app: AppHandle, id: String, document: String) -> Result<(), String> {
    let root = storage(&app)?;
    let state = app.state::<AppState>();
    let annotated = library::pin_image_or_file(&state, &id)?;
    let base = pin_session::take_base(state.inner(), &id).unwrap_or_else(|| annotated.clone());
    library::queue_pin_history(&root, &annotated, &base, &document);
    Ok(())
}

#[tauri::command]
pub async fn copy_capture_region(app: AppHandle, x: i32, y: i32, width: u32, height: u32) -> Result<(), String> {
    let root = storage(&app)?;
    let state_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = state_app.state::<AppState>();
        library::copy_capture_region(&state, &root, x, y, width, height)
    })
    .await
    .map_err(err)??;
    let _ = app.emit("capture-error", "已复制截图");
    Ok(())
}

pub async fn pin_clipboard_now(app: &AppHandle) -> Result<(), String> {
    let (image, x, y) = tauri::async_runtime::spawn_blocking(library::pin_clipboard).await.map_err(err)??;
    let width = image.width();
    let height = image.height();
    eprintln!("pin: clipboard memory at {x},{y} ({width}x{height})");
    let root = storage(app)?;
    let wait = ui::open_memory(app, root, image, x, y, width, height)?;
    reveal_pin_in_background(wait);
    let _ = app.emit("capture-error", "已贴图");
    Ok(())
}

#[tauri::command(async)]
pub fn pin_pixels(state: State<AppState>, id: String) -> Result<tauri::ipc::Response, String> {
    let bytes = library::pin_rgba(&state, &id)?;
    if bytes.len() > 96 * 1024 * 1024 {
        return Err("截图太大，无法显示".into());
    }
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub fn copy_pin(app: AppHandle, id: String) -> Result<(), String> {
    library::copy_pin(&app.state::<AppState>(), &id)?;
    let _ = app.emit("capture-error", "已复制截图");
    Ok(())
}

#[tauri::command]
pub fn pin_standby(window: tauri::WebviewWindow) {
    if pin_session::report_standby(window.state::<AppState>().inner(), window.label()) {
        library::trace("pin: standby ready");
    }
}

#[tauri::command]
pub fn pin_zoom(window: tauri::WebviewWindow, scale: f64) {
    ui::zoom_pin(&window, scale);
}

#[tauri::command]
pub fn pin_ready(window: tauri::WebviewWindow) {
    library::trace("pin: page ready");
    ui::present_pin(window.app_handle(), window.label());
}

#[tauri::command]
pub fn pin_path(state: State<AppState>, id: String) -> Result<String, String> {
    library::pin_path(&state, &id)
}

#[tauri::command]
pub async fn read_image_bytes(path: String) -> Result<tauri::ipc::Response, String> {
    let bytes = tauri::async_runtime::spawn_blocking(move || library::read_image_bytes(Path::new(&path)))
        .await
        .map_err(err)??;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command(async)]
pub fn preview_rgba(path: String) -> Result<tauri::ipc::Response, String> {
    let bytes = capture_session::preview_bytes(&path)?;
    if bytes.len() > 96 * 1024 * 1024 {
        return Err("截图太大，无法显示".into());
    }
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub async fn ocr_png_base64(data: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || library::recognize_png(&data)).await.map_err(err)?
}

/// OCR a PNG sent as a raw binary body.
///
/// OCR is CPU-bound and can take seconds, so it must not sit on the async
/// runtime's workers: copy the body out of the request and hand it to the
/// blocking pool, matching `ocr_png_base64`.
#[tauri::command]
pub async fn ocr_png_raw(request: tauri::ipc::Request<'_>) -> Result<String, String> {
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.to_vec(),
        _ => return Err("expected raw PNG bytes".into()),
    };
    tauri::async_runtime::spawn_blocking(move || library::recognize_png_bytes(&bytes))
        .await
        .map_err(err)?
}

#[derive(serde::Serialize)]
pub struct OcrLineView {
    text: String,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// OCR lines with boxes, in the PNG's pixel space. Nothing is copied to the clipboard.
#[tauri::command]
pub async fn ocr_lines_raw(request: tauri::ipc::Request<'_>) -> Result<Vec<OcrLineView>, String> {
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.to_vec(),
        _ => return Err("expected raw PNG bytes".into()),
    };
    let lines = tauri::async_runtime::spawn_blocking(move || library::recognize_line_boxes(&bytes))
        .await
        .map_err(err)??;
    Ok(lines
        .into_iter()
        .map(|line| OcrLineView { text: line.text, x: line.x, y: line.y, w: line.w, h: line.h })
        .collect())
}

#[tauri::command]
pub async fn ai_models(config: crate::ai::AiConfig) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || crate::ai::list_models(&config))
        .await
        .map_err(err)?
}

#[tauri::command]
pub async fn ai_test(config: crate::ai::AiConfig) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || crate::ai::test_connection(&config))
        .await
        .map_err(err)?
}

#[tauri::command]
pub async fn ai_see(app: AppHandle, request: tauri::ipc::Request<'_>) -> Result<String, String> {
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.to_vec(),
        _ => return Err("expected raw PNG bytes".into()),
    };
    let mode = request
        .headers()
        .get("x-shotlet-see")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("translate")
        .to_string();
    let settings = ui::read_settings(&app)?;
    let config = crate::ai::AiConfig {
        provider: settings.ai_provider,
        endpoint: settings.ai_endpoint,
        api_key: settings.ai_api_key,
        model: settings.ai_model,
    };
    tauri::async_runtime::spawn_blocking(move || crate::ai::see(&config, &bytes, &mode))
        .await
        .map_err(err)?
}

#[tauri::command]
pub async fn ai_translate(app: AppHandle, text: String) -> Result<String, String> {
    let settings = ui::read_settings(&app)?;
    let config = crate::ai::AiConfig {
        provider: settings.ai_provider,
        endpoint: settings.ai_endpoint,
        api_key: settings.ai_api_key,
        model: settings.ai_model,
    };
    tauri::async_runtime::spawn_blocking(move || crate::ai::translate(&config, &text))
        .await
        .map_err(err)?
}

#[tauri::command]
pub fn open_editor_record(app: AppHandle, id: String) -> Result<(), String> {
    let session = editor::from_history(&storage(&app)?, &id)?;
    editor::store(&app.state::<AppState>(), session)?;
    ui::open_editor(&app)
}

#[tauri::command]
pub fn current_editor(state: State<AppState>) -> Result<Option<EditorView>, String> {
    editor::current_view(&state)
}

#[tauri::command]
pub fn scroll_finish(action: String) {
    scroll_session::finish_request(&action);
}

#[tauri::command]
pub fn scroll_cancel() {
    scroll_session::cancel_request();
}

#[tauri::command]
pub fn scroll_progress() -> ScrollStatus {
    scroll_session::status()
}

#[tauri::command]
pub fn set_scroll_auto(enabled: bool) {
    scroll_session::set_auto(enabled);
}

#[tauri::command]
pub async fn scroll_capture(app: AppHandle, request: ScrollRequest) -> Result<EditorView, String> {
    let frame = scroll_session::prepare(&request)?;
    let vx = frame.visual_x;
    let vy = frame.visual_y;
    let vw = frame.visual_w;
    let vh = frame.visual_h;
    let screen_x = frame.screen_x;
    let screen_y = frame.screen_y;
    let screen_w = frame.screen_w;
    let screen_h = frame.screen_h;
    eprintln!("scroll: open chrome at {vx},{vy} {vw}x{vh} on {screen_x},{screen_y} {screen_w}x{screen_h}");
    if let Err(error) = ui::on_main_wait_for(&app, std::time::Duration::from_secs(8), move |app| {
        ui::open_scroll_chrome(app, vx, vy, vw, vh, screen_x, screen_y, screen_w, screen_h)
    }) {
        eprintln!("scroll: chrome failed: {error}");
        scroll_session::end();
        let _ = ui::on_main_wait_for(&app, std::time::Duration::from_secs(4), |app| ui::close_scroll_chrome(app));
        return Err(error);
    }
    let hidden = match ui::on_main_wait_for(&app, std::time::Duration::from_secs(8), |app| ui::conceal_visible_overlays(app)) {
        Ok(labels) => labels,
        Err(error) => {
            eprintln!("scroll: hide overlay failed: {error}");
            scroll_session::end();
            let _ = ui::on_main_wait_for(&app, std::time::Duration::from_secs(4), |app| ui::close_scroll_chrome(app));
            return Err(error);
        }
    };
    let captured = tauri::async_runtime::spawn_blocking(move || {
        std::thread::sleep(std::time::Duration::from_millis(160));
        scroll_session::capture(frame)
    })
    .await;
    scroll_session::end();
    let _ = ui::on_main_wait_for(&app, std::time::Duration::from_secs(4), |app| ui::close_scroll_chrome(app));
    let result = match captured.map_err(err).and_then(|inner| inner) {
        Ok(result) => result,
        Err(error) => {
            let labels = hidden;
            let _ = ui::on_main_wait_for(&app, std::time::Duration::from_secs(4), move |app| ui::show_overlay_labels(app, &labels));
            if !scroll_session::is_cancelled(&error) {
                let _ = app.emit("capture-error", error.clone());
            }
            return Err(error);
        }
    };
    let action = scroll_session::take_action();
    let path = result.path.clone();
    if action == "copy" {
        library::copy_png_file(Path::new(&path))?;
        ui::close_overlay(&app);
        let _ = app.emit("capture-error", "已复制长图");
        let (_, view) = editor::from_scroll(result);
        return Ok(view);
    }
    if action == "save" {
        let dialog_app = app.clone();
        let dest = tauri::async_runtime::spawn_blocking(move || save_long_image(dialog_app))
            .await
            .map_err(err)
            .ok()
            .flatten();
        if let Some(dest) = dest {
            std::fs::copy(&path, &dest).map_err(err)?;
            ui::close_overlay(&app);
            let _ = app.emit("capture-error", "已保存长图");
            let (_, view) = editor::from_scroll(result);
            return Ok(view);
        }
    }
    let (session, view) = editor::from_scroll(result);
    editor::store(&app.state::<AppState>(), session)?;
    ui::close_overlay(&app);
    ui::open_editor(&app)?;
    Ok(view)
}

fn save_long_image(app: AppHandle) -> Option<PathBuf> {
    let (tx, rx) = std::sync::mpsc::channel();
    let dialog_app = app.clone();
    if app
        .run_on_main_thread(move || {
            dialog_app
                .dialog()
                .file()
                .add_filter("PNG", &["png"])
                .set_file_name("shotlet-scroll.png")
                .set_title("保存长图")
                .save_file(move |file| {
                    let _ = tx.send(file.and_then(|path| path.into_path().ok()));
                });
        })
        .is_err()
    {
        return None;
    }
    rx.recv_timeout(std::time::Duration::from_secs(8)).ok().flatten()
}

#[tauri::command]
pub fn ffmpeg_available() -> bool {
    record_session::ffmpeg_available()
}

#[tauri::command(async)]
pub fn open_recording(
    app: AppHandle,
    screen_x: f64,
    screen_y: f64,
    local_x: f64,
    local_y: f64,
    width: f64,
    height: f64,
    bitmap_w: f64,
    bitmap_h: f64,
    point_w: f64,
    point_h: f64,
) -> Result<(), String> {
    let (x, y, w, h) = capture_session::desktop_rect(
        screen_x.round() as i32,
        screen_y.round() as i32,
        local_x.round() as i32,
        local_y.round() as i32,
        width.round().max(1.0) as u32,
        height.round().max(1.0) as u32,
        bitmap_w.round().max(1.0) as u32,
        bitmap_h.round().max(1.0) as u32,
        point_w.round().max(0.0) as u32,
        point_h.round().max(0.0) as u32,
    );
    let planned = record_session::place(&app.state::<AppState>(), f64::from(x), f64::from(y), f64::from(w), f64::from(h))?;
    match ui::on_main_wait(&app, move |app| {
        ui::hide_overlay(app);
        let _ = ui::open_record_border(app, planned.x, planned.y, planned.width, planned.height);
        ui::open_recorder(app, planned.x, planned.y, planned.height)
    }) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) | Err(error) => {
            record_session::clear_pending(&app.state::<AppState>());
            ui::close_record_border(&app);
            Err(error)
        }
    }
}

#[tauri::command(async)]
pub fn begin_recording(app: AppHandle, format: String, audio: String, cursor: bool) -> Result<(), String> {
    let recorder = record_session::begin(&app.state::<AppState>(), &format, &audio, cursor)?;
    record_session::store(&app.state::<AppState>(), recorder)?;
    Ok(())
}

#[tauri::command]
pub fn set_recording_paused(state: State<AppState>, paused: bool) -> Result<(), String> {
    record_session::set_paused(&state, paused)
}

#[tauri::command]
pub fn recording_status(state: State<AppState>) -> Result<RecordStatus, String> {
    record_session::status(&state)
}

#[tauri::command]
pub async fn stop_recording(app: AppHandle) -> Result<String, String> {
    let recorder = record_session::take(&app.state::<AppState>())?;
    record_session::clear_pending(&app.state::<AppState>());
    ui::close_record_border(&app);
    let Some(mut recorder) = recorder else {
        ui::close_recorder(&app);
        return Ok(String::new());
    };
    let path = match tauri::async_runtime::spawn_blocking(move || record_session::finish(&mut recorder)).await.map_err(err)? {
        Ok(path) => path,
        Err(error) => return Err(error),
    };
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
    let file = Path::new(&path);
    if !file.exists() {
        return Err("找不到文件".into());
    }
    app.opener().reveal_item_in_dir(file).map_err(err)
}
