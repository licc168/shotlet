use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::capture::{self, FrozenCapture};
use crate::pin_session::{self, PinCut, PinFrame};
use crate::settings::{self, Settings};
use crate::state::{AppState, OverlayPlace};
use crate::util::{err, new_id};

const OVERLAY: &str = "overlay";

/// The overlay window is up. The page only hears Esc after that webview has keyboard
/// focus, which a borderless show often does not get.
static OVERLAY_OPEN: AtomicBool = AtomicBool::new(false);
static OVERLAY_HWNDS: Mutex<Vec<isize>> = Mutex::new(Vec::new());

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
    on_main_wait_for(app, Duration::from_secs(2), task)
}

pub fn on_main_wait_for<T: Send + 'static>(
    app: &AppHandle,
    timeout: Duration,
    task: impl FnOnce(&AppHandle) -> T + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let app = app.clone();
    app.clone()
        .run_on_main_thread(move || {
            let _ = tx.send(task(&app));
        })
        .map_err(err)?;
    rx.recv_timeout(timeout).map_err(|_| "界面线程没有响应".to_string())
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
    /// Which overlay window this show belongs to. Every overlay page hears every emit.
    slot: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlayArm {
    slot: usize,
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

fn place_of(screen: &capture::ScreenImage, index: usize, generation: u64) -> OverlayPlace {
    OverlayPlace {
        label: overlay_label(index),
        x: screen.screen_x,
        y: screen.screen_y,
        width: screen.width,
        height: screen.height,
        point_width: if screen.point_width == 0 { screen.width } else { screen.point_width },
        point_height: if screen.point_height == 0 { screen.height } else { screen.point_height },
        generation,
    }
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
    frozen.screens.iter().enumerate().map(|(index, screen)| place_of(screen, index, generation)).collect()
}

/// Hides every overlay. Returns whether one was on screen, so the grab can wait
/// for that window to leave the pixels.
pub fn conceal_overlays(app: &AppHandle) -> bool {
    let mut covered = false;
    for label in overlay_labels(app) {
        if let Some(window) = app.get_webview_window(&label) {
            if window.is_visible().unwrap_or(false) {
                covered = true;
            }
            let _ = window.hide();
        }
    }
    covered
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
    OVERLAY_OPEN.store(true, Ordering::Relaxed);
    let aim = (frozen.origin_x, frozen.origin_y);
    for (slot, place) in places.iter().enumerate() {
        // Show before the page paints. A hidden WebView throttles the page, so the
        // frozen pixels would sit for about a second after F1.
        let _ = show_overlay_place(app, place, place.x == aim.0 && place.y == aim.1);
        app.emit_to(
            &place.label,
            "overlay-show",
            OverlayShow {
                generation,
                armed: place.x == aim.0 && place.y == aim.1,
                slot,
            },
        )
        .map_err(err)?;
    }
    note_overlay_hwnds(app);
    Ok(())
}

pub fn present_more_overlays(app: &AppHandle, frozen: &FrozenCapture, generation: u64, from: usize) -> Result<(), String> {
    if from >= frozen.screens.len() {
        return Ok(());
    }
    let aim = (frozen.origin_x, frozen.origin_y);
    let mut extra = Vec::new();
    for index in from..frozen.screens.len() {
        prepare_overlay_slot(app, index)?;
        extra.push(place_of(&frozen.screens[index], index, generation));
    }
    {
        let state = app.state::<AppState>();
        let mut pending = state.overlay_pending.lock().map_err(err)?;
        pending.extend(extra.iter().cloned());
    }
    for (offset, place) in extra.iter().enumerate() {
        let _ = show_overlay_place(app, place, place.x == aim.0 && place.y == aim.1);
        app.emit_to(
            &place.label,
            "overlay-show",
            OverlayShow {
                generation,
                armed: place.x == aim.0 && place.y == aim.1,
                slot: from + offset,
            },
        )
        .map_err(err)?;
    }
    note_overlay_hwnds(app);
    Ok(())
}

fn note_overlay_hwnds(app: &AppHandle) {
    let mut keys = Vec::new();
    #[cfg(windows)]
    {
        for label in overlay_labels(app) {
            let Some(window) = app.get_webview_window(&label) else { continue };
            if !window.is_visible().unwrap_or(false) {
                continue;
            }
            if let Ok(hwnd) = window.hwnd() {
                keys.push(hwnd.0 as isize);
            }
        }
    }
    if let Ok(mut slot) = OVERLAY_HWNDS.lock() {
        *slot = keys;
    }
}

fn overlay_has_foreground() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        let foreground = unsafe { GetForegroundWindow() };
        let raw = foreground.0 as isize;
        if raw == 0 {
            return false;
        }
        return OVERLAY_HWNDS.lock().map(|slot| slot.contains(&raw)).unwrap_or(false);
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn show_overlay_place(app: &AppHandle, place: &OverlayPlace, focus: bool) -> Result<(), String> {
    let Some(window) = app.get_webview_window(&place.label) else {
        return Ok(());
    };
    place_overlay_window(&window, place, focus)
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
        let _ = app.emit_to(overlay_label(slot), "overlay-arm", OverlayArm { slot, armed });
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
    let index = if let Some(index) = screens.iter().position(|screen| screen.screen_x == x && screen.screen_y == y) {
        index
    } else {
        let rects = rects_of(&screens);
        let Some(monitor) = capture::monitor_at_point(&rects, x, y) else {
            return;
        };
        let Some(index) = screens.iter().position(|screen| screen.screen_x == monitor.x && screen.screen_y == monitor.y) else {
            return;
        };
        index
    };
    let Some(screen) = screens.get(index) else {
        return;
    };
    let key = (screen.screen_x, screen.screen_y);
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        let state = app.state::<AppState>();
        if state.overlay_generation.load(Ordering::Relaxed) != generation {
            return;
        }
        let aimed = state.overlay_aim.lock().ok().and_then(|slot| *slot);
        if aimed == Some(key) {
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
    // The bitmap can be larger than the monitor slot. Sizing the window to the bitmap
    // lets the left screen cover the right one, so the right overlay never receives the pointer.
    let width = if place.point_width == 0 { place.width } else { place.point_width };
    let height = if place.point_height == 0 { place.height } else { place.point_height };
    window.set_position(PhysicalPosition::new(place.x, place.y)).map_err(err)?;
    window.set_size(PhysicalSize::new(width, height)).map_err(err)?;
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

fn escape_key_down() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
        return unsafe { GetAsyncKeyState(i32::from(VK_ESCAPE.0)) } < 0;
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn follow_loop(app: AppHandle) {
    static FOCUSING: AtomicBool = AtomicBool::new(false);
    let mut stable: Option<(i32, i32)> = None;
    let mut stable_hits = 0u8;
    let mut escape_was_down = false;
    loop {
        std::thread::sleep(Duration::from_millis(40));
        let escape_down = escape_key_down();
        let escape_hit = escape_down && !escape_was_down;
        escape_was_down = escape_down;
        // The focused overlay page handles Esc itself, so a text field can take the first press.
        // This path is for when the borderless window never became the foreground.
        if escape_hit && OVERLAY_OPEN.load(Ordering::Relaxed) && !overlay_has_foreground() {
            let app_for_close = app.clone();
            let _ = app.run_on_main_thread(move || dismiss_overlay(&app_for_close));
            stable = None;
            stable_hits = 0;
            continue;
        }
        let state = app.state::<AppState>();
        if !state.overlay_follow.load(Ordering::Relaxed) {
            stable = None;
            stable_hits = 0;
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
        }
        if stable_hits < 2 {
            continue;
        }
        let aimed = state.overlay_aim.lock().ok().and_then(|slot| *slot);
        if aimed == Some(key) {
            continue;
        }
        let generation = state.overlay_generation.load(Ordering::Relaxed);
        drop(state);
        let Some(index) = screens.iter().position(|screen| screen.screen_x == key.0 && screen.screen_y == key.1) else {
            continue;
        };
        if FOCUSING.swap(true, Ordering::SeqCst) {
            continue;
        }
        let app_for_window = app.clone();
        let screens_for_arm = screens;
        let queued = app_for_window.clone().run_on_main_thread(move || {
            let release = || FOCUSING.store(false, Ordering::SeqCst);
            let state = app_for_window.state::<AppState>();
            if !state.overlay_follow.load(Ordering::Relaxed) || state.overlay_generation.load(Ordering::Relaxed) != generation {
                release();
                return;
            }
            if app_for_window.get_webview_window(&overlay_label(index)).is_none() {
                release();
                return;
            }
            let already = state.overlay_aim.lock().ok().and_then(|slot| *slot);
            if already == Some(key) {
                drop(state);
                release();
                return;
            }
            if let Ok(mut aim) = state.overlay_aim.lock() {
                *aim = Some(key);
            }
            drop(state);
            // Show the other displays first, then key the one under the pointer.
            // Repeating this while the pointer stays steals focus and flashes the freeze.
            raise_hidden_overlays(&app_for_window, &screens_for_arm, index);
            focus_overlay(&app_for_window, &screens_for_arm, index);
            emit_overlay_arm(&app_for_window, &screens_for_arm, key);
            release();
        });
        if queued.is_err() {
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
    OVERLAY_OPEN.store(false, Ordering::Relaxed);
    if let Ok(mut slot) = OVERLAY_HWNDS.lock() {
        slot.clear();
    }
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
/// Safety reveal if the page never reports that the picture is painted.
const PIN_REVEAL_FALLBACK: Duration = Duration::from_secs(8);
const PIN_REVEAL_WAIT: Duration = Duration::from_secs(3);

fn pin_signals() -> &'static Mutex<HashMap<String, std::sync::mpsc::Sender<Result<(), String>>>> {
    static SIGNALS: OnceLock<Mutex<HashMap<String, std::sync::mpsc::Sender<Result<(), String>>>>> = OnceLock::new();
    SIGNALS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pin_lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn signal_pin(label: &str, result: Result<(), String>) {
    if let Some(sender) = pin_lock(&pin_signals()).remove(label) {
        let _ = sender.send(result);
    }
}

/// The caller can wait until the sticker is actually on screen.
/// Dropping it does not cancel the window.
#[must_use]
pub struct PinWait {
    rx: std::sync::mpsc::Receiver<Result<(), String>>,
    app: AppHandle,
    label: String,
    id: String,
}

impl PinWait {
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl PinWait {
    pub fn wait(self) -> Result<(), String> {
        match self.rx.recv_timeout(PIN_REVEAL_WAIT) {
            Ok(result) => result,
            Err(_) => {
                let app = self.app.clone();
                let label = self.label;
                let _ = on_main(&app, move |app| reveal_pin(app, &label));
                Ok(())
            }
        }
    }

    pub async fn ready(self) -> Result<(), String> {
        tauri::async_runtime::spawn_blocking(move || self.wait())
            .await
            .map_err(err)?
    }
}

/// Show the sticker only after its page has painted. Until then the window
/// stays cloaked, and the capture layer stays up, so neither a blank webview
/// nor the desktop flashes through the selection.
pub fn present_pin(app: &AppHandle, label: &str) {
    if !label.starts_with("pin-") {
        return;
    }
    reveal_pin(app, label);
}

fn pin_frame(x: i32, y: i32, width: u32, height: u32) -> PinFrame {
    PinFrame {
        x: x - PIN_GLOW,
        y: y - PIN_GLOW,
        width: width.max(1) as i32 + PIN_GLOW * 2,
        height: height.max(1) as i32 + PIN_GLOW * 2,
    }
}

fn place_pin_window(app: &AppHandle, window: &WebviewWindow, label: &str, frame: PinFrame) {
    #[cfg(windows)]
    place_pin_win32(window, frame);
    #[cfg(not(windows))]
    {
        let size = PhysicalSize::new(frame.width.max(1) as u32, frame.height.max(1) as u32);
        let origin = PhysicalPosition::new(frame.x, frame.y);
        let _ = window.set_position(origin);
        let _ = window.set_size(size);
        // macOS sometimes applies the first size before the window has a screen.
        let _ = window.set_position(origin);
        let _ = window.set_size(size);
    }
    let _ = window.set_always_on_top(true);
    pin_session::note_frame(app.state::<AppState>().inner(), label, frame);
}

#[cfg(windows)]
fn place_pin_win32(window: &WebviewWindow, frame: PinFrame) {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE};
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), frame.x, frame.y, frame.width, frame.height, SWP_NOACTIVATE);
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_ok() {
            let got_w = rect.right - rect.left;
            let got_h = rect.bottom - rect.top;
            if got_w != frame.width || got_h != frame.height {
                let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), frame.x, frame.y, frame.width, frame.height, SWP_NOACTIVATE);
            }
        }
    }
}

pub fn zoom_pin(window: &WebviewWindow, scale: f64) {
    let label = window.label().to_string();
    let Some((content_w, content_h)) = pin_session::content_size(window.state::<AppState>().inner(), &label) else {
        return;
    };
    let Ok(pos) = window.outer_position() else {
        return;
    };
    let Ok(size) = window.outer_size() else {
        return;
    };
    let center_x = pos.x.saturating_add(size.width as i32 / 2);
    let center_y = pos.y.saturating_add(size.height as i32 / 2);
    let frame = pin_session::zoomed_frame(content_w, content_h, scale, center_x, center_y, PIN_GLOW);
    place_pin_window(window.app_handle(), window, &label, frame);
}

fn remember_pin(
    app: &AppHandle,
    label: &str,
    image: Option<pin_session::PinImage>,
    path: Option<PathBuf>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let group = *state.pin_group.lock().map_err(err)?;
    if let Ok(mut hidden) = state.pins_hidden.lock() {
        *hidden = false;
    }
    pin_session::assign(
        state.inner(),
        label,
        group,
        image,
        path,
        PinCut { x, y, width: width.max(1), height: height.max(1) },
    );
    Ok(())
}

fn publish_pin(app: &AppHandle, label: &str, root: PathBuf, image: pin_session::PinImage) {
    let app = app.clone();
    let label = label.to_string();
    std::thread::spawn(move || {
        let path = match shotlet_core::library::store_pin(&root, &image) {
            Ok(path) => path,
            Err(error) => {
                eprintln!("pin: store failed: {error}");
                return;
            }
        };
        let app = app.clone();
        let _ = app.clone().run_on_main_thread(move || {
            let state = app.state::<AppState>();
            pin_session::set_file(state.inner(), &label, path);
            pin_session::release_image(state.inner(), &label);
        });
    });
}

fn arm_pin_wait(app: &AppHandle, label: &str, id: &str) -> PinWait {
    let (tx, rx) = std::sync::mpsc::channel();
    pin_lock(&pin_signals()).insert(label.to_string(), tx);
    let timeout_app = app.clone();
    let timeout_label = label.to_string();
    std::thread::spawn(move || {
        std::thread::sleep(PIN_REVEAL_FALLBACK);
        let _ = on_main(&timeout_app, move |app| reveal_pin(app, &timeout_label));
    });
    PinWait { rx, app: app.clone(), label: label.to_string(), id: id.to_string() }
}

fn publish_file(app: &AppHandle, label: &str, image: pin_session::PinImage) {
    let app = app.clone();
    let label = label.to_string();
    std::thread::spawn(move || {
        let path = match shotlet_core::library::write_pin_file(&image) {
            Ok((path, _png)) => path,
            Err(error) => {
                eprintln!("pin: store failed: {error}");
                return;
            }
        };
        let app = app.clone();
        let _ = app.clone().run_on_main_thread(move || {
            let state = app.state::<AppState>();
            pin_session::set_file(state.inner(), &label, path);
            pin_session::release_image(state.inner(), &label);
        });
    });
}

fn take_spare(app: &AppHandle) -> Option<String> {
    loop {
        let label = pin_session::take_standby(app.state::<AppState>().inner())?;
        if app.get_webview_window(&label).is_some() {
            return Some(label);
        }
        pin_session::forget(app.state::<AppState>().inner(), &label);
    }
}

pub fn open_pin(app: &AppHandle, path: String, x: i32, y: i32, width: u32, height: u32) -> Result<PinWait, String> {
    let width = width.max(1);
    let height = height.max(1);
    open_file(app, path, x, y, width, height, width, height)
}

/// A file pin whose bitmap is not the same size as the desktop window.
pub fn open_file(
    app: &AppHandle,
    path: String,
    x: i32,
    y: i32,
    window_w: u32,
    window_h: u32,
    pixels_w: u32,
    pixels_h: u32,
) -> Result<PinWait, String> {
    open_picture(app, None, Some(PathBuf::from(path)), None, None, false, x, y, window_w, window_h, pixels_w, pixels_h)
}

pub fn open_memory(
    app: &AppHandle,
    root: PathBuf,
    image: pin_session::PinImage,
    x: i32,
    y: i32,
    window_w: u32,
    window_h: u32,
) -> Result<PinWait, String> {
    let pixels_w = image.width();
    let pixels_h = image.height();
    open_picture(app, Some(image), None, Some(root), None, true, x, y, window_w, window_h, pixels_w, pixels_h)
}

/// Annotated pixels stay in memory. The unmarked crop is kept for `pin_history`.
pub fn open_annotated(
    app: &AppHandle,
    image: pin_session::PinImage,
    base: pin_session::PinImage,
    x: i32,
    y: i32,
    window_w: u32,
    window_h: u32,
) -> Result<PinWait, String> {
    let pixels_w = image.width();
    let pixels_h = image.height();
    open_picture(app, Some(image), None, None, Some(base), false, x, y, window_w, window_h, pixels_w, pixels_h)
}

fn open_picture(
    app: &AppHandle,
    image: Option<pin_session::PinImage>,
    path: Option<PathBuf>,
    root: Option<PathBuf>,
    base: Option<pin_session::PinImage>,
    record_history: bool,
    x: i32,
    y: i32,
    window_w: u32,
    window_h: u32,
    pixels_w: u32,
    pixels_h: u32,
) -> Result<PinWait, String> {
    let window_w = window_w.max(1);
    let window_h = window_h.max(1);
    let pixels_w = pixels_w.max(1);
    let pixels_h = pixels_h.max(1);
    let reused = take_spare(app);
    // A warmed window already has its label. The picture id has to be that label,
    // or hiding and switching groups cannot find the window.
    let id = match &reused {
        Some(label) => label.trim_start_matches("pin-").to_string(),
        None => new_id(),
    };
    let label = reused.clone().unwrap_or_else(|| format!("pin-{id}"));
    remember_pin(app, &label, image.clone(), path, x, y, window_w, window_h)?;
    if let Some(base) = base {
        pin_session::note_base(app.state::<AppState>().inner(), &label, base);
    }
    if let Some(image) = image {
        if record_history {
            if let Some(root) = root {
                publish_pin(app, &label, root, image);
            }
        } else {
            publish_file(app, &label, image);
        }
    }
    let wait = arm_pin_wait(app, &label, &id);
    if reused.is_some() {
        hand_pin(app, label, id, x, y, window_w, window_h, pixels_w, pixels_h)?;
    } else {
        open_fresh_pin(app, label, id, x, y, window_w, window_h, pixels_w, pixels_h)?;
    }
    replenish_pin(app);
    Ok(wait)
}

fn hand_pin(
    app: &AppHandle,
    label: String,
    id: String,
    x: i32,
    y: i32,
    window_w: u32,
    window_h: u32,
    pixels_w: u32,
    pixels_h: u32,
) -> Result<(), String> {
    crate::util::trace("pin: reuse standby");
    eprintln!("pin: reuse {label} as {id} at {x},{y} size {window_w}x{window_h}");
    let frame = pin_frame(x, y, window_w, window_h);
    let payload = PinAssign { id, width: pixels_w, height: pixels_h };
    on_main(app, move |app| {
        if let Some(window) = app.get_webview_window(&label) {
            place_pin_window(app, &window, &label, frame);
        }
        let _ = app.emit_to(&label, "pin-assign", &payload);
    })
}

fn open_fresh_pin(
    app: &AppHandle,
    label: String,
    id: String,
    x: i32,
    y: i32,
    window_w: u32,
    window_h: u32,
    pixels_w: u32,
    pixels_h: u32,
) -> Result<(), String> {
    let frame = pin_frame(x, y, window_w, window_h);
    let query = format!("view=pin&id={id}&w={pixels_w}&h={pixels_h}");
    crate::util::trace("pin: window begin");
    eprintln!("pin: open {label} at {x},{y} size {window_w}x{window_h} frame {},{} {}x{}", frame.x, frame.y, frame.width, frame.height);
    on_main(app, move |app| {
        let window = WebviewWindowBuilder::new(app, &label, view_url(&query))
            .title("Shotlet Pin")
            .decorations(false)
            .always_on_top(true)
            .resizable(true)
            .transparent(true)
            .shadow(false)
            .skip_taskbar(true)
            .focused(false)
            .visible(false)
            .inner_size(frame.width.max(1) as f64, frame.height.max(1) as f64)
            .min_inner_size(1.0, 1.0)
            .background_color(tauri::utils::config::Color(0, 0, 0, 0))
            .initialization_script(
                "document.documentElement.dataset.view='pin';document.documentElement.style.background='transparent';var clearPin=function(){if(!document.body)return;document.body.style.margin='0';document.body.style.background='transparent';var root=document.getElementById('root');if(root)root.style.background='transparent';};clearPin();document.addEventListener('DOMContentLoaded',clearPin);",
            )
            .build();
        let window = match window {
            Ok(window) => window,
            Err(error) => {
                abandon_pin(&label, error.to_string());
                return;
            }
        };
        place_pin_window(app, &window, &label, frame);
        cloak_pin(&window);
        crate::util::trace("pin: window cloaked");
    })
}


/// Build one hidden pin window. It carries no picture until a pin reuses it.
pub fn replenish_pin(app: &AppHandle) {
    if !pin_session::needs_spare(app.state::<AppState>().inner()) {
        return;
    }
    let app = app.clone();
    let _ = on_main(&app, |app| {
        if !pin_session::needs_spare(app.state::<AppState>().inner()) {
            return;
        }
        let label = format!("pin-{}", new_id());
        let window = WebviewWindowBuilder::new(app, &label, view_url("view=pin"))
            .title("Shotlet Pin")
            .decorations(false)
            .always_on_top(true)
            .resizable(true)
            .transparent(true)
            .shadow(false)
            .skip_taskbar(true)
            .focused(false)
            .visible(false)
            .inner_size(1.0, 1.0)
            .min_inner_size(1.0, 1.0)
            .background_color(tauri::utils::config::Color(0, 0, 0, 0))
            .initialization_script(
                "document.documentElement.dataset.view='pin';document.documentElement.style.background='transparent';var clearPin=function(){if(!document.body)return;document.body.style.margin='0';document.body.style.background='transparent';var root=document.getElementById('root');if(root)root.style.background='transparent';};clearPin();document.addEventListener('DOMContentLoaded',clearPin);",
            )
            .build();
        if let Ok(window) = window {
            pin_session::warm(app.state::<AppState>().inner(), &label);
            cloak_pin(&window);
            crate::util::trace("pin: standby built");
        }
    });
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PinAssign {
    id: String,
    width: u32,
    height: u32,
}

fn abandon_pin(label: &str, error: String) {
    eprintln!("pin: create window failed: {error}");
    signal_pin(label, Err(error));
}

fn trace_pin_rect(label: &str, window: &WebviewWindow, expected: Option<PinFrame>) {
    let Ok(pos) = window.outer_position() else {
        return;
    };
    let Ok(size) = window.outer_size() else {
        return;
    };
    let Some(expected) = expected else {
        return;
    };
    crate::util::trace(&format!(
        "pin: rect expected {},{} {}x{} actual {},{} {}x{}",
        expected.x, expected.y, expected.width, expected.height, pos.x, pos.y, size.width, size.height
    ));
    let off = (pos.x - expected.x).abs() > 1
        || (pos.y - expected.y).abs() > 1
        || (size.width as i32 - expected.width).abs() > 1
        || (size.height as i32 - expected.height).abs() > 1;
    if off {
        eprintln!(
            "pin: window rect mismatch {label} expected {},{} {}x{} actual {},{} {}x{}",
            expected.x, expected.y, expected.width, expected.height, pos.x, pos.y, size.width, size.height
        );
    }
}

fn reveal_pin(app: &AppHandle, label: &str) {
    // The fallback timer can run before the window exists. Leave the slot
    // assigned so the painted page can still reveal it.
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    if !pin_session::mark_shown(app.state::<AppState>().inner(), label) {
        return;
    }
    let expected = pin_session::frame_of(app.state::<AppState>().inner(), label);
    trace_pin_rect(label, &window, expected);
    uncloak_pin(&window);
    if let Err(error) = window.show() {
        eprintln!("pin: show window failed: {error}");
        signal_pin(label, Err(error.to_string()));
        return;
    }
    if let Err(error) = window.set_always_on_top(true) {
        eprintln!("pin: always-on-top failed: {error}");
    }
    // The sticker is on screen before the frozen capture goes away.
    dismiss_overlay(app);
    let _ = window.set_always_on_top(true);
    let _ = window.set_focus();
    signal_pin(label, Ok(()));
    crate::util::trace("pin: revealed");
    eprintln!("pin: window shown {label}");
}

#[cfg(windows)]
fn cloak_pin(window: &WebviewWindow) {
    use windows::Win32::Foundation::TRUE;
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK, DWMWA_TRANSITIONS_FORCEDISABLED};
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOWNOACTIVATE};
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    unsafe {
        let disable = TRUE;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            &disable as *const _ as *const _,
            std::mem::size_of_val(&disable) as u32,
        );
        let cloak = TRUE;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            &cloak as *const _ as *const _,
            std::mem::size_of_val(&cloak) as u32,
        );
        // Visible to the webview so it can paint, hidden from the desktop by the cloak.
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}

