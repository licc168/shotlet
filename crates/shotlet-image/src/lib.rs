use image::RgbaImage;
#[cfg(test)]
use image::Rgba;

/// Append the newly revealed rows when `next` is the same region after a downward scroll.
/// `next[0]` is expected to line up with `prev[shift]`.
#[cfg(test)]
pub fn append_scrolled(prev: &RgbaImage, next: &RgbaImage) -> (RgbaImage, bool) {
    if prev.width() != next.width() || prev.height() < 8 || next.height() < 8 {
        return (prev.clone(), false);
    }
    let Some(shift) = best_shift(prev, next) else {
        return (prev.clone(), false);
    };
    if shift < 2 {
        return (prev.clone(), false);
    }
    let mut out = RgbaImage::new(prev.width(), prev.height() + shift);
    for y in 0..prev.height() {
        for x in 0..prev.width() {
            out.put_pixel(x, y, *prev.get_pixel(x, y));
        }
    }
    let start = next.height() - shift;
    for row in 0..shift {
        for x in 0..next.width() {
            out.put_pixel(x, prev.height() + row, *next.get_pixel(x, start + row));
        }
    }
    (out, true)
}

#[cfg(test)]
fn best_shift(prev: &RgbaImage, next: &RgbaImage) -> Option<u32> {
    let height = prev.height().min(next.height());
    let max_shift = height / 2;
    if max_shift < 2 {
        return None;
    }
    let mut best_shift = 0u32;
    let mut best_score = i64::MAX;
    for shift in 1..max_shift {
        let overlap = height - shift;
        let mut score = 0i64;
        let step = (overlap / 12).max(1);
        let mut row = 0;
        while row < overlap {
            score += row_diff(prev, shift + row, next, row);
            if score >= best_score {
                break;
            }
            row += step;
        }
        if score < best_score {
            best_score = score;
            best_shift = shift;
        }
    }
    let samples = ((height - best_shift) / ((height - best_shift) / 12).max(1)).max(1);
    let average = best_score / samples as i64;
    if average > 18 {
        None
    } else {
        Some(best_shift)
    }
}

