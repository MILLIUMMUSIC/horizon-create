// How Create's kinetic blocks choose their state when placed
// (`getStateForPlacement`), ported from Create 6.0.10 (MIT, The Create Team):
// RotatedPillarKineticBlock, DirectionalKineticBlock, HorizontalKineticBlock,
// HorizontalAxisKineticBlock and the blocks that change them (cogwheels, the
// fan, the creative motor, the hand crank, the press, the gearbox), with
// vanilla's `Direction.orderedByNearest` for the placer's look. No Horizon
// types: the neighbours are asked of `Around`.

use super::create_kinetics::{Axis, Dir, Kind};

/// What `BlockPlaceContext` says of the placement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ctx {
    /// `getClickedFace`.
    pub face: Dir,
    /// The placer's rotation in degrees.
    pub yaw: f32,
    pub pitch: f32,
    /// `getPlayer().isShiftKeyDown()`.
    pub sneaking: bool,
}

/// The blocks around the placed one.
pub trait Around {
    /// The kinetic block beside the placed one, on `side`.
    fn kind(&self, side: Dir) -> Option<Kind>;
    /// Whether the block on `side` is a chute (the fan blows into one).
    fn chute(&self, side: Dir) -> bool;
}

/// `Direction.orderedByNearest(entity)[0]`: the direction the placer looks
/// nearest to (`getNearestLookingDirection`).
pub fn nearest_looking(yaw: f32, pitch: f32) -> Dir {
    let f = pitch.to_radians();
    let g = -yaw.to_radians();
    let (h, i) = (f.sin(), f.cos());
    let (j, k) = (g.sin(), g.cos());
    let east = j > 0.0;
    let up = h < 0.0;
    let south = k > 0.0;
    let l = if east { j } else { -j };
    let m = if up { -h } else { h };
    let n = if south { k } else { -k };
    let o = l * i;
    let p = n * i;
    let dx = if east { Dir::East } else { Dir::West };
    let dy = if up { Dir::Up } else { Dir::Down };
    let dz = if south { Dir::South } else { Dir::North };
    if l > n {
        if m > o { dy } else { dx }
    } else if m > p {
        dy
    } else {
        dz
    }
}

/// `getHorizontalDirection`: the horizontal way the placer faces
/// (`Direction.fromYRot`).
pub fn horizontal(yaw: f32) -> Dir {
    match ((yaw / 90.0 + 0.5).floor() as i32).rem_euclid(4) {
        0 => Dir::South,
        1 => Dir::West,
        2 => Dir::North,
        _ => Dir::East,
    }
}

const HORIZONTALS: [Dir; 4] = [Dir::South, Dir::West, Dir::North, Dir::East];

/// The sides whose block has a shaft towards the placed one, in
/// `Iterate.directions` order (down, up, north, south, west, east).
fn shafts<'a>(around: &'a dyn Around, sides: &'a [Dir]) -> impl Iterator<Item = Dir> + 'a {
    sides.iter().copied().filter(move |side| {
        around
            .kind(*side)
            .is_some_and(|k| k.has_shaft_towards(side.opposite()))
    })
}

/// `getPreferredAxis` / `getPreferredFacing` / the horizontal ones: the one
/// side (or axis) shafts come from, none when they come along two axes.
fn preferred_side(around: &dyn Around, sides: &[Dir]) -> Option<Dir> {
    let mut preferred: Option<Dir> = None;
    for side in shafts(around, sides) {
        match preferred {
            Some(p) if p.axis() != side.axis() => return None,
            _ => preferred = Some(side),
        }
    }
    preferred
}

/// `RotatedPillarKineticBlock.getPreferredAxis`.
pub fn preferred_axis(around: &dyn Around) -> Option<Axis> {
    preferred_side(around, &Dir::ALL).map(Dir::axis)
}