#[cfg(not(windows))]
fn cloak_pin(window: &WebviewWindow) {
    let _ = window;
}

#[cfg(windows)]
fn uncloak_pin(window: &WebviewWindow) {
    use windows::Win32::Foundation::FALSE;
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    unsafe {
        let cloak = FALSE;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            &cloak as *const _ as *const _,
            std::mem::size_of_val(&cloak) as u32,
        );
    }
}

#[cfg(not(windows))]
fn uncloak_pin(window: &WebviewWindow) {
    let _ = window;
}

const RECORDER_W: f64 = 400.0;
const RECORDER_H: f64 = 248.0;

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
    let groups = pin_session::groups(state.inner());
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
    let groups = pin_session::groups(state.inner());
    let app = app.clone();
    let _ = on_main(&app, move |app| {
        set_pin_visibility(app, next, &groups, true);
        let _ = app.emit("capture-error", format!("已切换到贴图组 {next}"));
    });
}

const SCROLL_FRAME: &str = "scroll-frame";
const SCROLL_HUD: &str = "scroll-hud";
const RECORD_BORDER: &str = "record-border";
const HUD_W: f64 = 560.0;
const HUD_H: f64 = 58.0;
const FRAME_PAD: i32 = 8;

#[derive(Clone, Copy)]
struct FramePlace {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    hole_x: i32,
    hole_y: i32,
    hole_w: u32,
    hole_h: u32,
}

