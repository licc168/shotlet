use image::{imageops, RgbaImage};
use serde::Serialize;
use std::path::PathBuf;
use xcap::Monitor;

use crate::util::{err, new_id, write_png};
use crate::windows_list::{self, WindowInfo};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

fn default_mode() -> String {
    "edit".into()
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenImage {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Logical points on macOS, physical pixels on Windows. Window placement uses this, not `width`.
    #[serde(default)]
    pub point_width: u32,
    #[serde(default)]
    pub point_height: u32,
    pub path: String,
    pub screen_x: i32,
    pub screen_y: i32,
    /// Window rects in this screen's local coordinates. `x` above is the packed crop origin, not a window origin.
    #[serde(default)]
    pub windows: Vec<WindowInfo>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrozenCapture {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub origin_x: i32,
    pub origin_y: i32,
    pub windows: Vec<WindowInfo>,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub preset: Option<PresetRect>,
    #[serde(default)]
    pub screens: Vec<ScreenImage>,
    #[serde(default)]
    pub generation: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub fn monitor_rects() -> Vec<MonitorRect> {
    let Ok(monitors) = Monitor::all() else {
        return Vec::new();
    };
    let mut rects = Vec::new();
    for monitor in monitors {
        let Ok(x) = monitor.x() else { continue };
        let Ok(y) = monitor.y() else { continue };
        let Ok(width) = monitor.width() else { continue };
        let Ok(height) = monitor.height() else { continue };
        if width == 0 || height == 0 {
            continue;
        }
        rects.push(MonitorRect { x, y, width, height });
    }
    rects
}

pub fn monitor_at_point(rects: &[MonitorRect], x: i32, y: i32) -> Option<MonitorRect> {
    rects
        .iter()
        .copied()
        .filter(|rect| x >= rect.x && y >= rect.y && x < rect.x + rect.width as i32 && y < rect.y + rect.height as i32)
        // A scaled primary can report a rect that also contains the next display.
        // The smaller one is the display the pointer is actually on.
        .min_by_key(|rect| u64::from(rect.width) * u64::from(rect.height))
}

/// Monitor rectangles in the same coordinate space as `GetCursorPos`.
/// `xcap` device mode can be larger than that slot and make the left screen swallow the right one.
#[cfg(windows)]
fn cursor_space_monitors() -> Vec<(String, MonitorRect)> {
    use std::mem::{size_of, zeroed};
    use windows::core::BOOL;
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW};

    unsafe extern "system" fn each(monitor: HMONITOR, _dc: HDC, _rect: *mut RECT, data: LPARAM) -> BOOL {
        unsafe {
        let list = &mut *(data.0 as *mut Vec<(String, MonitorRect)>);
        let mut info: MONITORINFOEXW = zeroed();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(monitor, &mut info.monitorInfo).as_bool() {
            let name = String::from_utf16_lossy(&info.szDevice)
                .trim_end_matches('\0')
                .to_string();
            let bounds = info.monitorInfo.rcMonitor;
            let width = bounds.right.saturating_sub(bounds.left);
            let height = bounds.bottom.saturating_sub(bounds.top);
            if width > 0 && height > 0 {
                list.push((
                    name,
                    MonitorRect {
                        x: bounds.left,
                        y: bounds.top,
                        width: width as u32,
                        height: height as u32,
                    },
                ));
            }
        }
        BOOL(1)
        }
    }

    let mut list = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut list as *mut _ as isize));
    }
    list
}

#[cfg(not(windows))]
fn cursor_space_monitors() -> Vec<(String, MonitorRect)> {
    Vec::new()
}

fn cursor_space_for(name: &str, fallback: MonitorRect, os: &[(String, MonitorRect)]) -> MonitorRect {
    os.iter()
        .find(|(device, _)| device == name)
        .map(|(_, rect)| *rect)
        .unwrap_or(fallback)
}

fn scale_windows_to_image(windows: &mut [WindowInfo], slot_w: u32, slot_h: u32, image_w: u32, image_h: u32) {
    if slot_w == 0 || slot_h == 0 || (slot_w == image_w && slot_h == image_h) {
        return;
    }
    let sx = image_w as f32 / slot_w as f32;
    let sy = image_h as f32 / slot_h as f32;
    for window in windows {
        window.x = (window.x as f32 * sx).round() as i32;
        window.y = (window.y as f32 * sy).round() as i32;
        window.width = (window.width as f32 * sx).round().max(1.0) as i32;
        window.height = (window.height as f32 * sy).round().max(1.0) as i32;
    }
}

