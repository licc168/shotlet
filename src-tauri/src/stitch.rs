use image::RgbaImage;
#[cfg(test)]
use image::Rgba;

/// Append the newly revealed rows when `next` is the same region after a downward scroll.
/// `next[0]` is expected to line up with `prev[shift]`.
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
}
