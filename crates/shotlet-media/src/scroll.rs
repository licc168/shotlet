use image::RgbaImage;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use shotlet_capture::capture_dir;
use shotlet_image::frames_stable;
use shotlet_platform::{new_id, write_png};

#[path = "scroll_watch.rs"]
mod drive;
#[cfg(windows)]
#[path = "scroll_wheel.rs"]
mod wheel;
#[cfg(not(windows))]
#[path = "scroll_wheel_noop.rs"]
mod wheel;

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
static AUTO: AtomicBool = AtomicBool::new(false);
static PRIOR: AtomicI32 = AtomicI32::new(0);
static ACTION: Mutex<String> = Mutex::new(String::new());

pub fn progress() -> u32 {
    PROGRESS.load(Ordering::Relaxed)
}

pub fn reversed() -> bool {
    REVERSED.load(Ordering::Relaxed)
}

pub fn can_auto() -> bool {
    cfg!(windows)
}

pub fn auto_enabled() -> bool {
    AUTO.load(Ordering::Relaxed)
}

pub fn set_auto(on: bool) {
    if can_auto() {
        AUTO.store(on, Ordering::Relaxed);
    }
}

pub(super) fn set_scroll_prior(rows: i32) {
    PRIOR.store(rows, Ordering::Relaxed);
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
    AUTO.store(false, Ordering::Relaxed);
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

/// Watch the selection while the user scrolls. This does not send wheel or pointer input.
pub fn capture_manual(screen_x: i32, screen_y: i32, x: u32, y: u32, width: u32, height: u32) -> Result<ScrollResult, String> {
    if width < 20 || height < 20 {
        return Err("滚动选区太小".into());
    }
    drive::run(screen_x, screen_y, x, y, width, height)
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
    result: RgbaImage,
    previous: RgbaImage,
    total: u32,
    /// 1 grows downward, -1 grows upward, 0 is not chosen yet.
    direction: i8,
    misses: u32,
    reversed: bool,
    last_down: u32,
}

impl Roll {
    fn new(head: RgbaImage) -> Self {
        let total = head.height();
        PROGRESS.store(total, Ordering::Relaxed);
        Self {
            previous: head.clone(),
            result: head,
            total,
            direction: 0,
            misses: 0,
            reversed: false,
            last_down: 0,
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
        let join = match locate(&self.previous, &frame, self.direction) {
            Locate::Reverse => {
                self.reversed = true;
                return;
            }
            Locate::Miss => {
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
            Locate::Hit(join) => join,
        };
        if join.net() > room {
            return;
        }
        let before = self.total;
        if !self.apply(&frame, &join) {
            self.misses += 1;
            return;
        }
        if before / 200 != self.total / 200 {
            eprintln!("scroll: height {}", self.total);
        }
    }

    /// Auto-scroll only. A frame the strict matcher rejects can still be a real
    /// step with a dirty overlap (images loading, subpixel text).
    fn absorb_auto(&mut self, frame: RgbaImage) {
        let before = self.total;
        self.absorb(frame.clone());
        if self.total > before || self.full() || self.direction < 0 || frames_stable(&self.previous, &frame) {
            return;
        }
        let Some(rows) = lenient_shift(&self.previous, &frame, self.last_down) else {
            return;
        };
        let before = self.total;
        self.append_down(frame, rows);
        if before / 200 != self.total / 200 {
            eprintln!("scroll: height {}", self.total);
        }
    }

    /// ShareX keeps the last good scroll distance when this frame will not match,
    /// so one dirty step does not end the capture.
    fn carry_down(&mut self, frame: RgbaImage) -> bool {
        let rows = self.last_down;
        if rows < 2 || self.full() || self.direction < 0 || frames_stable(&self.previous, &frame) || !content_shifted(&self.previous, &frame) {
            return false;
        }
        let before = self.total;
        self.append_down(frame, rows);
        if self.total > before {
            eprintln!("scroll: kept last step of {rows}px");
        }
        self.total > before
    }

    fn append_down(&mut self, frame: RgbaImage, rows: u32) {
        let rows = rows.min(MAX_HEIGHT.saturating_sub(self.total)).min(frame.height().saturating_sub(1));
        if rows < 2 {
            return;
        }
        let join = Join {
            down: true,
            trim: 0,
            src_y: frame.height() - rows,
            copy: rows,
            run: rows,
        };
        let _ = self.apply(&frame, &join);
    }

    /// Cut the ignored edge off the picture we already have, then copy the new
    /// rows (including that edge) back from this frame. That is how ShareX keeps
    /// a taskbar at the bottom without pasting it into the middle.
    fn apply(&mut self, frame: &RgbaImage, join: &Join) -> bool {
        let Some(painted) = paint(&self.result, frame, join) else {
            return false;
        };
        let grew = painted.height().saturating_sub(self.result.height());
        if grew < 2 {
            return false;
        }
        self.direction = if join.down { 1 } else { -1 };
        if join.down {
            self.last_down = grew;
        }
        self.reversed = false;
        self.misses = 0;
        self.total = painted.height();
        self.result = painted;
        self.previous = frame.clone();
        PROGRESS.store(self.total, Ordering::Relaxed);
        true
    }

    fn compose(&self) -> RgbaImage {
        self.result.clone()
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

struct Join {
    down: bool,
    trim: u32,
    src_y: u32,
    copy: u32,
    run: u32,
}

impl Join {
    fn net(&self) -> u32 {
        self.copy.saturating_sub(self.trim)
    }
}

enum Locate {
    Hit(Join),
    Reverse,
    Miss,
}

/// ShareX `CombineImages`: ignore the side margins (the scrollbar lives there)
/// and a fixed bottom or top band, then keep the longest run of equal rows.
fn locate(prev: &RgbaImage, next: &RgbaImage, direction: i8) -> Locate {
    if prev.dimensions() != next.dimensions() || prev.height() < 16 {
        return Locate::Miss;
    }
    // Exact rows first. A real screen capture is a few levels off, so a cheap
    // scored shift is kept when a run of those rows still lines up.
    let prior = PRIOR.swap(0, Ordering::Relaxed);
    let down = signature_join(prev, next, true, prior, 0.60)
        .or_else(|| seek_down(prev, next))
        .or_else(|| confirm_scored(prev, next, true));
    // The open-source controller paints "reverse" only when the scroll input
    // itself changes direction. A fuzzy upward score on a comment list is not that.
    // Once this capture is going down, only an exact upward seam counts.
    let up = if prior > 0 {
        None
    } else if direction == 1 {
        seek_up(prev, next)
    } else {
        signature_join(prev, next, false, 0, 0.60)
            .or_else(|| seek_up(prev, next))
            .or_else(|| confirm_scored(prev, next, false))
    };
    let pick = if direction == 1 {
        down.or(up)
    } else if direction == -1 {
        up.or(down)
    } else {
        match (down, up) {
            (Some(down), Some(up)) if up.run > down.run.saturating_add(4) => Some(up),
            (Some(down), _) => Some(down),
            (None, Some(up)) => Some(up),
            _ => None,
        }
    };
    let Some(pick) = pick else {
        return Locate::Miss;
    };
    if pick.net() < 2 {
        return Locate::Miss;
    }
    match direction {
        1 if !pick.down => Locate::Reverse,
        -1 if pick.down => Locate::Reverse,
        _ => Locate::Hit(pick),
    }
}

fn manual_step(prev: &RgbaImage, next: &RgbaImage, direction: i8) -> Align {
    match locate(prev, next, direction) {
        Locate::Hit(join) => Align::Hit(join.down, join.net()),
        Locate::Reverse => Align::Reverse,
        Locate::Miss => Align::Miss,
    }
}

/// imjakey/shotlet scores a shift by how many rows keep the same 16-part
/// brightness signature. A scrollbar column is left out of the score only.
fn signature_join(prev: &RgbaImage, next: &RgbaImage, downward: bool, prior: i32, min_ratio: f32) -> Option<Join> {
    let height = prev.height().min(next.height());
    let width = prev.width().min(next.width());
    if height < 16 || width < 8 {
        return None;
    }
    let margin = (width / 20).clamp(8, 48).min(width / 3);
    let x1 = width.saturating_sub(margin).max(8);
    let min_overlap = (height / 8).max(8);
    let max_shift = height.saturating_sub(min_overlap);
    if max_shift < 2 {
        return None;
    }
    let step = if height > 240 { (height / 80).max(1) } else { 1 };
    let prev_rows = build_signatures(prev, x1);
    let next_rows = build_signatures(next, x1);
    let still = stationary_rows(&prev_rows, &next_rows);
    let window = if prior >= 8 {
        let guess = prior as u32;
        let low = (guess / 2).max(2);
        let high = guess.saturating_mul(2).min(max_shift);
        if high > low { Some((low, high)) } else { None }
    } else {
        None
    };
    let found = signature_best(&prev_rows, &next_rows, &still, downward, step, max_shift, window, (min_ratio - 0.05).max(0.5))
        .or_else(|| signature_best(&prev_rows, &next_rows, &still, downward, step, max_shift, None, min_ratio))?;
    eprintln!(
        "scroll: signature {} {}px conf {:.2}",
        if downward { "down" } else { "up" },
        found.0,
        found.1
    );
    join_for_shift(prev, next, found.0, downward, found.2)
}

fn signature_best(
    prev_rows: &[[u16; 16]],
    next_rows: &[[u16; 16]],
    still: &[bool],
    downward: bool,
    step: u32,
    max_shift: u32,
    window: Option<(u32, u32)>,
    min_ratio: f32,
) -> Option<(u32, f32, u32)> {
    let (from, to) = window.unwrap_or((2, max_shift));
    let mut best_shift = 0u32;
    let mut best_ratio = 0.0f32;
    let mut second = 0.0f32;
    let mut best_matched = 0u32;
    for shift in from..=to {
        let (matched, compared) = signature_score(prev_rows, next_rows, still, shift, downward, step);
        if compared < 8 {
            continue;
        }
        let ratio = matched as f32 / compared as f32;
        if ratio > best_ratio {
            second = best_ratio;
            best_ratio = ratio;
            best_shift = shift;
            best_matched = matched;
        } else if ratio > second {
            second = ratio;
        }
    }
    if best_shift < 2 || best_ratio < min_ratio {
        return None;
    }
    if best_ratio - second < 0.08 && second >= 0.50 {
        return None;
    }
    Some((best_shift, best_ratio, best_matched))
}

fn build_signatures(image: &RgbaImage, x1: u32) -> Vec<[u16; 16]> {
    (0..image.height()).map(|y| row_signature(image, y, x1)).collect()
}

fn stationary_rows(prev_rows: &[[u16; 16]], next_rows: &[[u16; 16]]) -> Vec<bool> {
    let count = prev_rows.len().min(next_rows.len());
    (0..count).map(|y| signature_distance(&prev_rows[y], &next_rows[y]) <= 6).collect()
}

fn signature_score(
    prev_rows: &[[u16; 16]],
    next_rows: &[[u16; 16]],
    still: &[bool],
    shift: u32,
    downward: bool,
    step: u32,
) -> (u32, u32) {
    let height = prev_rows.len().min(next_rows.len()) as u32;
    let (start, end) = if downward { (0, height.saturating_sub(shift)) } else { (shift, height) };
    let mut matched = 0u32;
    let mut compared = 0u32;
    let mut y = start;
    while y < end {
        if still.get(y as usize).copied().unwrap_or(false) {
            y += step;
            continue;
        }
        compared += 1;
        let prev_y = if downward { y + shift } else { y - shift };
        let distance = match (prev_rows.get(prev_y as usize), next_rows.get(y as usize)) {
            (Some(prev_row), Some(next_row)) => signature_distance(prev_row, next_row),
            _ => u32::MAX,
        };
        if distance <= 10 {
            matched += 1;
        }
        y += step;
    }
    (matched, compared)
}

fn signature_distance(prev_row: &[u16; 16], next_row: &[u16; 16]) -> u32 {
    let mut total = 0u32;
    for i in 0..16 {
        total += (i32::from(prev_row[i]) - i32::from(next_row[i])).unsigned_abs();
    }
    total / 16
}

fn row_signature(image: &RgbaImage, y: u32, x1: u32) -> [u16; 16] {
    let mut sig = [0u16; 16];
    if y >= image.height() || x1 < 16 {
        return sig;
    }
    let raw = image.as_raw();
    let stride = image.width() as usize * 4;
    let span = x1;
    let seg = (span / 16).max(1);
    for i in 0..16 {
        let start = i as u32 * seg;
        if start >= x1 {
            break;
        }
        let end = (start + seg).min(x1);
        let mut sum = 0u32;
        let mut count = 0u32;
        let mut x = start;
        let hop = if end - start > 12 { 2 } else { 1 };
        while x < end {
            let offset = y as usize * stride + x as usize * 4;
            let luma = (u32::from(raw[offset]) * 77 + u32::from(raw[offset + 1]) * 150 + u32::from(raw[offset + 2]) * 29) >> 8;
            sum += luma;
            count += 1;
            x += hop;
        }
        if count > 0 {
            sig[i] = (sum / count) as u16;
        }
    }
    sig
}

fn join_for_shift(prev: &RgbaImage, next: &RgbaImage, shift: u32, downward: bool, run: u32) -> Option<Join> {
    let height = prev.height().min(next.height());
    let ignore = edge_ignore(prev, next, !downward, true);
    if ignore.saturating_add(shift).saturating_add(2) >= height {
        return None;
    }
    if downward {
        let anchor = height - ignore - 1;
        let current_y = anchor.checked_sub(shift)?;
        let copy = height - current_y - 1;
        if copy.saturating_sub(ignore) < 2 {
            return None;
        }
        Some(Join { down: true, trim: ignore, src_y: current_y + 1, copy, run })
    } else {
        let current_y = ignore.saturating_add(shift);
        if current_y >= height || current_y.saturating_sub(ignore) < 2 {
            return None;
        }
        Some(Join { down: false, trim: ignore, src_y: 0, copy: current_y, run })
    }
}

fn seek_down(prev: &RgbaImage, next: &RgbaImage) -> Option<Join> {
    prefer(seek_edge(prev, next, true, true), seek_edge(prev, next, true, false))
}

fn seek_up(prev: &RgbaImage, next: &RgbaImage) -> Option<Join> {
    prefer(seek_edge(prev, next, false, true), seek_edge(prev, next, false, false))
}

/// The scored matcher already picked this distance. Keep it when the rows
/// around that seam stay close for a real stretch, not just a few samples.
fn confirm_scored(prev: &RgbaImage, next: &RgbaImage, downward: bool) -> Option<Join> {
    let hit = best_shift(prev, next, downward)?;
    if hit.cost > 18 {
        return None;
    }
    let height = prev.height().min(next.height());
    let ignore = edge_ignore(prev, next, !downward, true);
    if ignore.saturating_add(8) >= height || hit.shift < 2 || hit.shift >= height {
        return None;
    }
    let (x0, x1) = center_span(prev.width());
    let (current_y, copy, trim) = if downward {
        let anchor = height - ignore - 1;
        let current_y = anchor.checked_sub(hit.shift)?;
        let copy = height - current_y - 1;
        (current_y, copy, ignore)
    } else {
        let current_y = ignore.saturating_add(hit.shift);
        if current_y >= height {
            return None;
        }
        (current_y, current_y, ignore)
    };
    if copy.saturating_sub(trim) < 2 {
        return None;
    }
    let anchor = if downward { height - ignore - 1 } else { ignore };
    let run = if downward {
        run_close(prev, next, anchor, current_y, x0, x1, true)
    } else {
        run_close(prev, next, anchor, current_y, x0, x1, false)
    };
    if run < 36 {
        return None;
    }
    let src_y = if downward { current_y + 1 } else { 0 };
    eprintln!("scroll: stitched {} {shift}px run {run}", if downward { "down" } else { "up" }, shift = hit.shift);
    Some(Join { down: downward, trim, src_y, copy, run })
}

fn run_close(prev: &RgbaImage, next: &RgbaImage, anchor: u32, current_y: u32, x0: u32, x1: u32, upward: bool) -> u32 {
    let height = prev.height().min(next.height());
    let mut count = 0u32;
    let mut gaps = 0u32;
    let mut k = 0u32;
    while count < height {
        let pair = if upward {
            match (anchor.checked_sub(k), current_y.checked_sub(k)) {
                (Some(py), Some(ny)) => Some((py, ny)),
                _ => None,
            }
        } else {
            let py = anchor + k;
            let ny = current_y + k;
            if py >= height || ny >= height { None } else { Some((py, ny)) }
        };
        let Some((py, ny)) = pair else { break };
        if row_close(prev, py, next, ny, x0, x1) {
            count += 1;
            gaps = 0;
        } else {
            gaps += 1;
            if gaps > 2 {
                break;
            }
        }
        k += 1;
    }
    count
}

fn row_close(prev: &RgbaImage, prev_y: u32, next: &RgbaImage, next_y: u32, x0: u32, x1: u32) -> bool {
    row_mean(prev, prev_y, next, next_y, x0, x1) <= 12
}

fn prefer(primary: Option<Join>, tight: Option<Join>) -> Option<Join> {
    match (primary, tight) {
        (Some(primary), Some(tight)) if tight.run > primary.run.saturating_add(4) => Some(tight),
        (Some(primary), _) => Some(primary),
        (None, tight) => tight,
    }
}

fn seek_edge(prev: &RgbaImage, next: &RgbaImage, downward: bool, cushion: bool) -> Option<Join> {
    let height = prev.height();
    let ignore = edge_ignore(prev, next, !downward, cushion);
    if ignore.saturating_add(8) >= height {
        return None;
    }
    let anchor = if downward { height - ignore - 1 } else { ignore };
    let (x0, x1) = center_span(prev.width());
    let limit = (height / 2).max(8);
    let min_run = (height / 8).max(6);
    let mut best_count = 0u32;
    let mut best_y = 0u32;
    if downward {
        let mut current_y = height - 1;
        loop {
            if best_count >= limit {
                break;
            }
            let count = run_up(prev, next, anchor, current_y, x0, x1, limit);
            if count > best_count {
                best_count = count;
                best_y = current_y;
            }
            if current_y == 0 {
                break;
            }
            current_y -= 1;
        }
        if best_count < min_run {
            return None;
        }
        let copy = height - best_y - 1;
        if copy.saturating_sub(ignore) < 2 || best_y.saturating_add(1).saturating_add(copy) > height {
            return None;
        }
        Some(Join { down: true, trim: ignore, src_y: best_y + 1, copy, run: best_count })
    } else {
        let mut current_y = anchor;
        while current_y < height {
            if best_count >= limit {
                break;
            }
            let count = run_down(prev, next, anchor, current_y, x0, x1, limit);
            if count > best_count {
                best_count = count;
                best_y = current_y;
            }
            current_y += 1;
        }
        if best_count < min_run || best_y.saturating_sub(ignore) < 2 {
            return None;
        }
        Some(Join { down: false, trim: ignore, src_y: 0, copy: best_y, run: best_count })
    }
}

fn run_up(prev: &RgbaImage, next: &RgbaImage, anchor: u32, current_y: u32, x0: u32, x1: u32, limit: u32) -> u32 {
    let mut count = 0u32;
    let mut k = 0u32;
    while count < limit {
        let Some(py) = anchor.checked_sub(k) else { break };
        let Some(ny) = current_y.checked_sub(k) else { break };
        if row_equal(prev, py, next, ny, x0, x1) {
            count += 1;
            k += 1;
        } else {
            break;
        }
    }
    count
}

fn run_down(prev: &RgbaImage, next: &RgbaImage, anchor: u32, current_y: u32, x0: u32, x1: u32, limit: u32) -> u32 {
    let height = prev.height().min(next.height());
    let mut count = 0u32;
    let mut k = 0u32;
    while count < limit {
        let py = anchor + k;
        let ny = current_y + k;
        if py >= height || ny >= height {
            break;
        }
        if row_equal(prev, py, next, ny, x0, x1) {
            count += 1;
            k += 1;
        } else {
            break;
        }
    }
    count
}

/// ShareX starts the ignored edge at `max(50, height / 10)`, capped at a third,
/// then adds rows that did not move. The picture itself is not cropped.
fn edge_ignore(prev: &RgbaImage, next: &RgbaImage, from_top: bool, cushion: bool) -> u32 {
    let height = prev.height().min(next.height());
    let cap = (height / 3).max(1);
    let fixed = identical_edge(prev, next, from_top, cap);
    if !cushion {
        return fixed.min(cap);
    }
    let base = (height / 10).max(50).min(cap);
    base.saturating_add(fixed).min(cap)
}

fn identical_edge(prev: &RgbaImage, next: &RgbaImage, from_top: bool, cap: u32) -> u32 {
    let height = prev.height().min(next.height());
    let (x0, x1) = center_span(prev.width());
    let mut count = 0u32;
    while count < cap {
        let y = if from_top { count } else { height - 1 - count };
        if !row_equal(prev, y, next, y, x0, x1) {
            break;
        }
        count += 1;
    }
    count
}

fn center_span(width: u32) -> (u32, u32) {
    if width < 8 {
        return (0, width);
    }
    let cap = (width / 3).max(1);
    let ignore = (width / 20).max(50).min(cap);
    let end = width.saturating_sub(ignore);
    if end <= ignore.saturating_add(4) {
        (0, width)
    } else {
        (ignore, end)
    }
}

fn row_equal(prev: &RgbaImage, prev_y: u32, next: &RgbaImage, next_y: u32, x0: u32, x1: u32) -> bool {
    if prev_y >= prev.height() || next_y >= next.height() || x1 <= x0 {
        return false;
    }
    let width = prev.width().min(next.width());
    let x1 = x1.min(width);
    let x0 = x0.min(x1);
    let prev_raw = prev.as_raw();
    let next_raw = next.as_raw();
    let prev_stride = prev.width() as usize * 4;
    let next_stride = next.width() as usize * 4;
    let start_prev = prev_y as usize * prev_stride + x0 as usize * 4;
    let start_next = next_y as usize * next_stride + x0 as usize * 4;
    let len = (x1 - x0) as usize * 4;
    let pa = &prev_raw[start_prev..start_prev + len];
    let pb = &next_raw[start_next..start_next + len];
    if pa == pb {
        return true;
    }
    row_mean(prev, prev_y, next, next_y, x0, x1) <= 3
}

fn row_mean(prev: &RgbaImage, prev_y: u32, next: &RgbaImage, next_y: u32, x0: u32, x1: u32) -> i64 {
    if prev_y >= prev.height() || next_y >= next.height() || x1 <= x0 {
        return i64::MAX / 4;
    }
    let width = prev.width().min(next.width());
    let x1 = x1.min(width);
    let x0 = x0.min(x1);
    let prev_raw = prev.as_raw();
    let next_raw = next.as_raw();
    let start_prev = prev_y as usize * prev.width() as usize * 4 + x0 as usize * 4;
    let start_next = next_y as usize * next.width() as usize * 4 + x0 as usize * 4;
    let len = (x1 - x0) as usize * 4;
    let pa = &prev_raw[start_prev..start_prev + len];
    let pb = &next_raw[start_next..start_next + len];
    let mut total = 0i64;
    let mut count = 0i64;
    let mut index = 0usize;
    while index + 3 < len {
        total += (i64::from(pa[index]) - i64::from(pb[index])).abs();
        total += (i64::from(pa[index + 1]) - i64::from(pb[index + 1])).abs();
        total += (i64::from(pa[index + 2]) - i64::from(pb[index + 2])).abs();
        count += 3;
        index += 4;
    }
    if count == 0 { i64::MAX / 4 } else { total / count }
}

fn paint(base: &RgbaImage, frame: &RgbaImage, join: &Join) -> Option<RgbaImage> {
    if frame.width() != base.width() || join.net() < 2 || join.trim >= base.height() {
        return None;
    }
    if join.src_y.saturating_add(join.copy) > frame.height() {
        return None;
    }
    let keep = base.height() - join.trim;
    let new_h = keep.saturating_add(join.copy);
    if new_h <= base.height() || new_h > MAX_HEIGHT {
        return None;
    }
    let mut out = RgbaImage::new(base.width(), new_h);
    if join.down {
        blit(&mut out, 0, base, 0, keep);
        blit(&mut out, keep, frame, join.src_y, join.copy);
    } else {
        blit(&mut out, 0, frame, join.src_y, join.copy);
        blit(&mut out, join.copy, base, join.trim, keep);
    }
    Some(out)
}

fn blit(dst: &mut RgbaImage, dst_y: u32, src: &RgbaImage, src_y: u32, rows: u32) {
    if rows == 0 || dst.width() != src.width() {
        return;
    }
    let width = dst.width() as usize;
    let row_bytes = width * 4;
    let dst_buf = dst.as_mut();
    let src_buf = src.as_raw();
    for row in 0..rows as usize {
        let d = (dst_y as usize + row) * row_bytes;
        let s = (src_y as usize + row) * row_bytes;
        dst_buf[d..d + row_bytes].copy_from_slice(&src_buf[s..s + row_bytes]);
    }
}

struct RankedShift {
    best: ShiftHit,
    zero: i64,
    clear: bool,
}

fn best_shift(prev: &RgbaImage, next: &RgbaImage, downward: bool) -> Option<ShiftHit> {
    let ranked = rank_shift(prev, next, downward)?;
    if !ranked.clear || ranked.best.cost > 28 || ranked.best.cost.saturating_add(8) >= ranked.zero {
        None
    } else {
        Some(ranked.best)
    }
}

/// A scroll step whose overlap is a bit dirty. `hint` is the last accepted
/// downward distance, so a wild offset is not treated as the same step.
fn lenient_shift(prev: &RgbaImage, next: &RgbaImage, hint: u32) -> Option<u32> {
    let ranked = rank_shift(prev, next, true)?;
    let hit = ranked.best;
    if !ranked.clear || hit.cost.saturating_add(8) >= ranked.zero || hit.cost > 64 {
        return None;
    }
    if hint >= 2 {
        let low = (hint / 2).max(2);
        let high = hint.saturating_mul(2).max(low);
        if hit.shift < low || hit.shift > high {
            return None;
        }
    } else if hit.cost > 40 || hit.shift > prev.height() / 3 {
        return None;
    }
    Some(hit.shift)
}

fn content_shifted(prev: &RgbaImage, next: &RgbaImage) -> bool {
    if prev.dimensions() != next.dimensions() {
        return false;
    }
    let columns = moving_columns(prev, next);
    let rows = same_row_diffs(prev, next, &columns);
    alignment_cost(prev, next, 0, true, &rows, &columns, fixed_top_rows(prev, next), fixed_bottom_rows(prev, next)).unwrap_or(0) > 12
}

fn rank_shift(prev: &RgbaImage, next: &RgbaImage, downward: bool) -> Option<RankedShift> {
    let height = prev.height();
    if prev.width() != next.width() || prev.height() != next.height() || height < 8 {
        return None;
    }
    let min_overlap = (height / 8).max(8);
    let max_shift = height.saturating_sub(min_overlap);
    if max_shift < 2 {
        return None;
    }
    let columns = moving_columns(prev, next);
    let rows = same_row_diffs(prev, next, &columns);
    let top = fixed_top_rows(prev, next);
    let bot = fixed_bottom_rows(prev, next);
    let zero = alignment_cost(prev, next, 0, true, &rows, &columns, top, bot).unwrap_or(i64::MAX / 8);
    let mut hits = Vec::new();
    for shift in 2..=max_shift {
        let Some(cost) = alignment_cost(prev, next, shift, downward, &rows, &columns, top, bot) else { continue };
        hits.push(ShiftHit { shift, cost });
    }
    if hits.is_empty() {
        return None;
    }
    hits.sort_by_key(|hit| hit.cost);
    let best = pick_shift(&hits, height, prev, next, downward);
    let slack = 3;
    let rival = hits.iter().find(|hit| hit.shift.abs_diff(best.shift) > 4 && hit.cost.saturating_add(slack) < best.cost);
    let clear = rival.is_none();
    Some(RankedShift { best, zero, clear })
}

/// A repeating layout also lines up at a later copy of the same step. Keep the
/// shorter step when it is nearly as good, so one notch is not pasted as a whole screen.
fn pick_shift(hits: &[ShiftHit], height: u32, prev: &RgbaImage, next: &RgbaImage, downward: bool) -> ShiftHit {
    let best = hits.iter().min_by_key(|hit| hit.cost).copied().expect("shift");
    let band = best.cost.saturating_add(3);
    let modest = (height / 2).max(24);
    let mut tied: Vec<ShiftHit> = hits.iter().copied().filter(|hit| hit.shift <= modest && hit.cost <= band).collect();
    if tied.is_empty() {
        return best;
    }
    if tied.len() == 1 {
        return tied[0];
    }
    // Repeated lines line up at more than one distance. A scrollbar thumb that
    // lines up at one of them is the real step. Otherwise keep the shortest.
    let best_edge = tied.iter().map(|hit| edge_cost(prev, next, hit.shift, downward)).min().unwrap_or(0);
    if best_edge <= 12 {
        tied.sort_by_key(|hit| (edge_cost(prev, next, hit.shift, downward), hit.shift));
        return tied[0];
    }
    tied.into_iter().min_by_key(|hit| hit.shift).unwrap_or(best)
}

fn edge_cost(prev: &RgbaImage, next: &RgbaImage, shift: u32, downward: bool) -> i64 {
    let width = prev.width().min(next.width());
    let height = prev.height().min(next.height());
    let edge = 20.min(width / 5).max(4);
    let mut columns = Vec::new();
    let mut x = width - edge;
    let step = (edge / 4).max(1);
    while x < width {
        columns.push(x);
        x += step;
    }
    let mut sum = 0i64;
    let mut count = 0i64;
    let mut y = if downward { shift } else { 0u32 };
    let y_end = if downward { height } else { height.saturating_sub(shift) };
    let y_step = (height / 40).max(1);
    while y < y_end {
        // The thumb travels the other way from the page text.
        let prev_y = if downward { y - shift } else { y + shift };
        if prev_y >= height {
            break;
        }
        sum += sample_diff(prev, prev_y, next, y, &columns);
        count += 1;
        y += y_step;
    }
    if count == 0 { i64::MAX / 8 } else { sum / count }
}

fn fixed_top_rows(prev: &RgbaImage, next: &RgbaImage) -> u32 {
    scan_fixed(prev, next, true)
}

fn fixed_bottom_rows(prev: &RgbaImage, next: &RgbaImage) -> u32 {
    scan_fixed(prev, next, false)
}

/// Leading or trailing rows that stay put. A busy run is a toolbar; a long
/// plain run is just page background that happened to match, and is not one.
fn scan_fixed(prev: &RgbaImage, next: &RgbaImage, from_top: bool) -> u32 {
    let height = prev.height().min(next.height());
    let max_band = height / 3;
    if prev.width() != next.width() || max_band < 2 {
        return 0;
    }
    let columns = edge_columns(prev.width());
    let mut band = 0u32;
    let mut gaps = 0u32;
    let mut index = 0u32;
    while index < max_band {
        let y = if from_top { index } else { height - 1 - index };
        // A clock or the pointer can change a few pixels. The row is still the taskbar
        // when almost every other sample stayed put.
        if row_mostly_same(prev, next, y, &columns) {
            band = index + 1;
            gaps = 0;
        } else {
            gaps += 1;
            if gaps > 3 {
                break;
            }
        }
        index += 1;
    }
    refine_edge(prev, band, from_top)
}

fn refine_edge(image: &RgbaImage, band: u32, from_top: bool) -> u32 {
    if band == 0 {
        return 0;
    }
    let height = image.height();
    let mut busy_end = 0u32;
    for index in 0..band {
        let y = if from_top { index } else { height - 1 - index };
        if row_contrast(image, y) > 36 {
            busy_end = index + 1;
        }
    }
    if busy_end == 0 {
        return 0;
    }
    let plain = band - busy_end;
    if plain <= 8 { band } else { busy_end + 4 }
}

fn row_mostly_same(prev: &RgbaImage, next: &RgbaImage, y: u32, columns: &[u32]) -> bool {
    if columns.is_empty() || y >= prev.height() || y >= next.height() {
        return false;
    }
    let mut same = 0i64;
    let mut count = 0i64;
    for x in columns {
        let a = prev.get_pixel(*x, y);
        let b = next.get_pixel(*x, y);
        let close = (i64::from(a[0]) - i64::from(b[0])).abs() <= 18
            && (i64::from(a[1]) - i64::from(b[1])).abs() <= 18
            && (i64::from(a[2]) - i64::from(b[2])).abs() <= 18;
        if close {
            same += 1;
        }
        count += 1;
    }
    count > 0 && same * 100 >= count * 85
}

fn edge_columns(width: u32) -> Vec<u32> {
    let step = (width / 48).max(1);
    let mut columns = Vec::new();
    let mut x = 0u32;
    while x < width {
        columns.push(x);
        x += step;
    }
    columns
}

fn row_contrast(image: &RgbaImage, y: u32) -> i64 {
    let width = image.width();
    if width < 2 || y >= image.height() {
        return 0;
    }
    let step = (width / 64).max(1);
    let mut low = i64::MAX;
    let mut high = i64::MIN;
    let mut x = 0u32;
    while x < width {
        let pixel = image.get_pixel(x, y);
        let level = i64::from(pixel[0]) + i64::from(pixel[1]) + i64::from(pixel[2]);
        low = low.min(level);
        high = high.max(level);
        x += step;
    }
    high - low
}

fn same_row_diffs(prev: &RgbaImage, next: &RgbaImage, columns: &[u32]) -> Vec<i64> {
    (0..prev.height()).map(|y| sample_diff(prev, y, next, y, columns)).collect()
}

fn alignment_cost(prev: &RgbaImage, next: &RgbaImage, shift: u32, downward: bool, rows: &[i64], columns: &[u32], top: u32, bot: u32) -> Option<i64> {
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
    let step = ((end - start) / 96).max(1);
    let mut sum = 0i64;
    let mut count = 0i64;
    let mut y = start;
    while y < end {
        if y < top || y.saturating_add(bot) >= height {
            y += step;
            continue;
        }
        let prev_y = if shift == 0 || downward { y.saturating_add(shift) } else { y.saturating_sub(shift) };
        if prev_y >= height || prev_y < top || prev_y.saturating_add(bot) >= height {
            y += step;
            continue;
        }
        let here = rows.get(y as usize).copied().unwrap_or(i64::MAX / 4);
        let source = rows.get(prev_y as usize).copied().unwrap_or(i64::MAX / 4);
        // Unchanged rows are a toolbar, a reply bar, or blank page. They do not say how far the page moved.
        if shift != 0 && (here <= 8 || source <= 8) {
            y += step;
            continue;
        }
        if shift == 0 && here <= 8 {
            y += step;
            continue;
        }
        sum = sum.saturating_add(sample_diff(prev, prev_y, next, y, columns));
        count += 1;
        y += step;
    }
    if count < 4 {
        None
    } else {
        Some(sum / count)
    }
}

fn sample_columns(width: u32) -> (u32, u32) {
    let skip = if width > 96 { 32 } else { 0 };
    let usable = width.saturating_sub(skip);
    let step = (usable / 32).max(1);
    (usable, step)
}

/// Columns that actually changed. A fixed sidebar or scrollbar is left out of the score
/// so it cannot hide the part of the page that scrolled.
fn moving_columns(prev: &RgbaImage, next: &RgbaImage) -> Vec<u32> {
    let width = prev.width().min(next.width());
    let height = prev.height().min(next.height());
    let (usable, step) = sample_columns(width);
    if usable == 0 || height == 0 {
        return Vec::new();
    }
    let y_step = (height / 12).max(1);
    let mut moving = Vec::new();
    let mut x = 0u32;
    while x < usable {
        let mut sum = 0i64;
        let mut count = 0i64;
        let mut y = 0u32;
        while y < height {
            let a = prev.get_pixel(x, y);
            let b = next.get_pixel(x, y);
            sum += (a[0] as i64 - b[0] as i64).abs();
            sum += (a[1] as i64 - b[1] as i64).abs();
            sum += (a[2] as i64 - b[2] as i64).abs();
            count += 3;
            y += y_step;
        }
        if count > 0 && sum / count > 12 {
            moving.push(x);
        }
        x += step;
    }
    if moving.len() >= 4 {
        return moving;
    }
    let mut all = Vec::new();
    let mut x = 0u32;
    while x < usable {
        all.push(x);
        x += step;
    }
    all
}

fn sample_diff(prev: &RgbaImage, prev_y: u32, next: &RgbaImage, next_y: u32, columns: &[u32]) -> i64 {
    if columns.is_empty() || prev_y >= prev.height() || next_y >= next.height() {
        return i64::MAX / 4;
    }
    let mut total = 0i64;
    let mut count = 0i64;
    for x in columns {
        let a = prev.get_pixel(*x, prev_y);
        let b = next.get_pixel(*x, next_y);
        total += (a[0] as i64 - b[0] as i64).abs();
        total += (a[1] as i64 - b[1] as i64).abs();
        total += (a[2] as i64 - b[2] as i64).abs();
        count += 3;
    }
    if count == 0 { 0 } else { total / count }
}

fn describe_shifts(prev: &RgbaImage, next: &RgbaImage, direction: i8) -> String {
    let columns = moving_columns(prev, next);
    let rows = same_row_diffs(prev, next, &columns);
    let top = fixed_top_rows(prev, next);
    let bot = fixed_bottom_rows(prev, next);
    let zero = alignment_cost(prev, next, 0, true, &rows, &columns, top, bot).unwrap_or(-1);
    let down = if direction != -1 { best_shift(prev, next, true) } else { None };
    let up = if direction != 1 { best_shift(prev, next, false) } else { None };
    format!(
        "zero={zero} down={} up={}",
        down.map(|hit| format!("{}@{}", hit.shift, hit.cost)).unwrap_or_else(|| "-".into()),
        up.map(|hit| format!("{}@{}", hit.shift, hit.cost)).unwrap_or_else(|| "-".into()),
    )
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

    fn stepped(scroll: u32) -> RgbaImage {
        let mut image = RgbaImage::new(24, 10);
        for y in 0..10 {
            let level = ((y + scroll) * 20) as u8;
            for x in 0..24 {
                image.put_pixel(x, y, Rgba([level, level, level, 255]));
            }
        }
        image
    }

    #[test]
    fn auto_accepts_a_dirty_step_the_strict_matcher_rejects() {
        let mut roll = Roll::new(stepped(0));
        let mut next = stepped(2);
        for pixel in next.pixels_mut() {
            pixel[0] = pixel[0].saturating_add(90);
        }
        roll.absorb(next.clone());
        assert_eq!(roll.compose().height(), 10);
        roll.absorb_auto(next);
        assert_eq!(roll.compose().height(), 12);
    }

    #[test]
    fn a_repeated_layout_keeps_the_shorter_step() {
        let page = page(30, 96);
        let chosen = super::pick_shift(
            &[
                super::ShiftHit { shift: 48, cost: 4 },
                super::ShiftHit { shift: 24, cost: 5 },
                super::ShiftHit { shift: 12, cost: 6 },
            ],
            96,
            &page,
            &page,
            true,
        );
        assert_eq!(chosen.shift, 12);
    }

    #[test]
    fn a_real_long_scroll_is_not_replaced_by_a_bad_short_one() {
        let page = page(30, 80);
        let chosen = super::pick_shift(
            &[
                super::ShiftHit { shift: 50, cost: 4 },
                super::ShiftHit { shift: 12, cost: 40 },
            ],
            80,
            &page,
            &page,
            true,
        );
        assert_eq!(chosen.shift, 50);
    }

    #[test]
    fn a_fixed_toolbar_is_not_pasted_into_the_seam() {
        let shot = |scroll: u32| {
            let mut image = RgbaImage::new(40, 48);
            for y in 0..12 {
                for x in 0..40 {
                    let pixel = if x % 5 == 0 { Rgba([20, 20, 20, 255]) } else { Rgba([230, 230, 230, 255]) };
                    image.put_pixel(x, y, pixel);
                }
            }
            for y in 12..48 {
                for x in 0..40 {
                    let row = y + scroll;
                    image.put_pixel(x, y, Rgba([(row * 17) as u8, (row * 13) as u8, (row * 9) as u8, 255]));
                }
            }
            image
        };
        assert_eq!(super::fixed_top_rows(&shot(0), &shot(40)), 12);
        let mut roll = Roll::new(shot(0));
        roll.absorb(shot(10));
        let stitched = roll.compose();
        assert!(stitched.height() > 48);
        assert_eq!(stitched.get_pixel(1, 0), shot(0).get_pixel(1, 0));
        assert_eq!(stitched.get_pixel(0, 48), stitched.get_pixel(1, 48));
    }

    fn browser(scroll: u32) -> RgbaImage {
        let width = 80u32;
        let height = 180u32;
        let chrome = 36u32;
        let mut image = RgbaImage::new(width, height);
        for y in 0..chrome {
            for x in 0..width {
                let mut pixel = if x % 4 == 0 { Rgba([15, 15, 15, 255]) } else { Rgba([240, 240, 240, 255]) };
                if y == 0 && x == 0 {
                    pixel = Rgba([9, 8, 7, 255]);
                }
                if y == 8 {
                    pixel = Rgba([(40 + scroll.wrapping_mul(3)) as u8, 5, 5, 255]);
                }
                image.put_pixel(x, y, pixel);
            }
        }
        for y in chrome..height {
            let row = y - chrome + scroll;
            for x in 0..width {
                image.put_pixel(x, y, Rgba([(row.wrapping_mul(17)) as u8, (row.wrapping_mul(13)) as u8, (row.wrapping_mul(9)) as u8, 255]));
            }
        }
        image
    }

    #[test]
    fn a_browser_toolbar_is_kept_once_while_scrolling_down() {
        match super::manual_step(&browser(0), &browser(50), 0) {
            super::Align::Hit(true, shift) => assert_eq!(shift, 50),
            super::Align::Hit(false, shift) => panic!("upward {shift}"),
            super::Align::Miss => panic!("miss"),
            super::Align::Reverse => panic!("reverse"),
        }
        assert_eq!(super::fixed_top_rows(&browser(0), &browser(50)), 36);
        let mut roll = Roll::new(browser(0));
        roll.absorb(browser(50));
        roll.absorb(browser(100));
        let stitched = roll.compose();
        assert_eq!(stitched.height(), 280);
        let mut toolbars = 0u32;
        for y in 0..stitched.height() {
            if stitched.get_pixel(0, y).0 == [9, 8, 7, 255] {
                toolbars += 1;
            }
        }
        assert_eq!(toolbars, 1);
        assert_eq!(stitched.get_pixel(3, 36), browser(0).get_pixel(3, 36));
        let last = browser(100);
        assert_eq!(stitched.get_pixel(3, stitched.height() - 1), last.get_pixel(3, last.height() - 1));
    }

    #[test]
    fn scrolling_up_inserts_new_rows_under_the_toolbar() {
        let mut roll = Roll::new(browser(40));
        roll.absorb(browser(0));
        let stitched = roll.compose();
        assert_eq!(stitched.height(), 220);
        assert_eq!(stitched.get_pixel(0, 0).0, [9, 8, 7, 255]);
        assert_eq!(stitched.get_pixel(3, 36), browser(0).get_pixel(3, 36));
        assert_eq!(stitched.get_pixel(3, stitched.height() - 1), browser(40).get_pixel(3, 179));
        let mut toolbars = 0u32;
        for y in 0..stitched.height() {
            if stitched.get_pixel(0, y).0 == [9, 8, 7, 255] {
                toolbars += 1;
            }
        }
        assert_eq!(toolbars, 1);
    }

    #[test]
    fn a_plain_page_top_is_not_a_toolbar() {
        let plain = |scroll: u32| {
            let mut image = RgbaImage::new(40, 120);
            for y in 0..120 {
                let row = y + scroll;
                for x in 0..40 {
                    let pixel = if row < 70 {
                        Rgba([255, 255, 255, 255])
                    } else {
                        Rgba([(row.wrapping_mul(17)) as u8, (row.wrapping_mul(13)) as u8, (row.wrapping_mul(9)) as u8, 255])
                    };
                    image.put_pixel(x, y, pixel);
                }
            }
            image
        };
        assert_eq!(super::fixed_top_rows(&plain(0), &plain(20)), 0);
        let mut roll = Roll::new(plain(0));
        roll.absorb(plain(20));
        assert_eq!(roll.height(), 140);
    }

    #[test]
    fn a_document_beside_a_fixed_tree_still_stitches() {
        let shot = |scroll: u32| {
            let mut image = RgbaImage::new(180, 100);
            for y in 0..100 {
                for x in 0..48 {
                    image.put_pixel(x, y, Rgba([230, 232, 236, 255]));
                    if x > 8 && x < 40 && y % 14 > 2 && y % 14 < 10 {
                        image.put_pixel(x, y, Rgba([40, 40, 40, 255]));
                    }
                }
                for x in 48..166 {
                    let row = y + scroll;
                    image.put_pixel(x, y, Rgba([(row * 17) as u8, (row * 13) as u8, (row * 9) as u8, 255]));
                }
                for x in 166..180 {
                    let thumb = y + 10 > (scroll % 70) && y < (scroll % 70) + 30;
                    image.put_pixel(x, y, if thumb { Rgba([120, 120, 120, 255]) } else { Rgba([245, 245, 245, 255]) });
                }
            }
            image
        };
        let mut roll = Roll::new(shot(0));
        roll.absorb(shot(28));
        assert_eq!(roll.compose().height(), 128);
    }

    #[test]
    fn a_taskbar_with_a_changing_clock_stays_at_the_bottom() {
        let shot = |scroll: u32| {
            let mut image = RgbaImage::new(96, 80);
            for y in 0..62 {
                for x in 0..96 {
                    let row = y + scroll;
                    image.put_pixel(x, y, Rgba([(row * 17) as u8, (row * 13) as u8, (row * 9) as u8, 255]));
                }
            }
            for y in 62..80 {
                for x in 0..96 {
                    let icon = x % 11 == 0;
                    let clock = x >= 88;
                    let pixel = if icon {
                        Rgba([250, 250, 250, 255])
                    } else if clock {
                        Rgba([if scroll == 0 { 220 } else { 30 }, 30, 30, 255])
                    } else {
                        Rgba([15, 25, 40, 255])
                    };
                    image.put_pixel(x, y, pixel);
                }
            }
            image
        };
        let mut roll = Roll::new(shot(0));
        roll.absorb(shot(18));
        let stitched = roll.compose();
        assert!(stitched.height() > 80, "the page below the taskbar was not appended");
        let bottom = stitched.height() - 1;
        assert_eq!(stitched.get_pixel(4, bottom).0, [15, 25, 40, 255]);
        let mut stray = 0u32;
        for y in 0..bottom.saturating_sub(17) {
            if stitched.get_pixel(4, y).0 == [15, 25, 40, 255] {
                stray += 1;
            }
        }
        assert_eq!(stray, 0, "the taskbar was inserted into the middle of the page");
    }

    #[test]
    fn a_scroll_taller_than_the_middle_is_still_stitched() {
        let shot = |scroll: u32| {
            let mut image = RgbaImage::new(48, 60);
            for y in 0..14 {
                for x in 0..48 {
                    image.put_pixel(x, y, Rgba([20, 20, 20, 255]));
                }
            }
            for y in 46..60 {
                for x in 0..48 {
                    image.put_pixel(x, y, Rgba([80, 80, 80, 255]));
                }
            }
            for y in 14..46 {
                for x in 0..48 {
                    let row = y + scroll;
                    image.put_pixel(x, y, Rgba([(row * 17) as u8, (row * 13) as u8, (row * 9) as u8, 255]));
                }
            }
            image
        };
        let mut roll = Roll::new(shot(0));
        roll.absorb(shot(24));
        assert!(roll.compose().height() > 60, "scrolled rows were dropped");
    }

    #[test]
    fn stitches_past_a_sidebar_that_does_not_move() {
        let shot = |scroll: u32| {
            let mut image = RgbaImage::new(80, 48);
            for y in 0..48 {
                for x in 0..52 {
                    let row = y + scroll;
                    image.put_pixel(x, y, Rgba([(row * 17) as u8, (row * 13) as u8, (row * 9) as u8, 255]));
                }
                for x in 52..80 {
                    image.put_pixel(x, y, Rgba([24, (y * 7) as u8, 90, 255]));
                }
            }
            image
        };
        let mut roll = Roll::new(shot(0));
        roll.absorb(shot(12));
        let stitched = roll.compose();
        assert_eq!(stitched.width(), 80);
        assert!(stitched.height() > 48);
    }

    #[test]
    fn carry_repeats_the_last_downward_step() {
        let full = page(30, 120);
        let mut roll = Roll::new(view(&full, 0, 40));
        roll.absorb(view(&full, 12, 40));
        assert_eq!(roll.height(), 52);
        let jumped = view(&full, 70, 40);
        assert!(roll.carry_down(jumped));
        assert_eq!(roll.height(), 64);
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