/// Green ring around the selection. The center is a hole so the scrollbar underneath can be dragged.
fn frame_place(sel_x: i32, sel_y: i32, sel_w: u32, sel_h: u32, screen_x: i32, screen_y: i32, screen_w: u32, screen_h: u32, pad: i32) -> Option<FramePlace> {
    if sel_w == 0 || sel_h == 0 || screen_w == 0 || screen_h == 0 || pad <= 0 {
        return None;
    }
    let screen_r = screen_x.saturating_add(screen_w as i32);
    let screen_b = screen_y.saturating_add(screen_h as i32);
    let left = (sel_x - pad).max(screen_x);
    let top = (sel_y - pad).max(screen_y);
    let right = (sel_x.saturating_add(sel_w as i32).saturating_add(pad)).min(screen_r);
    let bottom = (sel_y.saturating_add(sel_h as i32).saturating_add(pad)).min(screen_b);
    if right <= left || bottom <= top {
        return None;
    }
    let hole_x = (sel_x - left).max(0);
    let hole_y = (sel_y - top).max(0);
    let hole_r = (sel_x.saturating_add(sel_w as i32) - left).clamp(hole_x, right - left);
    let hole_b = (sel_y.saturating_add(sel_h as i32) - top).clamp(hole_y, bottom - top);
    if hole_r <= hole_x || hole_b <= hole_y {
        return None;
    }
    let ring = hole_x > 1 || hole_y > 1 || (right - left) - hole_r > 1 || (bottom - top) - hole_b > 1;
    if !ring {
        return None;
    }
    Some(FramePlace {
        x: left,
        y: top,
        w: (right - left) as u32,
        h: (bottom - top) as u32,
        hole_x,
        hole_y,
        hole_w: (hole_r - hole_x) as u32,
        hole_h: (hole_b - hole_y) as u32,
    })
}

