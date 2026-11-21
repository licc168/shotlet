//! macOS global shortcuts.
//!
//! `RegisterEventHotKey` returns success on macOS 26 and an in-process probe reports
//! the key as held, but Carbon never sees F1. A session CGEventTap does. Install the
//! tap after the event loop is ready and keep Carbon only as a fallback.

use std::collections::HashMap;
use std::ffi::c_void;
use std::os::raw::c_ulong;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use tauri::{AppHandle, Emitter};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};
use tauri_plugin_opener::OpenerExt;

use crate::commands;

const SIGNATURE: u32 = 0x7368_6c74; // 'shlt'
const HOTKEY_EXISTS: i32 = -9878;
const EVENT_NOT_HANDLED: i32 = -9874;
const KEY_DOWN: u32 = 10;
const TAP_DISABLED_TIMEOUT: u32 = 0xFFFF_FFFE;
const TAP_DISABLED_USER: u32 = 0xFFFF_FFFF;
const RETRY_DELAYS_MS: [u64; 4] = [800, 2000, 4000, 8000];

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[derive(Clone)]
struct Binding {
    shortcut: Shortcut,
    keycode: u32,
    carbon_mods: u32,
}

struct MacState {
    handler: *mut c_void,
    hotkeys: Vec<*mut c_void>,
    bindings: HashMap<u32, Binding>,
    tap_bindings: Vec<Binding>,
    tap_port: usize,
    #[allow(dead_code)]
    tap_source: usize,
    tap_active: bool,
    tap_wanted: bool,
    app: Option<AppHandle>,
    next_id: u32,
    attempt: u8,
}

unsafe impl Send for MacState {}

static STATE: LazyLock<Mutex<MacState>> = LazyLock::new(|| Mutex::new(MacState {
    handler: std::ptr::null_mut(),
    hotkeys: Vec::new(),
    bindings: HashMap::new(),
    tap_bindings: Vec::new(),
    tap_port: 0,
    tap_source: 0,
    tap_active: false,
    tap_wanted: false,
    app: None,
    next_id: 1,
    attempt: 0,
}));

/// Immutable snapshot consumed by the CGEventTap / Carbon callbacks. Callbacks
/// take the Arc and release the lock immediately, so a slow `apply_now` holding
/// STATE never blocks the system input callback (which macOS may time out and
/// disable the tap entirely). Replaced atomically whenever the tap is applied.
struct TapSnapshot {
    bindings: Vec<Binding>,
    carbon: HashMap<u32, Shortcut>,
    app: Option<AppHandle>,
    tap_active: bool,
    tap_wanted: bool,
    tap_port: usize,
}

static TAP: LazyLock<RwLock<Arc<TapSnapshot>>> = LazyLock::new(|| {
    RwLock::new(Arc::new(TapSnapshot {
        bindings: Vec::new(),
        carbon: HashMap::new(),
        app: None,
        tap_active: false,
        tap_wanted: false,
        tap_port: 0,
    }))
});

