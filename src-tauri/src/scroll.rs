use image::RgbaImage;
use std::thread;
use std::time::{Duration, Instant};

use crate::capture::{self, capture_rect};
use crate::stitch::{append_rows, frames_stable, revealed_rows, scroll_motion, ScrollMotion};
use crate::util::{new_id, write_png};

pub struct ScrollResult {
    pub path: String,
    pub width: u32,
    pub height: u32,
}

const MAX_HEIGHT: u32 = 14_000;
const MAX_STEPS: u32 = 60;

/// Wheel step in pixels: about 28% of the viewport, clamped to 36..=280 and under a third of the height.
pub fn scroll_step(height: u32) -> u32 {
    let limit = (height / 3).saturating_sub(1);
    if limit < 2 {
        return 1;
    }
    let raw = height.saturating_mul(28) / 100;
    raw.clamp(36, 280).min(limit)
}

pub fn ensure_scroll_access() -> Result<(), String> {
    input::ensure_scroll_access()
}

pub fn capture_scrolled(x: i32, y: i32, width: u32, height: u32) -> Result<ScrollResult, String> {
    if width < 20 || height < 20 {
        return Err("滚动选区太小".into());
    }
    let center_x = x + width as i32 / 2;
    let center_y = y + height as i32 / 2;
    let session = input::Session::engage(center_x, center_y);
    input::focus_at(center_x, center_y);
    thread::sleep(Duration::from_millis(80));
    if input::escape_down() {
        return Err("滚动截图已取消".into());
    }

    let baseline = capture_rect(x, y, width, height)?;
    let mut head = baseline.clone();
    let mut previous = baseline;
    let mut strips: Vec<RgbaImage> = Vec::new();
    let step = scroll_step(height);
    let mut sign: i32 = -1;
    let mut flipped = false;
    let mut phase = Phase::Pixel;
    let mut misses = 0u32;
    let mut cancelled = false;

    for _ in 0..MAX_STEPS {
        if phase == Phase::Stop || input::escape_down() {
            cancelled = phase != Phase::Stop && input::escape_down();
            break;
        }
        let used = stacked_height(&head, &strips);
        if used >= MAX_HEIGHT {
            break;
        }
        let pixel = phase.uses_pixels();
        let magnitude = if pixel { step as i32 } else { line_amount(step) };
        input::scroll(&session, sign * magnitude, pixel);
        let settled = wait_settled(x, y, width, height)?;
        if settled.cancelled {
            cancelled = true;
            break;
        }
        let frame = settled.frame;
        let expected = Some(if pixel { step } else { (line_amount(step) as u32).saturating_mul(10) });
        if phase.is_tracking() {
            match revealed_rows(&previous, &frame, expected).filter(|rows| *rows >= 2) {
                Some(rows) => {
                    let rows = rows.min(MAX_HEIGHT - used);
                    if rows < 2 {
                        break;
                    }
                    strips.push(bottom_rows(&frame, rows));
                    previous = frame;
                    misses = 0;
                }
                None => {
                    misses += 1;
                    if misses >= 2 {
                        break;
                    }
                }
            }
            continue;
        }

        match scroll_motion(&previous, &frame) {
            ScrollMotion::Down => {
                if let Some(rows) = revealed_rows(&previous, &frame, expected).filter(|rows| *rows >= 2) {
                    let rows = rows.min(MAX_HEIGHT - used);
                    if rows < 2 {
                        break;
                    }
                    strips.push(bottom_rows(&frame, rows));
                    previous = frame;
                    misses = 0;
                    phase = if pixel { Phase::TrackPixel } else { Phase::TrackLine };
                    continue;
                }
                phase = advance_probe(phase, &session, center_x, center_y, x, y, width, height, &mut head, &mut previous)?;
            }
            ScrollMotion::Up if !flipped => {
                sign = -sign;
                flipped = true;
                input::scroll(&session, sign * magnitude, pixel);
                let back = wait_settled(x, y, width, height)?;
                if back.cancelled {
                    cancelled = true;
                    break;
                }
                previous = back.frame;
                if strips.is_empty() {
                    head = previous.clone();
                }
            }
            ScrollMotion::Up | ScrollMotion::Still => {
                phase = advance_probe(phase, &session, center_x, center_y, x, y, width, height, &mut head, &mut previous)?;
            }
        }
    }

    if strips.is_empty() {
        if cancelled {
            return Err("滚动截图已取消".into());
        }
        return Err("没有检测到滚动。请把选区放在可以上下滚动的内容上，并尽量避开大块固定表头。".into());
    }
    save_long(compose(&head, &strips))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Pixel,
    Line,
    AfterClick,
    TrackPixel,
    TrackLine,
    Stop,
}

