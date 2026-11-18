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
    pub path: String,
    pub screen_x: i32,
    pub screen_y: i32,
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
        .find(|rect| x >= rect.x && y >= rect.y && x < rect.x + rect.width as i32 && y < rect.y + rect.height as i32)
}

pub fn freeze_under_cursor() -> Result<FrozenCapture, String> {
    let (cursor_x, cursor_y) = cursor_point().ok_or("无法读取鼠标位置")?;
    let monitors = Monitor::all().map_err(err)?;
    if monitors.is_empty() {
        return Err("没有找到显示器".into());
    }
    let mut chosen = 0usize;
    for (index, monitor) in monitors.iter().enumerate() {
        let Ok(x) = monitor.x() else { continue };
        let Ok(y) = monitor.y() else { continue };
        let Ok(width) = monitor.width() else { continue };
        let Ok(height) = monitor.height() else { continue };
        if cursor_x >= x && cursor_y >= y && cursor_x < x + width as i32 && cursor_y < y + height as i32 {
            chosen = index;
            break;
        }
    }
    let monitor = monitors.into_iter().nth(chosen).ok_or("没有找到显示器")?;
    let x = monitor.x().map_err(err)?;
    let y = monitor.y().map_err(err)?;
    let image = monitor.capture_image().map_err(err)?;
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 || width > 20000 || height > 20000 {
        return Err("屏幕尺寸无效".into());
    }
    let path = capture_dir().join(format!("freeze-{}.png", new_id()));
    write_png(&path, &image)?;
    let windows = windows_list::list_windows(x, y, width as i32, height as i32);
    let screen = ScreenImage {
        x: 0,
        y: 0,
        width,
        height,
        path: path.to_string_lossy().to_string(),
        screen_x: x,
        screen_y: y,
    };
    Ok(FrozenCapture {
        path: screen.path.clone(),
        width,
        height,
        origin_x: x,
        origin_y: y,
        windows,
        mode: default_mode(),
        preset: None,
        screens: vec![screen],
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
    #[cfg(not(windows))]
    {
        None
    }
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
