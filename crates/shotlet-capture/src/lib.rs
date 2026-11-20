mod windows_list;

use image::{imageops, RgbaImage};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};
use xcap::Monitor;

use shotlet_platform::{err, new_id, trace, write_frozen};
pub use shotlet_types::{FrozenCapture, PresetRect, ScreenImage, WindowInfo};

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

fn cached_screen(path: &str) -> Result<Arc<RgbaImage>, String> {
    if let Ok(slot) = preview_cache().lock() {
        if let Some(image) = slot.images.get(path) {
            return Ok(Arc::clone(image));
        }
    }
    Ok(Arc::new(shotlet_platform::read_png_file(std::path::Path::new(path))?))
}

struct PreviewCache {
    images: HashMap<String, Arc<RgbaImage>>,
    order: VecDeque<String>,
}

fn preview_cache() -> &'static Mutex<PreviewCache> {
    static CACHE: OnceLock<Mutex<PreviewCache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(PreviewCache {
            images: HashMap::new(),
            order: VecDeque::new(),
        })
    })
}

pub fn remember_preview(key: &str, image: Arc<RgbaImage>) {
    if let Ok(mut cache) = preview_cache().lock() {
        store_preview(&mut cache, key.to_string(), image);
    }
}

fn store_preview(cache: &mut PreviewCache, key: String, image: Arc<RgbaImage>) {
    if cache.images.insert(key.clone(), image).is_none() {
        cache.order.push_back(key);
    }
    // The newest frames stay. Dropping one whose file is still being written only
    // happens after several later shots, when that overlay is already gone.
    while cache.order.len() > 6 {
        if let Some(old) = cache.order.pop_front() {
            cache.images.remove(&old);
            forget_preview_file(&old);
        }
    }
}

/// Drop cached pixels. The freeze files stay until eviction or the startup sweep,
/// because a crop can still be reading the one that was just taken.
pub fn release_previews() {
    if let Ok(mut cache) = preview_cache().lock() {
        cache.images.clear();
        cache.order.clear();
    }
}

fn forget_preview_file(key: &str) {
    let path = std::path::Path::new(key);
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    if name.starts_with("freeze-") && name.ends_with(".shot") {
        let _ = std::fs::remove_file(path);
    }
}

