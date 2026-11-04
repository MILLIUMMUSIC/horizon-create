// Create's weighted ejector: `EntityLauncher`'s arc, `EjectorBlockEntity`'s lid
// and `EjectorTargetHandler`'s choice of target. No Horizon types.

use super::create_kinetics::{Axis, Dir, Pos};

/// `CKinetics.maxEjectorDistance`.
pub const MAX_DISTANCE: i32 = 32;
/// `CKinetics.ejectorScanInterval`.
pub const SCAN_INTERVAL: i32 = 120;

/// `VecHelper.rotate` about Y.
fn rotate_y(v: [f64; 3], degrees: f32) -> [f64; 3] {
    let (sin, cos) = f64::from(degrees.to_radians()).sin_cos();
    [v[0] * cos + v[2] * sin, v[1], v[2] * cos - v[0] * sin]
}

/// `EntityLauncher`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Launcher {
    pub horizontal: i32,
    pub vertical: i32,
    y_motion: f64,
    x_motion: f64,
    pub total_ticks: f64,
}

impl Launcher {
    /// `set`, with `recalculateTrajectory`.
    pub fn new(horizontal: i32, vertical: i32) -> Launcher {
        let x_target = f64::from(horizontal);
        let y_target = f64::from(vertical);
        let x_error = (-0.0007 * (x_target + 0.5).powi(2) + 0.484)
            - (y_target.min(5.0) / 5.0)
                * (0.076 * x_target - 0.0014 * x_target * x_target).min(1.0);
        let y_peak =
            (y_target + (x_target + 0.5) / 8.0).max(0.0) + if x_target <= 1.0 { 1.0 } else { 4.0 };
        let y_motion = (2.0 * y_peak / 13.0).sqrt() + 0.015 * y_peak;
        let t_peak = (98.0 / (25.0 * y_motion + 98.0)).ln()
            / (2.0 * 7f64.ln() - 2.0 * 5f64.ln() - 2f64.ln());
        let total_ticks = (y_peak - y_motion).sqrt() * 4.87 + 0.115 * (y_peak - y_motion) + t_peak;
        let x_motion = (x_target - x_error + 0.5) / (-10.6033 * (-1.0 + 0.91f64.powf(total_ticks)));
        Launcher {
            horizontal,
            vertical,
            y_motion,
            x_motion,
            total_ticks,
        }
    }
    fn x(&self, t: f64) -> f64 {
        self.x_motion * -10.6033 * (-1.0 + 0.91f64.powf(t))
    }
    fn y(&self, t: f64) -> f64 {
        let f = 0.98f64.powf(t);
        self.y_motion * -49.4983 * f + 49.4983 * self.y_motion - 194.033 * f - 3.92 * t + 194.033
    }
    /// `getGlobalPos(t, facing.getOpposite(), launcher)`.
    pub fn position(&self, t: f64, facing: Dir, at: Pos) -> [f64; 3] {
        let (xt, yt) = (self.x(t), self.y(t));
        let progress = (t / self.total_ticks).clamp(0.0, 1.0);
        let correction = progress.powi(3);
        let v = [
            0.0,
            yt + (f64::from(self.vertical) - yt) * correction * 0.5,
            xt + (f64::from(self.horizontal) - xt) * correction,
        ];
        let r = rotate_y(v, 180.0 + facing.opposite().horizontal_angle());
        [
            r[0] + f64::from(at[0]) + 0.5,
            r[1] + f64::from(at[1]) + 0.5,
            r[2] + f64::from(at[2]) + 0.5,
        ]
    }
    /// `getLaunchedItemMotion`.
    pub fn item_motion(&self, t: f64, facing: Dir) -> [f64; 3] {
        let f = 0.98f64.powf(t);
        let dy = self.y_motion * f + ((f - 1.0) / (0.98 - 1.0)) * -0.0784;
        let dx = self.x_motion * 0.91f64.powf(t);
        rotate_y([0.0, dy, dx], 180.0 + facing.opposite().horizontal_angle()).map(|v| v * 0.5)
    }
    /// `applyMotion`.
    pub fn entity_motion(&self, facing: Dir) -> [f64; 3] {
        let v = rotate_y(
            [0.0, self.y_motion, self.x_motion],
            facing.horizontal_angle(),
        );
        [v[0] * 0.91, v[1] * 0.98, v[2] * 0.91]
    }
    /// The block `getTargetOpenInv` looks at.
    pub fn target(&self, facing: Dir, at: Pos) -> Pos {
        let n = facing.normal();
        let h = self.horizontal.max(1);
        [at[0] + n[0] * h, at[1] + self.vertical, at[2] + n[2] * h]
    }
}