fn publish_tap(state: &MacState) {
    let snapshot = TapSnapshot {
        bindings: state.tap_bindings.clone(),
        carbon: state.bindings.iter().map(|(id, binding)| (*id, binding.shortcut)).collect(),
        app: state.app.clone(),
        tap_active: state.tap_active,
        tap_wanted: state.tap_wanted,
        tap_port: state.tap_port,
    };
    if let Ok(mut tap) = TAP.write() {
        *tap = Arc::new(snapshot);
    }
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: Option<
            unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32,
        >,
        num_types: c_ulong,
        types: *const EventTypeSpec,
        user_data: *mut c_void,
        out_handler: *mut *mut c_void,
    ) -> i32;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        hotkey_id: EventHotKeyId,
        target: *mut c_void,
        options: u32,
        out_ref: *mut *mut c_void,
    ) -> i32;
    fn UnregisterEventHotKey(hotkey: *mut c_void) -> i32;
    fn GetEventParameter(
        event: *mut c_void,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        buffer_size: c_ulong,
        actual_size: *mut c_ulong,
        data: *mut c_void,
    ) -> i32;
    fn GetEventKind(event: *mut c_void) -> u32;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events: u64,
        callback: unsafe extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void,
        user_info: *mut c_void,
    ) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    fn CGEventGetIntegerValueField(event: *mut c_void, field: u32) -> i64;
    fn CGEventGetFlags(event: *mut c_void) -> u64;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopCommonModes: *const c_void;
    fn CFRunLoopGetMain() -> *mut c_void;
    fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: *mut c_void,
        order: isize,
    ) -> *mut c_void;
    fn CFRunLoopAddSource(run_loop: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRelease(cf: *const c_void);
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

pub fn schedule_apply(app: &AppHandle, fresh: bool) {
    let app_for_task = app.clone();
    if let Err(error) = app.run_on_main_thread(move || apply_now(app_for_task, fresh)) {
        commands::shortcut_log(app, &format!("main thread dispatch failed: {error}"));
    }
}

fn apply_now(app: AppHandle, fresh: bool) {
    let settings = match crate::ui::read_settings(&app) {
        Ok(settings) => settings,
        Err(error) => {
            commands::shortcut_log(&app, &format!("read settings failed: {error}"));
            return;
        }
    };
    let mut desired = Vec::new();
    for text in commands::shortcut_values(&settings) {
        if text.trim().is_empty() {
            continue;
        }
        let shortcut: Shortcut = match text.parse() {
            Ok(shortcut) => shortcut,
            Err(error) => {
                commands::shortcut_log(&app, &format!("parse {text} failed: {error}"));
                continue;
            }
        };
        match key_to_scancode(shortcut.key) {
            Some(keycode) => desired.push(Binding {
                carbon_mods: carbon_mods(shortcut.mods),
                keycode,
                shortcut,
            }),
            None => commands::shortcut_log(&app, &format!("unsupported key {text}")),
        }
    }

    let mut state = STATE.lock().expect("shortcut state");
    if fresh {
        state.attempt = 0;
    }
    state.app = Some(app.clone());
    clear_hotkeys(&mut state);
    state.tap_bindings = desired.clone();

    let tap_ok = if state.tap_bindings.is_empty() {
        state.tap_wanted = false;
        set_tap_enabled(&state, false);
        commands::shortcut_log(&app, "no shortcuts configured");
        true
    } else if ensure_tap(&mut state) {
        state.tap_wanted = true;
        set_tap_enabled(&state, true);
        let mode = if state.tap_active { "active" } else { "listen-only" };
        commands::shortcut_log(
            &app,
            &format!("event tap active ({mode}), {} shortcuts", state.tap_bindings.len()),
        );
        true
    } else {
        state.tap_wanted = false;
        false
    };

    if tap_ok {
        // One real key must not reach both the tap and Carbon.
        state.attempt = 0;
        publish_tap(&state);
        return;
    }

    if let Err(error) = ensure_handler(&mut state) {
        commands::shortcut_log(&app, &error);
    }
    let mut registered = 0usize;
    let mut busy = Vec::new();
    for binding in &desired {
        let id = state.next_id;
        state.next_id = state.next_id.wrapping_add(1).max(1);
        let mut hotkey_ref = std::ptr::null_mut();
        let status = unsafe {
            RegisterEventHotKey(
                binding.keycode,
                binding.carbon_mods,
                EventHotKeyId { signature: SIGNATURE, id },
                GetApplicationEventTarget(),
                0,
                &mut hotkey_ref,
            )
        };
        commands::shortcut_log(
            &app,
            &format!(
                "carbon fallback {} key=0x{:02x} mods=0x{:x} status={status}",
                binding.shortcut, binding.keycode, binding.carbon_mods
            ),
        );
        if status == 0 && !hotkey_ref.is_null() {
            state.hotkeys.push(hotkey_ref);
            state.bindings.insert(id, binding.clone());
            registered += 1;
        } else if status == HOTKEY_EXISTS {
            busy.push(binding.shortcut.to_string());
        }
    }
    let trusted = unsafe { AXIsProcessTrusted() } != 0;
    commands::shortcut_log(
        &app,
        &format!(
            "event tap unavailable, accessibility={trusted}; carbon fallback, {registered} hotkeys"
        ),
    );
    publish_tap(&state);
    drop(state);
    if !trusted {
        prompt_accessibility(&app);
    }
    let mut state = STATE.lock().expect("shortcut state");

    if state.attempt >= RETRY_DELAYS_MS.len() as u8 {
        if !busy.is_empty() {
            let message = format!("这些快捷键被其他程序占用：{}", busy.join("、"));
            commands::shortcut_log(&app, &message);
            let _ = app.emit("capture-error", message);
        } else {
            commands::shortcut_log(&app, "stopped retrying event tap");
        }
        publish_tap(&state);
        return;
    }
    state.attempt += 1;
    let wait = RETRY_DELAYS_MS[state.attempt as usize - 1];
    commands::shortcut_log(&app, &format!("retry shortcuts in {wait}ms (attempt {})", state.attempt));
    publish_tap(&state);
    drop(state);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(wait));
        schedule_apply(&app, false);
    });
}