fn hud_place(sel_x: i32, sel_y: i32, sel_w: u32, sel_h: u32, screen_x: i32, screen_y: i32, screen_w: u32, screen_h: u32, hud_w: i32, hud_h: i32) -> (i32, i32) {
    let screen_r = screen_x.saturating_add(screen_w as i32);
    let screen_b = screen_y.saturating_add(screen_h as i32);
    let max_x = (screen_r - hud_w - 4).max(screen_x);
    let max_y = (screen_b - hud_h - 4).max(screen_y);
    let mut x = sel_x + (sel_w as i32 - hud_w) / 2;
    let below = sel_y.saturating_add(sel_h as i32).saturating_add(12);
    let above = sel_y - 12 - hud_h;
    let y = if below.saturating_add(hud_h) <= screen_b - 4 {
        below
    } else if above >= screen_y + 4 {
        above
    } else {
        screen_y + 8
    };
    let min_x = (screen_x + 4).min(max_x);
    let min_y = screen_y.min(max_y);
    x = x.clamp(min_x, max_x);
    (x, y.clamp(min_y, max_y))
}

pub fn conceal_visible_overlays(app: &AppHandle) -> Vec<String> {
    let mut shown = Vec::new();
    for label in overlay_labels(app) {
        let Some(window) = app.get_webview_window(&label) else { continue };
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
            shown.push(label);
        }
    }
    shown
}

