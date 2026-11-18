use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::capture::{self, ScreenImage};
use crate::settings::{self, Settings};
use crate::state::{AppState, OverlayPlace};
use crate::util::{err, new_id};

const OVERLAY: &str = "overlay";

pub fn view_url(query: &str) -> WebviewUrl {
    if cfg!(debug_assertions) {
        let url = format!("http://localhost:1420/?{query}");
        WebviewUrl::External(url.parse().expect("dev url"))
    } else {
        WebviewUrl::App(format!("index.html?{query}").into())
    }
}

fn on_main(app: &AppHandle, task: impl FnOnce(&AppHandle) + Send + 'static) -> Result<(), String> {
    let app = app.clone();
    // Already-on-main-thread callbacks run inline. Creating a WebView2 window from
    // inside the IPC handler that is still on that thread deadlocks Windows, and the
    // fullscreen capture overlay then swallows the whole desktop. Queue the task from
    // another thread so it runs on a later turn of the event loop.
    std::thread::spawn(move || {
        let app_for_task = app.clone();
        let _ = app.run_on_main_thread(move || task(&app_for_task));
    });
    Ok(())
}

pub fn on_main_wait<T: Send + 'static>(
    app: &AppHandle,
    task: impl FnOnce(&AppHandle) -> T + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let app = app.clone();
    app.clone()
        .run_on_main_thread(move || {
            let _ = tx.send(task(&app));
        })
        .map_err(err)?;
    rx.recv_timeout(Duration::from_secs(2))
        .map_err(|_| "界面线程没有响应".to_string())
}

pub fn open_history(app: &AppHandle) {
    let app = app.clone();
    let _ = on_main(&app, move |app| {
        let _ = show_history(app);
    });
}

pub fn open_settings(app: &AppHandle) {
    let app = app.clone();
    let _ = on_main(&app, move |app| {
        let _ = show_settings(app);
    });
}