fn intersects_local(x: i32, y: i32, w: i32, h: i32, width: i32, height: i32) -> bool {
    if w <= 0 || h <= 0 || width <= 0 || height <= 0 {
        return false;
    }
    let x0 = i64::from(x);
    let y0 = i64::from(y);
    let x1 = x0 + i64::from(w);
    let y1 = y0 + i64::from(h);
    x1 > 0 && y1 > 0 && x0 < i64::from(width) && y0 < i64::from(height)
}

pub fn freeze_under_cursor() -> Result<FrozenCapture, String> {
    ensure_screen_capture()?;
    let (cursor_x, cursor_y) = cursor_point().ok_or("无法读取鼠标位置")?;
    let mut monitors = Monitor::all().map_err(err)?;
    if monitors.is_empty() {
        return Err("没有找到显示器".into());
    }
    monitors.sort_by_key(|monitor| (monitor.x().unwrap_or(i32::MAX), monitor.y().unwrap_or(i32::MAX)));
    let os_monitors = cursor_space_monitors();
    let mut screens = Vec::new();
    let mut packed_x: i32 = 0;
    let mut chosen: Option<usize> = None;
    for monitor in monitors {
        let Ok(sx) = monitor.x() else { continue };
        let Ok(sy) = monitor.y() else { continue };
        let Ok(point_w) = monitor.width() else { continue };
        let Ok(point_h) = monitor.height() else { continue };
        if point_w == 0 || point_h == 0 {
            continue;
        }
        let slot = cursor_space_for(
            &monitor.name().unwrap_or_default(),
            MonitorRect { x: sx, y: sy, width: point_w, height: point_h },
            &os_monitors,
        );
        let Ok(image) = monitor.capture_image() else { continue };
        let width = image.width();
        let height = image.height();
        if width == 0 || height == 0 || width > 20000 || height > 20000 {
            continue;
        }
        let path = capture_dir().join(format!("freeze-{}.png", new_id()));
        write_png(&path, &image)?;
        // Window positions share the monitor's point space, which is not the packed crop origin.
        let mut windows = windows_list::list_windows(slot.x, slot.y, slot.width as i32, slot.height as i32);
        windows.retain(|window| intersects_local(window.x, window.y, window.width, window.height, slot.width as i32, slot.height as i32));
        scale_windows_to_image(&mut windows, slot.width, slot.height, width, height);
        if cursor_x >= slot.x && cursor_y >= slot.y && cursor_x < slot.x + slot.width as i32 && cursor_y < slot.y + slot.height as i32 {
            chosen = Some(screens.len());
        }
        screens.push(ScreenImage {
            x: packed_x,
            y: 0,
            width,
            height,
            point_width: slot.width,
            point_height: slot.height,
            path: path.to_string_lossy().to_string(),
            screen_x: slot.x,
            screen_y: slot.y,
            windows,
        });
        packed_x = packed_x.saturating_add(width as i32);
    }
    if screens.is_empty() {
        return Err("没有找到显示器".into());
    }
    let primary = screens[chosen.unwrap_or(0)].clone();
    Ok(FrozenCapture {
        path: primary.path.clone(),
        width: primary.width,
        height: primary.height,
        origin_x: primary.screen_x,
        origin_y: primary.screen_y,
        windows: primary.windows.clone(),
        mode: default_mode(),
        preset: None,
        screens,
        generation: 0,
    })
}

pub fn crop_screens(screens: &[ScreenImage], x: i32, y: i32, width: u32, height: u32) -> Result<RgbaImage, String> {
    if width == 0 || height == 0 {
        return Err("选区尺寸无效".into());
    }
    let mut canvas = RgbaImage::new(width, height);
    let right = x.saturating_add(width as i32);
    let bottom = y.saturating_add(height as i32);
    let mut hit = false;
    for screen in screens {
        let x0 = x.max(screen.x);
        let y0 = y.max(screen.y);
        let x1 = right.min(screen.x + screen.width as i32);
        let y1 = bottom.min(screen.y + screen.height as i32);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let image = crate::util::read_png_file(std::path::Path::new(&screen.path))?;
        let piece = crop_frozen(&image, x0 - screen.x, y0 - screen.y, (x1 - x0) as u32, (y1 - y0) as u32)?;
        imageops::overlay(&mut canvas, &piece, (x0 - x) as i64, (y0 - y) as i64);
        hit = true;
    }
    if !hit {
        return Err("选区超出画面".into());
    }
    Ok(canvas)
}