impl Phase {
    fn uses_pixels(self) -> bool {
        matches!(self, Phase::Pixel | Phase::AfterClick | Phase::TrackPixel)
    }

    fn is_tracking(self) -> bool {
        matches!(self, Phase::TrackPixel | Phase::TrackLine)
    }
}

fn advance_probe(
    phase: Phase,
    session: &input::Session,
    center_x: i32,
    center_y: i32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    head: &mut RgbaImage,
    previous: &mut RgbaImage,
) -> Result<Phase, String> {
    match phase {
        Phase::Pixel => Ok(Phase::Line),
        Phase::Line => {
            input::focus_at(center_x, center_y);
            input::click(session);
            thread::sleep(Duration::from_millis(160));
            let frame = capture_rect(x, y, width, height)?;
            *previous = frame;
            *head = previous.clone();
            Ok(Phase::AfterClick)
        }
        _ => Ok(Phase::Stop),
    }
}

fn line_amount(step: u32) -> i32 {
    (step / 10).clamp(2, 8) as i32
}

fn stacked_height(head: &RgbaImage, strips: &[RgbaImage]) -> u32 {
    head.height().saturating_add(strips.iter().map(|strip| strip.height()).sum())
}

fn bottom_rows(image: &RgbaImage, rows: u32) -> RgbaImage {
    let rows = rows.min(image.height());
    let top = image.height() - rows;
    image::imageops::crop_imm(image, 0, top, image.width(), rows).to_image()
}

fn compose(head: &RgbaImage, strips: &[RgbaImage]) -> RgbaImage {
    let mut image = head.clone();
    for strip in strips {
        image = append_rows(&image, strip);
    }
    image
}

struct Settled {
    frame: RgbaImage,
    cancelled: bool,
}

fn wait_settled(x: i32, y: i32, width: u32, height: u32) -> Result<Settled, String> {
    thread::sleep(Duration::from_millis(120));
    if input::escape_down() {
        return Ok(Settled { frame: capture_rect(x, y, width, height)?, cancelled: true });
    }
    let mut previous = capture_rect(x, y, width, height)?;
    let deadline = Instant::now() + Duration::from_millis(700);
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(80));
        if input::escape_down() {
            return Ok(Settled { frame: previous, cancelled: true });
        }
        let frame = capture_rect(x, y, width, height)?;
        if frames_stable(&previous, &frame) {
            return Ok(Settled { frame, cancelled: false });
        }
        previous = frame;
    }
    Ok(Settled { frame: previous, cancelled: false })
}

fn save_long(image: RgbaImage) -> Result<ScrollResult, String> {
    let path = capture::capture_dir().join(format!("scroll-{}.png", new_id()));
    write_png(&path, &image)?;
    Ok(ScrollResult {
        path: path.to_string_lossy().to_string(),
        width: image.width(),
        height: image.height(),
    })
}

#[cfg(target_os = "macos")]
mod input {
    use super::*;
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CgPoint {
        x: f64,
        y: f64,
    }