/// `RotatedPillarKineticBlock.getStateForPlacement`.
pub fn rotated_pillar(ctx: &Ctx, around: &dyn Around) -> Axis {
    let preferred = preferred_axis(around);
    match preferred {
        Some(a) if !ctx.sneaking => a,
        Some(_) => ctx.face.axis(),
        None => nearest_looking(ctx.yaw, ctx.pitch).axis(),
    }
}

/// `DirectionalKineticBlock.getStateForPlacement`.
pub fn directional(ctx: &Ctx, around: &dyn Around) -> Dir {
    let looking = nearest_looking(ctx.yaw, ctx.pitch);
    match preferred_side(around, &Dir::ALL) {
        Some(p) if !ctx.sneaking => p.opposite(),
        _ if ctx.sneaking => looking,
        _ => looking.opposite(),
    }
}

/// `CogWheelBlock.getAxisForPlacement` (no speed controller below: not
/// ported).
pub fn cog_axis(ctx: &Ctx, around: &dyn Around) -> Axis {
    if ctx.sneaking {
        return ctx.face.axis();
    }
    // Placed against a small cog: its axis.
    if let Some(k) = around.kind(ctx.face.opposite())
        && k.is_small_cog()
    {
        return k.rotation_axis();
    }
    preferred_axis(around).unwrap_or(ctx.face.axis())
}