pub fn show_overlay_labels(app: &AppHandle, labels: &[String]) {
    for label in labels {
        let Some(window) = app.get_webview_window(label) else { continue };
        let _ = window.show();
        let _ = window.set_always_on_top(true);
    }
}

pub fn close_scroll_chrome(app: &AppHandle) {
    for label in [SCROLL_FRAME, SCROLL_HUD] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.destroy();
        }
    }
}

pub fn open_scroll_chrome(
    app: &AppHandle,
    sel_x: i32,
    sel_y: i32,
    sel_w: u32,
    sel_h: u32,
    screen_x: i32,
    screen_y: i32,
    screen_w: u32,
    screen_h: u32,
) -> Result<(), String> {
    close_scroll_chrome(app);
    if let Some(place) = frame_place(sel_x, sel_y, sel_w, sel_h, screen_x, screen_y, screen_w, screen_h, FRAME_PAD) {
        open_scroll_frame(app, place)?;
    }
    open_scroll_hud(app, sel_x, sel_y, sel_w, sel_h, screen_x, screen_y, screen_w, screen_h)
}

pub fn close_record_border(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(RECORD_BORDER) {
        let _ = window.destroy();
    }
}

pub fn open_record_border(app: &AppHandle, x: i32, y: i32, width: u32, height: u32) -> Result<(), String> {
    close_record_border(app);
    let rects = capture::monitor_rects();
    let Some(monitor) = capture::monitor_at_point(&rects, x.saturating_add(8), y.saturating_add(8)) else {
        return Ok(());
    };
    let Some(place) = frame_place(x, y, width, height, monitor.x, monitor.y, monitor.width, monitor.height, FRAME_PAD) else {
        return Ok(());
    };
    open_ring(app, RECORD_BORDER, "view=scroll&role=record", place)
}

