use image::RgbaImage;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use shotlet_capture::{capture_dir, capture_local};
use shotlet_image::{append_rows, frames_stable, prepend_rows};
use shotlet_platform::{new_id, write_png};

pub struct ScrollResult {
    pub path: String,
    pub width: u32,
    pub height: u32,
}

pub const CANCELLED: &str = "滚动截图已取消";

const MAX_HEIGHT: u32 = 14_000;
const MAX_WAIT: Duration = Duration::from_secs(600);

struct Gate {
    finish: AtomicBool,
    cancel: AtomicBool,
}

static GATE: Mutex<Option<Arc<Gate>>> = Mutex::new(None);
static PROGRESS: AtomicU32 = AtomicU32::new(0);
static REVERSED: AtomicBool = AtomicBool::new(false);
static ACTION: Mutex<String> = Mutex::new(String::new());

pub fn progress() -> u32 {
    PROGRESS.load(Ordering::Relaxed)
}

pub fn reversed() -> bool {
    REVERSED.load(Ordering::Relaxed)
}

pub fn take_action() -> String {
    ACTION
        .lock()
        .map(|mut slot| std::mem::replace(&mut *slot, "edit".into()))
        .unwrap_or_else(|_| "edit".into())
}

pub fn begin() -> Result<(), String> {
    let mut slot = GATE.lock().map_err(|error| error.to_string())?;
    if slot.is_some() {
        return Err("滚动截图还在进行".into());
    }
    PROGRESS.store(0, Ordering::Relaxed);
    REVERSED.store(false, Ordering::Relaxed);
    if let Ok(mut action) = ACTION.lock() {
        *action = "edit".into();
    }
    *slot = Some(Arc::new(Gate {
        finish: AtomicBool::new(false),
        cancel: AtomicBool::new(false),
    }));
    Ok(())
}

pub fn end() {
    if let Ok(mut slot) = GATE.lock() {
        *slot = None;
    }
}

pub fn request_finish(action: &str) {
    let action = match action {
        "save" | "copy" => action.to_string(),
        _ => "edit".to_string(),
    };
    if let Ok(mut slot) = ACTION.lock() {
        *slot = action;
    }
    if let Ok(slot) = GATE.lock() {
        if let Some(gate) = slot.as_ref() {
            gate.finish.store(true, Ordering::SeqCst);
        }
    }
}

pub fn request_cancel() {
    if let Ok(slot) = GATE.lock() {
        if let Some(gate) = slot.as_ref() {
            gate.cancel.store(true, Ordering::SeqCst);
        }
    }
}

/// Watch the selection while the page inside it scrolls. This does not send wheel or pointer input.
pub fn capture_manual(screen_x: i32, screen_y: i32, x: u32, y: u32, width: u32, height: u32) -> Result<ScrollResult, String> {
    if width < 20 || height < 20 {
        return Err("滚动选区太小".into());
    }
    let gate = GATE.lock().map_err(|error| error.to_string())?.clone().ok_or("滚动截图没有开始")?;
    let baseline = capture_local(screen_x, screen_y, x, y, width, height)?;
    let mut roll = Roll::new(baseline);
    let mut esc = Esc::snapshot();
    let deadline = Instant::now() + MAX_WAIT;
    let mut still = 0u32;
    let mut moved = 0u32;
    let mut told_still = false;
    loop {
        match tick(&gate, &mut esc) {
            Tick::Cancel => return Err(CANCELLED.into()),
            Tick::Finish => break,
            Tick::Continue => {}
        }
        if roll.full() || Instant::now() >= deadline {
            break;
        }
        let frame = capture_local(screen_x, screen_y, x, y, width, height)?;
        if roll.same(&frame) {
            still += 1;
        } else {
            moved += 1;
            roll.absorb(frame);
        }
        REVERSED.store(roll.reversed(), Ordering::Relaxed);
        if !told_still && still >= 20 && moved == 0 {
            told_still = true;
            eprintln!("scroll: captured region is not changing");
        }
    }
    eprintln!("scroll: finish height {}", roll.height());
    save_long(roll.compose())
}

