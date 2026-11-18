use serde::Serialize;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowInfo {
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub z: u32,
}

pub fn list_windows(origin_x: i32, origin_y: i32, desktop_w: i32, desktop_h: i32) -> Vec<WindowInfo> {
    #[cfg(windows)]
    {
        return list_windows_win(origin_x, origin_y, desktop_w, desktop_h);
    }
    #[cfg(not(windows))]
    {
        let _ = (desktop_w, desktop_h);
        list_windows_xcap(origin_x, origin_y)
    }
}

#[cfg(not(windows))]
fn list_windows_xcap(origin_x: i32, origin_y: i32) -> Vec<WindowInfo> {
    let Ok(windows) = xcap::Window::all() else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for (index, window) in windows.into_iter().enumerate() {
        if window.is_minimized().unwrap_or(true) {
            continue;
        }
        let Ok(title) = window.title() else { continue };
        if title.trim().is_empty() {
            continue;
        }
        let Ok(x) = window.x() else { continue };
        let Ok(y) = window.y() else { continue };
        let Ok(width) = window.width() else { continue };
        let Ok(height) = window.height() else { continue };
        if width < 40 || height < 40 {
            continue;
        }
        items.push(WindowInfo {
            title,
            x: x - origin_x,
            y: y - origin_y,
            width: width as i32,
            height: height as i32,
            z: index as u32,
        });
    }
    items
}

#[cfg(windows)]
fn list_windows_win(origin_x: i32, origin_y: i32, desktop_w: i32, desktop_h: i32) -> Vec<WindowInfo> {
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::core::BOOL;
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
        IsIconic, IsWindowVisible,
    };

    struct Collect {
        origin_x: i32,
        origin_y: i32,
        desktop_w: i32,
        desktop_h: i32,
        pid: u32,
        items: Vec<WindowInfo>,
    }

    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> BOOL {
        each_window(hwnd, lparam)
    }

    fn each_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
        unsafe {
        let collect = &mut *(lparam.0 as *mut Collect);
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return true.into();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == collect.pid {
            return true.into();
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _,
            std::mem::size_of::<u32>() as u32,
        );
        if cloaked != 0 {
            return true.into();
        }
        let mut rect = RECT::default();
        let extended = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut _,
            std::mem::size_of::<RECT>() as u32,
        );
        if extended.is_err() {
            let _ = GetWindowRect(hwnd, &mut rect);
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width < 48 || height < 48 {
            return true.into();
        }
        let length = GetWindowTextLengthW(hwnd);
        if length <= 0 {
            return true.into();
        }
        let mut buf = vec![0u16; length as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buf[..]);
        if copied <= 0 {
            return true.into();
        }
        let title = String::from_utf16_lossy(&buf[..copied as usize]).trim().to_string();
        if title.is_empty() {
            return true.into();
        }
        let x = rect.left - collect.origin_x;
        let y = rect.top - collect.origin_y;
        if x > collect.desktop_w || y > collect.desktop_h || x + width < 0 || y + height < 0 {
            return true.into();
        }
        let z = collect.items.len() as u32;
        collect.items.push(WindowInfo { title, x, y, width, height, z });
        true.into()
        }
    }

    let mut collect = Collect {
        origin_x,
        origin_y,
        desktop_w,
        desktop_h,
        pid: std::process::id(),
        items: Vec::new(),
    };
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut collect as *mut Collect as isize));
    }
    collect.items
}
