//! Where the cursor is drawn while it catches up with the cell it is really
//! in, behind `[ui.cursor] animate`.
//!
//! kitty draws its trail in a shader and neovide springs the cursor quad's
//! four corners; this painter draws one rect and glides it. Two of their rules
//! survive the simplification: a jump shorter than a few cells is not worth
//! animating (kitty's `cursor_trail_start_threshold`), and a destination that
//! moves mid-flight replays from where the eye last saw the cursor.

use std::time::Instant;

use crate::config::CursorMotion;

/// Fractional column and row in the viewport.  Whole numbers are cell corners,
/// which is where the cursor sits whenever it is not moving.
pub(crate) type CellPos = (f32, f32);

#[derive(Debug, Default)]
pub(crate) struct CursorAnimation {
    glide: Option<Glide>,
}

#[derive(Debug)]
struct Glide {
    from: CellPos,
    to: CellPos,
    at: CellPos,
    started: Instant,
    /// Scrollback position the stored cells were recorded against.  Scrolling
    /// renumbers every row at once, and sliding the cursor along with a jump
    /// the whole screen made is not an animation of anything.
    display_offset: i32,
}

impl CursorAnimation {
    /// Place the cursor for this frame.  `target` is the cell the terminal has
    /// it in, or `None` while it is hidden or scrolled out of view, which also
    /// clears the animation so its next appearance starts where it appears.
    pub(crate) fn place(
        &mut self,
        motion: &CursorMotion,
        target: Option<CellPos>,
        display_offset: i32,
        now: Instant,
    ) -> Option<CellPos> {
        let Some(target) = target else {
            self.glide = None;
            return None;
        };
        if !motion.animate || motion.duration.is_zero() {
            self.glide = None;
            return Some(target);
        }

        let glide = self.glide.get_or_insert_with(|| Glide::parked(target, now, display_offset));
        glide.follow_scroll(display_offset);
        if glide.to != target {
            *glide = if jump_cells(glide.to, target) < motion.min_cells {
                Glide::parked(target, now, display_offset)
            } else {
                Glide { from: glide.at, to: target, at: glide.at, started: now, display_offset }
            };
        }

        let elapsed = now.saturating_duration_since(glide.started);
        glide.at = if elapsed >= motion.duration {
            // Landed exactly, so `settled` can compare rather than measure.
            glide.to
        } else {
            let t = elapsed.as_secs_f32() / motion.duration.as_secs_f32();
            lerp(glide.from, glide.to, ease_out_cubic(t))
        };
        Some(glide.at)
    }

    /// Whether the cursor has reached its cell.  A frame that says no has to
    /// ask for another one: nothing else wakes egui while the cursor moves on
    /// its own.
    pub(crate) fn settled(&self) -> bool {
        self.glide.as_ref().is_none_or(|glide| glide.at == glide.to)
    }
}

impl Glide {
    fn parked(at: CellPos, started: Instant, display_offset: i32) -> Self {
        Self { from: at, to: at, at, started, display_offset }
    }

    fn follow_scroll(&mut self, display_offset: i32) {
        let rows = (display_offset - self.display_offset) as f32;
        if rows == 0.0 {
            return;
        }
        self.from.1 += rows;
        self.to.1 += rows;
        self.at.1 += rows;
        self.display_offset = display_offset;
    }
}

/// How far the cursor jumped, in cells.  The larger of the two axes rather
/// than the diagonal, so one threshold reads the same whichever way it moved.
fn jump_cells(from: CellPos, to: CellPos) -> f32 {
    (to.0 - from.0).abs().max((to.1 - from.1).abs())
}

