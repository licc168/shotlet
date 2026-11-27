//! No wheel injection. This file is the `wheel` module outside Windows.

pub(super) fn release_cursor() {}

pub(super) fn use_keys() {}

pub(super) fn reset_step() {}

pub(super) fn take_prior() -> i32 {
    0
}

pub(super) fn nudge(_screen_x: i32, _screen_y: i32, _x: u32, _y: u32, _width: u32, _height: u32) -> bool {
    false
}