pub fn open_home(app: &AppHandle) {
    let app = app.clone();
    let _ = on_main(&app, move |app| {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    });
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlayShow {
    generation: u64,
    path: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    screen_x: i32,
    screen_y: i32,
}

pub fn prepare_overlay(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window(OVERLAY).is_some() {
        return Ok(());
    }
    WebviewWindowBuilder::new(app, OVERLAY, view_url("view=overlay"))
        .title("Shotlet")
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .shadow(false)
        .disable_drag_drop_handler()
        .inner_size(800.0, 600.0)
        .build()
        .map_err(err)?;
    Ok(())
}

pub fn stage_overlay(app: &AppHandle, screen: &ScreenImage, generation: u64) -> Result<(), String> {
    prepare_overlay(app)?;
    if app.get_webview_window(OVERLAY).is_none() {
        return Err("截屏窗口没有准备好".into());
    }
    {
        let state = app.state::<AppState>();
        let mut pending = state.overlay_pending.lock().map_err(err)?;
        *pending = Some(OverlayPlace {
            x: screen.screen_x,
            y: screen.screen_y,
            width: screen.width,
            height: screen.height,
            generation,
        });
    }
    app.emit_to(
        OVERLAY,
        "overlay-show",
        OverlayShow {
            generation,
            path: screen.path.clone(),
            x: screen.x,
            y: screen.y,
            width: screen.width,
            height: screen.height,
            screen_x: screen.screen_x,
            screen_y: screen.screen_y,
        },
    )
    .map_err(err)?;
    Ok(())
}

pub fn reveal_overlay(app: &AppHandle, generation: u64) -> Result<(), String> {
    let place = {
        let state = app.state::<AppState>();
        let mut pending = state.overlay_pending.lock().map_err(err)?;
        match pending.as_ref() {
            Some(place) if place.generation == generation => pending.take(),
            _ => None,
        }
    };
    let Some(place) = place else {
        return Ok(());
    };
    let window = app.get_webview_window(OVERLAY).ok_or("截屏窗口没有准备好")?;
    let _ = window.hide();
    window.set_position(PhysicalPosition::new(place.x, place.y)).map_err(err)?;
    window.set_size(PhysicalSize::new(place.width, place.height)).map_err(err)?;
    window.show().map_err(err)?;
    window.set_position(PhysicalPosition::new(place.x, place.y)).map_err(err)?;
    window.set_size(PhysicalSize::new(place.width, place.height)).map_err(err)?;
    let _ = window.set_focus();
    if let Ok(mut aim) = app.state::<AppState>().overlay_aim.lock() {
        *aim = Some((place.x, place.y));
    }
    Ok(())
}

pub fn set_overlay_follow(app: &AppHandle, follow: bool) {
    let state = app.state::<AppState>();
    state.overlay_follow.store(follow, Ordering::Relaxed);
    if !follow {
        state.overlay_epoch.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn begin_follow(app: &AppHandle) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || follow_loop(app));
}

fn follow_loop(app: AppHandle) {
    let mut rects = Vec::new();
    let mut refreshed = Instant::now() - Duration::from_secs(10);
    let mut stable: Option<(i32, i32)> = None;
    let mut stable_hits = 0u8;
    loop {
        std::thread::sleep(Duration::from_millis(40));
        let state = app.state::<AppState>();
        if !state.overlay_follow.load(Ordering::Relaxed) {
            stable = None;
            stable_hits = 0;
            continue;
        }
        if refreshed.elapsed() > Duration::from_millis(500) {
            rects = capture::monitor_rects();
            refreshed = Instant::now();
        }
        let Some((cursor_x, cursor_y)) = capture::cursor_point() else { continue };
        let Some(monitor) = capture::monitor_at_point(&rects, cursor_x, cursor_y) else { continue };
        let key = (monitor.x, monitor.y);
        if stable == Some(key) {
            stable_hits = stable_hits.saturating_add(1);
        } else {
            stable = Some(key);
            stable_hits = 1;
        }
        if stable_hits < 2 {
            continue;
        }
        let aimed = state.overlay_aim.lock().ok().and_then(|slot| *slot);
        if aimed == Some(key) {
            continue;
        }
        let epoch = state.overlay_epoch.load(Ordering::Relaxed);
        drop(state);
        let frozen = match capture::freeze_under_cursor() {
            Ok(frozen) => frozen,
            Err(error) => {
                eprintln!("overlay follow: {error}");
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
        };
        let Some(screen) = frozen.screens.first().cloned() else { continue };
        let state = app.state::<AppState>();
        if !state.overlay_follow.load(Ordering::Relaxed) || state.overlay_epoch.load(Ordering::Relaxed) != epoch {
            continue;
        }
        let still = capture::cursor_point().and_then(|(x, y)| capture::monitor_at_point(&rects, x, y));
        if still.map(|item| (item.x, item.y)) != Some((screen.screen_x, screen.screen_y)) {
            continue;
        }
        let generation = state.overlay_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let mode = state
            .capture
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|item| item.mode.clone()))
            .unwrap_or_else(|| "edit".into());
        let mut frozen = frozen;
        frozen.generation = generation;
        frozen.mode = mode;
        frozen.preset = None;
        if let Ok(mut slot) = state.capture.lock() {
            *slot = Some(frozen);
        }
        if let Ok(mut aim) = state.overlay_aim.lock() {
            *aim = Some((screen.screen_x, screen.screen_y));
        }
        drop(state);
        let app_for_window = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Err(error) = stage_overlay(&app_for_window, &screen, generation) {
                eprintln!("overlay follow: {error}");
            }
        });
    }
}

fn overlay_labels(app: &AppHandle) -> Vec<String> {
    app.webview_windows()
        .keys()
        .filter(|label| label.starts_with("overlay"))
        .cloned()
        .collect()
}

pub fn hide_overlay(app: &AppHandle) {
    dismiss_overlay(app);
}

pub fn close_overlay(app: &AppHandle) {
    dismiss_overlay(app);
}

fn dismiss_overlay(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.overlay_follow.store(false, Ordering::Relaxed);
        state.overlay_epoch.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut aim) = state.overlay_aim.lock() {
            *aim = None;
        }
        if let Ok(mut pending) = state.overlay_pending.lock() {
            *pending = None;
        }
    }
    for label in overlay_labels(app) {
        if let Some(window) = app.get_webview_window(&label) {
            if label == OVERLAY {
                let _ = window.hide();
            } else {
                let _ = window.close();
            }
        }
    }
}