fn lerp(from: CellPos, to: CellPos, t: f32) -> CellPos {
    (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
}

/// Fast off the mark and easing into the destination, which is what makes a
/// short glide read as the cursor arriving rather than as lag.
fn ease_out_cubic(t: f32) -> f32 {
    let n = t - 1.0;
    n * n * n + 1.0
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const DURATION: Duration = Duration::from_millis(100);

    fn motion(animate: bool) -> CursorMotion {
        CursorMotion { animate, duration: DURATION, min_cells: 2.0 }
    }

    #[test]
    fn the_first_frame_draws_the_cursor_where_it_is() {
        let mut anim = CursorAnimation::default();
        let now = Instant::now();
        assert_eq!(anim.place(&motion(true), Some((4.0, 2.0)), 0, now), Some((4.0, 2.0)));
        assert!(anim.settled());
    }

    #[test]
    fn a_jump_lands_behind_the_cell_it_is_heading_for() {
        let mut anim = CursorAnimation::default();
        let now = Instant::now();
        let cfg = motion(true);
        anim.place(&cfg, Some((0.0, 0.0)), 0, now);
        // The frame that notices the move starts the clock and still draws the
        // old cell, so the one after it is the first that has moved.
        anim.place(&cfg, Some((20.0, 0.0)), 0, now);

        let drawn = anim.place(&cfg, Some((20.0, 0.0)), 0, now + DURATION / 4).unwrap();
        assert!(drawn.0 > 0.0, "the cursor left its old cell");
        assert!(drawn.0 < 20.0, "and has not arrived yet");
        assert!(!anim.settled());
    }

    #[test]
    fn the_glide_ends_on_the_cell_exactly() {
        let mut anim = CursorAnimation::default();
        let now = Instant::now();
        let cfg = motion(true);
        anim.place(&cfg, Some((0.0, 0.0)), 0, now);
        anim.place(&cfg, Some((20.0, 0.0)), 0, now);

        anim.place(&cfg, Some((20.0, 0.0)), 0, now + DURATION / 2);
        assert_eq!(anim.place(&cfg, Some((20.0, 0.0)), 0, now + DURATION), Some((20.0, 0.0)));
        assert!(anim.settled());
    }

    #[test]
    fn a_short_hop_snaps() {
        let mut anim = CursorAnimation::default();
        let now = Instant::now();
        let cfg = motion(true);
        anim.place(&cfg, Some((0.0, 0.0)), 0, now);

        assert_eq!(anim.place(&cfg, Some((1.0, 0.0)), 0, now), Some((1.0, 0.0)));
        assert!(anim.settled());
    }

    #[test]
    fn a_destination_that_moves_mid_glide_replays_from_the_drawn_cell() {
        let mut anim = CursorAnimation::default();
        let now = Instant::now();
        let cfg = motion(true);
        anim.place(&cfg, Some((0.0, 0.0)), 0, now);
        anim.place(&cfg, Some((40.0, 0.0)), 0, now);

        let midway = anim.place(&cfg, Some((40.0, 0.0)), 0, now + DURATION / 2).unwrap();
        let redirected = anim.place(&cfg, Some((0.0, 10.0)), 0, now + DURATION / 2).unwrap();
        assert_eq!(redirected, midway, "the new glide starts where the last frame drew it");
    }

    #[test]
    fn scrolling_moves_the_cursor_without_animating_it() {
        let mut anim = CursorAnimation::default();
        let now = Instant::now();
        let cfg = motion(true);
        anim.place(&cfg, Some((3.0, 20.0)), 0, now);

        // Ten lines of scrollback push every row down by ten.
        assert_eq!(anim.place(&cfg, Some((3.0, 30.0)), 10, now), Some((3.0, 30.0)));
        assert!(anim.settled());
    }

    #[test]
    fn the_option_being_off_draws_every_cell_directly() {
        let mut anim = CursorAnimation::default();
        let now = Instant::now();
        let cfg = motion(false);
        anim.place(&cfg, Some((0.0, 0.0)), 0, now);
        assert_eq!(anim.place(&cfg, Some((40.0, 0.0)), 0, now), Some((40.0, 0.0)));
        assert!(anim.settled());
    }

    #[test]
    fn a_zero_duration_draws_every_cell_directly() {
        let mut anim = CursorAnimation::default();
        let now = Instant::now();
        let mut cfg = motion(true);
        cfg.duration = Duration::ZERO;
        anim.place(&cfg, Some((0.0, 0.0)), 0, now);
        assert_eq!(anim.place(&cfg, Some((40.0, 0.0)), 0, now), Some((40.0, 0.0)));
        assert!(anim.settled());
    }

    #[test]
    fn a_hidden_cursor_reappears_where_it_reappears() {
        let mut anim = CursorAnimation::default();
        let now = Instant::now();
        let cfg = motion(true);
        anim.place(&cfg, Some((0.0, 0.0)), 0, now);
        assert_eq!(anim.place(&cfg, None, 0, now), None);
        assert_eq!(anim.place(&cfg, Some((70.0, 20.0)), 0, now), Some((70.0, 20.0)));
        assert!(anim.settled());
    }
}