/// `Block.getStateForPlacement` of Create's kinetic block `kind` (its own
/// default orientation ignored): the kind it is placed as, none for a block
/// whose placement stays the template's.
pub fn place(kind: Kind, ctx: &Ctx, around: &dyn Around) -> Option<Kind> {
    Some(match kind {
        Kind::Shaft(_) => Kind::Shaft(rotated_pillar(ctx, around)),
        Kind::Cogwheel(_) => Kind::Cogwheel(cog_axis(ctx, around)),
        Kind::LargeCogwheel(_) => Kind::LargeCogwheel(cog_axis(ctx, around)),
        // GearboxBlock: always upright.
        Kind::Gearbox(_) => Kind::Gearbox(Axis::Y),
        // CreativeMotorBlock: facing the one shaft beside it.
        Kind::CreativeMotor(_) => Kind::CreativeMotor(match preferred_side(around, &Dir::ALL) {
            Some(p) if !ctx.sneaking => p,
            _ => directional(ctx, around),
        }),
        // HandCrankBlock: facing a shaft beside it, else away from the
        // clicked block.
        Kind::HandCrank(_) => Kind::HandCrank(match preferred_side(around, &Dir::ALL) {
            Some(p) if !ctx.sneaking => p.opposite(),
            _ => ctx.face,
        }),
        // MechanicalPressBlock: towards a shaft beside it, else
        // HorizontalKineticBlock's facing the placer.
        Kind::MechanicalPress(_) => Kind::MechanicalPress(
            preferred_side(around, &HORIZONTALS).unwrap_or_else(|| horizontal(ctx.yaw).opposite()),
        ),
        // EncasedFanBlock: blowing into a chute it is placed against or
        // under, else away from a shaft beside it or the placer.
        Kind::EncasedFan(_) => Kind::EncasedFan(if around.chute(ctx.face.opposite()) {
            ctx.face.opposite()
        } else if around.chute(ctx.face) {
            ctx.face
        } else {
            let p =
                preferred_side(around, &Dir::ALL).unwrap_or(nearest_looking(ctx.yaw, ctx.pitch));
            if ctx.sneaking { p } else { p.opposite() }
        }),
        // WaterWheelBlock: DirectionalKineticBlock's (its turn to the
        // positive side is thrown away in Create 6.0.10).
        Kind::WaterWheel(_) => Kind::WaterWheel(directional(ctx, around)),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Near(Vec<(Dir, Kind)>, Vec<Dir>);
    impl Around for Near {
        fn kind(&self, side: Dir) -> Option<Kind> {
            self.0.iter().find(|(d, _)| *d == side).map(|(_, k)| *k)
        }
        fn chute(&self, side: Dir) -> bool {
            self.1.contains(&side)
        }
    }

    fn ctx(face: Dir, yaw: f32, pitch: f32, sneaking: bool) -> Ctx {
        Ctx {
            face,
            yaw,
            pitch,
            sneaking,
        }
    }

    #[test]
    fn the_look_picks_the_nearest_direction() {
        assert_eq!(nearest_looking(0.0, 0.0), Dir::South);
        assert_eq!(nearest_looking(90.0, 10.0), Dir::West);
        assert_eq!(nearest_looking(180.0, 0.0), Dir::North);
        assert_eq!(nearest_looking(-90.0, 0.0), Dir::East);
        assert_eq!(nearest_looking(0.0, 80.0), Dir::Down);
        assert_eq!(nearest_looking(0.0, -80.0), Dir::Up);
        assert_eq!(horizontal(180.0), Dir::North);
        assert_eq!(horizontal(-90.0), Dir::East);
    }

    #[test]
    fn shafts_line_up_with_a_shaft_beside() {
        let near = Near(vec![(Dir::East, Kind::Shaft(Axis::X))], vec![]);
        let c = ctx(Dir::Up, 0.0, 0.0, false);
        assert_eq!(
            place(Kind::Shaft(Axis::Y), &c, &near),
            Some(Kind::Shaft(Axis::X))
        );
        // Sneaking: the clicked face's axis.
        let s = ctx(Dir::Up, 0.0, 0.0, true);
        assert_eq!(
            place(Kind::Shaft(Axis::Y), &s, &near),
            Some(Kind::Shaft(Axis::Y))
        );
        // Nothing beside: the look's axis.
        let none = Near(vec![], vec![]);
        assert_eq!(
            place(Kind::Shaft(Axis::Y), &c, &none),
            Some(Kind::Shaft(Axis::Z))
        );
        // Shafts along two axes: no preference.
        let two = Near(
            vec![
                (Dir::East, Kind::Shaft(Axis::X)),
                (Dir::Up, Kind::Shaft(Axis::Y)),
            ],
            vec![],
        );
        assert_eq!(preferred_axis(&two), None);
    }

    #[test]
    fn motors_face_their_shaft_and_fans_blow_away() {
        let near = Near(vec![(Dir::North, Kind::Shaft(Axis::Z))], vec![]);
        let c = ctx(Dir::Up, 0.0, 0.0, false);
        assert_eq!(
            place(Kind::CreativeMotor(Dir::Up), &c, &near),
            Some(Kind::CreativeMotor(Dir::North))
        );
        assert_eq!(
            place(Kind::EncasedFan(Dir::Up), &c, &near),
            Some(Kind::EncasedFan(Dir::South))
        );
        // Looking south with nothing around: the fan faces the placer.
        let none = Near(vec![], vec![]);
        assert_eq!(
            place(Kind::EncasedFan(Dir::Up), &c, &none),
            Some(Kind::EncasedFan(Dir::North))
        );
        // Placed on top of a chute: blowing into it.
        let chute = Near(vec![], vec![Dir::Down]);
        assert_eq!(
            place(Kind::EncasedFan(Dir::Up), &c, &chute),
            Some(Kind::EncasedFan(Dir::Down))
        );
    }

    #[test]
    fn cogs_take_a_small_cogs_axis() {
        let near = Near(vec![(Dir::Down, Kind::Cogwheel(Axis::X))], vec![]);
        let c = ctx(Dir::Up, 0.0, 0.0, false);
        assert_eq!(
            place(Kind::Cogwheel(Axis::Y), &c, &near),
            Some(Kind::Cogwheel(Axis::X))
        );
        assert_eq!(
            place(Kind::Gearbox(Axis::X), &c, &near),
            Some(Kind::Gearbox(Axis::Y))
        );
    }
}