fn open_scroll_frame(app: &AppHandle, place: FramePlace) -> Result<(), String> {
    open_ring(app, SCROLL_FRAME, "view=scroll&role=frame", place)
}

fn open_ring(app: &AppHandle, label: &str, query: &str, place: FramePlace) -> Result<(), String> {
    let window = WebviewWindowBuilder::new(app, label, view_url(query))
        .title("Shotlet")
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .shadow(false)
        .transparent(true)
        .accept_first_mouse(true)
        .background_color(tauri::utils::config::Color(0, 0, 0, 0))
        .inner_size(place.w as f64, place.h as f64)
        .initialization_script(
            "document.documentElement.style.background='transparent';document.addEventListener('DOMContentLoaded',function(){document.body.style.margin='0';document.body.style.background='transparent';});",
        )
        .on_page_load(move |window, payload| {
            if !matches!(payload.event(), PageLoadEvent::Finished) {
                return;
            }
            let _ = window.set_position(PhysicalPosition::new(place.x, place.y));
            let _ = window.set_size(PhysicalSize::new(place.w, place.h));
            clip_frame_ring(&window, place.w, place.h, place.hole_x, place.hole_y, place.hole_w, place.hole_h);
            let _ = window.set_ignore_cursor_events(true);
            let _ = window.show();
        })
        .build()
        .map_err(err)?;
    let _ = window.set_position(PhysicalPosition::new(place.x, place.y));
    let _ = window.set_size(PhysicalSize::new(place.w, place.h));
    clip_frame_ring(&window, place.w, place.h, place.hole_x, place.hole_y, place.hole_w, place.hole_h);
    let _ = window.set_ignore_cursor_events(true);
    window.show().map_err(err)?;
    Ok(())
}

