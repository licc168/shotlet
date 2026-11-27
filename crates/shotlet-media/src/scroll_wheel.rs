//! Optional wheel nudge. This file is the `wheel` module on Windows.
//! Other platforms use `scroll_wheel_noop.rs` under the same name.
//! The capture loop only calls it after the user turns auto-scroll on.
//! The pointer is never moved, and finishing the capture does not put it somewhere else.

use std::sync::atomic::{AtomicI32, Ordering};

static DELTA: AtomicI32 = AtomicI32::new(-240);
static USE_KEYS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LOGGED_AUTO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PRIOR_PX: AtomicI32 = AtomicI32::new(0);

pub(super) fn reset_step() {
    DELTA.store(-240, Ordering::Relaxed);
    USE_KEYS.store(false, Ordering::Relaxed);
    LOGGED_AUTO.store(false, Ordering::Relaxed);
    PRIOR_PX.store(0, Ordering::Relaxed);
}

pub(super) fn take_prior() -> i32 {
    PRIOR_PX.swap(0, Ordering::Relaxed)
}

pub(super) fn use_keys() {
    USE_KEYS.store(true, Ordering::Relaxed);
}

pub(super) fn release_cursor() {}

pub(super) fn nudge(screen_x: i32, screen_y: i32, x: u32, y: u32, width: u32, height: u32) -> bool {
    let aim = aim_point(screen_x, screen_y, x, y, width, height);
    let left = screen_x.saturating_add(x as i32);
    let top = screen_y.saturating_add(y as i32);
    wheel_at(aim, left, top, width, height, DELTA.load(Ordering::Relaxed));
    true
}

fn aim_point(screen_x: i32, screen_y: i32, x: u32, y: u32, width: u32, height: u32) -> (i32, i32) {
    let left = screen_x.saturating_add(x as i32);
    let top = screen_y.saturating_add(y as i32);
    let inset = if width > 80 { 36 } else { 8 };
    let max_x = left.saturating_add(width as i32).saturating_sub(inset);
    let mut aim_x = left.saturating_add(width as i32 / 2);
    if aim_x > max_x {
        aim_x = max_x;
    }
    if aim_x < left.saturating_add(8) {
        aim_x = left.saturating_add(8);
    }
    (aim_x, top.saturating_add(height as i32 / 2))
}

fn wheel_at(aim: (i32, i32), left: i32, top: i32, width: u32, height: u32, delta: i32) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GetCursorPos, SetForegroundWindow, GA_ROOT};
    let hwnd = foreign_window_at(aim.0, aim.1);
    if hwnd.0.is_null() {
        eprintln!("scroll: auto found no page window");
        return;
    }
    unsafe {
        let root = GetAncestor(hwnd, GA_ROOT);
        let foreground = if root.0.is_null() { hwnd } else { root };
        let _ = SetForegroundWindow(foreground);
    }
    let mut cursor = POINT::default();
    let inside = unsafe { GetCursorPos(&mut cursor) }.is_ok()
        && cursor.x >= left
        && cursor.y >= top
        && cursor.x < left.saturating_add(width as i32)
        && cursor.y < top.saturating_add(height as i32);
    if inside {
        // The reference tool injects a real wheel, which follows the pointer.
        // That only reaches the page when the pointer is already inside the selection.
        if !LOGGED_AUTO.swap(true, Ordering::Relaxed) {
            eprintln!("scroll: auto injecting a wheel inside the selection");
        }
        send_wheel(delta);
        PRIOR_PX.store(100, Ordering::Relaxed);
        return;
    }
    if !LOGGED_AUTO.swap(true, Ordering::Relaxed) {
        eprintln!("scroll: auto scrolling the page with down-arrow");
    }
    tap_down(6);
    PRIOR_PX.store(72, Ordering::Relaxed);
}

fn send_wheel(delta: i32) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_WHEEL, MOUSEINPUT,
    };
    let mut input = [INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: delta as u32,
                dwFlags: MOUSEEVENTF_WHEEL,
                time: 0,
                dwExtraInfo: 0x5348,
            },
        },
    }];
    unsafe {
        let _ = SendInput(&mut input, std::mem::size_of::<INPUT>() as i32);
    }
}

fn foreign_window_at(x: i32, y: i32) -> windows::Win32::Foundation::HWND {
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        ChildWindowFromPointEx, GetTopWindow, GetWindow, GetWindowRect, IsWindowVisible,
        CWP_SKIPINVISIBLE, CWP_SKIPTRANSPARENT, GW_HWNDNEXT,
    };
    unsafe {
        let Ok(mut hwnd) = GetTopWindow(None) else {
            return HWND::default();
        };
        while !hwnd.0.is_null() {
            if IsWindowVisible(hwnd).as_bool() && !same_process(hwnd) {
                let mut rect = RECT::default();
                if GetWindowRect(hwnd, &mut rect).is_ok() && x >= rect.left && y >= rect.top && x < rect.right && y < rect.bottom {
                    let local = POINT { x: x - rect.left, y: y - rect.top };
                    let child = ChildWindowFromPointEx(hwnd, local, CWP_SKIPINVISIBLE | CWP_SKIPTRANSPARENT);
                    if !child.0.is_null() && !same_process(child) {
                        return child;
                    }
                    return hwnd;
                }
            }
            hwnd = match GetWindow(hwnd, GW_HWNDNEXT) {
                Ok(next) => next,
                Err(_) => break,
            };
        }
        HWND::default()
    }
}

fn same_process(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let mut owner = 0u32;
    unsafe {
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut owner));
    }
    owner == 0 || owner == std::process::id()
}

fn tap_down(times: u32) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_DOWN,
    };
    for _ in 0..times {
        let mut inputs = [
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_DOWN,
                        wScan: 0,
                        dwFlags: Default::default(),
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_DOWN,
                        wScan: 0,
                        dwFlags: KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
        ];
        unsafe {
            let _ = SendInput(&mut inputs, std::mem::size_of::<INPUT>() as i32);
        }
    }
}
