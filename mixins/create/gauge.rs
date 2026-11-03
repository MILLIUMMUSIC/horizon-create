// Create's speedometer and stressometer, ported from Create 6.0.10 (MIT, The
// Create Team): GaugeBlockEntity's dial (its target by speed or stress, its
// chase), SpeedGaugeBlockEntity.getDialTarget, StressGaugeBlockEntity's
// target and colour, and GaugeBlock.shouldRenderHeadOnFace. Injected as
// `crate::game::create_gauge`; no Horizon types, tested with `cargo test`.

use super::create_kinetics::{Axis, Dir};

/// `CKinetics.mediumSpeed`, `fastSpeed` and `maxRotationSpeed` defaults.
const MEDIUM_SPEED: f32 = 30.0;
const FAST_SPEED: f32 = 100.0;
const MAX_ROTATION_SPEED: f32 = 256.0;

fn lerp(t: f32, a: f32, b: f32) -> f32 {
    a + (b - a) * t
}

/// `SpeedGaugeBlockEntity.getDialTarget`.
pub fn speed_dial_target(speed: f32) -> f32 {
    let speed = speed.abs();
    if speed == 0.0 {
        0.0
    } else if speed < MEDIUM_SPEED {
        lerp(speed / MEDIUM_SPEED, 0.0, 0.45)
    } else if speed < FAST_SPEED {
        lerp(
            (speed - MEDIUM_SPEED) / (FAST_SPEED - MEDIUM_SPEED),
            0.45,
            0.75,
        )
    } else {
        lerp(
            (speed - FAST_SPEED) / (MAX_ROTATION_SPEED - FAST_SPEED),
            0.75,
            1.125,
        )
    }
}

/// `StressGaugeBlockEntity.updateFromNetwork` and `onSpeedChanged`: the dial
/// follows the network's stress over its capacity (1.125 when overstressed,
/// nothing while it doesn't turn).
pub fn stress_dial_target(speed: f32, stress: f32, capacity: f32) -> f32 {
    if speed == 0.0 {
        return 0.0;
    }
    if capacity < stress {
        1.125
    } else if capacity == 0.0 {
        0.0
    } else {
        stress / capacity
    }
}

/// `Color.mixColors(a, b, w)` on 0xRRGGBB.
#[allow(dead_code)]
fn mix(a: u32, b: u32, w: f32) -> u32 {
    let channel = |shift: u32| {
        let (x, y) = (((a >> shift) & 255) as f32, ((b >> shift) & 255) as f32);
        ((x + (y - x) * w).round() as u32).min(255) << shift
    };
    channel(16) | channel(8) | channel(0)
}

/// The stress gauge's colour for a dial `target` above nothing (for its
/// particles, which aren't ported).
#[allow(dead_code)]
pub fn stress_color(target: f32) -> u32 {
    if target < 0.5 {
        mix(0x00FF00, 0xFFFF00, target * 2.0)
    } else if target < 1.0 {
        mix(0xFFFF00, 0xFF0000, target * 2.0 - 1.0)
    } else {
        0xFF0000
    }
}

/// One tick of `GaugeBlockEntity.tick`: the dial moves an eighth of the way
/// to its target. Ticks (also fractions of one) are applied together.
pub fn chase(state: f32, target: f32, ticks: f32) -> f32 {
    let keep = (1.0f32 - 0.125).powf(ticks);
    target + (state - target) * keep
}

/// `GaugeBlock.shouldRenderHeadOnFace` (with `open`: whether the block beside
/// `face` leaves the face showing).
pub fn shows_head(facing: Dir, rotation_axis: Axis, face: Dir, open: bool) -> bool {
    if face.axis() == Axis::Y {
        return false;
    }
    if face == facing.opposite() {
        return false;
    }
    if face.axis() == rotation_axis {
        return false;
    }
    if rotation_axis == Axis::Y && face != facing {
        return false;
    }
    open
}

/// `Direction.toYRot`.
pub fn y_rot(face: Dir) -> f32 {
    match face {
        Dir::South => 0.0,
        Dir::West => 90.0,
        Dir::North => 180.0,
        Dir::East => 270.0,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_speed_dial_follows_create_s_bands() {
        assert_eq!(speed_dial_target(0.0), 0.0);
        assert!((speed_dial_target(15.0) - 0.225).abs() < 1e-6);
        assert!((speed_dial_target(-30.0) - 0.45).abs() < 1e-6);
        assert!((speed_dial_target(100.0) - 0.75).abs() < 1e-6);
        assert!((speed_dial_target(256.0) - 1.125).abs() < 1e-6);
    }

    #[test]
    fn the_stress_dial_is_the_load_and_overstress_pegs_it() {
        assert_eq!(stress_dial_target(0.0, 5.0, 10.0), 0.0);
        assert_eq!(stress_dial_target(16.0, 5.0, 10.0), 0.5);
        assert_eq!(stress_dial_target(16.0, 11.0, 10.0), 1.125);
        assert_eq!(stress_color(0.0), 0x00FF00);
        assert_eq!(stress_color(0.5), 0xFFFF00);
        assert_eq!(stress_color(1.2), 0xFF0000);
    }

    #[test]
    fn the_dial_chases_an_eighth_a_tick() {
        assert!((chase(0.0, 1.0, 1.0) - 0.125).abs() < 1e-6);
        assert!((chase(0.0, 1.0, 2.0) - (1.0 - 0.875 * 0.875)).abs() < 1e-6);
    }

    #[test]
    fn heads_sit_on_the_sides_the_rules_allow() {
        // Facing up, turning about X: the heads on the north and south sides.
        assert!(shows_head(Dir::Up, Axis::X, Dir::North, true));
        assert!(!shows_head(Dir::Up, Axis::X, Dir::East, true));
        // Turning about Y, only the facing side.
        assert!(shows_head(Dir::East, Axis::Y, Dir::East, true));
        assert!(!shows_head(Dir::East, Axis::Y, Dir::North, true));
        assert!(!shows_head(Dir::East, Axis::Z, Dir::West, true));
        assert!(!shows_head(Dir::Up, Axis::X, Dir::North, false));
    }
}
