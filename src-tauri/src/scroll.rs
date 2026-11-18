use image::RgbaImage;
use std::thread;
use std::time::Duration;

use crate::capture::{self, capture_rect};
use crate::stitch::append_scrolled;
use crate::util::{new_id, write_png};

pub struct ScrollResult {
    pub path: String,
    pub width: u32,
    pub height: u32,
}

pub fn capture_scrolled(x: i32, y: i32, width: u32, height: u32) -> Result<ScrollResult, String> {
    if width < 20 || height < 20 {
        return Err("滚动选区太小".into());
    }
    let mut current = capture_rect(x, y, width, height)?;
    let center_x = x + width as i32 / 2;
    let center_y = y + height as i32 / 2;
    for _ in 0..18 {
        if current.height() > 14000 {
            break;
        }
        wheel_at(center_x, center_y);
        thread::sleep(Duration::from_millis(320));
        let next = capture_rect(x, y, width, height)?;
        let (stitched, grew) = append_scrolled(&current, &next);
        if !grew {
            break;
        }
        current = stitched;
    }
    save_long(current)
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

fn wheel_at(x: i32, y: i32) {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::{mouse_event, MOUSEEVENTF_WHEEL};
        use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;
        unsafe {
            let _ = SetCursorPos(x, y);
            mouse_event(MOUSEEVENTF_WHEEL, 0, 0, -360, 0);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (x, y);
    }
}