fn show_history(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("history") {
        window.show().ok();
        window.unminimize().ok();
        window.set_focus().ok();
        return Ok(());
    }
    WebviewWindowBuilder::new(app, "history", view_url("view=history"))
        .title("Shotlet 历史")
        .inner_size(980.0, 680.0)
        .min_inner_size(720.0, 480.0)
        .center()
        .build()
        .map(|_| ())
        .map_err(err)
}

fn show_settings(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("settings") {
        window.show().ok();
        window.unminimize().ok();
        window.set_focus().ok();
        return Ok(());
    }
    WebviewWindowBuilder::new(app, "settings", view_url("view=settings"))
        .title("Shotlet 设置")
        .inner_size(560.0, 900.0)
        .resizable(false)
        .center()
        .build()
        .map(|_| ())
        .map_err(err)
}

pub fn open_editor(app: &AppHandle) -> Result<(), String> {
    let app = app.clone();
    on_main(&app, move |app| {
        if let Some(window) = app.get_webview_window("editor") {
            let _ = window.close();
        }
        let _ = WebviewWindowBuilder::new(app, "editor", view_url("view=editor"))
            .title("Shotlet 编辑")
            .inner_size(1100.0, 760.0)
            .min_inner_size(760.0, 520.0)
            .center()
            .build();
    })
}

pub fn open_pin(app: &AppHandle, path: String, x: i32, y: i32) -> Result<(), String> {
    let id = new_id();
    let state = app.state::<AppState>();
    state.pins.lock().map_err(err)?.insert(id.clone(), path);
    let group = *state.pin_group.lock().map_err(err)?;
    state.pin_groups.lock().map_err(err)?.insert(id.clone(), group);
    if let Ok(mut hidden) = state.pins_hidden.lock() {
        *hidden = false;
    }
    let label = format!("pin-{id}");
    let query = format!("view=pin&id={id}");
    on_main(app, move |app| {
        dismiss_overlay(app);
        let Ok(window) = WebviewWindowBuilder::new(app, &label, view_url(&query))
            .title("Shotlet Pin")
            .decorations(false)
            .always_on_top(true)
            .resizable(true)
            .transparent(true)
            .shadow(false)
            .skip_taskbar(true)
            .inner_size(420.0, 280.0)
            .visible(false)
            .build()
        else {
            return;
        };
        let _ = window.set_position(PhysicalPosition::new(x, y));
        let _ = window.show();
    })
}

const RECORDER_W: f64 = 360.0;
const RECORDER_H: f64 = 112.0;

fn recorder_origin(x: i32, y: i32, height: u32) -> (i32, i32) {
    let bar_w = RECORDER_W as i32;
    let bar_h = RECORDER_H as i32;
    let mut bar_x = x;
    let mut bar_y = y.saturating_add(height as i32).saturating_add(16);
    let rects = capture::monitor_rects();
    if let Some(monitor) = capture::monitor_at_point(&rects, x.saturating_add(8), y.saturating_add(8)) {
        let right = monitor.x + monitor.width as i32;
        let bottom = monitor.y + monitor.height as i32;
        if bar_x + bar_w > right - 8 {
            bar_x = right - bar_w - 8;
        }
        if bar_x < monitor.x + 8 {
            bar_x = monitor.x + 8;
        }
        if bar_y + bar_h > bottom - 8 {
            bar_y = y - bar_h - 16;
        }
        if bar_y < monitor.y + 8 {
            bar_y = monitor.y + 8;
        }
    }
    (bar_x, bar_y)
}

fn place_recorder_window(window: &WebviewWindow, x: i32, y: i32) -> Result<(), String> {
    window.set_position(PhysicalPosition::new(x, y)).map_err(err)?;
    window.set_size(LogicalSize::new(RECORDER_W, RECORDER_H + 1.0)).map_err(err)?;
    window.show().map_err(err)?;
    window.set_size(LogicalSize::new(RECORDER_W, RECORDER_H)).map_err(err)?;
    let _ = window.set_focus();
    Ok(())
}

