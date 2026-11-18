use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::capture::{self, FrozenCapture};
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
    armed: bool,
}

fn overlay_label(index: usize) -> String {
    if index == 0 {
        OVERLAY.to_string()
    } else {
        format!("overlay-{index}")
    }
}

fn is_overlay_label(label: &str) -> bool {
    label == OVERLAY || label.starts_with("overlay-")
}

pub fn prepare_overlay(app: &AppHandle) -> Result<(), String> {
    let mut count = capture::monitor_rects().len();
    if count == 0 {
        count = 1;
    }
    for slot in 0..count {
        prepare_overlay_slot(app, slot)?;
    }
    Ok(())
}

fn prepare_overlay_slot(app: &AppHandle, slot: usize) -> Result<(), String> {
    let label = overlay_label(slot);
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.set_visible_on_all_workspaces(true);
        return Ok(());
    }
    WebviewWindowBuilder::new(app, label, view_url(&format!("view=overlay&slot={slot}")))
        .title("Shotlet")
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .shadow(false)
        .visible_on_all_workspaces(true)
        .accept_first_mouse(true)
        .disable_drag_drop_handler()
        .inner_size(800.0, 600.0)
        .build()
        .map_err(err)?;
    Ok(())
}

fn places_for(frozen: &FrozenCapture, generation: u64) -> Vec<OverlayPlace> {
    if frozen.screens.is_empty() {
        return vec![OverlayPlace {
            label: overlay_label(0),
            x: frozen.origin_x,
            y: frozen.origin_y,
            width: frozen.width,
            height: frozen.height,
            point_width: frozen.width,
            point_height: frozen.height,
            generation,
        }];
    }
    frozen
        .screens
        .iter()
        .enumerate()
        .map(|(index, screen)| OverlayPlace {
            label: overlay_label(index),
            x: screen.screen_x,
            y: screen.screen_y,
            width: screen.width,
            height: screen.height,
            point_width: if screen.point_width == 0 { screen.width } else { screen.point_width },
            point_height: if screen.point_height == 0 { screen.height } else { screen.point_height },
            generation,
        })
        .collect()
}

pub fn conceal_overlays(app: &AppHandle) {
    for label in overlay_labels(app) {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.hide();
        }
    }
}

pub fn present_overlays(app: &AppHandle, frozen: &FrozenCapture, generation: u64) -> Result<(), String> {
    let places = places_for(frozen, generation);
    let slots = places.len().max(1);
    for slot in 0..slots {
        prepare_overlay_slot(app, slot)?;
    }
    {
        let state = app.state::<AppState>();
        let mut pending = state.overlay_pending.lock().map_err(err)?;
        *pending = places.clone();
        if let Ok(mut aim) = state.overlay_aim.lock() {
            *aim = Some((frozen.origin_x, frozen.origin_y));
        }
    }
    hide_extra_overlays(app, places.len());
    let aim = (frozen.origin_x, frozen.origin_y);
    for place in &places {
        app.emit_to(
            &place.label,
            "overlay-show",
            OverlayShow {
                generation,
                armed: place.x == aim.0 && place.y == aim.1,
            },
        )
        .map_err(err)?;
    }
    Ok(())
}

pub fn reveal_overlay(window: &WebviewWindow, generation: u64) -> Result<(), String> {
    let label = window.label().to_string();
    let place = {
        let state = window.state::<AppState>();
        let mut pending = state.overlay_pending.lock().map_err(err)?;
        let index = pending.iter().position(|place| place.label == label && place.generation == generation);
        index.map(|index| pending.remove(index))
    };
    let Some(place) = place else {
        return Ok(());
    };
    let aim = window
        .state::<AppState>()
        .overlay_aim
        .lock()
        .ok()
        .and_then(|slot| *slot);
    let focus = aim == Some((place.x, place.y));
    place_overlay_window(window, &place, focus)
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn shotlet_place_overlay(
        window: *mut std::ffi::c_void,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        make_key: i32,
    ) -> i32;
}

fn point_size_of(screen: &capture::ScreenImage) -> (u32, u32) {
    let width = if screen.point_width == 0 { screen.width } else { screen.point_width };
    let height = if screen.point_height == 0 { screen.height } else { screen.point_height };
    (width, height)
}

