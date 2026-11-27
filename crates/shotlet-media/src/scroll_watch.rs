//! Scroll capture. The user drags the scrollbar unless they turn auto-scroll on.
//! Auto-scroll asks `wheel::nudge`, which is a no-op outside Windows.

use image::RgbaImage;
use shotlet_capture::capture_local;
use shotlet_image::frames_stable;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::{save_long, tick, Esc, Roll, ScrollResult, Tick, CANCELLED, GATE, MAX_WAIT, REVERSED};

pub(super) fn run(screen_x: i32, screen_y: i32, x: u32, y: u32, width: u32, height: u32) -> Result<ScrollResult, String> {
    let gate = GATE.lock().map_err(|error| error.to_string())?.clone().ok_or("滚动截图没有开始")?;
    let baseline = capture_local(screen_x, screen_y, x, y, width, height)?;
    let mut roll = Roll::new(baseline);
    let mut esc = Esc::snapshot();
    let deadline = Instant::now() + MAX_WAIT;
    let mut still = 0u32;
    let mut moved = 0u32;
    let mut told_still = false;
    let mut quiet = 0u32;
    super::wheel::reset_step();
    let _put_back = PutBack;
    loop {
        match tick(&gate, &mut esc) {
            Tick::Cancel => return Err(CANCELLED.into()),
            Tick::Finish => break,
            Tick::Continue => {}
        }
        if roll.full() || Instant::now() >= deadline {
            break;
        }
        if super::auto_enabled() && super::wheel::nudge(screen_x, screen_y, x, y, width, height) {
            super::set_scroll_prior(super::wheel::take_prior());
            // ShareX waits out one notch before comparing. A page that pauses to
            // load is not the bottom, and one unmatched frame must not stop the run.
            match follow_notch(&gate, &mut esc, &mut roll, screen_x, screen_y, x, y, width, height) {
                Motion::Cancel => return Err(CANCELLED.into()),
                Motion::Finish => break,
                Motion::Grew => {
                    quiet = 0;
                    told_still = false;
                    moved += 1;
                    REVERSED.store(roll.reversed(), Ordering::Relaxed);
                }
                Motion::Still => {
                    super::wheel::use_keys();
                    quiet += 1;
                    // Only a page that no longer moves is the bottom. A frame that
                    // changed but did not stitch is a miss in the middle, not the end.
                    if quiet >= 6 {
                        eprintln!("scroll: stopped at the end");
                        super::set_auto(false);
                        quiet = 0;
                    }
                }
                Motion::Unmatched => {
                    quiet = 0;
                }
            }
            continue;
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

struct PutBack;

impl Drop for PutBack {
    fn drop(&mut self) {
        super::wheel::release_cursor();
    }
}

enum Motion {
    Grew,
    Still,
    Unmatched,
    Finish,
    Cancel,
}

/// One wheel notch. Wait until the page sits still, then stitch that one frame.
/// Stitching while it is still moving repeats the same rows.
fn follow_notch(
    gate: &super::Gate,
    esc: &mut Esc,
    roll: &mut Roll,
    screen_x: i32,
    screen_y: i32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Motion {
    let before = roll.height();
    let started = Instant::now();
    let limit = Duration::from_millis(280);
    let mut previous: Option<RgbaImage> = None;
    let mut changed = false;
    while started.elapsed() < limit {
        match tick(gate, esc) {
            Tick::Cancel => return Motion::Cancel,
            Tick::Finish => return Motion::Finish,
            Tick::Continue => {}
        }
        let Ok(frame) = capture_local(screen_x, screen_y, x, y, width, height) else {
            continue;
        };
        let settled = previous.as_ref().is_some_and(|prior| frames_stable(prior, &frame));
        if !roll.same(&frame) {
            changed = true;
        }
        let elapsed = started.elapsed();
        previous = Some(frame);
        if changed && settled && elapsed >= Duration::from_millis(90) {
            break;
        }
        if !changed && elapsed >= Duration::from_millis(140) {
            break;
        }
    }
    let Some(frame) = previous else {
        return Motion::Still;
    };
    if !changed {
        return Motion::Still;
    }
    roll.absorb(frame.clone());
    if roll.height() == before {
        roll.absorb_auto(frame);
    }
    if roll.height() > before { Motion::Grew } else { Motion::Unmatched }
}