pub fn window_under_cursor(origin_x: i32, origin_y: i32, windows: &[WindowInfo]) -> Option<PresetRect> {
    let (screen_x, screen_y) = cursor_point()?;
    top_window_at(windows, screen_x - origin_x, screen_y - origin_y)
}

pub fn top_window_at(windows: &[WindowInfo], x: i32, y: i32) -> Option<PresetRect> {
    windows
        .iter()
        .filter(|window| window.width > 0 && window.height > 0)
        .filter(|window| x >= window.x && y >= window.y && x < window.x + window.width && y < window.y + window.height)
        .min_by_key(|window| window.z)
        .map(|window| PresetRect {
            x: window.x,
            y: window.y,
            w: window.width as u32,
            h: window.height as u32,
        })
}

pub fn cursor_point() -> Option<(i32, i32)> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut point = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut point) }.is_ok() {
            return Some((point.x, point.y));
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        macos_cursor_point()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        None
    }
}

/// Ask for Screen Recording on the main thread. Capture itself only checks.
#[cfg(target_os = "macos")]
pub fn prepare_capture_access() {
    unsafe {
        if !CGPreflightScreenCaptureAccess() {
            let _ = CGRequestScreenCaptureAccess();
        }
    }
}

pub fn ensure_screen_capture() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        if unsafe { CGPreflightScreenCaptureAccess() } {
            return Ok(());
        }
        return Err("没有屏幕录制权限。请到系统设置 → 隐私与安全性 → 屏幕录制，允许 Shotlet 后再按 F1。".into());
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn macos_cursor_point() -> Option<(i32, i32)> {
    unsafe {
        let event = CGEventCreate(std::ptr::null_mut());
        if event.is_null() {
            return None;
        }
        let point = CGEventGetLocation(event);
        CFRelease(event as *const std::ffi::c_void);
        if !point.x.is_finite() || !point.y.is_finite() {
            return None;
        }
        Some((point.x.round() as i32, point.y.round() as i32))
    }
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct CgPoint {
    x: f64,
    y: f64,
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreate(source: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn CGEventGetLocation(event: *mut std::ffi::c_void) -> CgPoint;
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const std::ffi::c_void);
}

pub fn crop_frozen(image: &RgbaImage, x: i32, y: i32, width: u32, height: u32) -> Result<RgbaImage, String> {
    if width == 0 || height == 0 {
        return Err("选区尺寸无效".into());
    }
    let limit_x = image.width() as i32;
    let limit_y = image.height() as i32;
    let x0 = x.clamp(0, limit_x);
    let y0 = y.clamp(0, limit_y);
    let x1 = x.saturating_add(width as i32).clamp(0, limit_x);
    let y1 = y.saturating_add(height as i32).clamp(0, limit_y);
    if x1 <= x0 || y1 <= y0 {
        return Err("选区超出画面".into());
    }
    Ok(imageops::crop_imm(image, x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32).to_image())
}

/// Capture a rectangle in one monitor's image pixels.
/// `screen_x` / `screen_y` are that monitor's origin in cursor space, the same values stored on a frozen screen.
pub fn capture_local(screen_x: i32, screen_y: i32, x: u32, y: u32, width: u32, height: u32) -> Result<RgbaImage, String> {
    if width == 0 || height == 0 {
        return Err("选区尺寸无效".into());
    }
    let monitors = Monitor::all().map_err(err)?;
    let os = cursor_space_monitors();
    for monitor in monitors {
        let Ok(mx) = monitor.x() else { continue };
        let Ok(my) = monitor.y() else { continue };
        let Ok(mw) = monitor.width() else { continue };
        let Ok(mh) = monitor.height() else { continue };
        if mw == 0 || mh == 0 {
            continue;
        }
        let slot = cursor_space_for(
            &monitor.name().unwrap_or_default(),
            MonitorRect { x: mx, y: my, width: mw, height: mh },
            &os,
        );
        if slot.x != screen_x || slot.y != screen_y {
            continue;
        }
        let x0 = x.min(mw.saturating_sub(1));
        let y0 = y.min(mh.saturating_sub(1));
        let w = width.min(mw.saturating_sub(x0)).max(1);
        let h = height.min(mh.saturating_sub(y0)).max(1);
        return monitor.capture_region(x0, y0, w, h).map_err(err);
    }
    capture_rect(screen_x.saturating_add(x as i32), screen_y.saturating_add(y as i32), width, height)
}

pub fn capture_rect(x: i32, y: i32, width: u32, height: u32) -> Result<RgbaImage, String> {
    if width == 0 || height == 0 || width > 16000 || height > 16000 {
        return Err("选区尺寸无效".into());
    }
    let monitors = Monitor::all().map_err(err)?;
    let mut canvas = RgbaImage::new(width, height);
    let right = x.saturating_add(width as i32);
    let bottom = y.saturating_add(height as i32);
    for monitor in monitors {
        let mx = monitor.x().map_err(err)?;
        let my = monitor.y().map_err(err)?;
        let mw = monitor.width().map_err(err)? as i32;
        let mh = monitor.height().map_err(err)? as i32;
        let x0 = x.max(mx);
        let y0 = y.max(my);
        let x1 = right.min(mx + mw);
        let y1 = bottom.min(my + mh);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let shot = monitor
            .capture_region(
                (x0 - mx) as u32,
                (y0 - my) as u32,
                (x1 - x0) as u32,
                (y1 - y0) as u32,
            )
            .map_err(err)?;
        imageops::overlay(&mut canvas, &shot, (x0 - x) as i64, (y0 - y) as i64);
    }
    Ok(canvas)
}

#[cfg(test)]
mod tests {
    use super::{crop_frozen, top_window_at};
    use crate::windows_list::WindowInfo;
    use image::{Rgba, RgbaImage};

    #[test]
    fn crop_clamps_to_the_frozen_image() {
        let image = RgbaImage::from_pixel(10, 8, Rgba([1, 2, 3, 255]));
        let cropped = crop_frozen(&image, -2, -2, 6, 5).unwrap();
        assert_eq!(cropped.dimensions(), (4, 3));
        assert!(crop_frozen(&image, 20, 20, 4, 4).is_err());
    }

    #[test]
    fn local_window_keeps_only_the_intersection() {
        assert!(super::intersects_local(-20, 10, 40, 30, 100, 80));
        assert!(!super::intersects_local(100, 0, 40, 30, 100, 80));
        assert!(!super::intersects_local(10, 90, 20, 20, 100, 80));
        assert!(!super::intersects_local(10, 10, 0, 20, 100, 80));
    }

    #[test]
    fn monitor_at_point_hits_the_display_under_the_cursor() {
        let rects = vec![
            super::MonitorRect { x: 0, y: 0, width: 1920, height: 1080 },
            super::MonitorRect { x: 1920, y: 0, width: 1920, height: 1080 },
        ];
        assert_eq!(super::monitor_at_point(&rects, 2000, 40).unwrap().x, 1920);
        assert_eq!(super::monitor_at_point(&rects, 1919, 1079).unwrap().x, 0);
        assert!(super::monitor_at_point(&rects, 1920, 1080).is_none());
        let overlapping = vec![
            super::MonitorRect { x: 0, y: 0, width: 3000, height: 1080 },
            super::MonitorRect { x: 1920, y: 0, width: 1920, height: 1080 },
        ];
        assert_eq!(super::monitor_at_point(&overlapping, 2000, 40).unwrap().x, 1920);
    }

    #[test]
    fn top_window_at_picks_the_front_window() {
        let windows = vec![
            WindowInfo { title: "back".into(), x: 0, y: 0, width: 100, height: 80, z: 2 },
            WindowInfo { title: "front".into(), x: 10, y: 10, width: 40, height: 30, z: 0 },
        ];
        let picked = top_window_at(&windows, 20, 20).unwrap();
        assert_eq!(picked.x, 10);
        assert_eq!(picked.w, 40);
        assert!(top_window_at(&windows, 200, 200).is_none());
    }
}

pub fn capture_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("shotlet");
    std::fs::create_dir_all(&dir).ok();
    dir
}
