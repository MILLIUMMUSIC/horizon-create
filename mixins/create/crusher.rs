// Create's crushing wheels, ported from Create 6.0.10 (MIT, The Create Team):
// CrushingWheelBlock.updateControllers and canSurvive, and
// CrushingWheelControllerBlockEntity's processing timer, where its results go
// and how it throws them out. Injected as `crate::game::create_crusher`; no
// Horizon types, tested with `cargo test`.

use super::create_kinetics::{Axis, Dir, Pos};

/// `ProcessingInventory`'s 32 slots: the input first.
pub const SLOTS: usize = 32;
/// `CrushingWheelControllerBlockEntity.itemInserted` without a recipe.
pub const NO_RECIPE_TIME: f32 = 100.0;

/// What the wheel at a position wants between it and the block two away on
/// `side` (`updateControllers`): none for no controller, else whether it
/// works and the way items go through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub valid: bool,
    pub facing: Dir,
}

/// `CrushingWheelBlock.updateControllers` for one side: `other` is the block
/// two away when it is a crushing wheel (its axis and speed).
pub fn wanted_controller(
    axis: Axis,
    side: Dir,
    speed: f32,
    other: Option<(Axis, f32)>,
) -> Option<Wanted> {
    let (other_axis, other_speed) = other?;
    if other_axis != axis || side.axis() == axis {
        return None;
    }
    let opposite = (speed > 0.0) != (other_speed > 0.0) && speed != 0.0 && other_speed != 0.0;
    if !opposite {
        return Some(Wanted {
            valid: false,
            facing: Dir::Down,
        });
    }
    let ado = speed.signum() as i32 * if side.positive() { 1 } else { -1 };
    let a = unit(axis);
    let b = unit(side.axis());
    let cross = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    Some(Wanted {
        valid: true,
        facing: Dir::nearest(cross.map(|c| c * ado)),
    })
}

fn unit(a: Axis) -> Pos {
    match a {
        Axis::X => [1, 0, 0],
        Axis::Y => [0, 1, 0],
        Axis::Z => [0, 0, 1],
    }
}

/// `CrushingWheelBlock.canSurvive` for one neighbour: `controller` when it
/// is a controller, `wheel` its axis when it is a wheel.
pub fn survives_beside(axis: Axis, direction: Dir, controller: bool, wheel: Option<Axis>) -> bool {
    if controller && direction.axis() != axis {
        return false;
    }
    wheel.is_none_or(|w| w == axis && axis == direction.axis())
}

/// `updateSpeed`: `crushingspeed` from a wheel's speed.
pub fn crushing_speed(wheel_speed: f32) -> f32 {
    (wheel_speed / 50.0).abs()
}

/// What a tick of a controller holding items asks of its block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Wait,
    /// `applyRecipe`.
    Apply,
    /// `remainingTime` ran out: put out the results.
    Output,
}

/// `tick`'s timer: `remaining` counts down at the wheels' speed, faster for
/// a single item than for a stack.
pub fn tick(remaining: &mut f32, applied: bool, crushing: f32, input_count: u32) -> Step {
    let speed = crushing * 4.0;
    let divisor = if applied {
        1.0
    } else {
        (input_count as f32).ln() / 2f32.ln()
    };
    let processing = (speed / divisor).clamp(0.25, 20.0);
    *remaining -= processing;
    if *remaining < 20.0 && !applied {
        return Step::Apply;
    }
    if *remaining > 0.0 {
        return Step::Wait;
    }
    *remaining = 0.0;
    Step::Output
}

/// `ItemHelper.addToList` over (id, count): onto stacks of the same item up
/// to `max`, the rest as a new stack.
pub fn add_to_list(list: &mut Vec<(String, u32)>, id: &str, mut count: u32, max: u32) {
    for (other, n) in list.iter_mut() {
        if other == id && *n < max {
            let moved = (max - *n).min(count);
            *n += moved;
            count -= moved;
            if count == 0 {
                return;
            }
        }
    }
    while count > 0 {
        let n = count.min(max.max(1));
        list.push((id.to_owned(), n));
        count -= n;
    }
}