fn present_recorder(app: &AppHandle, x: i32, y: i32, height: u32) -> Result<(), String> {
    let (bar_x, bar_y) = recorder_origin(x, y, height);
    if let Some(window) = app.get_webview_window("recorder") {
        return place_recorder_window(&window, bar_x, bar_y);
    }
    let window = WebviewWindowBuilder::new(app, "recorder", view_url("view=recorder"))
        .title("Shotlet 录制")
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .resizable(false)
        .skip_taskbar(true)
        .visible(false)
        .inner_size(RECORDER_W, RECORDER_H)
        .min_inner_size(280.0, 96.0)
        .background_color(tauri::utils::config::Color(28, 25, 21, 255))
        .initialization_script(
            "window.__SHOTLET_VIEW__='recorder';document.addEventListener('DOMContentLoaded',function(){document.documentElement.style.background='#1c1915';document.body.style.margin='0';document.body.style.background='#1c1915';document.body.style.color='#fff';});",
        )
        .on_page_load(|window, payload| {
            if !matches!(payload.event(), PageLoadEvent::Finished) {
                return;
            }
            let _ = window.set_size(LogicalSize::new(RECORDER_W, RECORDER_H));
            let _ = window.show();
            let _ = window.set_focus();
        })
        .build()
        .map_err(err)?;
    place_recorder_window(&window, bar_x, bar_y)
}

pub fn open_recorder(app: &AppHandle, x: i32, y: i32, height: u32) -> Result<(), String> {
    on_main_wait(app, move |app| present_recorder(app, x, y, height))?
}

pub fn close_recorder(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("recorder") {
        let _ = window.close();
    }
}

pub fn toggle_pins(app: &AppHandle) {
    let state = app.state::<AppState>();
    let hidden = {
        let Ok(mut flag) = state.pins_hidden.lock() else {
            return;
        };
        *flag = !*flag;
        *flag
    };
    let group = state.pin_group.lock().map(|value| *value).unwrap_or(0);
    let groups = state.pin_groups.lock().map(|value| value.clone()).unwrap_or_default();
    let app = app.clone();
    let _ = on_main(&app, move |app| {
        set_pin_visibility(app, group, &groups, !hidden);
        let message = if hidden { "已隐藏贴图" } else { "已显示贴图" };
        let _ = app.emit("capture-error", message);
    });
}

pub fn switch_pin_group(app: &AppHandle) {
    let state = app.state::<AppState>();
    let next = {
        let Ok(mut group) = state.pin_group.lock() else {
            return;
        };
        *group = if *group == 0 { 1 } else { 0 };
        *group
    };
    if let Ok(mut hidden) = state.pins_hidden.lock() {
        *hidden = false;
    }
    let groups = state.pin_groups.lock().map(|value| value.clone()).unwrap_or_default();
    let app = app.clone();
    let _ = on_main(&app, move |app| {
        set_pin_visibility(app, next, &groups, true);
        let _ = app.emit("capture-error", format!("已切换到贴图组 {next}"));
    });
}

fn set_pin_visibility(app: &AppHandle, group: u8, groups: &HashMap<String, u8>, show_current: bool) {
    for (label, window) in app.webview_windows() {
        let Some(id) = label.strip_prefix("pin-") else {
            continue;
        };
        let mine = groups.get(id).copied().unwrap_or(0) == group;
        if show_current && mine {
            let _ = window.show();
        } else {
            let _ = window.hide();
        }
    }
}

pub fn install_tray(app: &AppHandle) -> Result<(), String> {
    let menu = tray_menu(app)?;
    // Keeps the proc-macro icon in sync when icons/32x32.png changes.
    let _tray_icon_stamp: u64 = include!(concat!(env!("OUT_DIR"), "/tray_icon_stamp.rs"));
    let icon = tauri::include_image!("icons/32x32.png");
    TrayIconBuilder::with_id("tray")
        .icon(icon)
        .menu(&menu)
        .tooltip("Shotlet")
        .on_menu_event(|app, event| dispatch_menu(app, event.id.as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                open_home(tray.app_handle());
            }
        })
        .build(app)
        .map(|_| ())
        .map_err(err)
}