enum Tick {
    Continue,
    Finish,
    Cancel,
}

fn tick(gate: &Gate, esc: &mut Esc) -> Tick {
    for _ in 0..2 {
        if gate.cancel.load(Ordering::SeqCst) || esc.pressed() {
            return Tick::Cancel;
        }
        if gate.finish.load(Ordering::SeqCst) {
            return Tick::Finish;
        }
        thread::sleep(Duration::from_millis(10));
    }
    Tick::Continue
}

struct Esc {
    down: bool,
}

impl Esc {
    fn snapshot() -> Self {
        Self { down: escape_down() }
    }

    fn pressed(&mut self) -> bool {
        let down = escape_down();
        let hit = down && !self.down;
        self.down = down;
        hit
    }
}

fn escape_down() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
        return unsafe { GetAsyncKeyState(i32::from(VK_ESCAPE.0)) } < 0;
    }
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
        }
        return unsafe { CGEventSourceKeyState(1, 53) };
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        false
    }
}

/// Selection rectangle mapped from frozen-image pixels into the monitor's cursor-space pixels.
pub fn visual_rect(
    screen_x: i32,
    screen_y: i32,
    local_x: u32,
    local_y: u32,
    local_w: u32,
    local_h: u32,
    image_w: u32,
    image_h: u32,
    point_w: u32,
    point_h: u32,
) -> (i32, i32, u32, u32) {
    let point_w = if point_w == 0 { image_w } else { point_w };
    let point_h = if point_h == 0 { image_h } else { point_h };
    (
        screen_x.saturating_add(map_len(local_x, image_w, point_w) as i32),
        screen_y.saturating_add(map_len(local_y, image_h, point_h) as i32),
        map_len(local_w, image_w, point_w).max(1),
        map_len(local_h, image_h, point_h).max(1),
    )
}

fn map_len(value: u32, from: u32, to: u32) -> u32 {
    if from == 0 || to == 0 || from == to {
        return value;
    }
    ((u64::from(value) * u64::from(to) + u64::from(from) / 2) / u64::from(from)) as u32
}

struct Roll {
    head: RgbaImage,
    previous: RgbaImage,
    above: Vec<RgbaImage>,
    below: Vec<RgbaImage>,
    total: u32,
    /// 1 grows downward, -1 grows upward, 0 is not chosen yet.
    direction: i8,
    misses: u32,
    reversed: bool,
}

impl Roll {
    fn new(head: RgbaImage) -> Self {
        let total = head.height();
        PROGRESS.store(total, Ordering::Relaxed);
        Self {
            previous: head.clone(),
            head,
            above: Vec::new(),
            below: Vec::new(),
            total,
            direction: 0,
            misses: 0,
            reversed: false,
        }
    }

    fn reversed(&self) -> bool {
        self.reversed
    }

    fn full(&self) -> bool {
        self.total >= MAX_HEIGHT
    }

    fn height(&self) -> u32 {
        self.total
    }

    fn same(&self, frame: &RgbaImage) -> bool {
        frames_stable(&self.previous, frame)
    }