fn open_scroll_hud(
    app: &AppHandle,
    sel_x: i32,
    sel_y: i32,
    sel_w: u32,
    sel_h: u32,
    screen_x: i32,
    screen_y: i32,
    screen_w: u32,
    screen_h: u32,
) -> Result<(), String> {
    let window = WebviewWindowBuilder::new(app, SCROLL_HUD, view_url("view=scroll&role=hud"))
        .title("Shotlet")
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .shadow(false)
        .transparent(true)
        .accept_first_mouse(true)
        .background_color(tauri::utils::config::Color(255, 255, 255, 255))
        .inner_size(HUD_W, HUD_H)
        .initialization_script(
            "document.documentElement.style.background='transparent';document.addEventListener('DOMContentLoaded',function(){document.body.style.margin='0';document.body.style.background='transparent';});",
        )
        .on_page_load(|window, payload| {
            if !matches!(payload.event(), PageLoadEvent::Finished) {
                return;
            }
            let _ = window.show();
        })
        .build()
        .map_err(err)?;
    let scale = window.scale_factor().unwrap_or(1.0).max(1.0);
    let hud_w = (HUD_W * scale).round() as i32;
    let hud_h = (HUD_H * scale).round() as i32;
    let (x, y) = hud_place(sel_x, sel_y, sel_w, sel_h, screen_x, screen_y, screen_w, screen_h, hud_w, hud_h);
    window.set_position(PhysicalPosition::new(x, y)).map_err(err)?;
    window.show().map_err(err)?;
    eprintln!("scroll: hud at {x},{y}");
    Ok(())
}

