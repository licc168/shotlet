use image::RgbaImage;

pub fn paint_cursor(image: &mut RgbaImage, origin_x: i32, origin_y: i32) {
    #[cfg(windows)]
    {
        paint_windows(image, origin_x, origin_y);
    }
    #[cfg(not(windows))]
    {
        let _ = (image, origin_x, origin_y);
    }
}

pub fn blend_pixel(dst: &mut [u8; 4], r: u8, g: u8, b: u8, a: u8) {
    if a == 0 {
        return;
    }
    let straight = r > a || g > a || b > a;
    let alpha = a as u32;
    let inv = 255 - alpha;
    let (sr, sg, sb) = if straight {
        (r as u32 * alpha / 255, g as u32 * alpha / 255, b as u32 * alpha / 255)
    } else {
        (r as u32, g as u32, b as u32)
    };
    dst[0] = (sr + dst[0] as u32 * inv / 255).min(255) as u8;
    dst[1] = (sg + dst[1] as u32 * inv / 255).min(255) as u8;
    dst[2] = (sb + dst[2] as u32 * inv / 255).min(255) as u8;
    dst[3] = 255;
}

#[cfg(windows)]
fn paint_windows(image: &mut RgbaImage, origin_x: i32, origin_y: i32) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{GetCursorInfo, GetIconInfo, CURSORINFO, CURSOR_SHOWING, HCURSOR, HICON, ICONINFO};

    unsafe {
        let mut info = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            flags: Default::default(),
            hCursor: HCURSOR::default(),
            ptScreenPos: POINT::default(),
        };
        if GetCursorInfo(&mut info).is_err() || info.flags.0 & CURSOR_SHOWING.0 == 0 || info.hCursor.is_invalid() {
            return;
        }
        let cursor_x = info.ptScreenPos.x;
        let cursor_y = info.ptScreenPos.y;
        let width = image.width() as i32;
        let height = image.height() as i32;
        if cursor_x < origin_x - 256 || cursor_y < origin_y - 256 || cursor_x > origin_x + width + 256 || cursor_y > origin_y + height + 256 {
            return;
        }
        let mut icon = ICONINFO::default();
        if GetIconInfo(HICON::from(info.hCursor), &mut icon).is_err() {
            return;
        }
        let _kept = IconBitmaps(icon);
        let Some(stamp) = read_stamp(icon) else {
            return;
        };
        let left = cursor_x - origin_x - stamp.hotspot_x;
        let top = cursor_y - origin_y - stamp.hotspot_y;
        composite(image, &stamp, left, top);
    }
}

#[cfg(windows)]
struct IconBitmaps(windows::Win32::UI::WindowsAndMessaging::ICONINFO);

#[cfg(windows)]
impl Drop for IconBitmaps {
    fn drop(&mut self) {
        use windows::Win32::Graphics::Gdi::{DeleteObject, HGDIOBJ};
        unsafe {
            if !self.0.hbmMask.is_invalid() {
                let _ = DeleteObject(HGDIOBJ::from(self.0.hbmMask));
            }
            if !self.0.hbmColor.is_invalid() {
                let _ = DeleteObject(HGDIOBJ::from(self.0.hbmColor));
            }
        }
    }
}

#[cfg(windows)]
struct Stamp {
    pixels: Vec<u8>,
    width: i32,
    height: i32,
    hotspot_x: i32,
    hotspot_y: i32,
}

#[cfg(windows)]
fn read_stamp(icon: windows::Win32::UI::WindowsAndMessaging::ICONINFO) -> Option<Stamp> {
    use windows::Win32::Graphics::Gdi::{CreateCompatibleDC, DeleteDC};
    let dc = unsafe { CreateCompatibleDC(None) };
    if dc.is_invalid() {
        return None;
    }
    let stamp = read_stamp_dc(dc, icon);
    unsafe { let _ = DeleteDC(dc); }
    stamp
}

#[cfg(windows)]
fn read_stamp_dc(dc: windows::Win32::Graphics::Gdi::HDC, icon: windows::Win32::UI::WindowsAndMessaging::ICONINFO) -> Option<Stamp> {
    let hotspot_x = icon.xHotspot as i32;
    let hotspot_y = icon.yHotspot as i32;
    if !icon.hbmColor.is_invalid() {
        let (width, height) = bitmap_size(icon.hbmColor)?;
        let color = read_bgra(dc, icon.hbmColor, width, height)?;
        let has_alpha = color.chunks_exact(4).any(|pixel| pixel[3] > 0);
        let mut pixels = vec![0u8; color.len()];
        if has_alpha {
            copy_bgra(&color, &mut pixels);
        } else {
            let mask = read_and_mask(dc, icon.hbmMask, width, height);
            copy_opaque_color(&color, mask.as_deref(), &mut pixels);
        }
        return Some(Stamp { pixels, width, height, hotspot_x, hotspot_y });
    }
    if icon.hbmMask.is_invalid() {
        return None;
    }
    let (width, full_height) = bitmap_size(icon.hbmMask)?;
    if full_height < 2 || full_height % 2 != 0 {
        return None;
    }
    let height = full_height / 2;
    let mask = read_bgra(dc, icon.hbmMask, width, full_height)?;
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let and_i = ((y * width + x) * 4) as usize;
            let xor_i = (((y + height) * width + x) * 4) as usize;
            let out = and_i;
            let and_white = mask.get(and_i).copied().unwrap_or(0) > 128;
            let xor_white = mask.get(xor_i).copied().unwrap_or(0) > 128;
            let (r, g, b, a) = match (and_white, xor_white) {
                (true, false) => (0, 0, 0, 0),
                (false, false) => (0, 0, 0, 255),
                _ => (255, 255, 255, 255),
            };
            pixels[out] = r;
            pixels[out + 1] = g;
            pixels[out + 2] = b;
            pixels[out + 3] = a;
        }
    }
    Some(Stamp { pixels, width, height, hotspot_x, hotspot_y })
}