fn row_diff(prev: &RgbaImage, prev_y: u32, next: &RgbaImage, next_y: u32) -> i64 {
    let width = prev.width().min(next.width());
    if width == 0 || prev_y >= prev.height() || next_y >= next.height() {
        return i64::MAX / 4;
    }
    let step = (width / 48).max(1);
    let mut total = 0i64;
    let mut count = 0i64;
    let mut x = 0;
    while x < width {
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


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollMotion {
    Down,
    Up,
    Still,
}

/// True when two captures of the same viewport have settled.
pub fn frames_stable(prev: &RgbaImage, next: &RgbaImage) -> bool {
    if prev.dimensions() != next.dimensions() || prev.width() == 0 || prev.height() == 0 {
        return false;
    }
    let samples = 12u32.min(prev.height());
    let mut total = 0i64;
    for index in 0..samples {
        let y = index * prev.height() / samples;
        let diff = row_diff(prev, y, next, y);
        if diff > 3 {
            return false;
        }
        total += diff;
    }
    total / i64::from(samples) <= 2
}

/// How many new rows appeared at the bottom of `next` after a downward scroll.
/// `expected` breaks a near tie; a real ambiguity returns `None` so a strip is not repeated.
pub fn revealed_rows(prev: &RgbaImage, next: &RgbaImage, expected: Option<u32>) -> Option<u32> {
    choose_shift(&down_candidates(prev, next), expected).map(|score| score.shift)
}

/// How many new rows appeared at the top of `next` after an upward scroll.
pub fn revealed_top_rows(prev: &RgbaImage, next: &RgbaImage) -> Option<u32> {
    choose_shift(&up_candidates(prev, next), None).map(|score| score.shift)
}

pub fn scroll_motion(prev: &RgbaImage, next: &RgbaImage) -> ScrollMotion {
    let down_scores = down_candidates(prev, next);
    let up_scores = up_candidates(prev, next);
    let down = leading(&down_scores);
    let up = leading(&up_scores);
    match (down, up) {
        (Some(down), Some(up)) if clearly_more(down, up) => ScrollMotion::Down,
        (Some(down), Some(up)) if clearly_more(up, down) => ScrollMotion::Up,
        (Some(_), Some(_)) => ScrollMotion::Still,
        (Some(_), None) => ScrollMotion::Down,
        (None, Some(_)) => ScrollMotion::Up,
        (None, None) => ScrollMotion::Still,
    }
}

/// Put `rows` above `base`. Widths must match; an empty strip leaves `base` unchanged.
pub fn prepend_rows(base: &RgbaImage, rows: &RgbaImage) -> RgbaImage {
    if rows.height() == 0 || rows.width() != base.width() {
        return base.clone();
    }
    let width = base.width();
    let height = rows.height().saturating_add(base.height());
    let mut raw = Vec::with_capacity((height as usize).saturating_mul(width as usize).saturating_mul(4));
    raw.extend_from_slice(rows.as_raw());
    raw.extend_from_slice(base.as_raw());
    RgbaImage::from_raw(width, height, raw).unwrap_or_else(|| base.clone())
}

/// Append `rows` under `base`. Widths must match; an empty strip leaves `base` unchanged.
pub fn append_rows(base: &RgbaImage, rows: &RgbaImage) -> RgbaImage {
    if rows.height() == 0 || rows.width() != base.width() {
        return base.clone();
    }
    let width = base.width();
    let height = base.height().saturating_add(rows.height());
    let mut raw = Vec::with_capacity((height as usize).saturating_mul(width as usize).saturating_mul(4));
    raw.extend_from_slice(base.as_raw());
    raw.extend_from_slice(rows.as_raw());
    RgbaImage::from_raw(width, height, raw).unwrap_or_else(|| base.clone())
}

struct ShiftScore {
    shift: u32,
    support: u32,
    error: i64,
}

fn clearly_more(better: &ShiftScore, other: &ShiftScore) -> bool {
    better.support > other.support.saturating_add(other.support / 5).max(other.support.saturating_add(1))
}

fn down_candidates(prev: &RgbaImage, next: &RgbaImage) -> Vec<ShiftScore> {
    shift_candidates(prev, next, true)
}

fn up_candidates(prev: &RgbaImage, next: &RgbaImage) -> Vec<ShiftScore> {
    shift_candidates(prev, next, false)
}

fn shift_candidates(prev: &RgbaImage, next: &RgbaImage, downward: bool) -> Vec<ShiftScore> {
    if prev.width() != next.width() || prev.height() < 8 || next.height() < 8 || prev.height() != next.height() {
        return Vec::new();
    }
    let height = prev.height();
    let max_shift = (height * 2 / 3).min(height.saturating_sub(4));
    if max_shift < 2 {
        return Vec::new();
    }
    let mut found = Vec::new();
    for shift in 2..max_shift {
        let overlap_start = if downward { 0 } else { shift };
        let overlap_end = if downward { height - shift } else { height };
        if overlap_end <= overlap_start {
            continue;
        }
        let span = overlap_end - overlap_start;
        let step = if height <= 240 { 1 } else { (span / 80).max(1) };
        let mut support = 0u32;
        let mut error = 0i64;
        let mut y = overlap_start;
        while y < overlap_end {
            let prev_y = if downward { y + shift } else { y - shift };
            if let Some(diff) = aligned_support(prev, next, prev_y, y) {
                support += 1;
                error += diff;
            }
            y += step;
        }
        if support >= 3 {
            found.push(ShiftScore {
                shift,
                support,
                error: error / i64::from(support),
            });
        }
    }
    found
}

/// A row counts only when it actually moved, and lines up after `shift` (difference <= 20).
fn aligned_support(prev: &RgbaImage, next: &RgbaImage, prev_y: u32, next_y: u32) -> Option<i64> {
    let aligned = row_diff(prev, prev_y, next, next_y);
    if aligned > 20 {
        return None;
    }
    let unaligned = row_diff(prev, next_y, next, next_y);
    if unaligned < 40 || unaligned < aligned.saturating_add(24) {
        return None;
    }
    Some(aligned)
}

fn leading(candidates: &[ShiftScore]) -> Option<&ShiftScore> {
    candidates.iter().max_by(|left, right| {
        left.support.cmp(&right.support).then(right.error.cmp(&left.error)).then(right.shift.cmp(&left.shift))
    })
}

fn choose_shift(candidates: &[ShiftScore], expected: Option<u32>) -> Option<&ShiftScore> {
    let best = leading(candidates)?.support;
    let close: Vec<&ShiftScore> = candidates
        .iter()
        .filter(|score| score.support + 1 >= best || score.support.saturating_mul(10) >= best.saturating_mul(9))
        .collect();
    if close.len() == 1 {
        return Some(close[0]);
    }
    let expected = expected?;
    let nearest = close.iter().map(|score| score.shift.abs_diff(expected)).min()?;
    let tied: Vec<&ShiftScore> = close.into_iter().filter(|score| score.shift.abs_diff(expected) == nearest).collect();
    if tied.len() == 1 { Some(tied[0]) } else { None }
}

#[cfg(test)]
fn solid(width: u32, height: u32, color_at: impl Fn(u32, u32) -> Rgba<u8>) -> RgbaImage {
    let mut image = RgbaImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            image.put_pixel(x, y, color_at(x, y));
        }
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stitches_a_downward_scroll() {
        let full = solid(30, 70, |_x, y| Rgba([y as u8 * 3, 20, 255 - y as u8, 255]));
        let prev = image::imageops::crop_imm(&full, 0, 0, 30, 40).to_image();
        let next = image::imageops::crop_imm(&full, 0, 15, 30, 40).to_image();
        let (stitched, grew) = append_scrolled(&prev, &next);
        assert!(grew);
        assert_eq!(stitched.height(), 55);
        assert_eq!(stitched.get_pixel(4, 0), full.get_pixel(4, 0));
        assert_eq!(stitched.get_pixel(4, 54), full.get_pixel(4, 54));
    }

    #[test]
    fn ignores_an_unrelated_frame() {
        let prev = solid(20, 30, |_x, y| Rgba([y as u8, 0, 0, 255]));
        let next = solid(20, 30, |_x, y| Rgba([0, y as u8, 40, 255]));
        let (same, grew) = append_scrolled(&prev, &next);
        assert!(!grew);
        assert_eq!(same.height(), prev.height());
    }

    #[test]
    fn stitches_two_viewport_steps() {
        let full = solid(30, 70, |_x, y| Rgba([(y * 17) as u8, (y * 13) as u8, (y * 9) as u8, 255]));
        let first = image::imageops::crop_imm(&full, 0, 0, 30, 40).to_image();
        let second = image::imageops::crop_imm(&full, 0, 15, 30, 40).to_image();
        let third = image::imageops::crop_imm(&full, 0, 30, 30, 40).to_image();
        let step_a = revealed_rows(&first, &second, None).unwrap();
        let step_b = revealed_rows(&second, &third, Some(15)).unwrap();
        assert_eq!(step_a, 15);
        assert_eq!(step_b, 15);
        let mut stitched = append_rows(&first, &image::imageops::crop_imm(&second, 0, second.height() - step_a, 30, step_a).to_image());
        stitched = append_rows(&stitched, &image::imageops::crop_imm(&third, 0, third.height() - step_b, 30, step_b).to_image());
        assert_eq!(stitched.height(), 70);
        assert_eq!(stitched.get_pixel(4, 0), full.get_pixel(4, 0));
        assert_eq!(stitched.get_pixel(4, 69), full.get_pixel(4, 69));
    }

    #[test]
    fn identical_and_unrelated_frames_do_not_grow() {
        let prev = solid(20, 40, |_x, y| Rgba([(y * 17) as u8, 10, 30, 255]));
        let same = prev.clone();
        assert!(revealed_rows(&prev, &same, None).is_none());
        assert_eq!(scroll_motion(&prev, &same), ScrollMotion::Still);
        assert!(frames_stable(&prev, &same));
        let unrelated = solid(20, 40, |_x, y| Rgba([0, (y * 13) as u8, 200, 255]));
        assert!(revealed_rows(&prev, &unrelated, Some(12)).is_none());
        assert!(!frames_stable(&prev, &unrelated));
        let kept = append_rows(&prev, &image::RgbaImage::new(prev.width(), 0));
        assert_eq!(kept.height(), prev.height());
    }

    #[test]
    fn detects_an_upward_scroll() {
        let full = solid(30, 70, |_x, y| Rgba([(y * 17) as u8, (y * 13) as u8, (y * 9) as u8, 255]));
        let prev = image::imageops::crop_imm(&full, 0, 15, 30, 40).to_image();
        let next = image::imageops::crop_imm(&full, 0, 0, 30, 40).to_image();
        assert_eq!(scroll_motion(&prev, &next), ScrollMotion::Up);
        assert!(revealed_rows(&prev, &next, Some(15)).is_none());
        assert_eq!(revealed_top_rows(&prev, &next), Some(15));
        let top = image::imageops::crop_imm(&next, 0, 0, 30, 15).to_image();
        let stitched = prepend_rows(&prev, &top);
        assert_eq!(stitched.height(), 55);
        assert_eq!(stitched.get_pixel(4, 0), full.get_pixel(4, 0));
        assert_eq!(stitched.get_pixel(4, 54), full.get_pixel(4, 54));
    }

    #[test]
    fn stitches_past_a_sticky_header() {
        let frame = |scroll: u32| {
            solid(24, 40, move |_x, y| {
                if y < 10 {
                    Rgba([20, 30, 40, 255])
                } else {
                    let row = y - 10 + scroll;
                    Rgba([(row * 17) as u8, (row * 13) as u8, (row * 9) as u8, 255])
                }
            })
        };
        let prev = frame(0);
        let next = frame(14);
        assert_eq!(scroll_motion(&prev, &next), ScrollMotion::Down);
        assert_eq!(revealed_rows(&prev, &next, Some(14)), Some(14));
        let strip = image::imageops::crop_imm(&next, 0, next.height() - 14, 24, 14).to_image();
        let stitched = append_rows(&prev, &strip);
        assert_eq!(stitched.height(), 54);
        assert_eq!(stitched.get_pixel(0, 0), prev.get_pixel(0, 0));
        assert_eq!(stitched.get_pixel(3, 53), next.get_pixel(3, 39));
    }
}