fn rects_of(screens: &[capture::ScreenImage]) -> Vec<capture::MonitorRect> {
    screens
        .iter()
        .map(|screen| {
            let (width, height) = point_size_of(screen);
            capture::MonitorRect {
                x: screen.screen_x,
                y: screen.screen_y,
                width,
                height,
            }
        })
        .collect()
}

fn emit_overlay_arm(app: &AppHandle, screens: &[capture::ScreenImage], key: (i32, i32)) {
    for (slot, screen) in screens.iter().enumerate() {
        let armed = screen.screen_x == key.0 && screen.screen_y == key.1;
        let _ = app.emit_to(overlay_label(slot), "overlay-arm", armed);
    }
}

fn focus_overlay(app: &AppHandle, screens: &[capture::ScreenImage], index: usize) {
    show_overlay_screen(app, screens, index, true);
}

fn show_overlay_screen(app: &AppHandle, screens: &[capture::ScreenImage], index: usize, make_key: bool) {
    let Some(screen) = screens.get(index) else {
        return;
    };
    let Some(window) = app.get_webview_window(&overlay_label(index)) else {
        return;
    };
    // A window that failed to appear is still the one for this display. Place it
    // again instead of bailing out, or the other screen can never be selected.
    let (point_width, point_height) = point_size_of(screen);
    let place = OverlayPlace {
        label: overlay_label(index),
        x: screen.screen_x,
        y: screen.screen_y,
        width: screen.width,
        height: screen.height,
        point_width,
        point_height,
        generation: 0,
    };
    #[cfg(target_os = "macos")]
    {
        let _ = place_mac_overlay(&window, &place, make_key);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = move_overlay_window(&window, &place);
        let _ = window.show();
    }
    if make_key {
        let _ = window.set_focus();
    }
}

fn raise_hidden_overlays(app: &AppHandle, screens: &[capture::ScreenImage], key_index: usize) {
    for index in 0..screens.len() {
        if index == key_index {
            continue;
        }
        let Some(window) = app.get_webview_window(&overlay_label(index)) else {
            continue;
        };
        if window.is_visible().unwrap_or(false) {
            continue;
        }
        show_overlay_screen(app, screens, index, false);
    }
}

pub fn aim_overlay_at(app: &AppHandle, x: i32, y: i32) {
    let state = app.state::<AppState>();
    let screens = match state.capture.lock() {
        Ok(slot) => slot.as_ref().map(|item| item.screens.clone()).unwrap_or_default(),
        Err(_) => return,
    };
    let generation = state.overlay_generation.load(Ordering::Relaxed);
    drop(state);
    let rects = rects_of(&screens);
    let Some(monitor) = capture::monitor_at_point(&rects, x, y) else {
        return;
    };
    let key = (monitor.x, monitor.y);
    let Some(index) = screens.iter().position(|screen| screen.screen_x == key.0 && screen.screen_y == key.1) else {
        return;
    };
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        let state = app.state::<AppState>();
        if state.overlay_generation.load(Ordering::Relaxed) != generation {
            return;
        }
        let aimed = state.overlay_aim.lock().ok().and_then(|slot| *slot);
        if aimed == Some(key) {
            drop(state);
            // Pointer enter can race a synthesized mouseleave that already dimmed this screen.
            emit_overlay_arm(&app, &screens, key);
            return;
        }
        if let Ok(mut aim) = state.overlay_aim.lock() {
            *aim = Some(key);
        }
        drop(state);
        focus_overlay(&app, &screens, index);
        emit_overlay_arm(&app, &screens, key);
    });
}