    fn absorb(&mut self, frame: RgbaImage) {
        if self.full() || frames_stable(&self.previous, &frame) {
            return;
        }
        let room = MAX_HEIGHT.saturating_sub(self.total);
        if room < 2 {
            return;
        }
        let (down, rows) = match manual_step(&self.previous, &frame, self.direction) {
            Align::Reverse => {
                self.reversed = true;
                return;
            }
            Align::Miss => {
                self.misses += 1;
                if self.misses == 1 || self.misses % 15 == 0 {
                    eprintln!(
                        "scroll: no alignment ({}) {}",
                        self.misses,
                        describe_shifts(&self.previous, &frame, self.direction)
                    );
                }
                return;
            }
            Align::Hit(down, rows) => {
                self.reversed = false;
                (down, rows)
            }
        };
        let rows = rows.min(room);
        if rows < 2 {
            return;
        }
        let before = self.total;
        if down {
            self.direction = 1;
            self.below.push(bottom_rows(&frame, rows));
        } else {
            self.direction = -1;
            self.above.push(top_rows(&frame, rows));
        }
        self.total = self.total.saturating_add(rows);
        self.previous = frame;
        PROGRESS.store(self.total, Ordering::Relaxed);
        if before / 200 != self.total / 200 {
            eprintln!("scroll: height {}", self.total);
        }
    }

    fn compose(&self) -> RgbaImage {
        let width = self.head.width();
        let mut parts: Vec<&RgbaImage> = Vec::new();
        let mut height = 0u32;
        for strip in self.above.iter().rev().chain(std::iter::once(&self.head)).chain(self.below.iter()) {
            if strip.width() != width || strip.height() == 0 {
                continue;
            }
            height = height.saturating_add(strip.height());
            parts.push(strip);
        }
        if parts.is_empty() {
            return self.head.clone();
        }
        let mut raw = Vec::with_capacity((height as usize).saturating_mul(width as usize).saturating_mul(4));
        for strip in parts {
            raw.extend_from_slice(strip.as_raw());
        }
        RgbaImage::from_raw(width, height, raw).unwrap_or_else(|| self.head.clone())
    }
}

#[derive(Clone, Copy)]
struct ShiftHit {
    shift: u32,
    cost: i64,
}

enum Align {
    Hit(bool, u32),
    Reverse,
    Miss,
}

/// Down is true when new rows belong at the bottom.
fn manual_step(prev: &RgbaImage, next: &RgbaImage, direction: i8) -> Align {
    let down = best_shift(prev, next, true);
    let up = best_shift(prev, next, false);
    let pick = match (down, up) {
        (Some(down), Some(up)) if down.cost.saturating_add(8) < up.cost => Some((true, down.shift)),
        (Some(down), Some(up)) if up.cost.saturating_add(8) < down.cost => Some((false, up.shift)),
        (Some(down), None) => Some((true, down.shift)),
        (None, Some(up)) => Some((false, up.shift)),
        _ => None,
    };
    let Some(pick) = pick else {
        return Align::Miss;
    };
    // A step back lines up the other way. Ignore it instead of pasting a duplicate strip.
    match direction {
        1 if !pick.0 => Align::Reverse,
        -1 if pick.0 => Align::Reverse,
        _ => Align::Hit(pick.0, pick.1),
    }
}

fn best_shift(prev: &RgbaImage, next: &RgbaImage, downward: bool) -> Option<ShiftHit> {
    let height = prev.height();
    if prev.width() != next.width() || prev.height() != next.height() || height < 8 {
        return None;
    }
    let min_overlap = (height / 8).max(8);
    let max_shift = height.saturating_sub(min_overlap);
    if max_shift < 2 {
        return None;
    }
    let rows = same_row_diffs(prev, next);
    let zero = alignment_cost(prev, next, 0, true, &rows).unwrap_or(i64::MAX / 8);
    let mut hits = Vec::new();
    for shift in 2..=max_shift {
        let Some(cost) = alignment_cost(prev, next, shift, downward, &rows) else { continue };
        hits.push(ShiftHit { shift, cost });
    }
    if hits.is_empty() {
        return None;
    }
    hits.sort_by_key(|hit| hit.cost);
    let best = hits[0];
    let rival = hits.iter().find(|hit| hit.shift.abs_diff(best.shift) > 4);
    let clear = match rival {
        None => true,
        Some(other) => best.cost.saturating_add(8) <= other.cost || best.cost.saturating_mul(2) < other.cost,
    };
    if !clear || best.cost > 28 || best.cost.saturating_add(8) >= zero {
        return None;
    }
    Some(best)
}