fn ensure_handler(state: &mut MacState) -> Result<(), String> {
    if !state.handler.is_null() {
        return Ok(());
    }
    let types = [
        EventTypeSpec { event_class: 1801812322, event_kind: 5 },
        EventTypeSpec { event_class: 1801812322, event_kind: 6 },
    ];
    let mut handler = std::ptr::null_mut();
    let status = unsafe {
        InstallEventHandler(
            GetApplicationEventTarget(),
            Some(carbon_handler),
            2,
            types.as_ptr(),
            std::ptr::null_mut(),
            &mut handler,
        )
    };
    if status != 0 || handler.is_null() {
        return Err(format!("InstallEventHandler failed: {status}"));
    }
    state.handler = handler;
    Ok(())
}

fn clear_hotkeys(state: &mut MacState) {
    for hotkey in state.hotkeys.drain(..) {
        unsafe {
            UnregisterEventHotKey(hotkey);
        }
    }
    state.bindings.clear();
}

fn ensure_tap(state: &mut MacState) -> bool {
    if state.tap_port != 0 {
        return true;
    }
    let app = state.app.clone();
    // Session tap, inserted at the head. Active (0) can swallow the key.
    // Listen-only (1) is the fallback when an active tap is refused.
    for (options, active) in [(0u32, true), (1u32, false)] {
        let port = unsafe {
            CGEventTapCreate(
                1,
                0,
                options,
                1 << KEY_DOWN,
                tap_callback,
                std::ptr::null_mut(),
            )
        };
        if port.is_null() {
            if let Some(app) = &app {
                let kind = if active { "active" } else { "listen-only" };
                commands::shortcut_log(app, &format!("event tap create failed ({kind})"));
            }
            continue;
        }
        let source = unsafe { CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0) };
        if source.is_null() {
            unsafe { CFRelease(port); }
            if let Some(app) = &app {
                commands::shortcut_log(app, "event tap source failed");
            }
            continue;
        }
        unsafe {
            CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopCommonModes);
            CGEventTapEnable(port, true);
        }
        state.tap_port = port as usize;
        state.tap_source = source as usize;
        state.tap_active = active;
        return true;
    }
    false
}

fn set_tap_enabled(state: &MacState, enabled: bool) {
    if state.tap_port != 0 {
        unsafe { CGEventTapEnable(state.tap_port as *mut c_void, enabled); }
    }
}

fn prompt_accessibility(app: &AppHandle) {
    static SHOWN: AtomicBool = AtomicBool::new(false);
    if SHOWN.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    app.dialog()
        .message("F1 全局快捷键需要“辅助功能”权限。请在系统设置中允许 Shotlet，然后重新打开应用。")
        .title("Shotlet 快捷键")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCustom("打开系统设置".into()))
        .show(move |ok| {
            if ok {
                let _ = app.opener().open_url(
                    "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility",
                    None::<&str>,
                );
            }
        });
}