#[cfg(windows)]
fn bitmap_size(bitmap: windows::Win32::Graphics::Gdi::HBITMAP) -> Option<(i32, i32)> {
    use windows::Win32::Graphics::Gdi::{GetObjectW, BITMAP, HGDIOBJ};
    let mut info = BITMAP::default();
    let wrote = unsafe {
        GetObjectW(
            HGDIOBJ::from(bitmap),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut info as *mut BITMAP as *mut _),
        )
    };
    if wrote == 0 || info.bmWidth <= 0 || info.bmHeight <= 0 || info.bmWidth > 1024 || info.bmHeight > 1024 {
        return None;
    }
    Some((info.bmWidth, info.bmHeight))
}

#[cfg(windows)]
fn read_bgra(dc: windows::Win32::Graphics::Gdi::HDC, bitmap: windows::Win32::Graphics::Gdi::HBITMAP, width: i32, height: i32) -> Option<Vec<u8>> {
    use windows::Win32::Graphics::Gdi::{GetDIBits, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS};
    if width <= 0 || height <= 0 || width > 1024 || height > 1024 {
        return None;
    }
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let lines = unsafe {
        GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    if lines == 0 {
        None
    } else {
        Some(pixels)
    }
}

#[cfg(windows)]
fn read_and_mask(dc: windows::Win32::Graphics::Gdi::HDC, mask: windows::Win32::Graphics::Gdi::HBITMAP, width: i32, height: i32) -> Option<Vec<u8>> {
    if mask.is_invalid() {
        return None;
    }
    let (mask_w, mask_h) = bitmap_size(mask)?;
    if mask_w < width || mask_h < height {
        return None;
    }
    let full = read_bgra(dc, mask, mask_w, mask_h)?;
    if mask_w == width && mask_h == height {
        return Some(full);
    }
    let mut top = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        let src = (y * mask_w * 4) as usize;
        let dst = (y * width * 4) as usize;
        let count = (width * 4) as usize;
        top[dst..dst + count].copy_from_slice(&full[src..src + count]);
    }
    Some(top)
}

#[cfg(windows)]
fn copy_bgra(color: &[u8], pixels: &mut [u8]) {
    for (src, dst) in color.chunks_exact(4).zip(pixels.chunks_exact_mut(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }
}

#[cfg(windows)]
fn copy_opaque_color(color: &[u8], mask: Option<&[u8]>, pixels: &mut [u8]) {
    for (index, src) in color.chunks_exact(4).enumerate() {
        let transparent = mask.and_then(|mask| mask.get(index * 4)).copied().unwrap_or(0) > 128;
        if transparent {
            continue;
        }
        let dst = index * 4;
        if dst + 3 >= pixels.len() {
            break;
        }
        pixels[dst] = src[2];
        pixels[dst + 1] = src[1];
        pixels[dst + 2] = src[0];
        pixels[dst + 3] = 255;
    }
}

#[cfg(windows)]
fn composite(image: &mut RgbaImage, stamp: &Stamp, left: i32, top: i32) {
    let width = image.width() as i32;
    let height = image.height() as i32;
    for y in 0..stamp.height {
        let dest_y = top + y;
        if dest_y < 0 || dest_y >= height {
            continue;
        }
        for x in 0..stamp.width {
            let dest_x = left + x;
            if dest_x < 0 || dest_x >= width {
                continue;
            }
            let index = ((y * stamp.width + x) * 4) as usize;
            let alpha = stamp.pixels.get(index + 3).copied().unwrap_or(0);
            if alpha == 0 {
                continue;
            }
            let pixel = image.get_pixel_mut(dest_x as u32, dest_y as u32);
            blend_pixel(&mut pixel.0, stamp.pixels[index], stamp.pixels[index + 1], stamp.pixels[index + 2], alpha);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::blend_pixel;

    #[test]
    fn straight_red_on_gray_keeps_half_the_color() {
        let mut pixel = [100, 100, 100, 255];
        blend_pixel(&mut pixel, 255, 0, 0, 128);
        assert!((pixel[0] as i32 - 177).abs() <= 1, "red {}", pixel[0]);
        assert!((pixel[1] as i32 - 50).abs() <= 1, "green {}", pixel[1]);
        assert_eq!(pixel[3], 255);
    }

    #[test]
    fn premultiplied_red_does_not_wrap() {
        let mut pixel = [255, 255, 255, 255];
        blend_pixel(&mut pixel, 128, 0, 0, 128);
        assert!(pixel[0] > 180, "red {}", pixel[0]);
        assert!(pixel[1] < 140, "green {}", pixel[1]);
        assert_eq!(pixel[3], 255);
    }

    #[test]
    fn zero_alpha_leaves_the_pixel() {
        let mut pixel = [10, 20, 30, 255];
        blend_pixel(&mut pixel, 255, 255, 255, 0);
        assert_eq!(pixel, [10, 20, 30, 255]);
    }

    #[cfg(windows)]
    #[test]
    fn painting_the_cursor_does_not_panic() {
        let mut image = image::RgbaImage::new(80, 60);
        super::paint_cursor(&mut image, 0, 0);
    }
}
