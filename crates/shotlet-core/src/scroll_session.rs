use serde::{Deserialize, Serialize};

use crate::scroll::{self, ScrollResult};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollRequest {
    pub screen_x: i32,
    pub screen_y: i32,
    pub screen_w: u32,
    pub screen_h: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub image_w: u32,
    pub image_h: u32,
    pub point_w: u32,
    pub point_h: u32,
}

#[derive(Debug)]
pub struct ScrollFrame {
    pub screen_x: i32,
    pub screen_y: i32,
    pub screen_w: u32,
    pub screen_h: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub visual_x: i32,
    pub visual_y: i32,
    pub visual_w: u32,
    pub visual_h: u32,
}

/// Open the scroll gate. The shell calls [`end`] after its windows are closed.
pub fn prepare(request: &ScrollRequest) -> Result<ScrollFrame, String> {
    let x = request.x.max(0) as u32;
    let y = request.y.max(0) as u32;
    let width = request.width;
    let height = request.height;
    if width < 20 || height < 20 {
        return Err("滚动选区太小".into());
    }
    scroll::begin()?;
    let (visual_x, visual_y, visual_w, visual_h) = crate::capture_session::desktop_rect(
        request.screen_x,
        request.screen_y,
        x as i32,
        y as i32,
        width,
        height,
        request.image_w,
        request.image_h,
        request.point_w,
        request.point_h,
    );
    Ok(ScrollFrame {
        screen_x: request.screen_x,
        screen_y: request.screen_y,
        screen_w: request.screen_w,
        screen_h: request.screen_h,
        x,
        y,
        width,
        height,
        visual_x,
        visual_y,
        visual_w,
        visual_h,
    })
}

pub fn capture(frame: ScrollFrame) -> Result<ScrollResult, String> {
    scroll::capture_manual(frame.screen_x, frame.screen_y, frame.x, frame.y, frame.width, frame.height)
}

pub fn end() {
    scroll::end();
}

pub fn finish_request(action: &str) {
    scroll::request_finish(action);
}

pub fn cancel_request() {
    scroll::request_cancel();
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollStatus {
    pub height: u32,
    pub reverse: bool,
    pub auto: bool,
    pub can_auto: bool,
}

pub fn status() -> ScrollStatus {
    ScrollStatus {
        height: scroll::progress(),
        reverse: scroll::reversed(),
        auto: scroll::auto_enabled(),
        can_auto: scroll::can_auto(),
    }
}

pub fn set_auto(on: bool) {
    scroll::set_auto(on);
}

pub fn take_action() -> String {
    scroll::take_action()
}

pub fn is_cancelled(error: &str) -> bool {
    error == scroll::CANCELLED
}

#[cfg(test)]
mod tests {
    use super::{end, is_cancelled, prepare, ScrollRequest};

    fn request(width: u32, height: u32) -> ScrollRequest {
        ScrollRequest {
            screen_x: 5,
            screen_y: 6,
            screen_w: 800,
            screen_h: 600,
            x: -4,
            y: 0,
            width,
            height,
            image_w: 40,
            image_h: 40,
            point_w: 80,
            point_h: 80,
        }
    }

    #[test]
    fn a_small_selection_is_rejected_before_capture_starts() {
        assert_eq!(prepare(&request(19, 40)).unwrap_err(), "滚动选区太小");
        assert!(is_cancelled("滚动截图已取消"));
        assert!(!is_cancelled("滚动选区太小"));
    }

    #[test]
    fn prepare_maps_image_pixels_into_the_visual_rect() {
        let frame = prepare(&request(20, 20)).unwrap();
        assert_eq!(frame.x, 0);
        assert_eq!((frame.visual_x, frame.visual_y, frame.visual_w, frame.visual_h), (5, 6, 40, 40));
        end();
    }
}