unsafe extern "C" fn carbon_handler(
    _call: *mut c_void,
    event: *mut c_void,
    _user: *mut c_void,
) -> i32 {
    let mut bytes = [0u8; 8];
    let status = unsafe {
        GetEventParameter(
            event,
            757935405,
            1751869796,
            std::ptr::null_mut(),
            bytes.len() as c_ulong,
            std::ptr::null_mut(),
            bytes.as_mut_ptr().cast(),
        )
    };
    if status != 0 {
        return EVENT_NOT_HANDLED;
    }
    let signature = u32::from_ne_bytes(bytes[0..4].try_into().unwrap());
    let id = u32::from_ne_bytes(bytes[4..8].try_into().unwrap());
    if signature != SIGNATURE {
        return EVENT_NOT_HANDLED;
    }
    if unsafe { GetEventKind(event) } != 5 {
        return 0;
    }
    let found = {
        let Ok(tap) = TAP.read() else {
            return EVENT_NOT_HANDLED;
        };
        tap.carbon.get(&id).map(|shortcut| (tap.app.clone(), shortcut.clone()))
    };
    if let Some((Some(app), shortcut)) = found {
        commands::shortcut_log(&app, &format!("carbon pressed {shortcut}"));
        commands::on_shortcut(&app, &shortcut, ShortcutState::Pressed);
        return 0;
    }
    EVENT_NOT_HANDLED
}

unsafe extern "C" fn tap_callback(
    _proxy: *mut c_void,
    event_type: u32,
    event: *mut c_void,
    _user: *mut c_void,
) -> *mut c_void {
    if event_type == TAP_DISABLED_TIMEOUT || event_type == TAP_DISABLED_USER {
        if let Ok(tap) = TAP.read() {
            if tap.tap_wanted && tap.tap_port != 0 {
                unsafe { CGEventTapEnable(tap.tap_port as *mut c_void, true); }
            }
        }
        return event;
    }
    if event_type != KEY_DOWN || event.is_null() {
        return event;
    }
    let repeat = unsafe { CGEventGetIntegerValueField(event, 8) };
    if repeat != 0 {
        return event;
    }
    let keycode = unsafe { CGEventGetIntegerValueField(event, 9) } as u32;
    let mods = cg_flags_to_carbon_mods(unsafe { CGEventGetFlags(event) });
    let found = {
        let Ok(tap) = TAP.read() else {
            return event;
        };
        tap.bindings
            .iter()
            .find(|binding| binding.keycode == keycode && binding.carbon_mods == mods)
            .map(|binding| (tap.app.clone(), binding.shortcut, tap.tap_active))
    };
    if let Some((Some(app), shortcut, swallow)) = found {
        commands::shortcut_log(&app, &format!("tap pressed {shortcut}"));
        commands::on_shortcut(&app, &shortcut, ShortcutState::Pressed);
        if swallow {
            return std::ptr::null_mut();
        }
    }
    event
}

#[allow(deprecated)]
fn carbon_mods(mods: Modifiers) -> u32 {
    let mut value = 0;
    if mods.contains(Modifiers::SHIFT) {
        value |= 0x200;
    }
    if mods.contains(Modifiers::SUPER) || mods.contains(Modifiers::META) {
        value |= 0x100;
    }
    if mods.contains(Modifiers::ALT) {
        value |= 0x800;
    }
    if mods.contains(Modifiers::CONTROL) {
        value |= 0x1000;
    }
    value
}

fn cg_flags_to_carbon_mods(flags: u64) -> u32 {
    let mut mods = 0;
    if flags & 0x0002_0000 != 0 {
        mods |= 0x200;
    }
    if flags & 0x0004_0000 != 0 {
        mods |= 0x1000;
    }
    if flags & 0x0008_0000 != 0 {
        mods |= 0x800;
    }
    if flags & 0x0010_0000 != 0 {
        mods |= 0x100;
    }
    mods
}


