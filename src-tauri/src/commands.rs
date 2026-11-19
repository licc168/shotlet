use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
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
    let mode_for_capture = mode.clone();
    let shot = tauri::async_runtime::spawn_blocking(move || capture_session::grab(&mode_for_capture, covered))
        .await
        .map_err(err)??;
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
        let shown = capture_session::watch_shown(&state, generation);
        let app_later = app_for_window.clone();
        library::trace(&format!("f1: remaining queued count={}", later.len()));
        tauri::async_runtime::spawn(async move {
            // The other screens wait until the cursor screen has painted, so their
            // grab and window placement do not compete with that first frame.
            // A late or missing report must not stall them.
            let _ = tauri::async_runtime::spawn_blocking(move || shown.recv_timeout(std::time::Duration::from_millis(250))).await;
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

pub(crate) fn shortcut_log(app: &AppHandle, message: &str) {
    let stamp = chrono::Local::now().format("%H:%M:%S");
    let line = format!("[{stamp}] {message}\n");
    eprint!("shortcuts: {line}");
    if let Ok(dir) = ui::app_data_dir(app) {
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("shotlet.log")) {
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
    Ok(library::list(&storage(&app)?, &query, &filter))
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
pub fn write_png_base64(data: String, dest: String) -> Result<(), String> {
    library::write_png_base64(&data, Path::new(&dest))
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
pub fn pin_file(app: AppHandle, path: String, x: f64, y: f64) -> Result<(), String> {
    let x = round_i32(x);
    let y = round_i32(y);
    let (width, height) = library::image_size(Path::new(&path))?;
    eprintln!("pin: file {path} at {x},{y} ({width}x{height})");
    let _ = ui::open_pin(&app, path, x, y, width, height)?;
    Ok(())
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
    ui::open_pin(&app, path.to_string_lossy().to_string(), x, y, width, height)?
        .ready()
        .await?;
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
    ui::open_memory(&app, root, image, place_x, place_y, place_w, place_h)?
        .ready()
        .await?;
    Ok(())
}

#[tauri::command]
pub async fn pin_annotated(
    app: AppHandle,
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
) -> Result<(), String> {
    library::trace("pin: annotated command");
    let (place_x, place_y, place_w, place_h) = capture_session::desktop_rect(
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
    );
    let root = storage(&app)?;
    let (path, pixels_w, pixels_h) = tauri::async_runtime::spawn_blocking(move || {
        library::pin_annotated(&root, &png, &base, &document)
    })
    .await
    .map_err(err)??;
    library::trace("pin: annotated file written");
    ui::open_file(
        &app,
        path.to_string_lossy().to_string(),
        place_x,
        place_y,
        place_w,
        place_h,
        pixels_w,
        pixels_h,
    )?
    .ready()
    .await?;
    Ok(())
}

#[tauri::command]
pub fn copy_capture_region(app: AppHandle, x: i32, y: i32, width: u32, height: u32) -> Result<(), String> {
    library::copy_capture_region(&app.state::<AppState>(), &storage(&app)?, x, y, width, height)?;
    let _ = app.emit("capture-error", "已复制截图");
    Ok(())
}

pub async fn pin_clipboard_now(app: &AppHandle) -> Result<(), String> {
    let (image, x, y) = tauri::async_runtime::spawn_blocking(library::pin_clipboard).await.map_err(err)??;
    let width = image.width();
    let height = image.height();
    eprintln!("pin: clipboard memory at {x},{y} ({width}x{height})");
    let root = storage(app)?;
    ui::open_memory(app, root, image, x, y, width, height)?.ready().await?;
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
pub fn read_image_bytes(path: String) -> Result<tauri::ipc::Response, String> {
    let bytes = library::read_image_bytes(Path::new(&path))?;
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
    std::thread::sleep(std::time::Duration::from_millis(160));
    let captured = tauri::async_runtime::spawn_blocking(move || scroll_session::capture(frame)).await;
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
        if let Some(dest) = save_long_image(&app) {
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

fn save_long_image(app: &AppHandle) -> Option<PathBuf> {
    let file = app
        .dialog()
        .file()
        .add_filter("PNG", &["png"])
        .set_file_name("shotlet-scroll.png")
        .set_title("保存长图")
        .blocking_save_file()?;
    file.into_path().ok()
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
    app.opener().open_path(path, None::<&str>).map_err(err)
}