/// Where results leave (from the block's centre) and how fast. Create throws
/// them 0.55 out, with its collision empty for them (`BypassCrushingWheel`);
/// Horizon's collision is the same for every entity, so they start with
/// their box (.25 wide and high) clear of the block, or vanilla pushes them
/// out sideways.
pub fn ejection(facing: Dir) -> ([f64; 3], [f64; 3]) {
    let n = facing.normal().map(f64::from);
    let out = match facing {
        Dir::Up => 0.55,
        Dir::Down => 0.76,
        _ => 0.63,
    };
    let up = if facing == Dir::Up { 0.5 } else { 0.0 };
    (
        [n[0] * out, n[1] * out, n[2] * out],
        [n[0] * 0.25, up, n[2] * 0.25],
    )
}

/// The block results go to before they are ejected (never upward): a block
/// below the controller, one along a sideways `facing`.
pub fn output_target(pos: Pos, facing: Dir) -> Option<Pos> {
    if facing == Dir::Up {
        return None;
    }
    let n = if facing.axis() == Axis::Y {
        [0, 0, 0]
    } else {
        facing.normal()
    };
    Some([pos[0] + n[0], pos[1] - 1 + n[1], pos[2] + n[2]])
}

/// `supportsDirectBeltInput`: from above, or along the way it faces.
pub fn takes_from_side(facing: Dir, side: Dir) -> bool {
    facing == Dir::Down || facing == side
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheels_turning_toward_each_other_crush_downward() {
        // Two wheels along Z, the other on the east, turning opposite ways.
        let w = wanted_controller(Axis::Z, Dir::East, 32.0, Some((Axis::Z, -32.0))).unwrap();
        assert!(w.valid);
        assert_eq!(w.facing, Dir::Up);
        let w = wanted_controller(Axis::Z, Dir::East, -32.0, Some((Axis::Z, 32.0))).unwrap();
        assert_eq!(w.facing, Dir::Down);
        // Same way or still: a controller that doesn't work.
        let w = wanted_controller(Axis::Z, Dir::East, 32.0, Some((Axis::Z, 32.0))).unwrap();
        assert!(!w.valid);
        assert!(
            !wanted_controller(Axis::Z, Dir::East, 0.0, Some((Axis::Z, 32.0)))
                .unwrap()
                .valid
        );
        // None for another axis, none along the axis, none without a wheel.
        assert_eq!(
            wanted_controller(Axis::Z, Dir::East, 32.0, Some((Axis::X, -32.0))),
            None
        );
        assert_eq!(
            wanted_controller(Axis::Z, Dir::South, 32.0, Some((Axis::Z, -32.0))),
            None
        );
        assert_eq!(wanted_controller(Axis::Z, Dir::East, 32.0, None), None);
    }

    #[test]
    fn a_single_item_goes_fast_and_a_stack_slower() {
        let mut r = 250.0;
        assert_eq!(tick(&mut r, false, 1.28, 1), Step::Wait);
        assert_eq!(r, 230.0);
        let mut r = 250.0;
        tick(&mut r, false, 1.28, 16);
        assert!((r - (250.0 - 5.12 / 4.0)).abs() < 1e-3);
        let mut r = 19.0;
        assert_eq!(tick(&mut r, false, 1.28, 1), Step::Apply);
        let mut r = 0.0;
        assert_eq!(tick(&mut r, true, 1.28, 0), Step::Output);
        assert_eq!(r, 0.0);
    }

    #[test]
    fn results_stack_and_leave_the_way_it_faces() {
        let mut l = Vec::new();
        add_to_list(&mut l, "a", 60, 64);
        add_to_list(&mut l, "a", 10, 64);
        add_to_list(&mut l, "b", 1, 64);
        assert_eq!(
            l,
            vec![
                ("a".to_owned(), 64),
                ("a".to_owned(), 6),
                ("b".to_owned(), 1)
            ]
        );
        assert_eq!(ejection(Dir::Down), ([0.0, -0.76, 0.0], [0.0, 0.0, 0.0]));
        assert_eq!(ejection(Dir::Up).1, [0.0, 0.5, 0.0]);
        assert_eq!(output_target([0, 5, 0], Dir::Down), Some([0, 4, 0]));
        assert_eq!(output_target([0, 5, 0], Dir::East), Some([1, 4, 0]));
        assert_eq!(output_target([0, 5, 0], Dir::Up), None);
        assert!(survives_beside(Axis::Z, Dir::South, false, Some(Axis::Z)));
        assert!(!survives_beside(Axis::Z, Dir::East, false, Some(Axis::Z)));
        assert!(!survives_beside(Axis::Z, Dir::East, true, None));
        assert!(takes_from_side(Dir::Down, Dir::East) && !takes_from_side(Dir::West, Dir::East));
    }
}