fn place_overlay_window(window: &WebviewWindow, place: &OverlayPlace, focus: bool) -> Result<(), String> {
    // Do not hide() first. Tauri queues Hide, the native orderFront runs, then
    // the event loop orderOuts the window. The screen without a later set_focus
    // stays invisible, so the pointer can never select it.
    #[cfg(target_os = "macos")]
    {
        place_mac_overlay(window, place, focus)?;
        if focus {
            let _ = window.set_focus();
        }
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
    {
        move_overlay_window(window, place)?;
        window.show().map_err(err)?;
        move_overlay_window(window, place)?;
        if focus {
            let _ = window.set_focus();
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn place_mac_overlay(window: &WebviewWindow, place: &OverlayPlace, focus: bool) -> Result<(), String> {
    let point_w = if place.point_width == 0 { place.width } else { place.point_width };
    let point_h = if place.point_height == 0 { place.height } else { place.point_height };
    let raw = window.ns_window().map_err(err)?;
    let matched = unsafe {
        shotlet_place_overlay(
            raw,
            f64::from(place.x),
            f64::from(place.y),
            f64::from(point_w),
            f64::from(point_h),
            i32::from(focus),
        )
    };
    if matched == 0 {
        eprintln!(
            "shotlet overlay used fallback frame at {},{} {}x{}",
            place.x, place.y, point_w, point_h
        );
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn move_overlay_window(window: &WebviewWindow, place: &OverlayPlace) -> Result<(), String> {
    window.set_position(PhysicalPosition::new(place.x, place.y)).map_err(err)?;
    window.set_size(PhysicalSize::new(place.width, place.height)).map_err(err)?;
    Ok(())
}

fn hide_extra_overlays(app: &AppHandle, count: usize) {
    let mut slot = count;
    while slot < 32 {
        let Some(window) = app.get_webview_window(&overlay_label(slot)) else {
            break;
        };
        let _ = window.hide();
        slot += 1;
    }
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
    static FOCUSING: AtomicBool = AtomicBool::new(false);
    let mut stable: Option<(i32, i32)> = None;
    let mut stable_hits = 0u8;
    let mut pulse = 0u8;
    loop {
        std::thread::sleep(Duration::from_millis(40));
        let state = app.state::<AppState>();
        if !state.overlay_follow.load(Ordering::Relaxed) {
            stable = None;
            stable_hits = 0;
            pulse = 0;
            continue;
        }
        let screens = match state.capture.lock() {
            Ok(slot) => slot.as_ref().map(|item| item.screens.clone()).unwrap_or_default(),
            Err(_) => continue,
        };
        if screens.len() < 2 {
            continue;
        }
        let Some((cursor_x, cursor_y)) = capture::cursor_point() else { continue };
        let rects = rects_of(&screens);
        let Some(monitor) = capture::monitor_at_point(&rects, cursor_x, cursor_y) else { continue };
        let key = (monitor.x, monitor.y);
        if stable == Some(key) {
            stable_hits = stable_hits.saturating_add(1);
        } else {
            stable = Some(key);
            stable_hits = 1;
            pulse = 0;
        }
        if stable_hits < 2 {
            continue;
        }
        let aimed = state.overlay_aim.lock().ok().and_then(|slot| *slot);
        let changed = aimed != Some(key);
        pulse = pulse.wrapping_add(1);
        // A synthesized mouseleave can dim the screen after it was armed. Re-apply
        // the highlight while the pointer stays, but don't steal focus again.
        if !changed && pulse % 8 != 0 {
            continue;
        }
        let generation = state.overlay_generation.load(Ordering::Relaxed);
        drop(state);
        let Some(index) = screens.iter().position(|screen| screen.screen_x == key.0 && screen.screen_y == key.1) else {
            continue;
        };
        if changed && FOCUSING.swap(true, Ordering::SeqCst) {
            continue;
        }
        let app_for_window = app.clone();
        let screens_for_arm = screens;
        let queued = app_for_window.clone().run_on_main_thread(move || {
            let release = || {
                if changed {
                    FOCUSING.store(false, Ordering::SeqCst);
                }
            };
            let state = app_for_window.state::<AppState>();
            if !state.overlay_follow.load(Ordering::Relaxed) || state.overlay_generation.load(Ordering::Relaxed) != generation {
                release();
                return;
            }
            let cursor_window = app_for_window.get_webview_window(&overlay_label(index));
            let cursor_hidden = cursor_window.as_ref().is_some_and(|window| !window.is_visible().unwrap_or(false));
            if changed {
                if cursor_window.is_none() {
                    release();
                    return;
                }
                if let Ok(mut aim) = state.overlay_aim.lock() {
                    *aim = Some(key);
                }
            }
            drop(state);
            // Hidden overlays still belong on their own displays. Show those first,
            // then key the one under the pointer so it stays the front window.
            raise_hidden_overlays(&app_for_window, &screens_for_arm, index);
            if changed || cursor_hidden {
                focus_overlay(&app_for_window, &screens_for_arm, index);
            }
            emit_overlay_arm(&app_for_window, &screens_for_arm, key);
            release();
        });
        if queued.is_err() && changed {
            FOCUSING.store(false, Ordering::SeqCst);
        }
    }
}

fn overlay_labels(app: &AppHandle) -> Vec<String> {
    app.webview_windows()
        .keys()
        .filter(|label| is_overlay_label(label))
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
            pending.clear();
        }
    }
    for label in overlay_labels(app) {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.hide();
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

/// Physical pixels of transparent margin around a pin. Must match `PIN_GLOW` in src/lib/pin.ts.
const PIN_GLOW: i32 = 22;

pub fn open_pin(app: &AppHandle, path: String, x: i32, y: i32, width: u32, height: u32) -> Result<(), String> {
    let id = new_id();
    let state = app.state::<AppState>();
    state.pins.lock().map_err(err)?.insert(id.clone(), path);
    let group = *state.pin_group.lock().map_err(err)?;
    state.pin_groups.lock().map_err(err)?.insert(id.clone(), group);
    if let Ok(mut hidden) = state.pins_hidden.lock() {
        *hidden = false;
    }
    let width = width.max(1);
    let height = height.max(1);
    let label = format!("pin-{id}");
    // x/y/w/h stay the picture's cut. The window is larger so the edge light
    // is drawn in the transparent margin and the pixels stay on the selection.
    let frame_x = x - PIN_GLOW;
    let frame_y = y - PIN_GLOW;
    let frame_w = width.saturating_add((PIN_GLOW as u32).saturating_mul(2));
    let frame_h = height.saturating_add((PIN_GLOW as u32).saturating_mul(2));
    let query = format!("view=pin&id={id}&x={x}&y={y}&w={width}&h={height}");
    eprintln!("pin: open {label} at {x},{y} size {width}x{height} frame {frame_x},{frame_y} {frame_w}x{frame_h}");
    on_main(app, move |app| {
        let window = WebviewWindowBuilder::new(app, &label, view_url(&query))
            .title("Shotlet Pin")
            .decorations(false)
            .always_on_top(true)
            .resizable(true)
            .transparent(true)
            .shadow(false)
            .skip_taskbar(true)
            .focused(true)
            .visible(false)
            .inner_size(frame_w as f64, frame_h as f64)
            .min_inner_size(1.0, 1.0)
            .background_color(tauri::utils::config::Color(0, 0, 0, 0))
            .initialization_script(
                "document.documentElement.dataset.view='pin';document.documentElement.style.background='transparent';document.addEventListener('DOMContentLoaded',function(){document.body.style.margin='0';document.body.style.background='transparent';});",
            )
            .on_page_load(|window, payload| {
                if !matches!(payload.event(), PageLoadEvent::Finished) {
                    return;
                }
                let _ = window.show();
                let _ = window.set_always_on_top(true);
                let _ = window.set_focus();
            })
            .build();
        let window = match window {
            Ok(window) => window,
            Err(error) => {
                eprintln!("pin: create window failed: {error}");
                return;
            }
        };
        if let Err(error) = window.set_position(PhysicalPosition::new(frame_x, frame_y)) {
            eprintln!("pin: move window failed: {error}");
        }
        if let Err(error) = window.set_size(PhysicalSize::new(frame_w, frame_h)) {
            eprintln!("pin: size window failed: {error}");
        }
        // Set both again. macOS sometimes applies the first size before the
        // window has a screen, which leaves the sticker a few pixels off.
        if let Err(error) = window.set_position(PhysicalPosition::new(frame_x, frame_y)) {
            eprintln!("pin: move window failed: {error}");
        }
        if let Err(error) = window.set_size(PhysicalSize::new(frame_w, frame_h)) {
            eprintln!("pin: size window failed: {error}");
        }
        if let Err(error) = window.set_always_on_top(true) {
            eprintln!("pin: always-on-top failed: {error}");
        }
        if let Err(error) = window.show() {
            eprintln!("pin: show window failed: {error}");
        }
        let _ = window.set_focus();
        // Hide the capture layer only after the sticker window exists, so a
        // failed build does not look like the pin button did nothing.
        dismiss_overlay(app);
        eprintln!("pin: window shown {label}");
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