/// `EjectorBlockEntity.State`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Charged,
    Launching,
    Retracting,
}

impl State {
    pub fn code(self) -> i32 {
        match self {
            State::Charged => 0,
            State::Launching => 1,
            State::Retracting => 2,
        }
    }
    pub fn of(code: i32) -> State {
        match code {
            0 => State::Charged,
            1 => State::Launching,
            _ => State::Retracting,
        }
    }
}

/// `getWindUpSpeed`.
pub fn wind_up_speed(speed: f32, horizontal: i32, vertical: i32) -> f32 {
    let factor = speed.abs() / 256.0;
    if horizontal == 0 && vertical == 0 {
        factor
    } else {
        factor / ((horizontal * horizontal + vertical * vertical) as f32).sqrt()
    }
}

/// The lid's part of `tick`: from its state and progress (0 down, 1 up) to the
/// next, `wind_up` its speed.
pub fn lid_tick(state: State, lid: f32, wind_up: f32) -> (State, f32) {
    match state {
        State::Charged => (State::Charged, 0.0),
        State::Launching => {
            let lid = lid + (1.0 - lid) * 0.8;
            if lid > 1.0 - 1.0 / 16.0 {
                (State::Retracting, 1.0)
            } else {
                (State::Launching, lid)
            }
        }
        State::Retracting => {
            let next = if lid == 0.0 {
                State::Charged
            } else {
                State::Retracting
            };
            (next, (lid - wind_up).clamp(0.0, 1.0))
        }
    }
}

/// `getValidTargetDirection`: the way an ejector at `pos` faces to reach
/// `target`, on a line with it and in range.
pub fn target_direction(pos: Pos, target: Pos) -> Option<Dir> {
    let (x, z) = (target[0] - pos[0], target[2] - pos[2]);
    if x == 0 && z == 0 {
        return None;
    }
    if x.abs() > MAX_DISTANCE || z.abs() > MAX_DISTANCE {
        return None;
    }
    if x == 0 {
        return Some(if z < 0 { Dir::North } else { Dir::South });
    }
    if z == 0 {
        return Some(if x < 0 { Dir::West } else { Dir::East });
    }
    None
}

/// `flushSettings`: the horizontal and vertical distances of `target`.
pub fn target_distances(pos: Pos, target: Pos) -> (i32, i32) {
    (
        ((target[0] - pos[0]) + (target[2] - pos[2])).abs(),
        target[1] - pos[1],
    )
}