fn key_to_scancode(code: Code) -> Option<u32> {
    match code {
        Code::KeyA => Some(0x00),
        Code::KeyS => Some(0x01),
        Code::KeyD => Some(0x02),
        Code::KeyF => Some(0x03),
        Code::KeyH => Some(0x04),
        Code::KeyG => Some(0x05),
        Code::KeyZ => Some(0x06),
        Code::KeyX => Some(0x07),
        Code::KeyC => Some(0x08),
        Code::KeyV => Some(0x09),
        Code::KeyB => Some(0x0b),
        Code::KeyQ => Some(0x0c),
        Code::KeyW => Some(0x0d),
        Code::KeyE => Some(0x0e),
        Code::KeyR => Some(0x0f),
        Code::KeyY => Some(0x10),
        Code::KeyT => Some(0x11),
        Code::Digit1 => Some(0x12),
        Code::Digit2 => Some(0x13),
        Code::Digit3 => Some(0x14),
        Code::Digit4 => Some(0x15),
        Code::Digit6 => Some(0x16),
        Code::Digit5 => Some(0x17),
        Code::Equal => Some(0x18),
        Code::Digit9 => Some(0x19),
        Code::Digit7 => Some(0x1a),
        Code::Minus => Some(0x1b),
        Code::Digit8 => Some(0x1c),
        Code::Digit0 => Some(0x1d),
        Code::BracketRight => Some(0x1e),
        Code::KeyO => Some(0x1f),
        Code::KeyU => Some(0x20),
        Code::BracketLeft => Some(0x21),
        Code::KeyI => Some(0x22),
        Code::KeyP => Some(0x23),
        Code::Enter => Some(0x24),
        Code::KeyL => Some(0x25),
        Code::KeyJ => Some(0x26),
        Code::Quote => Some(0x27),
        Code::KeyK => Some(0x28),
        Code::Semicolon => Some(0x29),
        Code::Backslash => Some(0x2a),
        Code::Comma => Some(0x2b),
        Code::Slash => Some(0x2c),
        Code::KeyN => Some(0x2d),
        Code::KeyM => Some(0x2e),
        Code::Period => Some(0x2f),
        Code::Tab => Some(0x30),
        Code::Space => Some(0x31),
        Code::Backquote => Some(0x32),
        Code::Backspace => Some(0x33),
        Code::Escape => Some(0x35),
        Code::F17 => Some(0x40),
        Code::NumpadDecimal => Some(0x41),
        Code::NumpadMultiply => Some(0x43),
        Code::NumpadAdd => Some(0x45),
        Code::NumLock => Some(0x47),
        Code::AudioVolumeUp => Some(0x48),
        Code::AudioVolumeDown => Some(0x49),
        Code::AudioVolumeMute => Some(0x4a),
        Code::NumpadDivide => Some(0x4b),
        Code::NumpadEnter => Some(0x4c),
        Code::NumpadSubtract => Some(0x4e),
        Code::F18 => Some(0x4f),
        Code::F19 => Some(0x50),
        Code::NumpadEqual => Some(0x51),
        Code::Numpad0 => Some(0x52),
        Code::Numpad1 => Some(0x53),
        Code::Numpad2 => Some(0x54),
        Code::Numpad3 => Some(0x55),
        Code::Numpad4 => Some(0x56),
        Code::Numpad5 => Some(0x57),
        Code::Numpad6 => Some(0x58),
        Code::Numpad7 => Some(0x59),
        Code::F20 => Some(0x5a),
        Code::Numpad8 => Some(0x5b),
        Code::Numpad9 => Some(0x5c),
        Code::F5 => Some(0x60),
        Code::F6 => Some(0x61),
        Code::F7 => Some(0x62),
        Code::F3 => Some(0x63),
        Code::F8 => Some(0x64),
        Code::F9 => Some(0x65),
        Code::F11 => Some(0x67),
        Code::F13 => Some(0x69),
        Code::F16 => Some(0x6a),
        Code::F14 => Some(0x6b),
        Code::F10 => Some(0x6d),
        Code::F12 => Some(0x6f),
        Code::F15 => Some(0x71),
        Code::Insert => Some(0x72),
        Code::Home => Some(0x73),
        Code::PageUp => Some(0x74),
        Code::Delete => Some(0x75),
        Code::F4 => Some(0x76),
        Code::End => Some(0x77),
        Code::F2 => Some(0x78),
        Code::PageDown => Some(0x79),
        Code::F1 => Some(0x7a),
        Code::ArrowLeft => Some(0x7b),
        Code::ArrowRight => Some(0x7c),
        Code::ArrowDown => Some(0x7d),
        Code::ArrowUp => Some(0x7e),
        Code::CapsLock => Some(0x39),
        Code::PrintScreen => Some(0x46),
        _ => None,
    }
}