fn same_row_diffs(prev: &RgbaImage, next: &RgbaImage) -> Vec<i64> {
    (0..prev.height()).map(|y| sample_diff(prev, y, next, y)).collect()
}

fn alignment_cost(prev: &RgbaImage, next: &RgbaImage, shift: u32, downward: bool, rows: &[i64]) -> Option<i64> {
    let height = prev.height();
    let (start, end) = if shift == 0 {
        (0, height)
    } else if downward {
        (0, height - shift)
    } else if shift < height {
        (shift, height)
    } else {
        return None;
    };
    if end <= start {
        return None;
    }
    let step = ((end - start) / 36).max(1);
    let mut sum = 0i64;
    let mut count = 0i64;
    let mut y = start;
    while y < end {
        let prev_y = if shift == 0 || downward { y.saturating_add(shift) } else { y.saturating_sub(shift) };
        if prev_y >= height {
            break;
        }
        let here = rows.get(y as usize).copied().unwrap_or(i64::MAX / 4);
        let there = rows.get(prev_y as usize).copied().unwrap_or(i64::MAX / 4);
        let stationary = here <= 8 && there <= 8;
        if shift != 0 && (here <= 8 || there <= 8) {
            y += step;
            continue;
        }
        if shift == 0 && stationary {
            y += step;
            continue;
        }
        sum += sample_diff(prev, prev_y, next, y);
        count += 1;
        y += step;
    }
    if count < 4 {
        None
    } else {
        Some(sum / count)
    }
}

fn sample_diff(prev: &RgbaImage, prev_y: u32, next: &RgbaImage, next_y: u32) -> i64 {
    let width = prev.width().min(next.width());
    let skip = if width > 96 { 32 } else { 0 };
    let usable = width.saturating_sub(skip);
    if usable == 0 || prev_y >= prev.height() || next_y >= next.height() {
        return i64::MAX / 4;
    }
    let step = (usable / 32).max(1);
    let mut total = 0i64;
    let mut count = 0i64;
    let mut x = 0u32;
    while x < usable {
        let a = prev.get_pixel(x, prev_y);
        let b = next.get_pixel(x, next_y);
        total += (a[0] as i64 - b[0] as i64).abs();
        total += (a[1] as i64 - b[1] as i64).abs();
        total += (a[2] as i64 - b[2] as i64).abs();
        count += 3;
        x += step;
    }
    if count == 0 { 0 } else { total / count }
}

fn describe_shifts(prev: &RgbaImage, next: &RgbaImage, direction: i8) -> String {
    let rows = same_row_diffs(prev, next);
    let zero = alignment_cost(prev, next, 0, true, &rows).unwrap_or(-1);
    let down = if direction != -1 { best_shift(prev, next, true) } else { None };
    let up = if direction != 1 { best_shift(prev, next, false) } else { None };
    format!(
        "zero={zero} down={} up={}",
        down.map(|hit| format!("{}@{}", hit.shift, hit.cost)).unwrap_or_else(|| "-".into()),
        up.map(|hit| format!("{}@{}", hit.shift, hit.cost)).unwrap_or_else(|| "-".into()),
    )
}

fn top_rows(image: &RgbaImage, rows: u32) -> RgbaImage {
    let rows = rows.min(image.height());
    image::imageops::crop_imm(image, 0, 0, image.width(), rows).to_image()
}

fn bottom_rows(image: &RgbaImage, rows: u32) -> RgbaImage {
    let rows = rows.min(image.height());
    let top = image.height() - rows;
    image::imageops::crop_imm(image, 0, top, image.width(), rows).to_image()
}