/// `EjectorSlot`: where its value box is on `side`, in the block; none where it
/// has none (the top only while it isn't charged).
pub fn value_box(facing: Dir, state: State, side: Dir) -> Option<[f32; 3]> {
    if side == Dir::Up {
        if state == State::Charged {
            return None;
        }
        let v = rotate_y([0.0, 0.0, -5.0 / 16.0], facing.horizontal_angle());
        return Some([0.5 + v[0] as f32, 10.5 / 16.0, 0.5 + v[2] as f32]);
    }
    if side.axis() != facing.axis() || side.axis() == Axis::Y {
        return None;
    }
    let v = rotate_y(
        [0.0, 6.0 / 16.0 - 0.5, 15.5 / 16.0 - 0.5],
        side.horizontal_angle(),
    );
    Some([0.5 + v[0] as f32, 0.5 + v[1] as f32, 0.5 + v[2] as f32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arc_ends_near_its_target() {
        for (h, v) in [(1, 0), (4, 0), (8, 2), (16, 5), (32, 0), (6, -3)] {
            let l = Launcher::new(h, v);
            let end = l.position(l.total_ticks, Dir::East, [0, 64, 0]);
            assert!(
                (end[0] - (f64::from(h) + 0.5)).abs() < 0.01,
                "{h} {v}: {end:?}"
            );
            assert!((end[2] - 0.5).abs() < 1e-5);
            // Only half the height error is corrected: what catches it is the target.
            assert!(
                (end[1] - (f64::from(64 + v) + 0.5)).abs() < 2.5,
                "{h} {v}: {end:?}"
            );
            let start = l.position(0.0, Dir::East, [0, 64, 0]);
            assert!((start[0] - 0.5).abs() < 1e-3 && (start[1] - 64.5).abs() < 1e-3);
            assert!(l.total_ticks > 3.0);
        }
    }

    #[test]
    fn the_arc_goes_the_way_it_faces() {
        let l = Launcher::new(4, 0);
        for (d, n) in [
            (Dir::North, [0, -1]),
            (Dir::South, [0, 1]),
            (Dir::West, [-1, 0]),
            (Dir::East, [1, 0]),
        ] {
            let at = l.position(l.total_ticks, d, [0, 0, 0]);
            assert!(
                (at[0] - 0.5 - f64::from(n[0] * 4)).abs() < 0.01,
                "{d:?} {at:?}"
            );
            assert!(
                (at[2] - 0.5 - f64::from(n[1] * 4)).abs() < 0.01,
                "{d:?} {at:?}"
            );
            let m = l.entity_motion(d);
            assert!(m[1] > 0.0);
            assert_eq!(m[0] > 0.01, n[0] > 0);
            assert_eq!(m[2] < -0.01, n[1] < 0);
        }
        assert_eq!(l.target(Dir::West, [0, 5, 0]), [-4, 5, 0]);
    }

    #[test]
    fn the_lid_flies_up_then_winds_down() {
        let (mut s, mut lid) = (State::Launching, 0.0);
        (s, lid) = lid_tick(s, lid, 0.25);
        assert_eq!(s, State::Launching);
        assert!((lid - 0.8).abs() < 1e-6);
        (s, lid) = lid_tick(s, lid, 0.25);
        assert_eq!((s, lid), (State::Retracting, 1.0));
        for want in [0.75, 0.5, 0.25, 0.0] {
            (s, lid) = lid_tick(s, lid, 0.25);
            assert_eq!((s, lid), (State::Retracting, want));
        }
        (s, lid) = lid_tick(s, lid, 0.25);
        assert_eq!((s, lid), (State::Charged, 0.0));
        // No speed: it stays up.
        assert_eq!(
            lid_tick(State::Retracting, 1.0, 0.0),
            (State::Retracting, 1.0)
        );
    }

    #[test]
    fn wind_up_is_slower_for_a_longer_throw() {
        assert_eq!(wind_up_speed(256.0, 0, 0), 1.0);
        assert_eq!(wind_up_speed(-256.0, 3, 4), 0.2);
        assert_eq!(wind_up_speed(0.0, 3, 4), 0.0);
    }

    #[test]
    fn targets_are_on_a_line_and_in_range() {
        assert_eq!(target_direction([0, 0, 0], [0, 5, 5]), Some(Dir::South));
        assert_eq!(target_direction([0, 0, 0], [-3, 0, 0]), Some(Dir::West));
        assert_eq!(target_direction([0, 0, 0], [3, 0, 1]), None);
        assert_eq!(target_direction([0, 0, 0], [0, 4, 0]), None);
        assert_eq!(target_direction([0, 0, 0], [33, 0, 0]), None);
        assert_eq!(target_distances([0, 10, 0], [-3, 12, 0]), (3, 2));
    }

    #[test]
    fn value_boxes() {
        assert!(value_box(Dir::North, State::Charged, Dir::Up).is_none());
        assert!(value_box(Dir::North, State::Retracting, Dir::Up).is_some());
        assert!(value_box(Dir::North, State::Charged, Dir::East).is_none());
        let south = value_box(Dir::North, State::Charged, Dir::South).unwrap();
        assert!((south[2] - 15.5 / 16.0).abs() < 1e-5);
        assert!((south[1] - 6.0 / 16.0).abs() < 1e-5);
    }
}