/// Delete capture temps older than six hours. `keep` is the pin files still on screen.
pub fn sweep_temp_files(keep: &[PathBuf]) {
    let dir = capture_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let now = SystemTime::now();
    let max_age = Duration::from_secs(6 * 60 * 60);
    for entry in entries.flatten() {
        let path = entry.path();
        if keep.iter().any(|kept| kept == &path) {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let temporary = (name.starts_with("freeze-") && name.ends_with(".shot"))
            || (name.starts_with("scroll-") && name.ends_with(".png"))
            || (name.starts_with("pin-") && name.ends_with(".png"));
        if !temporary {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age > max_age {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn monitors_cached() -> Result<Vec<Monitor>, String> {
    thread_local! {
        static CACHED: RefCell<Option<(Instant, Vec<Monitor>)>> = RefCell::new(None);
    }
    CACHED.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some((stamp, monitors)) = slot.as_ref() {
            if stamp.elapsed() < Duration::from_secs(2) {
                return Ok(monitors.clone());
            }
        }
        let monitors = Monitor::all().map_err(err)?;
        *slot = Some((Instant::now(), monitors.clone()));
        Ok(monitors)
    })
}

fn blit_rgba(canvas: &mut RgbaImage, shot: &RgbaImage, dest_x: i64, dest_y: i64) {
    let canvas_w = i64::from(canvas.width());
    let canvas_h = i64::from(canvas.height());
    let shot_w = i64::from(shot.width());
    let shot_h = i64::from(shot.height());
    let x0 = dest_x.max(0);
    let y0 = dest_y.max(0);
    let x1 = (dest_x + shot_w).min(canvas_w);
    let y1 = (dest_y + shot_h).min(canvas_h);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let src_x = (x0 - dest_x) as usize;
    let copy_bytes = ((x1 - x0) as usize) * 4;
    let shot_stride = (shot.width() as usize) * 4;
    let canvas_stride = (canvas.width() as usize) * 4;
    let shot_raw = shot.as_raw();
    let canvas_raw = canvas.as_mut();
    for row in 0..(y1 - y0) as usize {
        let sy = (y0 - dest_y) as usize + row;
        let dy = y0 as usize + row;
        let src_off = sy * shot_stride + src_x * 4;
        let dst_off = dy * canvas_stride + (x0 as usize) * 4;
        canvas_raw[dst_off..dst_off + copy_bytes].copy_from_slice(&shot_raw[src_off..src_off + copy_bytes]);
    }
}

/// Pixels kept from the last freeze so the overlay can paint without decoding a file.
/// The copy happens after the cache lock is released, so a second screen asking for
/// its pixels does not wait behind the first screen's copy.
pub fn preview_bytes(path: &str) -> Result<Vec<u8>, String> {
    let cached = preview_cache()
        .lock()
        .ok()
        .and_then(|slot| slot.images.get(path).map(Arc::clone));
    if let Some(image) = cached {
        return Ok(image.as_raw().clone());
    }
    Ok(shotlet_platform::read_png_file(std::path::Path::new(path))?.into_raw())
}

/// A display grabbed after the monitor under the cursor is already on screen.
/// `device` is the geometry already read for this monitor, so the later grab matches
/// the handle by that instead of asking xcap for it again.
#[derive(Clone)]
pub struct LaterMonitor {
    device: MonitorRect,
    slot: MonitorRect,
}

pub struct FreezeShot {
    pub frozen: FrozenCapture,
    pub later: Vec<LaterMonitor>,
}

struct PreparedMonitor {
    monitor: Monitor,
    device: MonitorRect,
    slot: MonitorRect,
}

pub fn freeze_under_cursor() -> Result<FreezeShot, String> {
    trace("capture: monitors begin");
    ensure_screen_capture()?;
    let (cursor_x, cursor_y) = cursor_point().ok_or("无法读取鼠标位置")?;
    let monitors = Monitor::all().map_err(err)?;
    if monitors.is_empty() {
        return Err("没有找到显示器".into());
    }
    let os_monitors = cursor_space_monitors();
    let mut prepared = Vec::new();
    for monitor in monitors {
        let Some(device) = device_rect(&monitor) else { continue };
        let slot = cursor_space_for(&monitor.name().unwrap_or_default(), device, &os_monitors);
        prepared.push(PreparedMonitor { monitor, device, slot });
    }
    prepared.sort_by_key(|item| (item.slot.x, item.slot.y));
    trace(&format!("capture: monitors ready count={}", prepared.len()));
    if prepared.is_empty() {
        return Err("没有找到显示器".into());
    }
    let mut order: Vec<usize> = (0..prepared.len()).collect();
    if let Some(pos) = order.iter().position(|&index| hit_slot(prepared[index].slot, cursor_x, cursor_y)) {
        let index = order.remove(pos);
        order.insert(0, index);
    }
    let mut primary = None;
    let mut later = Vec::new();
    for index in order {
        let slot = prepared[index].slot;
        if primary.is_none() {
            if let Some(screen) = capture_published(&prepared[index].monitor, slot, 0) {
                primary = Some(screen);
                continue;
            }
        }
        later.push(LaterMonitor { device: prepared[index].device, slot });
    }
    let Some(primary) = primary else {
        return Err("没有找到显示器".into());
    };
    later.sort_by_key(|item| (item.slot.x, item.slot.y));
    Ok(FreezeShot {
        frozen: FrozenCapture {
            path: primary.path.clone(),
            width: primary.width,
            height: primary.height,
            origin_x: primary.screen_x,
            origin_y: primary.screen_y,
            windows: primary.windows.clone(),
            mode: shotlet_types::default_mode(),
            preset: None,
            screens: vec![primary],
            generation: 0,
        },
        later,
    })
}

pub fn capture_later(pending: &[LaterMonitor], mut packed_x: i32) -> Vec<ScreenImage> {
    if pending.is_empty() {
        return Vec::new();
    }
    let Ok(monitors) = Monitor::all() else {
        return Vec::new();
    };
    // The handle cannot cross a thread, so the monitors are listed again. Each one
    // is matched by the geometry stored the first time, which is not read again.
    let mut matched = Vec::new();
    for monitor in &monitors {
        if let Some(device) = device_rect(monitor) {
            matched.push((monitor, device));
        }
    }
    let mut screens = Vec::new();
    for item in pending {
        let Some(monitor) = matched.iter().find(|(_, device)| *device == item.device).map(|(monitor, _)| *monitor) else {
            continue;
        };
        let Some(screen) = capture_published(monitor, item.slot, packed_x) else {
            continue;
        };
        packed_x = packed_x.saturating_add(screen.width as i32);
        screens.push(screen);
    }
    screens
}

/// One read of a monitor's device geometry. xcap asks Windows again on every call.
fn device_rect(monitor: &Monitor) -> Option<MonitorRect> {
    let x = monitor.x().ok()?;
    let y = monitor.y().ok()?;
    let width = monitor.width().ok()?;
    let height = monitor.height().ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    Some(MonitorRect { x, y, width, height })
}

fn hit_slot(slot: MonitorRect, x: i32, y: i32) -> bool {
    x >= slot.x && y >= slot.y && x < slot.x + slot.width as i32 && y < slot.y + slot.height as i32
}

fn capture_published(monitor: &Monitor, slot: MonitorRect, packed_x: i32) -> Option<ScreenImage> {
    trace(&format!("capture: grab begin at {},{}", slot.x, slot.y));
    let windows_job = spawn_window_list(slot);
    let captured = monitor.capture_image();
    trace(&format!("capture: grab pixels at {},{}", slot.x, slot.y));
    let mut windows = windows_job.join().unwrap_or_default();
    trace(&format!("capture: windows listed at {},{}", slot.x, slot.y));
    let image = captured.ok()?;
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 || width > 20000 || height > 20000 {
        return None;
    }
    // Window positions share the monitor's point space, which is not the packed crop origin.
    scale_windows_to_image(&mut windows, slot.width, slot.height, width, height);
    let path = publish_image(image);
    Some(ScreenImage {
        x: packed_x,
        y: 0,
        width,
        height,
        point_width: slot.width,
        point_height: slot.height,
        path,
        screen_x: slot.x,
        screen_y: slot.y,
        windows,
    })
}

fn spawn_window_list(slot: MonitorRect) -> std::thread::JoinHandle<Vec<WindowInfo>> {
    std::thread::spawn(move || {
        let mut windows = windows_list::list_windows(slot.x, slot.y, slot.width as i32, slot.height as i32);
        windows.retain(|window| intersects_local(window.x, window.y, window.width, window.height, slot.width as i32, slot.height as i32));
        windows
    })
}

struct PendingWrite {
    path: PathBuf,
    image: Arc<RgbaImage>,
}

fn pending_writes() -> &'static Mutex<Vec<PendingWrite>> {
    static PENDING: OnceLock<Mutex<Vec<PendingWrite>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(Vec::new()))
}

/// Keep the pixels in memory. The file is written later, once the overlay is up,
/// because scanning a new file can take as long as the capture itself.
fn publish_image(image: RgbaImage) -> String {
    trace("capture: publish");
    let path = capture_dir().join(format!("freeze-{}.shot", new_id()));
    let key = path.to_string_lossy().to_string();
    let image = Arc::new(image);
    if let Ok(mut cache) = preview_cache().lock() {
        store_preview(&mut cache, key.clone(), Arc::clone(&image));
    }
    if let Ok(mut pending) = pending_writes().lock() {
        pending.push(PendingWrite { path, image });
    }
    key
}

/// Write the frozen frames that have been waiting. Called after the overlay is on screen.
pub fn flush_frozen() {
    let pending = pending_writes().lock().map(|mut slot| std::mem::take(&mut *slot)).unwrap_or_default();
    if pending.is_empty() {
        return;
    }
    trace(&format!("capture: flush files count={}", pending.len()));
    std::thread::spawn(move || {
        for item in pending {
            let _ = write_frozen(&item.path, &item.image);
        }
        trace("capture: flush files done");
    });
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
        let image = cached_screen(&screen.path)?;
        let piece = crop_frozen(&image, x0 - screen.x, y0 - screen.y, (x1 - x0) as u32, (y1 - y0) as u32)?;
        blit_rgba(&mut canvas, &piece, (x0 - x) as i64, (y0 - y) as i64);
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
    let monitors = monitors_cached()?;
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
    let monitors = monitors_cached()?;
    let right = x.saturating_add(width as i32);
    let bottom = y.saturating_add(height as i32);
    struct Hit {
        index: usize,
        mx: i32,
        my: i32,
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
    }
    let mut hits = Vec::new();
    for (index, monitor) in monitors.iter().enumerate() {
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
        hits.push(Hit { index, mx, my, x0, y0, x1, y1 });
    }
    if let [hit] = hits.as_slice() {
        if hit.x0 == x && hit.y0 == y && hit.x1 == right && hit.y1 == bottom {
            return monitors[hit.index]
                .capture_region((x - hit.mx) as u32, (y - hit.my) as u32, width, height)
                .map_err(err);
        }
    }
    let mut canvas = RgbaImage::new(width, height);
    for hit in hits {
        let shot = monitors[hit.index]
            .capture_region(
                (hit.x0 - hit.mx) as u32,
                (hit.y0 - hit.my) as u32,
                (hit.x1 - hit.x0) as u32,
                (hit.y1 - hit.y0) as u32,
            )
            .map_err(err)?;
        blit_rgba(&mut canvas, &shot, (hit.x0 - x) as i64, (hit.y0 - y) as i64);
    }
    Ok(canvas)
}

#[cfg(test)]
mod tests {
    use super::{crop_frozen, top_window_at};
    use shotlet_types::WindowInfo;
    use image::{Rgba, RgbaImage};

    #[test]
    fn preview_returns_the_captured_pixels() {
        let image = RgbaImage::from_pixel(2, 1, Rgba([9, 8, 7, 255]));
        super::remember_preview("memory-shot", std::sync::Arc::new(image.clone()));
        assert_eq!(super::preview_bytes("memory-shot").unwrap().as_slice(), image.as_raw().as_slice());
    }

    #[test]
    fn crop_reads_the_preview_cache_instead_of_disk() {
        use shotlet_types::ScreenImage;
        let image = RgbaImage::from_fn(4, 3, |x, y| Rgba([x as u8, y as u8, 9, 255]));
        super::remember_preview("cached-crop", std::sync::Arc::new(image));
        let screens = vec![ScreenImage {
            x: 10,
            y: 0,
            width: 4,
            height: 3,
            point_width: 4,
            point_height: 3,
            path: "cached-crop".into(),
            screen_x: 0,
            screen_y: 0,
            windows: Vec::new(),
        }];
        let cropped = super::crop_screens(&screens, 11, 1, 2, 2).unwrap();
        assert_eq!(cropped.dimensions(), (2, 2));
        assert_eq!(cropped.get_pixel(0, 0).0, [1, 1, 9, 255]);
    }

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