fn save_long(image: RgbaImage) -> Result<ScrollResult, String> {
    let path = capture_dir().join(format!("scroll-{}.png", new_id()));
    write_png(&path, &image)?;
    Ok(ScrollResult {
        path: path.to_string_lossy().to_string(),
        width: image.width(),
        height: image.height(),
    })
}

#[cfg(test)]
mod tests {
    use super::{visual_rect, Roll};
    use image::{Rgba, RgbaImage};

    fn page(width: u32, height: u32) -> RgbaImage {
        let mut image = RgbaImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                image.put_pixel(x, y, Rgba([(y * 17) as u8, (y * 13) as u8, (y * 9) as u8, 255]));
            }
        }
        image
    }

    fn view(full: &RgbaImage, y: u32, height: u32) -> RgbaImage {
        image::imageops::crop_imm(full, 0, y, full.width(), height).to_image()
    }

    #[test]
    fn visual_rect_scales_image_pixels_into_cursor_space() {
        assert_eq!(visual_rect(1920, 0, 100, 50, 200, 100, 2000, 1000, 1000, 500), (1970, 25, 100, 50));
        assert_eq!(visual_rect(-1920, 10, 0, 0, 40, 40, 800, 600, 0, 0), (-1920, 10, 40, 40));
    }

    #[test]
    fn stitches_one_downward_drag_and_ignores_a_reverse() {
        let full = page(30, 70);
        let mut roll = Roll::new(view(&full, 0, 40));
        roll.absorb(view(&full, 15, 40));
        let stitched = roll.compose();
        assert_eq!(stitched.height(), 55);
        assert_eq!(stitched.get_pixel(4, 0), full.get_pixel(4, 0));
        assert_eq!(stitched.get_pixel(4, 54), full.get_pixel(4, 54));
        roll.absorb(view(&full, 30, 40));
        let longer = roll.compose();
        assert_eq!(longer.height(), 70);
        assert_eq!(longer.get_pixel(4, 69), full.get_pixel(4, 69));
        roll.absorb(view(&full, 0, 40));
        assert!(roll.reversed());
        assert_eq!(roll.compose().height(), 70);
    }

    #[test]
    fn stitches_past_a_fixed_header() {
        let paint = |scroll: u32| {
            let mut image = RgbaImage::new(36, 48);
            for y in 0..48 {
                for x in 0..36 {
                    let pixel = if y < 8 {
                        Rgba([12, 24, 36, 255])
                    } else {
                        let row = y - 8 + scroll;
                        Rgba([(row * 17) as u8, (row * 13) as u8, (row * 9) as u8, 255])
                    };
                    image.put_pixel(x, y, pixel);
                }
            }
            image
        };
        let next = paint(15);
        let mut roll = Roll::new(paint(0));
        roll.absorb(next.clone());
        let stitched = roll.compose();
        assert_eq!(stitched.height(), 63);
        assert_eq!(stitched.get_pixel(4, 62), next.get_pixel(4, 47));
    }

    #[test]
    fn leaves_an_unrelated_frame_alone() {
        let mut roll = Roll::new(page(30, 40));
        let mut other = RgbaImage::new(30, 40);
        for y in 0..40 {
            for x in 0..30 {
                other.put_pixel(x, y, Rgba([255 - (y as u8), x as u8, 80, 255]));
            }
        }
        roll.absorb(other);
        assert_eq!(roll.compose().height(), 40);
    }

    #[test]
    fn stitches_one_upward_drag_and_ignores_a_reverse() {
        let full = page(30, 70);
        let mut roll = Roll::new(view(&full, 15, 40));
        roll.absorb(view(&full, 0, 40));
        let stitched = roll.compose();
        assert_eq!(stitched.height(), 55);
        assert_eq!(stitched.get_pixel(4, 0), full.get_pixel(4, 0));
        assert_eq!(stitched.get_pixel(4, 54), full.get_pixel(4, 54));
        roll.absorb(view(&full, 30, 40));
        assert_eq!(roll.compose().height(), 55);
    }
}