fn clip_frame_ring(window: &WebviewWindow, width: u32, height: u32, hole_x: i32, hole_y: i32, hole_w: u32, hole_h: u32) {
    #[cfg(windows)]
    {
        use windows::Win32::Graphics::Gdi::{CombineRgn, CreateRectRgn, DeleteObject, SetWindowRgn, RGN_DIFF};
        let Ok(hwnd) = window.hwnd() else { return };
        unsafe {
            let full = CreateRectRgn(0, 0, width as i32, height as i32);
            let hole = CreateRectRgn(hole_x, hole_y, hole_x.saturating_add(hole_w as i32), hole_y.saturating_add(hole_h as i32));
            let ring = CreateRectRgn(0, 0, 0, 0);
            if full.is_invalid() || hole.is_invalid() || ring.is_invalid() {
                if !full.is_invalid() { let _ = DeleteObject(full.into()); }
                if !hole.is_invalid() { let _ = DeleteObject(hole.into()); }
                if !ring.is_invalid() { let _ = DeleteObject(ring.into()); }
                return;
            }
            let combined = CombineRgn(Some(ring), Some(full), Some(hole), RGN_DIFF);
            let _ = DeleteObject(full.into());
            let _ = DeleteObject(hole.into());
            if combined.0 == 0 {
                let _ = DeleteObject(ring.into());
                return;
            }
            if SetWindowRgn(hwnd, Some(ring), true) == 0 {
                let _ = DeleteObject(ring.into());
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (window, width, height, hole_x, hole_y, hole_w, hole_h);
    }
}

fn set_pin_visibility(app: &AppHandle, group: u8, groups: &HashMap<String, u8>, show_current: bool) {
    for (label, window) in app.webview_windows() {
        let Some(id) = label.strip_prefix("pin-") else {
            continue;
        };
        // A warmed window has no picture and no group. Showing it would flash an empty sticker.
        if !groups.contains_key(id) {
            continue;
        }
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

#[cfg(test)]
mod scroll_chrome_tests {
    use super::{frame_place, hud_place};

    #[test]
    fn frame_is_a_ring_outside_the_selection() {
        let place = frame_place(100, 80, 400, 300, 0, 0, 1920, 1080, 8).unwrap();
        assert_eq!((place.x, place.y, place.w, place.h), (92, 72, 416, 316));
        assert_eq!((place.hole_x, place.hole_y, place.hole_w, place.hole_h), (8, 8, 400, 300));
    }

    #[test]
    fn frame_is_skipped_when_the_selection_fills_the_screen() {
        assert!(frame_place(0, 0, 1920, 1080, 0, 0, 1920, 1080, 8).is_none());
    }

    #[test]
    fn hud_sits_under_the_selection_and_moves_above_when_it_does_not_fit() {
        assert_eq!(hud_place(100, 80, 400, 200, 0, 0, 1920, 1080, 480, 58), (60, 292));
        let (x, y) = hud_place(100, 900, 400, 150, 0, 0, 1920, 1080, 480, 58);
        assert_eq!(y, 900 - 12 - 58);
        assert_eq!(x, 60);
    }
}