pub fn rebuild_tray_menu(app: &AppHandle) -> Result<(), String> {
    let menu = tray_menu(app)?;
    if let Some(tray) = app.tray_by_id("tray") {
        tray.set_menu(Some(menu)).map_err(err)?;
    }
    Ok(())
}

fn dispatch_menu(app: &AppHandle, id: &str) {
    match id {
        "capture" => crate::commands::spawn_launch(app, "edit"),
        "capture-copy" => crate::commands::spawn_launch(app, "copy"),
        "capture-window" => crate::commands::spawn_launch(app, "window"),
        "pin" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = crate::commands::pin_clipboard_now(&app).await {
                    let _ = app.emit("capture-error", error);
                }
            });
        }
        "hide-pins" => toggle_pins(app),
        "pin-group" => switch_pin_group(app),
        "history" => open_history(app),
        "settings" => open_settings(app),
        "quit" => app.exit(0),
        _ => {}
    }
}

fn tray_menu(app: &AppHandle) -> Result<Menu<tauri::Wry>, String> {
    let settings = app.state::<AppState>().settings.lock().map_err(err)?.clone();
    let zh = settings::resolved_language(&settings) == "zh";
    let capture = item(app, "capture", &action_label(if zh { "截屏" } else { "Snip" }, &settings.capture_shortcut))?;
    let capture_copy = item(
        app,
        "capture-copy",
        &action_label(if zh { "截屏并自动复制" } else { "Snip and copy" }, &settings.capture_copy_shortcut),
    )?;
    let capture_window = item(
        app,
        "capture-window",
        &action_label(if zh { "自定义截屏" } else { "Custom snip" }, &settings.window_shortcut),
    )?;
    let pin = item(app, "pin", &action_label(if zh { "贴图" } else { "Pin" }, &settings.pin_shortcut))?;
    let hide_pins = item(
        app,
        "hide-pins",
        &action_label(if zh { "隐藏/显示所有贴图" } else { "Hide or show pins" }, &settings.hide_pins_shortcut),
    )?;
    let pin_group = item(
        app,
        "pin-group",
        &action_label(if zh { "切换到另一贴图组" } else { "Switch pin group" }, &settings.pin_group_shortcut),
    )?;
    let separator = PredefinedMenuItem::separator(app).map_err(err)?;
    let history = item(app, "history", if zh { "历史" } else { "History" })?;
    let preferences = item(app, "settings", if zh { "设置" } else { "Settings" })?;
    let quit = item(app, "quit", if zh { "退出" } else { "Quit" })?;
    Menu::with_items(
        app,
        &[
            &capture,
            &capture_copy,
            &capture_window,
            &pin,
            &hide_pins,
            &pin_group,
            &separator,
            &history,
            &preferences,
            &quit,
        ],
    )
    .map_err(err)
}

fn action_label(name: &str, shortcut: &str) -> String {
    let key = pretty_shortcut(shortcut);
    if key.is_empty() { name.to_string() } else { format!("{name}    {key}") }
}

fn pretty_shortcut(shortcut: &str) -> String {
    shortcut
        .split('+')
        .filter(|part| !part.is_empty())
        .map(|part| match part {
            "Control" => "Ctrl",
            "Super" => "Win",
            other if other.starts_with("Key") && other.len() == 4 => &other[3..],
            other if other.starts_with("Digit") && other.len() == 6 => &other[5..],
            other => other,
        })
        .collect::<Vec<_>>()
        .join("+")
}

fn item(app: &AppHandle, id: &str, label: &str) -> Result<MenuItem<tauri::Wry>, String> {
    MenuItem::with_id(app, id, label, true, None::<&str>).map_err(err)
}

pub fn app_data_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path().app_data_dir().map_err(err)
}

pub fn read_settings(app: &AppHandle) -> Result<Settings, String> {
    Ok(app.state::<AppState>().settings.lock().map_err(err)?.clone())
}
