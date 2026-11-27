//! How the agent's cursor travels between targets: a gentle arc rather than a
//! straight jump, eased in and out, taking longer for longer moves (Fitts'
//! law), so the user can follow where the agent is going. Platform-free; the
//! macOS overlay samples it once per frame.

/// A point in global screen points, top-left origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// The frames per second the overlay plays a glide at.
pub const FRAMES_PER_SECOND: f64 = 60.0;

const SHORTEST_MS: f64 = 220.0;
const LONGEST_MS: f64 = 720.0;
const BASE_MS: f64 = 150.0;
const MS_PER_BIT: f64 = 110.0;
const TARGET_WIDTH: f64 = 24.0;
const BEND: f64 = 0.16;
const MAX_BEND: f64 = 110.0;
const STILL: f64 = 1.0;

/// One planned move from `from` to `to`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glide {
    from: Point,
    control: Point,
    to: Point,
    /// How long the move plays, in milliseconds; zero when there is nothing to move.
    pub millis: f64,
}

impl Glide {
    /// Plan a move. `bend_left` picks which side of the straight line the arc
    /// bows to, so consecutive moves do not all curve the same way.
    pub fn plan(from: Point, to: Point, bend_left: bool) -> Glide {
        let (dx, dy) = (to.x - from.x, to.y - from.y);
        let distance = dx.hypot(dy);
        if distance < STILL {
            return Glide {
                from,
                control: to,
                to,
                millis: 0.0,
            };
        }
        let bits = (1.0 + distance / TARGET_WIDTH).log2();
        let millis = (BASE_MS + MS_PER_BIT * bits).clamp(SHORTEST_MS, LONGEST_MS);
        let bow = (distance * BEND).min(MAX_BEND) * if bend_left { 1.0 } else { -1.0 };
        let (nx, ny) = (-dy / distance, dx / distance);
        let control = Point {
            x: (from.x + to.x) / 2.0 + nx * bow,
            y: (from.y + to.y) / 2.0 + ny * bow,
        };
        Glide {
            from,
            control,
            to,
            millis,
        }
    }

    /// Where the cursor is `elapsed_ms` into the move.
    pub fn at(&self, elapsed_ms: f64) -> Point {
        if self.millis <= 0.0 {
            return self.to;
        }
        let t = ease_in_out(elapsed_ms / self.millis);
        let u = 1.0 - t;
        Point {
            x: u * u * self.from.x + 2.0 * u * t * self.control.x + t * t * self.to.x,
            y: u * u * self.from.y + 2.0 * u * t * self.control.y + t * t * self.to.y,
        }
    }

    /// The points to show, one per frame, ending exactly on the target.
    pub fn frames(&self) -> Vec<Point> {
        let count = (self.millis / 1000.0 * FRAMES_PER_SECOND).ceil() as usize;
        let step = 1000.0 / FRAMES_PER_SECOND;
        let mut frames: Vec<Point> = (1..count).map(|i| self.at(i as f64 * step)).collect();
        frames.push(self.to);
        frames
    }
}

/// Cubic ease-in-out of `t` in 0..=1: slow to start, quick through the
/// middle, settling gently on the target.
pub fn ease_in_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: Point = Point { x: 0.0, y: 0.0 };

    #[test]
    fn longer_moves_take_longer_within_bounds() {
        let near = Glide::plan(ORIGIN, Point { x: 40.0, y: 0.0 }, true);
        let far = Glide::plan(
            ORIGIN,
            Point {
                x: 1200.0,
                y: 600.0,
            },
            true,
        );
        assert!(near.millis >= SHORTEST_MS);
        assert!(far.millis > near.millis);
        assert!(far.millis <= LONGEST_MS);
    }

    #[test]
    fn a_glide_starts_on_its_origin_and_ends_on_its_target() {
        let to = Point { x: 500.0, y: 300.0 };
        let glide = Glide::plan(ORIGIN, to, false);
        assert_eq!(glide.at(0.0), ORIGIN);
        assert_eq!(glide.at(glide.millis), to);
        assert_eq!(*glide.frames().last().unwrap(), to);
    }

    #[test]
    fn the_path_bows_to_the_chosen_side() {
        let to = Point { x: 400.0, y: 0.0 };
        let middle =
            |left| Glide::plan(ORIGIN, to, left).at(Glide::plan(ORIGIN, to, left).millis / 2.0);
        assert!(middle(true).y > 1.0);
        assert!(middle(false).y < -1.0);
    }

    #[test]
    fn staying_put_is_instant() {
        let glide = Glide::plan(ORIGIN, Point { x: 0.4, y: 0.0 }, true);
        assert_eq!(glide.millis, 0.0);
        assert_eq!(glide.frames().len(), 1);
    }

    #[test]
    fn easing_is_symmetric_and_bounded() {
        assert_eq!(ease_in_out(0.0), 0.0);
        assert_eq!(ease_in_out(1.0), 1.0);
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-9);
        assert!((ease_in_out(0.25) + ease_in_out(0.75) - 1.0).abs() < 1e-9);
    }
}