    const UNIT_PIXEL: u32 = 0;
    const UNIT_LINE: u32 = 1;
    const MOUSE_DOWN: u32 = 1;
    const MOUSE_UP: u32 = 2;
    const MOUSE_MOVED: u32 = 5;
    const HID_TAP: u32 = 0;
    const FIELD_LINE_DELTA: u32 = 11;
    const FIELD_CONTINUOUS: u32 = 88;
    const FIELD_FIXED_DELTA: u32 = 93;
    const FIELD_POINT_DELTA: u32 = 96;
    const FIELD_PHASE: u32 = 99;
    const PHASE_BEGAN: i64 = 1;
    const PHASE_CHANGED: i64 = 2;
    const PHASE_ENDED: i64 = 4;
    const HID_STATE: i32 = 1;
    const ESCAPE_KEY: u16 = 53;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreate(source: *mut c_void) -> *mut c_void;
        fn CGEventCreateScrollWheelEvent2(
            source: *mut c_void,
            units: u32,
            wheel_count: u32,
            wheel1: i32,
            wheel2: i32,
            wheel3: i32,
        ) -> *mut c_void;
        fn CGEventCreateMouseEvent(source: *mut c_void, mouse_type: u32, position: CgPoint, button: u32) -> *mut c_void;
        fn CGEventSetIntegerValueField(event: *mut c_void, field: u32, value: i64);
        fn CGEventSetDoubleValueField(event: *mut c_void, field: u32, value: f64);
        fn CGEventSetLocation(event: *mut c_void, location: CgPoint);
        fn CGEventPost(tap: u32, event: *mut c_void);
        fn CGEventGetLocation(event: *mut c_void) -> CgPoint;
        fn CGEventSourceCreate(state_id: i32) -> *mut c_void;
        fn CGEventSourceSetPixelsPerLine(source: *mut c_void, pixels: f64);
        fn CGEventSourceSetLocalEventsSuppressionInterval(source: *mut c_void, seconds: f64);
        fn CGWarpMouseCursorPosition(point: CgPoint) -> i32;
        fn CGAssociateMouseAndMouseCursorPosition(connected: u8) -> i32;
        fn CGDisplayHideCursor(display: u32) -> i32;
        fn CGDisplayShowCursor(display: u32) -> i32;
        fn CGEventSourceKeyState(state_id: i32, key: u16) -> bool;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> u8;
    }

    unsafe extern "C" {
        fn shotlet_focus_at(x: i32, y: i32);
    }

    pub struct Session {
        x: i32,
        y: i32,
        origin: Option<CgPoint>,
        hidden: bool,
        source: *mut c_void,
    }

    impl Session {
        pub fn engage(x: i32, y: i32) -> Self {
            let origin = cursor_point();
            let source = unsafe { CGEventSourceCreate(-1) };
            if !source.is_null() {
                unsafe {
                    CGEventSourceSetPixelsPerLine(source, 10.0);
                    CGEventSourceSetLocalEventsSuppressionInterval(source, 0.0);
                }
            }
            let hidden = unsafe { CGDisplayHideCursor(0) == 0 };
            let session = Self { x, y, origin, hidden, source };
            session.place(CgPoint { x: x as f64, y: y as f64 });
            session
        }

        fn place(&self, point: CgPoint) {
            unsafe {
                let _ = CGWarpMouseCursorPosition(point);
                let _ = CGAssociateMouseAndMouseCursorPosition(1);
            }
            post_mouse_moved(self.source, point);
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            if let Some(origin) = self.origin {
                self.place(origin);
            }
            if self.hidden {
                unsafe { let _ = CGDisplayShowCursor(0); }
            }
            if !self.source.is_null() {
                unsafe { CFRelease(self.source.cast()); }
            }
        }
    }

    pub fn ensure_scroll_access() -> Result<(), String> {
        if unsafe { AXIsProcessTrusted() } == 0 {
            Err("没有辅助功能权限。请到系统设置 → 隐私与安全性 → 辅助功能，允许 Shotlet。".into())
        } else {
            Ok(())
        }
    }

    pub fn focus_at(x: i32, y: i32) {
        unsafe { shotlet_focus_at(x, y) }
    }

    pub fn escape_down() -> bool {
        unsafe { CGEventSourceKeyState(HID_STATE, ESCAPE_KEY) }
    }

    pub fn click(session: &Session) {
        let point = CgPoint { x: session.x as f64, y: session.y as f64 };
        post_mouse(session.source, MOUSE_DOWN, point);
        thread::sleep(Duration::from_millis(30));
        post_mouse(session.source, MOUSE_UP, point);
    }

    /// `delta` is signed. Negative moves the page down (new rows appear at the bottom).
    pub fn scroll(session: &Session, delta: i32, pixel: bool) {
        if delta == 0 {
            return;
        }
        if !pixel {
            post_wheel(session, UNIT_LINE, delta, delta, 0, false);
            return;
        }
        let mut left = delta;
        let mut first = true;
        while left != 0 {
            let chunk = left.clamp(-40, 40);
            let phase = if first { PHASE_BEGAN } else { PHASE_CHANGED };
            let lines = chunk.signum() * (chunk.abs() / 10).max(1);
            post_wheel(session, UNIT_PIXEL, chunk, lines, phase, true);
            first = false;
            left -= chunk;
            if left != 0 {
                thread::sleep(Duration::from_millis(6));
            }
        }
        post_wheel(session, UNIT_PIXEL, 0, 0, PHASE_ENDED, true);
    }

    fn post_wheel(session: &Session, units: u32, pixel_delta: i32, line_delta: i32, phase: i64, continuous: bool) {
        let point = CgPoint { x: session.x as f64, y: session.y as f64 };
        unsafe {
            let event = CGEventCreateScrollWheelEvent2(session.source, units, 1, if units == UNIT_PIXEL { pixel_delta } else { line_delta }, 0, 0);
            if event.is_null() {
                return;
            }
            CGEventSetLocation(event, point);
            CGEventSetIntegerValueField(event, FIELD_CONTINUOUS, if continuous { 1 } else { 0 });
            CGEventSetIntegerValueField(event, FIELD_LINE_DELTA, i64::from(line_delta));
            CGEventSetIntegerValueField(event, FIELD_POINT_DELTA, i64::from(pixel_delta));
            CGEventSetDoubleValueField(event, FIELD_FIXED_DELTA, f64::from(pixel_delta));
            if continuous {
                CGEventSetIntegerValueField(event, FIELD_PHASE, phase);
            }
            CGEventPost(HID_TAP, event);
            CFRelease(event.cast());
        }
    }

    fn post_mouse(source: *mut c_void, kind: u32, point: CgPoint) {
        unsafe {
            let event = CGEventCreateMouseEvent(source, kind, point, 0);
            if event.is_null() {
                return;
            }
            CGEventSetLocation(event, point);
            CGEventPost(HID_TAP, event);
            CFRelease(event.cast());
        }
    }

    fn post_mouse_moved(source: *mut c_void, point: CgPoint) {
        post_mouse(source, MOUSE_MOVED, point);
    }

    fn cursor_point() -> Option<CgPoint> {
        unsafe {
            let event = CGEventCreate(std::ptr::null_mut());
            if event.is_null() {
                return None;
            }
            let point = CGEventGetLocation(event);
            CFRelease(event.cast());
            if point.x.is_finite() && point.y.is_finite() { Some(point) } else { None }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod input {
    pub struct Session {
        x: i32,
        y: i32,
    }

    impl Session {
        pub fn engage(x: i32, y: i32) -> Self {
            Self { x, y }
        }
    }

    pub fn ensure_scroll_access() -> Result<(), String> {
        Ok(())
    }

    pub fn focus_at(_x: i32, _y: i32) {}

    pub fn escape_down() -> bool {
        false
    }

    pub fn click(_session: &Session) {}

    pub fn scroll(session: &Session, delta: i32, _pixel: bool) {
        #[cfg(windows)]
        {
            use windows::Win32::UI::Input::KeyboardAndMouse::{mouse_event, MOUSEEVENTF_WHEEL};
            use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;
            unsafe {
                let _ = SetCursorPos(session.x, session.y);
                if delta < 0 {
                    mouse_event(MOUSEEVENTF_WHEEL, 0, 0, -360, 0);
                } else if delta > 0 {
                    mouse_event(MOUSEEVENTF_WHEEL, 0, 0, 360, 0);
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (session, delta);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::scroll_step;

    #[test]
    fn scroll_step_is_under_half_the_viewport() {
        for height in [40u32, 90, 120, 300, 1080, 1600] {
            let step = scroll_step(height);
            assert!(step < height / 2, "step {step} is not under half of {height}");
            assert!(step < height / 3, "step {step} is not under a third of {height}");
        }
        assert_eq!(scroll_step(1080), 280);
        assert!(scroll_step(400) >= 36);
    }
}
