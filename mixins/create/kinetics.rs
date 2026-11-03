// Create's kinetic network, ported from Create 6.0.10 (MIT, The Create Team):
// RotationPropagator, KineticNetwork, TorquePropagator, KineticBlockEntity and
// GeneratingKineticBlockEntity, with the blocks of the first milestone.
//
// This file is both the standalone Cargo library (its tests run with plain
// `cargo test`) and the injected `crate::game::create_kinetics` module. It
// knows nothing of Horizon: the game is reached through `Level`, and block
// entities live in `Kinetics`, keyed by position, one per dimension (as
// TorquePropagator keeps one network map per level).

use super::create_belt::{BeltState, Part, Slope};
use super::create_processing::{
    MixStep, Mixing, PressMode, PressStep, Pressing, water_wheel_speed, water_wheel_speed_sized,
};
use std::collections::{HashMap, HashSet, VecDeque};

pub type Pos = [i32; 3];

/// The deployer's blocks: one per `AXIS_ALONG_FIRST_COORDINATE` (no template
/// has that property).
pub const DEPLOYER: &str = "create:deployer";
pub const DEPLOYER_ALONG: &str = "create:hz_deployer_along";
pub const SPEEDOMETER: &str = "create:speedometer";
pub const SPEEDOMETER_ALONG: &str = "create:hz_speedometer_along";
pub const STRESSOMETER: &str = "create:stressometer";
pub const STRESSOMETER_ALONG: &str = "create:hz_stressometer_along";

/// `AllConfigs.server().kinetics.maxRotationSpeed` default.
pub const MAX_ROTATION_SPEED: f32 = 256.0;
/// `kineticValidationFrequency` default.
pub const VALIDATION_FREQUENCY: i32 = 60;
/// `RotationPropagator.MAX_FLICKER_SCORE`.
const MAX_FLICKER_SCORE: i32 = 128;
/// `CreativeMotorBlockEntity.DEFAULT_SPEED`.
pub const MOTOR_DEFAULT_SPEED: i32 = 16;
/// `HandCrankBlock.getRotationSpeed`.
const CRANK_SPEED: i32 = 32;
/// `HandCrankBlockEntity.turn`: ticks a click keeps the crank turning.
const CRANK_USE_TICKS: i32 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];
    /// `Direction.Axis.choose`.
    pub fn choose(self, v: Pos) -> i32 {
        match self {
            Axis::X => v[0],
            Axis::Y => v[1],
            Axis::Z => v[2],
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Axis::X => "x",
            Axis::Y => "y",
            Axis::Z => "z",
        }
    }
    pub fn parse(s: &str) -> Option<Axis> {
        match s {
            "x" => Some(Axis::X),
            "y" => Some(Axis::Y),
            "z" => Some(Axis::Z),
            _ => None,
        }
    }
}

/// `net.minecraft.core.Direction`, in vanilla's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dir {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Dir {
    pub const ALL: [Dir; 6] = [
        Dir::Down,
        Dir::Up,
        Dir::North,
        Dir::South,
        Dir::West,
        Dir::East,
    ];
    pub fn normal(self) -> Pos {
        match self {
            Dir::Down => [0, -1, 0],
            Dir::Up => [0, 1, 0],
            Dir::North => [0, 0, -1],
            Dir::South => [0, 0, 1],
            Dir::West => [-1, 0, 0],
            Dir::East => [1, 0, 0],
        }
    }
    pub fn axis(self) -> Axis {
        match self {
            Dir::Down | Dir::Up => Axis::Y,
            Dir::North | Dir::South => Axis::Z,
            Dir::West | Dir::East => Axis::X,
        }
    }
    pub fn positive(self) -> bool {
        matches!(self, Dir::Up | Dir::South | Dir::East)
    }
    pub fn opposite(self) -> Dir {
        match self {
            Dir::Down => Dir::Up,
            Dir::Up => Dir::Down,
            Dir::North => Dir::South,
            Dir::South => Dir::North,
            Dir::West => Dir::East,
            Dir::East => Dir::West,
        }
    }
    /// `Direction.getNearest(x, y, z)`: the first direction of the highest
    /// dot product, starting from `Float.MIN_VALUE` and north.
    pub fn nearest(v: Pos) -> Dir {
        let mut result = Dir::North;
        let mut highest = f32::from_bits(1);
        for d in Dir::ALL {
            let n = d.normal();
            let dot = (v[0] * n[0] + v[1] * n[1] + v[2] * n[2]) as f32;
            if dot > highest {
                highest = dot;
                result = d;
            }
        }
        result
    }
    pub fn name(self) -> &'static str {
        match self {
            Dir::Down => "down",
            Dir::Up => "up",
            Dir::North => "north",
            Dir::South => "south",
            Dir::West => "west",
            Dir::East => "east",
        }
    }
    pub fn parse(s: &str) -> Option<Dir> {
        Dir::ALL.into_iter().find(|d| d.name() == s)
    }
}

impl Dir {
    /// `Direction.getClockWise(axis)`: a quarter turn about `axis`; a
    /// direction along it stays.
    pub fn clockwise(self, axis: Axis) -> Dir {
        use Dir::*;
        match (axis, self) {
            // getClockWiseX
            (Axis::X, Down) => South,
            (Axis::X, Up) => North,
            (Axis::X, North) => Down,
            (Axis::X, South) => Up,
            // getClockWise
            (Axis::Y, North) => East,
            (Axis::Y, East) => South,
            (Axis::Y, South) => West,
            (Axis::Y, West) => North,
            // getClockWiseZ
            (Axis::Z, Down) => West,
            (Axis::Z, Up) => East,
            (Axis::Z, West) => Up,
            (Axis::Z, East) => Down,
            (_, d) => d,
        }
    }
    /// `Direction.get(POSITIVE, axis)` (`VoxelShaper.axisAsFace`).
    pub fn positive_of(axis: Axis) -> Dir {
        match axis {
            Axis::X => Dir::East,
            Axis::Y => Dir::Up,
            Axis::Z => Dir::South,
        }
    }
    /// `AngleHelper.horizontalAngle`.
    pub fn horizontal_angle(self) -> f32 {
        match self {
            Dir::South | Dir::Up | Dir::Down => 0.0,
            Dir::West => -90.0,
            Dir::North => 180.0,
            Dir::East => -270.0,
        }
    }
    /// `AngleHelper.verticalAngle`.
    pub fn vertical_angle(self) -> f32 {
        match self {
            Dir::Up => -90.0,
            Dir::Down => 90.0,
            _ => 0.0,
        }
    }
}

impl Kind {
    /// `IWrenchable.getRotatedBlockState`: the block after a wrench's click
    /// on its face `face` (`onWrenched`).
    pub fn wrenched(self, face: Dir) -> Kind {
        let turn_axis = |a: Axis| Dir::positive_of(a).clockwise(face.axis()).axis();
        // DirectionalKineticBlock.FACING: turned about the face until it
        // leaves the face's axis; along it, it stays.
        let turn_facing = |f: Dir| {
            if f.axis() == face.axis() {
                return f;
            }
            let mut f = f;
            loop {
                f = f.clockwise(face.axis());
                if f.axis() != face.axis() {
                    return f;
                }
            }
        };
        match self {
            // RotatedPillarKineticBlock.AXIS
            Kind::Shaft(a) => Kind::Shaft(turn_axis(a)),
            Kind::Cogwheel(a) => Kind::Cogwheel(turn_axis(a)),
            Kind::LargeCogwheel(a) => Kind::LargeCogwheel(turn_axis(a)),
            Kind::Gearbox(a) => Kind::Gearbox(turn_axis(a)),
            // HorizontalAxisKineticBlock: only from above or below.
            Kind::RopePulley(a) if face.axis() == Axis::Y => {
                Kind::RopePulley(if a == Axis::X { Axis::Z } else { Axis::X })
            }
            // HorizontalKineticBlock.HORIZONTAL_FACING: only from above or below.
            Kind::MechanicalPress(f) if face.axis() == Axis::Y => {
                Kind::MechanicalPress(f.clockwise(Axis::Y))
            }
            Kind::CreativeMotor(f) => Kind::CreativeMotor(turn_facing(f)),
            Kind::HandCrank(f) => Kind::HandCrank(turn_facing(f)),
            Kind::WaterWheel(f) => Kind::WaterWheel(turn_facing(f)),
            Kind::EncasedFan(f) => Kind::EncasedFan(turn_facing(f)),
            Kind::Drill(f) => Kind::Drill(turn_facing(f)),
            Kind::MechanicalBearing(f) => Kind::MechanicalBearing(turn_facing(f)),
            Kind::WindmillBearing(f) => Kind::WindmillBearing(turn_facing(f)),
            // PumpBlock.getRotatedBlockState: turned round, whatever the face.
            Kind::MechanicalPump(f) => Kind::MechanicalPump(f.opposite()),
            // No property to turn (millstone, mixer), or a press's side.
            k => k,
        }
    }
    /// The block state property a wrench changes, and its value.
    pub fn state_property(self) -> Option<(&'static str, &'static str)> {
        match self {
            Kind::Shaft(a)
            | Kind::Cogwheel(a)
            | Kind::LargeCogwheel(a)
            | Kind::Gearbox(a)
            | Kind::RopePulley(a) => Some(("axis", a.name())),
            Kind::CreativeMotor(f)
            | Kind::HandCrank(f)
            | Kind::MechanicalPress(f)
            | Kind::WaterWheel(f)
            | Kind::EncasedFan(f)
            | Kind::MechanicalPump(f)
            | Kind::MechanicalBearing(f)
            | Kind::WindmillBearing(f)
            | Kind::Drill(f) => Some(("facing", f.name())),
            Kind::Deployer(f, _)
            | Kind::MechanicalPiston(f, ..)
            | Kind::Saw(f, _)
            | Kind::Gauge(_, f, _) => Some(("facing", f.name())),
            Kind::GantryShaft(f) | Kind::GantryCarriage(f, _) => Some(("facing", f.name())),
            Kind::ElevatorPulley(f) => Some(("facing", f.name())),
            Kind::LargeWaterWheel(a) => Some(("axis", a.name())),
            Kind::Millstone | Kind::MechanicalMixer | Kind::Belt(_) | Kind::MechanicalArm => None,
        }
    }
}

/// `VecHelper.rotate(v, deg, axis)`.
pub fn rotate_vec(v: [f32; 3], deg: f32, axis: Axis) -> [f32; 3] {
    if deg == 0.0 {
        return v;
    }
    let (sin, cos) = deg.to_radians().sin_cos();
    let [x, y, z] = v;
    match axis {
        Axis::X => [x, y * cos - z * sin, z * cos + y * sin],
        Axis::Y => [x * cos + z * sin, y, z * cos - x * sin],
        Axis::Z => [x * cos - y * sin, y * cos + x * sin, z],
    }
}

/// `VecHelper.rotateCentered`: about the block's centre.
pub fn rotate_centered(v: [f32; 3], deg: f32, axis: Axis) -> [f32; 3] {
    let r = rotate_vec(v.map(|c| c - 0.5), deg, axis);
    r.map(|c| c + 0.5)
}

/// `ValueBoxTransform.getScale`: the value box's size, half a block.
const VALUE_BOX_SCALE: f32 = 0.5;

/// `CreativeMotorBlockEntity.MotorValueBox` (a `ValueBoxTransform.Sided`)
/// on the motor facing `facing`, looked at from `side`: the box's centre in
/// the block, none on a side without one (`isSideActive`).
pub fn motor_value_box(facing: Dir, side: Dir) -> Option<[f32; 3]> {
    if facing.axis() != Axis::Y && side == Dir::Down {
        return None;
    }
    if side.axis() == facing.axis() {
        return None;
    }
    // getSouthLocation: voxelSpace(8, 8, 12.5), turned to the side.
    let south = [0.5, 0.5, 12.5 / 16.0];
    let v = rotate_centered(south, side.horizontal_angle(), Axis::Y);
    let v = rotate_centered(v, side.vertical_angle(), Axis::X);
    let n = facing.normal();
    Some([0, 1, 2].map(|i| v[i] - n[i] as f32 / 16.0))
}

/// `ValueBoxTransform.testHit`: whether a click at `local` (in the block,
/// 0 to 1) on `side` is on the motor's value box.
pub fn hits_motor_value_box(facing: Dir, side: Dir, local: [f32; 3]) -> bool {
    motor_value_box(facing, side).is_some_and(|c| {
        let d = [0, 1, 2].map(|i| local[i] - c[i]);
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() < VALUE_BOX_SCALE / 2.0
    })
}

/// One wheel step of the creative motor's scroll value: Create 0.5's
/// `CreativeMotorTileEntity.step` (steps of 1, 4, 16 and 64 as the speed
/// grows, never 0), within `between(-256, 256)`. Create 6 sets it on a
/// value board instead, which Horizon can't open (docs/PLAN.md).
pub fn motor_scroll_step(current: i32, forward: bool) -> i32 {
    let mut step = 1;
    let magnitude = current.abs() - if forward == (current > 0) { 0 } else { 1 };
    for threshold in [4, 32, 128] {
        if magnitude >= threshold {
            step *= 4;
        }
    }
    let delta = if forward { step } else { -step };
    let step = if current + delta == 0 { step + 1 } else { step };
    let max = MAX_ROTATION_SPEED as i32;
    (current + if forward { step } else { -step }).clamp(-max, max)
}

fn sub(a: Pos, b: Pos) -> Pos {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: Pos, b: Pos) -> Pos {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `Math.signum` on floats.
fn signum(v: f32) -> f32 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// `BlockPos.asLong`: the id of a network a generator at `pos` starts
/// (`GeneratingKineticBlockEntity.createNetworkId`).
pub fn pos_as_long(p: Pos) -> i64 {
    ((i64::from(p[0]) & 0x3FF_FFFF) << 38)
        | ((i64::from(p[2]) & 0x3FF_FFFF) << 12)
        | (i64::from(p[1]) & 0xFFF)
}

/// The rotating blocks of the first milestone, with their state.
/// The gantry carriage with `AXIS_ALONG_FIRST_COORDINATE` (a hidden block:
/// the template keeps `facing`).
pub const GANTRY_CARRIAGE_ALONG: &str = "create:hz_gantry_carriage_along";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Shaft(Axis),
    Cogwheel(Axis),
    LargeCogwheel(Axis),
    Gearbox(Axis),
    CreativeMotor(Dir),
    HandCrank(Dir),
    /// `MechanicalPressBlock`: its horizontal facing.
    MechanicalPress(Dir),
    /// `MillstoneBlock`: turned from below.
    Millstone,
    /// `WaterWheelBlock`: the way it faces (its axis).
    WaterWheel(Dir),
    /// `LargeWaterWheelBlock`: its axis (the 3 by 3 around it are
    /// `WaterWheelStructuralBlock`s, contraption-free).
    LargeWaterWheel(Axis),
    /// `EncasedFanBlock`: the way its air blows out (its shaft is behind).
    EncasedFan(Dir),
    /// `PumpBlock`: the way it pushes (`FACING`); a small cog about that
    /// axis, no shafts (fluids.rs moves the fluid).
    MechanicalPump(Dir),
    /// `MechanicalMixerBlock`: a small cog about Y, no shafts.
    MechanicalMixer,
    /// `BeltBlock`: a segment of a mechanical belt (belt.rs).
    Belt(BeltState),
    /// `ArmBlock`: a small cog about Y, no shafts (arm.rs).
    MechanicalArm,
    /// `DeployerBlock`: its facing and `AXIS_ALONG_FIRST_COORDINATE`
    /// (deployer.rs).
    Deployer(Dir, bool),
    /// `MechanicalPistonBlock`: its facing, `AXIS_ALONG_FIRST_COORDINATE`
    /// and whether it is sticky; its `STATE` is in the block, not the kind
    /// (contraption.rs).
    MechanicalPiston(Dir, bool, bool),
    /// `MechanicalBearingBlock`: the way its top faces; its shaft is behind
    /// (contraption.rs turns what is in front).
    MechanicalBearing(Dir),
    /// `WindmillBearingBlock`: a generator while its sails turn.
    WindmillBearing(Dir),
    /// `PulleyBlock`: its horizontal shaft axis (`axis` here); the rope
    /// hangs below (contraption.rs).
    RopePulley(Axis),
    /// `GantryShaftBlock`: its facing (its `part` is in the block's name,
    /// contraption.rs's `gantry_shaft_block`).
    GantryShaft(Dir),
    /// `GantryCarriageBlock`: facing away from its shaft and
    /// `AXIS_ALONG_FIRST_COORDINATE`, as a deployer.
    GantryCarriage(Dir, bool),
    /// `DrillBlock`: the way its head faces; its shaft is behind (actors.rs
    /// breaks the block in front).
    Drill(Dir),
    /// `SawBlock`: its facing and `AXIS_ALONG_FIRST_COORDINATE`, as a
    /// deployer (`flipped` isn't kept).
    Saw(Dir, bool),
    /// A speedometer (false) or stressometer (true), facing, along first.
    Gauge(bool, Dir, bool),
    /// `ElevatorPulleyBlock`: its horizontal facing; the shaft runs across it
    /// (contraptions.rs runs the cabin).
    ElevatorPulley(Dir),
}

impl Kind {
    /// The block `name` with its state properties, when it is one of ours.
    pub fn from_block<'a>(name: &str, prop: impl Fn(&str) -> Option<&'a str>) -> Option<Kind> {
        let axis = || prop("axis").and_then(Axis::parse);
        let facing = || prop("facing").and_then(Dir::parse);
        if let Some(b) = BeltState::from_block(name, &prop) {
            return Some(Kind::Belt(b));
        }
        match name {
            "create:shaft" => axis().map(Kind::Shaft),
            "create:cogwheel" => axis().map(Kind::Cogwheel),
            "create:large_cogwheel" => axis().map(Kind::LargeCogwheel),
            "create:gearbox" => axis().map(Kind::Gearbox),
            "create:creative_motor" => facing().map(Kind::CreativeMotor),
            "create:hand_crank" => facing().map(Kind::HandCrank),
            "create:mechanical_press" => facing().map(Kind::MechanicalPress),
            "create:millstone" => Some(Kind::Millstone),
            "create:water_wheel" => facing().map(Kind::WaterWheel),
            "create:large_water_wheel" => axis().map(Kind::LargeWaterWheel),
            "create:encased_fan" => facing().map(Kind::EncasedFan),
            "create:mechanical_pump" => facing().map(Kind::MechanicalPump),
            "create:mechanical_mixer" => Some(Kind::MechanicalMixer),
            "create:mechanical_arm" => Some(Kind::MechanicalArm),
            "create:mechanical_bearing" => facing().map(Kind::MechanicalBearing),
            "create:mechanical_drill" => facing().map(Kind::Drill),
            "create:windmill_bearing" => facing().map(Kind::WindmillBearing),
            "create:rope_pulley" => axis().map(Kind::RopePulley),
            "create:gantry_carriage" => facing().map(|f| Kind::GantryCarriage(f, false)),
            GANTRY_CARRIAGE_ALONG => facing().map(|f| Kind::GantryCarriage(f, true)),
            n if super::create_contraption::gantry_shaft_of(n).is_some() => {
                facing().map(Kind::GantryShaft)
            }
            DEPLOYER => facing().map(|f| Kind::Deployer(f, false)),
            DEPLOYER_ALONG => facing().map(|f| Kind::Deployer(f, true)),
            super::create_actors::SAW => facing().map(|f| Kind::Saw(f, false)),
            SPEEDOMETER => facing().map(|f| Kind::Gauge(false, f, false)),
            SPEEDOMETER_ALONG => facing().map(|f| Kind::Gauge(false, f, true)),
            STRESSOMETER => facing().map(|f| Kind::Gauge(true, f, false)),
            STRESSOMETER_ALONG => facing().map(|f| Kind::Gauge(true, f, true)),
            "create:elevator_pulley" => facing().map(Kind::ElevatorPulley),
            super::create_actors::SAW_ALONG => facing().map(|f| Kind::Saw(f, true)),
            n => {
                let (sticky, along, _) = super::create_contraption::piston_of(n)?;
                facing().map(|f| Kind::MechanicalPiston(f, along, sticky))
            }
        }
    }
    pub fn block(self) -> &'static str {
        match self {
            Kind::Shaft(_) => "create:shaft",
            Kind::Cogwheel(_) => "create:cogwheel",
            Kind::LargeCogwheel(_) => "create:large_cogwheel",
            Kind::Gearbox(_) => "create:gearbox",
            Kind::CreativeMotor(_) => "create:creative_motor",
            Kind::HandCrank(_) => "create:hand_crank",
            Kind::MechanicalPress(_) => "create:mechanical_press",
            Kind::Millstone => "create:millstone",
            Kind::WaterWheel(_) => "create:water_wheel",
            Kind::LargeWaterWheel(_) => "create:large_water_wheel",
            Kind::EncasedFan(_) => "create:encased_fan",
            Kind::MechanicalPump(_) => "create:mechanical_pump",
            Kind::MechanicalMixer => "create:mechanical_mixer",
            Kind::Belt(b) => b.block().map_or("create:belt", |(name, _)| name),
            Kind::MechanicalArm => "create:mechanical_arm",
            Kind::MechanicalBearing(_) => "create:mechanical_bearing",
            Kind::Drill(_) => "create:mechanical_drill",
            Kind::WindmillBearing(_) => "create:windmill_bearing",
            Kind::RopePulley(_) => "create:rope_pulley",
            Kind::GantryShaft(_) => "create:gantry_shaft",
            Kind::GantryCarriage(_, false) => "create:gantry_carriage",
            Kind::GantryCarriage(_, true) => GANTRY_CARRIAGE_ALONG,
            Kind::ElevatorPulley(_) => "create:elevator_pulley",
            Kind::Saw(_, false) => super::create_actors::SAW,
            Kind::Saw(_, true) => super::create_actors::SAW_ALONG,
            Kind::Gauge(false, _, false) => SPEEDOMETER,
            Kind::Gauge(false, _, true) => SPEEDOMETER_ALONG,
            Kind::Gauge(true, _, false) => STRESSOMETER,
            Kind::Gauge(true, _, true) => STRESSOMETER_ALONG,
            Kind::Deployer(_, along) => {
                if along {
                    DEPLOYER_ALONG
                } else {
                    DEPLOYER
                }
            }
            Kind::MechanicalPiston(_, along, sticky) => super::create_contraption::piston_block(
                sticky,
                along,
                super::create_contraption::PistonState::Retracted,
            ),
        }
    }
    /// `IRotate.getRotationAxis`.
    pub fn rotation_axis(self) -> Axis {
        match self {
            Kind::Shaft(a)
            | Kind::Cogwheel(a)
            | Kind::LargeCogwheel(a)
            | Kind::Gearbox(a)
            | Kind::RopePulley(a) => a,
            Kind::CreativeMotor(f)
            | Kind::HandCrank(f)
            | Kind::MechanicalPress(f)
            | Kind::WaterWheel(f)
            | Kind::EncasedFan(f)
            | Kind::MechanicalPump(f)
            | Kind::MechanicalBearing(f)
            | Kind::WindmillBearing(f)
            | Kind::Drill(f) => f.axis(),
            Kind::Millstone | Kind::MechanicalMixer | Kind::MechanicalArm => Axis::Y,
            Kind::Belt(b) => b.rotation_axis(),
            Kind::Deployer(f, along)
            | Kind::MechanicalPiston(f, along, _)
            | Kind::GantryCarriage(f, along)
            | Kind::Saw(f, along)
            | Kind::Gauge(_, f, along) => super::create_deployer::rotation_axis(f, along),
            Kind::ElevatorPulley(f) => f.clockwise(Axis::Y).axis(),
            Kind::LargeWaterWheel(a) => a,
            Kind::GantryShaft(f) => f.axis(),
        }
    }
    /// `IRotate.hasShaftTowards`.
    pub fn has_shaft_towards(self, face: Dir) -> bool {
        match self {
            Kind::Shaft(a) | Kind::Cogwheel(a) | Kind::LargeCogwheel(a) | Kind::RopePulley(a) => {
                face.axis() == a
            }
            Kind::Gearbox(a) => face.axis() != a,
            Kind::CreativeMotor(f) => face == f,
            Kind::HandCrank(f) => face == f.opposite(),
            Kind::MechanicalPress(f) | Kind::WaterWheel(f) => face.axis() == f.axis(),
            Kind::Millstone => face == Dir::Down,
            Kind::EncasedFan(f)
            | Kind::MechanicalBearing(f)
            | Kind::WindmillBearing(f)
            | Kind::Drill(f) => face == f.opposite(),
            Kind::MechanicalPump(_) => false,
            Kind::MechanicalMixer | Kind::MechanicalArm => false,
            Kind::Belt(b) => b.has_shaft_towards(face),
            Kind::Deployer(f, along)
            | Kind::MechanicalPiston(f, along, _)
            | Kind::GantryCarriage(f, along)
            | Kind::Saw(f, along)
            | Kind::Gauge(_, f, along) => {
                face.axis() == super::create_deployer::rotation_axis(f, along)
            }
            Kind::GantryShaft(f) => face.axis() == f.axis(),
            Kind::ElevatorPulley(f) => face.axis() == f.clockwise(Axis::Y).axis(),
            Kind::LargeWaterWheel(a) => face.axis() == a,
        }
    }
    /// `ICogWheel.isSmallCog`: the mixer is an `ICogWheel` too.
    pub fn is_small_cog(self) -> bool {
        matches!(
            self,
            Kind::Cogwheel(_)
                | Kind::MechanicalMixer
                | Kind::MechanicalPump(_)
                | Kind::MechanicalArm
        )
    }
    /// `ICogWheel.isLargeCog`.
    pub fn is_large_cog(self) -> bool {
        matches!(self, Kind::LargeCogwheel(_))
    }
    /// `BlockStressValues.getImpact` with CStress's defaults.
    pub fn stress_impact(self) -> f32 {
        match self {
            Kind::MechanicalPress(_) => 8.0,
            Kind::Millstone => 4.0,
            Kind::EncasedFan(_) => 2.0,
            Kind::MechanicalPump(_) => 4.0,
            Kind::MechanicalMixer => 4.0,
            Kind::MechanicalArm => 2.0,
            Kind::Deployer(..) => 4.0,
            Kind::MechanicalPiston(..) => 4.0,
            Kind::MechanicalBearing(_) => 4.0,
            Kind::RopePulley(_) => 4.0,
            Kind::Drill(_) => 4.0,
            Kind::Saw(..) => 4.0,
            Kind::ElevatorPulley(_) => 4.0,
            _ => 0.0,
        }
    }
    /// `BlockStressValues.getCapacity` with CStress's defaults.
    pub fn stress_capacity(self) -> f32 {
        match self {
            Kind::CreativeMotor(_) => 16384.0,
            Kind::HandCrank(_) => 8.0,
            Kind::WaterWheel(_) => 32.0,
            Kind::LargeWaterWheel(_) => 128.0,
            Kind::WindmillBearing(_) => 512.0,
            _ => 0.0,
        }
    }
    /// A `GeneratingKineticBlockEntity`.
    pub fn is_generator(self) -> bool {
        matches!(
            self,
            Kind::CreativeMotor(_)
                | Kind::HandCrank(_)
                | Kind::WaterWheel(_)
                | Kind::LargeWaterWheel(_)
                | Kind::WindmillBearing(_)
        )
    }
    /// `KineticBlock.areStatesKineticallyEquivalent`: same block, same axis.
    pub fn equivalent(self, other: Kind) -> bool {
        self.block() == other.block() && self.rotation_axis() == other.rotation_axis()
    }
}

/// `KineticBlockEntity` (and the generators' fields), as it is saved.
#[derive(Clone, Debug, PartialEq)]
pub struct Kinetic {
    pub kind: Kind,
    pub speed: f32,
    pub source: Option<Pos>,
    pub network: Option<i64>,
    pub capacity: f32,
    pub stress: f32,
    pub network_size: i32,
    pub over_stressed: bool,
    pub network_dirty: bool,
    /// `updateSpeed`: attach on the next tick (a new or loaded entity).
    pub update_speed: bool,
    /// `SmartBlockEntity.initialized`.
    pub initialized: bool,
    pub flicker_tally: i32,
    pub validation_countdown: i32,
    pub last_stress_applied: f32,
    pub last_capacity_provided: f32,
    /// `GeneratingKineticBlockEntity.reActivateSource`.
    pub reactivate_source: bool,
    /// `CreativeMotorBlockEntity.generatedSpeed`'s value.
    pub motor_speed: i32,
    /// `HandCrankBlockEntity.inUse` and `backwards`.
    pub crank_in_use: i32,
    pub crank_backwards: bool,
    /// `WaterWheelBlockEntity.flowScore`; a windmill bearing's generated
    /// speed (`getGeneratedSpeed`, set by contraptions.rs).
    pub flow_score: i32,
    /// `MechanicalPressBlockEntity.pressingBehaviour`.
    pub pressing: Pressing,
    /// `MechanicalMixerBlockEntity`'s cycle.
    pub mixing: Mixing,
    /// `WaterWheelBlockEntity.material`: its index in `WHEEL_WOODS`; a belt
    /// segment's dye (`BeltBlockEntity.color`, `DYES`' index plus one).
    pub material: u8,
}

/// `LargeWaterWheelBlock.tick`: the `WaterWheelStructuralBlock`s around a
/// large wheel at `pos` about `axis`, each with the facing it needs (the
/// sides face the wheel, the corners the side next to them).
pub fn large_wheel_structure(pos: Pos, axis: Axis) -> Vec<(Pos, Dir)> {
    let mut out = Vec::new();
    for side in Dir::ALL {
        if side.axis() == axis {
            continue;
        }
        for secondary in [false, true] {
            let target = if secondary {
                side.clockwise(axis)
            } else {
                side
            };
            let base = if secondary {
                add(pos, side.normal())
            } else {
                pos
            };
            out.push((add(base, target.normal()), target.opposite()));
        }
    }
    out
}

/// The woods a water wheel can be made of (`AllBlockTags.WATER_WHEEL_PLANKS`
/// in vanilla), spruce first (its default).
pub const WHEEL_WOODS: [&str; 12] = [
    "spruce", "oak", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry", "pale_oak",
    "bamboo", "crimson", "warped",
];

/// `applyMaterialIfValid`: the wood of a planks item, if a wheel can be
/// made of it.
pub fn wheel_wood_of(item: &str) -> Option<u8> {
    let wood = item.strip_prefix("minecraft:")?.strip_suffix("_planks")?;
    WHEEL_WOODS.iter().position(|w| *w == wood).map(|i| i as u8)
}

/// `WaterWheelRenderer.generateModel`: the spruce wheel's sprite as the
/// wheel of wood `material` (planks for planks, its log or stem for logs).
pub fn wheel_sprite(sprite: &str, material: u8) -> String {
    let wood = WHEEL_WOODS
        .get(usize::from(material))
        .copied()
        .unwrap_or("spruce");
    let log = match wood {
        "crimson" | "warped" => format!("{wood}_stem"),
        "bamboo" => "bamboo_block".to_owned(),
        _ => format!("{wood}_log"),
    };
    match sprite {
        "block/spruce_planks" => format!("block/{wood}_planks"),
        "block/spruce_log" => format!("block/{log}"),
        "block/spruce_log_top" => format!("block/{log}_top"),
        other => other.to_owned(),
    }
}

impl Kinetic {
    pub fn new(kind: Kind) -> Self {
        Kinetic {
            kind,
            speed: 0.0,
            source: None,
            network: None,
            capacity: 0.0,
            stress: 0.0,
            network_size: 0,
            over_stressed: false,
            network_dirty: false,
            update_speed: true,
            initialized: false,
            flicker_tally: 0,
            validation_countdown: 0,
            last_stress_applied: 0.0,
            last_capacity_provided: 0.0,
            reactivate_source: false,
            motor_speed: MOTOR_DEFAULT_SPEED,
            crank_in_use: 0,
            crank_backwards: false,
            flow_score: 0,
            pressing: Pressing::default(),
            mixing: Mixing::new(),
            material: 0,
        }
    }
    /// `getGeneratedSpeed`.
    pub fn generated_speed(&self) -> f32 {
        let convert = |speed: i32, d: Dir| {
            if d.positive() {
                speed as f32
            } else {
                -(speed as f32)
            }
        };
        match self.kind {
            Kind::CreativeMotor(f) => convert(self.motor_speed, f),
            Kind::HandCrank(f) => {
                let sign = if self.crank_in_use == 0 {
                    0
                } else if self.crank_backwards {
                    -1
                } else {
                    1
                };
                convert(sign * CRANK_SPEED, f)
            }
            Kind::WaterWheel(_) => water_wheel_speed(self.flow_score),
            Kind::LargeWaterWheel(_) => water_wheel_speed_sized(self.flow_score, 2),
            Kind::WindmillBearing(_) => self.flow_score as f32,
            _ => 0.0,
        }
    }
    pub fn is_source(&self) -> bool {
        self.generated_speed() != 0.0
    }
    pub fn has_source(&self) -> bool {
        self.source.is_some()
    }
}

/// What the kinetics need of the world.
pub trait Level {
    /// The rotating block at `pos` (`instanceof IRotate`), from the world.
    fn kind(&self, pos: Pos) -> Option<Kind>;
    fn is_loaded(&self, pos: Pos) -> bool;
    /// `Level.destroyBlock(pos, true)`. The kinetics have already let go of
    /// the block's entity; the world change may come later.
    fn destroy_block(&mut self, pos: Pos);
    /// `tickRateManager().isFrozen()`.
    fn frozen(&self) -> bool {
        false
    }
}

/// `KineticNetwork`.
#[derive(Clone, Debug, Default)]
struct Network {
    initialized: bool,
    sources: HashMap<Pos, f32>,
    members: HashMap<Pos, f32>,
    // Insertion order of members, so "the first member" is stable.
    order: Vec<Pos>,
    current_capacity: f32,
    current_stress: f32,
    unloaded_capacity: f32,
    unloaded_stress: f32,
    unloaded_members: i32,
}

/// One dimension's kinetic block entities and networks.
#[derive(Debug, Default)]
pub struct Kinetics {
    entities: HashMap<Pos, Kinetic>,
    networks: HashMap<i64, Network>,
    /// `sendData`: entities whose client view changed.
    dirty: HashSet<Pos>,
}

impl Kinetics {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get(&self, pos: Pos) -> Option<&Kinetic> {
        self.entities.get(&pos)
    }
    pub fn positions(&self) -> Vec<Pos> {
        let mut v: Vec<Pos> = self.entities.keys().copied().collect();
        v.sort();
        v
    }
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entities.len()
    }
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }
    /// What a client is sent of the entity at `pos` (`writeSafe` with
    /// `clientPacket`).
    pub fn view(&self, level: &dyn Level, pos: Pos) -> Option<View> {
        let e = self.entities.get(&pos)?;
        Some(View {
            kind: e.kind,
            speed: self.speed_of(level, pos),
            source: e.source.map(|s| Dir::nearest(sub(s, pos))),
            stress: e.stress,
            capacity: e.capacity,
            aux: match e.kind {
                Kind::MechanicalPress(_) if e.pressing.running => {
                    e.pressing.running_ticks.clamp(0, i16::MAX as i32) as i16
                }
                Kind::MechanicalMixer if e.mixing.running => {
                    e.mixing.running_ticks.clamp(0, i16::MAX as i32) as i16
                }
                // The scroll value box shows the motor's setting.
                Kind::CreativeMotor(_) => e.motor_speed as i16,
                // The wheel's wood, the belt's dye.
                Kind::WaterWheel(_) | Kind::LargeWaterWheel(_) | Kind::Belt(_) => {
                    i16::from(e.material)
                }
                _ => -1,
            },
            press_belt: e.pressing.mode == PressMode::Belt,
        })
    }
    /// The changed entities since the last call; none for one that is gone.
    pub fn take_dirty(&mut self, level: &dyn Level) -> Vec<(Pos, Option<View>)> {
        let mut dirty: Vec<Pos> = self.dirty.drain().collect();
        dirty.sort();
        dirty
            .into_iter()
            .map(|p| (p, self.view(level, p)))
            .collect()
    }
    /// Every entity, for a player who just came.
    pub fn snapshot(&self, level: &dyn Level) -> Vec<(Pos, Option<View>)> {
        self.positions()
            .into_iter()
            .map(|p| (p, self.view(level, p)))
            .collect()
    }

    fn e(&self, pos: Pos) -> &Kinetic {
        &self.entities[&pos]
    }
    fn em(&mut self, pos: Pos) -> &mut Kinetic {
        self.entities.get_mut(&pos).expect("kinetic entity")
    }
    fn has(&self, pos: Pos) -> bool {
        self.entities.contains_key(&pos)
    }
    fn send(&mut self, pos: Pos) {
        self.dirty.insert(pos);
    }

    // ---- KineticBlockEntity -------------------------------------------

    /// `getSpeed`.
    pub fn speed_of(&self, level: &dyn Level, pos: Pos) -> f32 {
        match self.entities.get(&pos) {
            Some(e) if !e.over_stressed && !level.frozen() => e.speed,
            _ => 0.0,
        }
    }
    fn theoretical(&self, pos: Pos) -> f32 {
        self.e(pos).speed
    }
    fn calculate_stress_applied(&mut self, pos: Pos) -> f32 {
        let e = self.em(pos);
        let impact = e.kind.stress_impact();
        e.last_stress_applied = impact;
        impact
    }
    fn calculate_added_stress_capacity(&mut self, pos: Pos) -> f32 {
        let e = self.em(pos);
        let capacity = e.kind.stress_capacity();
        e.last_capacity_provided = capacity;
        capacity
    }
    fn on_speed_changed(&mut self, level: &dyn Level, pos: Pos, previous: f32) {
        let speed = self.speed_of(level, pos);
        let from_or_to_zero = (previous == 0.0) != (speed == 0.0);
        let direction_swap = !from_or_to_zero && signum(previous) != signum(speed);
        if from_or_to_zero || direction_swap {
            let e = self.em(pos);
            e.flicker_tally += 5;
        }
    }
    fn update_from_network(
        &mut self,
        level: &dyn Level,
        pos: Pos,
        max_stress: f32,
        current_stress: f32,
        size: i32,
    ) {
        let e = self.em(pos);
        e.network_dirty = false;
        e.capacity = max_stress;
        e.stress = current_stress;
        e.network_size = size;
        let over_stressed = max_stress < current_stress;
        if over_stressed != e.over_stressed {
            let previous = self.speed_of(level, pos);
            self.em(pos).over_stressed = over_stressed;
            self.on_speed_changed(level, pos, previous);
            self.send(pos);
        }
    }
    fn set_source(&mut self, level: &dyn Level, pos: Pos, source: Pos) {
        self.em(pos).source = Some(source);
        let Some(network) = self.entities.get(&source).map(|s| s.network) else {
            self.remove_source(level, pos);
            return;
        };
        self.set_network(level, pos, network);
        // GeneratingKineticBlockEntity.setSource.
        if self.e(pos).kind.is_generator()
            && self.e(pos).reactivate_source
            && self.speed_of(level, source).abs() >= self.e(pos).generated_speed().abs()
        {
            self.em(pos).reactivate_source = false;
        }
    }
    fn remove_source(&mut self, level: &dyn Level, pos: Pos) {
        // GeneratingKineticBlockEntity.removeSource.
        let e = self.e(pos);
        if e.kind.is_generator() && e.has_source() && e.is_source() {
            self.em(pos).reactivate_source = true;
        }
        let previous = self.speed_of(level, pos);
        let e = self.em(pos);
        e.speed = 0.0;
        e.source = None;
        self.set_network(level, pos, None);
        self.on_speed_changed(level, pos, previous);
    }
    fn set_network(&mut self, level: &dyn Level, pos: Pos, network: Option<i64>) {
        let old = self.e(pos).network;
        if old == network {
            return;
        }
        if let Some(id) = old {
            self.network_remove(level, id, pos);
        }
        self.em(pos).network = network;
        let Some(id) = network else {
            return;
        };
        self.networks.entry(id).or_default().initialized = true;
        self.network_add(level, id, pos);
    }
    fn attach_kinetics(&mut self, level: &mut dyn Level, pos: Pos) {
        self.em(pos).update_speed = false;
        self.handle_added(level, pos);
    }
    fn detach_kinetics(&mut self, level: &mut dyn Level, pos: Pos) {
        let (kind, speed) = (self.e(pos).kind, self.theoretical(pos));
        self.handle_removed(level, pos, kind, speed);
    }
    /// `KineticBlockEntity.initialize`, and the creative motor's.
    fn initialize(&mut self, level: &mut dyn Level, pos: Pos) {
        let e = self.e(pos);
        if let Some(id) = e.network {
            let (capacity, stress, size) = (e.capacity, e.stress, e.network_size);
            let (last_capacity, last_stress) = (e.last_capacity_provided, e.last_stress_applied);
            let initialized = self.networks.entry(id).or_default().initialized;
            if !initialized {
                self.network_init_from(level, id, capacity, stress, size);
            }
            self.network_add_silently(id, pos, last_capacity, last_stress);
        }
        let e = self.e(pos);
        if matches!(e.kind, Kind::CreativeMotor(_))
            && (!e.has_source() || e.generated_speed() > e.speed)
        {
            self.update_generated_rotation(level, pos);
        }
    }
    fn validate_kinetics(&mut self, level: &mut dyn Level, pos: Pos) {
        let e = self.e(pos);
        if let Some(source) = e.source {
            if e.network.is_none() {
                self.remove_source(level, pos);
                return;
            }
            if !level.is_loaded(source) {
                return;
            }
            if self.entities.get(&source).is_none_or(|s| s.speed == 0.0) {
                self.remove_source(level, pos);
                self.detach_kinetics(level, pos);
            }
            return;
        }
        if e.speed != 0.0 && e.generated_speed() == 0.0 {
            self.em(pos).speed = 0.0;
        }
    }

    // ---- GeneratingKineticBlockEntity ---------------------------------

    /// `updateGeneratedRotation`.
    pub fn update_generated_rotation(&mut self, level: &mut dyn Level, pos: Pos) {
        if !self.has(pos) {
            return;
        }
        let speed = self.e(pos).generated_speed();
        let previous = self.e(pos).speed;
        if previous != speed {
            self.apply_new_speed(level, pos, previous, speed);
        }
        if !self.has(pos) {
            return;
        }
        if let Some(id) = self.e(pos).network
            && speed != 0.0
        {
            let capacity = self.calculate_added_stress_capacity(pos);
            self.network_update_capacity_for(level, id, pos, capacity);
            let stress = self.calculate_stress_applied(pos);
            self.network_update_stress_for(level, id, pos, stress);
            self.network_update_stress(level, id);
        }
        if !self.has(pos) {
            return;
        }
        self.on_speed_changed(level, pos, previous);
        self.send(pos);
    }
    fn apply_new_speed(&mut self, level: &mut dyn Level, pos: Pos, previous: f32, speed: f32) {
        if speed == 0.0 {
            if self.e(pos).has_source() {
                if let Some(id) = self.e(pos).network {
                    self.network_update_capacity_for(level, id, pos, 0.0);
                    let stress = self.calculate_stress_applied(pos);
                    self.network_update_stress_for(level, id, pos, stress);
                }
                return;
            }
            self.detach_kinetics(level, pos);
            self.em(pos).speed = 0.0;
            self.set_network(level, pos, None);
            return;
        }
        if previous == 0.0 {
            self.em(pos).speed = speed;
            self.set_network(level, pos, Some(pos_as_long(pos)));
            self.attach_kinetics(level, pos);
            return;
        }
        if self.e(pos).has_source() {
            if previous.abs() >= speed.abs() {
                if signum(previous) != signum(speed) {
                    self.destroy(level, pos);
                }
                return;
            }
            self.detach_kinetics(level, pos);
            self.em(pos).speed = speed;
            self.em(pos).source = None;
            self.set_network(level, pos, Some(pos_as_long(pos)));
            self.attach_kinetics(level, pos);
            return;
        }
        self.detach_kinetics(level, pos);
        self.em(pos).speed = speed;
        self.attach_kinetics(level, pos);
    }

    // ---- KineticNetwork -----------------------------------------------

    fn network_init_from(
        &mut self,
        level: &dyn Level,
        id: i64,
        max_stress: f32,
        stress: f32,
        members: i32,
    ) {
        let n = self.networks.entry(id).or_default();
        n.unloaded_capacity = max_stress;
        n.unloaded_stress = stress;
        n.unloaded_members = members;
        n.initialized = true;
        self.network_update_stress(level, id);
        self.network_update_capacity(level, id);
    }
    fn network_add_silently(&mut self, id: i64, pos: Pos, last_capacity: f32, last_stress: f32) {
        if self
            .networks
            .get(&id)
            .is_some_and(|n| n.members.contains_key(&pos))
        {
            return;
        }
        let is_source = self.e(pos).is_source();
        let generated = self.e(pos).generated_speed().abs();
        let theoretical = self.theoretical(pos).abs();
        let added_capacity = if is_source {
            Some(self.calculate_added_stress_capacity(pos))
        } else {
            None
        };
        let applied = self.calculate_stress_applied(pos);
        let n = self.networks.entry(id).or_default();
        if let Some(capacity) = added_capacity {
            n.unloaded_capacity -= last_capacity * generated;
            n.sources.insert(pos, capacity);
        }
        n.unloaded_stress -= last_stress * theoretical;
        n.members.insert(pos, applied);
        n.order.push(pos);
        n.unloaded_members = (n.unloaded_members - 1).max(0);
        n.unloaded_capacity = n.unloaded_capacity.max(0.0);
        n.unloaded_stress = n.unloaded_stress.max(0.0);
    }
    fn network_add(&mut self, level: &dyn Level, id: i64, pos: Pos) {
        if self
            .networks
            .get(&id)
            .is_some_and(|n| n.members.contains_key(&pos))
        {
            return;
        }
        if self.e(pos).is_source() {
            let capacity = self.calculate_added_stress_capacity(pos);
            self.networks
                .entry(id)
                .or_default()
                .sources
                .insert(pos, capacity);
        }
        let applied = self.calculate_stress_applied(pos);
        let n = self.networks.entry(id).or_default();
        n.members.insert(pos, applied);
        n.order.push(pos);
        self.network_update_from(level, id, pos);
        self.em(pos).network_dirty = true;
    }
    fn network_update_capacity_for(&mut self, level: &dyn Level, id: i64, pos: Pos, c: f32) {
        self.networks.entry(id).or_default().sources.insert(pos, c);
        self.network_update_capacity(level, id);
    }
    fn network_update_stress_for(&mut self, level: &dyn Level, id: i64, pos: Pos, s: f32) {
        let n = self.networks.entry(id).or_default();
        if n.members.insert(pos, s).is_none() {
            n.order.push(pos);
        }
        self.network_update_stress(level, id);
    }
    fn network_remove(&mut self, level: &dyn Level, id: i64, pos: Pos) {
        let is_source = self.e(pos).is_source();
        let Some(n) = self.networks.get_mut(&id) else {
            return;
        };
        if !n.members.contains_key(&pos) {
            return;
        }
        if is_source {
            n.sources.remove(&pos);
        }
        n.members.remove(&pos);
        n.order.retain(|p| *p != pos);
        self.update_from_network(level, pos, 0.0, 0.0, 0);
        let n = &self.networks[&id];
        if n.members.is_empty() {
            self.networks.remove(&id);
            return;
        }
        if let Some(first) = n.order.first().copied()
            && let Some(e) = self.entities.get_mut(&first)
        {
            e.network_dirty = true;
        }
    }
    fn network_sync(&mut self, level: &dyn Level, id: i64) {
        let members = self.networks[&id].order.clone();
        for pos in members {
            if self.has(pos) {
                self.network_update_from(level, id, pos);
            }
        }
    }
    fn network_update_from(&mut self, level: &dyn Level, id: i64, pos: Pos) {
        let n = &self.networks[&id];
        let size = n.unloaded_members + n.members.len() as i32;
        let (capacity, stress) = (n.current_capacity, n.current_stress);
        self.update_from_network(level, pos, capacity, stress, size);
    }
    fn network_update_capacity(&mut self, level: &dyn Level, id: i64) {
        let capacity = self.network_calculate_capacity(id);
        let n = self.networks.get_mut(&id).expect("network");
        if n.current_capacity != capacity {
            n.current_capacity = capacity;
            self.network_sync(level, id);
        }
    }
    fn network_update_stress(&mut self, level: &dyn Level, id: i64) {
        let stress = self.network_calculate_stress(id);
        let n = self.networks.get_mut(&id).expect("network");
        if n.current_stress != stress {
            n.current_stress = stress;
            self.network_sync(level, id);
        }
    }
    /// `updateNetwork`.
    fn network_update(&mut self, level: &dyn Level, id: i64) {
        let stress = self.network_calculate_stress(id);
        let capacity = self.network_calculate_capacity(id);
        let n = self.networks.get_mut(&id).expect("network");
        if n.current_stress != stress || n.current_capacity != capacity {
            n.current_stress = stress;
            n.current_capacity = capacity;
            self.network_sync(level, id);
        }
    }
    fn network_calculate_capacity(&mut self, id: i64) -> f32 {
        let entities = &self.entities;
        let n = self.networks.get_mut(&id).expect("network");
        n.sources.retain(|p, _| entities.contains_key(p));
        let present: f32 = n
            .sources
            .iter()
            .map(|(p, c)| c * entities[p].generated_speed().abs())
            .sum();
        present + n.unloaded_capacity
    }
    fn network_calculate_stress(&mut self, id: i64) -> f32 {
        let entities = &self.entities;
        let n = self.networks.get_mut(&id).expect("network");
        n.members.retain(|p, _| entities.contains_key(p));
        n.order.retain(|p| entities.contains_key(p));
        let present: f32 = n
            .members
            .iter()
            .map(|(p, s)| s * entities[p].speed.abs())
            .sum();
        present + n.unloaded_stress
    }

    // ---- RotationPropagator -------------------------------------------

    /// `getRotationSpeedModifier`.
    fn rotation_speed_modifier(&self, from: Pos, to: Pos) -> f32 {
        let (kf, kt) = (self.e(from).kind, self.e(to).kind);
        let diff = sub(to, from);
        let direction = Dir::nearest(diff);
        let aligned_axes = Axis::ALL
            .iter()
            .all(|&a| a == direction.axis() || a.choose(diff) == 0);
        let connected_by_axis = aligned_axes
            && kf.has_shaft_towards(direction)
            && kt.has_shaft_towards(direction.opposite());
        let connected_by_gears = kf.is_small_cog() && kt.is_small_cog();

        // BeltBlockEntity.propagateRotationTo: segments of one belt.
        if let (Kind::Belt(a), Kind::Belt(b)) = (kf, kt)
            && !connected_by_axis
        {
            return if a.same_chain(from, b, to) { 1.0 } else { 0.0 };
        }

        // Axis <-> Axis
        if connected_by_axis {
            let mut axis_modifier = self.axis_modifier(to, direction.opposite());
            if axis_modifier != 0.0 {
                axis_modifier = 1.0 / axis_modifier;
            }
            return self.axis_modifier(from, direction) * axis_modifier;
        }
        // Large Gear <-> Large Gear
        if is_large_to_large_gear(kf, kt, diff) {
            let source_axis_diff = kf.rotation_axis().choose(diff);
            let target_axis_diff = kt.rotation_axis().choose(diff);
            return if (source_axis_diff > 0) ^ (target_axis_diff > 0) {
                -1.0
            } else {
                1.0
            };
        }
        // Gear <-> Large Gear
        if kf.is_large_cog() && kt.is_small_cog() && is_large_to_small_cog(kf, kt, diff) {
            return -2.0;
        }
        if kt.is_large_cog() && kf.is_small_cog() && is_large_to_small_cog(kt, kf, diff) {
            return -0.5;
        }
        // Gear <-> Gear
        if connected_by_gears {
            if diff[0].abs() + diff[1].abs() + diff[2].abs() != 1 {
                return 0.0;
            }
            if kt.is_large_cog() {
                return 0.0;
            }
            if direction.axis() == kf.rotation_axis() {
                return 0.0;
            }
            if kf.rotation_axis() == kt.rotation_axis() {
                return -1.0;
            }
        }
        0.0
    }
    /// `getAxisModifier`: gearboxes turn sideways shafts the other way.
    fn axis_modifier(&self, pos: Pos, direction: Dir) -> f32 {
        let e = self.e(pos);
        if !(e.has_source() || e.is_source()) {
            return 1.0;
        }
        let Kind::Gearbox(_) = e.kind else {
            return 1.0;
        };
        // DirectionalShaftHalvesBlockEntity.getSourceFacing.
        let Some(source) = e.source else {
            return 1.0;
        };
        let source = Dir::nearest(sub(source, pos));
        if direction.axis() == source.axis() {
            if direction == source { 1.0 } else { -1.0 }
        } else if direction.positive() == source.positive() {
            -1.0
        } else {
            1.0
        }
    }
    fn conveyed_speed(&self, from: Pos, to: Pos) -> f32 {
        self.theoretical(from) * self.rotation_speed_modifier(from, to)
    }
    /// `handleAdded`.
    fn handle_added(&mut self, level: &mut dyn Level, pos: Pos) {
        if !level.is_loaded(pos) {
            return;
        }
        self.propagate_new_source(level, pos);
    }
    /// `propagateNewSource`.
    fn propagate_new_source(&mut self, level: &mut dyn Level, current: Pos) {
        for neighbour in self.connected_neighbours(level, current) {
            if !self.has(current) {
                return;
            }
            if !self.has(neighbour) {
                continue;
            }
            let speed_of_current = self.theoretical(current);
            let speed_of_neighbour = self.theoretical(neighbour);
            let new_speed = self.conveyed_speed(current, neighbour);
            let opposite_speed = self.conveyed_speed(neighbour, current);
            if new_speed == 0.0 && opposite_speed == 0.0 {
                continue;
            }
            let incompatible = signum(new_speed) != signum(speed_of_neighbour)
                && new_speed != 0.0
                && speed_of_neighbour != 0.0;
            let too_fast =
                new_speed.abs() > MAX_ROTATION_SPEED || opposite_speed.abs() > MAX_ROTATION_SPEED;
            let speed_changed_too_often = self.e(current).flicker_tally > MAX_FLICKER_SCORE;
            if too_fast || speed_changed_too_often {
                self.destroy(level, current);
                return;
            }
            // Opposite directions
            if incompatible {
                self.destroy(level, current);
                return;
            }
            // Same direction: overpower the slower speed.
            // Neighbour faster, overpower the incoming tree.
            if opposite_speed.abs() > speed_of_current.abs() {
                let previous = self.speed_of(level, current);
                self.set_source(level, current, neighbour);
                let speed = self.conveyed_speed(neighbour, current);
                self.em(current).speed = speed;
                self.on_speed_changed(level, current, previous);
                self.send(current);
                self.propagate_new_source(level, current);
                return;
            }
            // Current faster, overpower the neighbours' tree.
            if new_speed.abs() >= speed_of_neighbour.abs() {
                // Do not overpower your own network -> cycle.
                let (cn, nn) = (self.e(current).network, self.e(neighbour).network);
                if cn.is_none() || cn == nn {
                    let epsilon = speed_of_neighbour.abs() / 256.0 / 256.0;
                    if new_speed.abs() > speed_of_neighbour.abs() + epsilon {
                        self.destroy(level, current);
                    }
                    continue;
                }
                if self.e(current).source == Some(neighbour) {
                    self.remove_source(level, current);
                }
                let previous = self.speed_of(level, neighbour);
                self.set_source(level, neighbour, current);
                let speed = self.conveyed_speed(current, neighbour);
                self.em(neighbour).speed = speed;
                self.on_speed_changed(level, neighbour, previous);
                self.send(neighbour);
                self.propagate_new_source(level, neighbour);
                continue;
            }
            if (self.theoretical(neighbour) - new_speed).abs() <= 1e-4 {
                continue;
            }
            let previous = self.speed_of(level, neighbour);
            self.em(neighbour).speed = new_speed;
            self.set_source(level, neighbour, current);
            self.on_speed_changed(level, neighbour, previous);
            self.send(neighbour);
            self.propagate_new_source(level, neighbour);
        }
    }
    /// `handleRemoved`, for the entity that was at `pos` (already gone from
    /// the table when the block was broken).
    fn handle_removed(&mut self, level: &mut dyn Level, pos: Pos, kind: Kind, speed: f32) {
        if speed == 0.0 {
            return;
        }
        for neighbour in self.potential_neighbour_locations(level, pos, kind) {
            if level.kind(neighbour).is_none() {
                continue;
            }
            let Some(n) = self.entities.get(&neighbour) else {
                continue;
            };
            if n.source != Some(pos) {
                continue;
            }
            self.propagate_missing_source(level, neighbour);
        }
    }
    /// `propagateMissingSource`.
    fn propagate_missing_source(&mut self, level: &mut dyn Level, start: Pos) {
        let mut potential_new_sources: Vec<Pos> = Vec::new();
        let mut frontier: VecDeque<Pos> = VecDeque::from([start]);
        let missing_source = self.entities.get(&start).and_then(|e| e.source);
        while let Some(pos) = frontier.pop_front() {
            if !self.has(pos) {
                continue;
            }
            self.remove_source(level, pos);
            self.send(pos);
            for neighbour in self.connected_neighbours(level, pos) {
                if Some(neighbour) == missing_source {
                    continue;
                }
                let n = self.e(neighbour);
                let Some(source) = n.source else {
                    continue;
                };
                if source != pos {
                    potential_new_sources.push(neighbour);
                    continue;
                }
                if n.is_source() {
                    potential_new_sources.push(neighbour);
                }
                frontier.push_back(neighbour);
            }
        }
        for source in potential_new_sources {
            let Some(e) = self.entities.get(&source) else {
                continue;
            };
            if e.has_source() || e.is_source() {
                self.propagate_new_source(level, source);
                return;
            }
        }
    }
    fn is_connected(&self, from: Pos, to: Pos) -> bool {
        self.rotation_speed_modifier(from, to) != 0.0
    }
    fn connected_neighbours(&self, level: &dyn Level, pos: Pos) -> Vec<Pos> {
        if !self.has(pos) {
            return Vec::new();
        }
        let kind = self.e(pos).kind;
        self.potential_neighbour_locations(level, pos, kind)
            .into_iter()
            .filter(|&n| {
                level.kind(n).is_some()
                    && self.has(n)
                    && (self.is_connected(pos, n) || self.is_connected(n, pos))
            })
            .collect()
    }
    /// `getPotentialNeighbourLocations` with `addPropagationLocations`.
    fn potential_neighbour_locations(&self, level: &dyn Level, pos: Pos, kind: Kind) -> Vec<Pos> {
        let mut out = Vec::new();
        if !level.is_loaded(pos) {
            return out;
        }
        for d in Dir::ALL {
            let relative = add(pos, d.normal());
            if level.is_loaded(relative) {
                out.push(relative);
            }
        }
        // canPropagateDiagonally: small cogs, and sloped belts.
        let diagonal = match kind {
            Kind::Belt(b) => b.propagates_diagonally(),
            k => k.is_small_cog(),
        };
        if !diagonal {
            return out;
        }
        let axis = kind.rotation_axis();
        // BlockPos.betweenClosed: x fastest, then y, then z.
        for z in -1..=1 {
            for y in -1..=1 {
                for x in -1..=1 {
                    let offset = [x, y, z];
                    if axis.choose(offset) != 0 {
                        continue;
                    }
                    if x * x + y * y + z * z != 2 {
                        continue;
                    }
                    out.push(add(pos, offset));
                }
            }
        }
        out
    }

    // ---- The world's side ---------------------------------------------

    /// A rotating block was placed (or loaded with no saved entity): its
    /// block entity attaches on its first tick.
    pub fn on_placed(&mut self, pos: Pos, kind: Kind) {
        self.entities.insert(pos, Kinetic::new(kind));
        self.send(pos);
    }
    /// The block at `pos` changed to `kind` (none: not ours any more).
    pub fn on_changed(&mut self, level: &mut dyn Level, pos: Pos, kind: Option<Kind>) {
        match (self.entities.get(&pos).map(|e| e.kind), kind) {
            (None, None) => {}
            (None, Some(k)) => self.on_placed(pos, k),
            (Some(_), None) => self.remove_entity(level, pos),
            (Some(old), Some(new)) if old == new => {}
            (Some(old), Some(new)) if old.equivalent(new) => {
                self.em(pos).kind = new;
                self.send(pos);
            }
            (Some(_), Some(new)) => {
                // KineticBlockEntity.switchToBlockState on a change that
                // isn't kinetically equivalent.
                if let Some(id) = self.e(pos).network {
                    self.network_remove(level, id, pos);
                }
                self.detach_kinetics(level, pos);
                self.remove_source(level, pos);
                self.em(pos).kind = new;
                self.em(pos).update_speed = true;
                self.send(pos);
            }
        }
    }
    /// `KineticBlockEntity.remove`: the block is gone.
    pub fn remove_entity(&mut self, level: &mut dyn Level, pos: Pos) {
        if !self.has(pos) {
            return;
        }
        if let Some(id) = self.e(pos).network {
            self.network_remove(level, id, pos);
        }
        let removed = self.entities.remove(&pos).expect("kinetic entity");
        self.dirty.insert(pos);
        self.handle_removed(level, pos, removed.kind, removed.speed);
    }
    /// `Level.destroyBlock` from inside the kinetics: the entity goes now,
    /// the block when the world applies it.
    fn destroy(&mut self, level: &mut dyn Level, pos: Pos) {
        self.remove_entity(level, pos);
        level.destroy_block(pos);
    }
    /// A server tick of every entity, in position order.
    pub fn tick(&mut self, level: &mut dyn Level) {
        for pos in self.positions() {
            if self.has(pos) {
                self.tick_one(level, pos);
            }
        }
    }
    fn tick_one(&mut self, level: &mut dyn Level, pos: Pos) {
        // KineticBlockEntity.tick
        if self.e(pos).update_speed {
            self.attach_kinetics(level, pos);
        }
        if !self.has(pos) {
            return;
        }
        if !self.e(pos).initialized {
            self.em(pos).initialized = true;
            self.initialize(level, pos);
            if !self.has(pos) {
                return;
            }
        }
        let e = self.em(pos);
        e.validation_countdown -= 1;
        if e.validation_countdown < 0 {
            e.validation_countdown = VALIDATION_FREQUENCY;
            self.validate_kinetics(level, pos);
        }
        if !self.has(pos) {
            return;
        }
        let e = self.em(pos);
        if e.flicker_tally > 0 {
            e.flicker_tally -= 1;
        }
        if e.network_dirty {
            if let Some(id) = e.network
                && self.networks.contains_key(&id)
            {
                self.network_update(level, id);
            }
            if let Some(e) = self.entities.get_mut(&pos) {
                e.network_dirty = false;
            }
        }
        if !self.has(pos) || !self.e(pos).kind.is_generator() {
            return;
        }
        // GeneratingKineticBlockEntity.tick
        if self.e(pos).reactivate_source {
            self.update_generated_rotation(level, pos);
            if let Some(e) = self.entities.get_mut(&pos) {
                e.reactivate_source = false;
            }
        }
        // HandCrankBlockEntity.tick
        if let Some(e) = self.entities.get_mut(&pos)
            && matches!(e.kind, Kind::HandCrank(_))
            && e.crank_in_use > 0
        {
            e.crank_in_use -= 1;
            if e.crank_in_use == 0 {
                self.update_generated_rotation(level, pos);
            }
        }
    }
    /// `HandCrankBlockEntity.turn`: a player turned the crank at `pos`
    /// (`back`: sneaking).
    pub fn turn_crank(&mut self, level: &mut dyn Level, pos: Pos, back: bool) -> bool {
        let Some(e) = self.entities.get_mut(&pos) else {
            return false;
        };
        if !matches!(e.kind, Kind::HandCrank(_)) {
            return false;
        }
        let update = e.generated_speed() == 0.0 || back != e.crank_backwards;
        e.crank_in_use = CRANK_USE_TICKS;
        e.crank_backwards = back;
        if update {
            self.update_generated_rotation(level, pos);
        }
        true
    }
    /// The press at `pos`'s pressing behaviour.
    pub fn pressing_mut(&mut self, pos: Pos) -> Option<&mut Pressing> {
        let e = self.entities.get_mut(&pos)?;
        if !matches!(e.kind, Kind::MechanicalPress(_)) {
            return None;
        }
        self.dirty.insert(pos);
        Some(&mut e.pressing)
    }
    /// `PressingBehaviour.tick` of the press at `pos`; clients hear of it
    /// while it runs and when it stops.
    pub fn tick_press(&mut self, level: &dyn Level, pos: Pos) -> Option<(PressStep, PressMode)> {
        let speed = self.speed_of(level, pos);
        let e = self.entities.get_mut(&pos)?;
        if !matches!(e.kind, Kind::MechanicalPress(_)) {
            return None;
        }
        let was = e.pressing.running;
        let step = e.pressing.tick(speed);
        if was || e.pressing.running {
            self.dirty.insert(pos);
        }
        Some((step, e.pressing.mode))
    }
    /// Every press, for their tick after the kinetics'.
    pub fn presses(&self) -> Vec<Pos> {
        self.positions()
            .into_iter()
            .filter(|p| matches!(self.entities[p].kind, Kind::MechanicalPress(_)))
            .collect()
    }
    /// `MechanicalMixerBlockEntity.tick` of the mixer at `pos`; clients hear
    /// of it while it runs and when it stops.
    pub fn tick_mixer(&mut self, level: &dyn Level, pos: Pos) -> Option<MixStep> {
        let speed = self.speed_of(level, pos);
        let e = self.entities.get_mut(&pos)?;
        if !matches!(e.kind, Kind::MechanicalMixer) {
            return None;
        }
        let was = (e.mixing.running, e.mixing.running_ticks);
        let step = e.mixing.tick(speed);
        if was.0 || e.mixing.running || was != (e.mixing.running, e.mixing.running_ticks) {
            self.dirty.insert(pos);
        }
        Some(step)
    }
    /// The mixer at `pos`'s cycle.
    pub fn mixing_mut(&mut self, pos: Pos) -> Option<&mut Mixing> {
        let e = self.entities.get_mut(&pos)?;
        if !matches!(e.kind, Kind::MechanicalMixer) {
            return None;
        }
        self.dirty.insert(pos);
        Some(&mut e.mixing)
    }
    /// Every mixer, for their tick after the kinetics'.
    pub fn mixers(&self) -> Vec<Pos> {
        self.positions()
            .into_iter()
            .filter(|p| matches!(self.entities[p].kind, Kind::MechanicalMixer))
            .collect()
    }
    /// Every encased fan, with its facing and speed, for their air currents.
    pub fn fans(&self, level: &dyn Level) -> Vec<(Pos, Dir, f32)> {
        self.positions()
            .into_iter()
            .filter_map(|p| match self.entities[&p].kind {
                Kind::EncasedFan(f) => Some((p, f, self.speed_of(level, p))),
                _ => None,
            })
            .collect()
    }
    /// Every water wheel, for their flow check.
    /// The water wheels: their place, axis and whether large.
    pub fn water_wheels(&self) -> Vec<(Pos, Axis, bool)> {
        self.positions()
            .into_iter()
            .filter_map(|p| match self.entities[&p].kind {
                Kind::WaterWheel(f) => Some((p, f.axis(), false)),
                Kind::LargeWaterWheel(a) => Some((p, a, true)),
                _ => None,
            })
            .collect()
    }
    /// `WaterWheelBlockEntity.setFlowScoreAndUpdate`.
    pub fn set_flow_score(&mut self, level: &mut dyn Level, pos: Pos, score: i32) {
        let Some(e) = self.entities.get_mut(&pos) else {
            return;
        };
        if e.flow_score == score {
            return;
        }
        e.flow_score = score;
        self.update_generated_rotation(level, pos);
    }
    /// `applyMaterialIfValid`: the wheel at `pos` is made of `material`
    /// (or the belt segment dyed so); false when it already was.
    pub fn set_wheel_material(&mut self, pos: Pos, material: u8) -> bool {
        let Some(e) = self.entities.get_mut(&pos) else {
            return false;
        };
        if e.material == material {
            return false;
        }
        e.material = material;
        self.dirty.insert(pos);
        true
    }
    /// The creative motor's scroll value changed.
    pub fn set_motor_speed(&mut self, level: &mut dyn Level, pos: Pos, rpm: i32) -> bool {
        let Some(e) = self.entities.get_mut(&pos) else {
            return false;
        };
        if !matches!(e.kind, Kind::CreativeMotor(_)) {
            return false;
        }
        e.motor_speed = rpm.clamp(-(MAX_ROTATION_SPEED as i32), MAX_ROTATION_SPEED as i32);
        self.update_generated_rotation(level, pos);
        true
    }
    #[allow(dead_code)]
    /// The goggles' numbers at `pos`: speed, network stress and capacity.
    pub fn stats(&self, level: &dyn Level, pos: Pos) -> Option<(f32, f32, f32, bool)> {
        let e = self.entities.get(&pos)?;
        Some((
            self.speed_of(level, pos),
            e.stress,
            e.capacity,
            e.over_stressed,
        ))
    }

    // ---- Saving -------------------------------------------------------

    /// One line per entity, as `KineticBlockEntity.write` saves it.
    pub fn save(&self) -> String {
        let mut out = String::from("create-kinetics 1\n");
        for pos in self.positions() {
            let e = &self.entities[&pos];
            let source = e
                .source
                .map_or("-".to_owned(), |s| format!("{},{},{}", s[0], s[1], s[2]));
            let network = e.network.map_or("-".to_owned(), |n| n.to_string());
            out.push_str(&format!(
                "{} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}\n",
                pos[0],
                pos[1],
                pos[2],
                kind_key(e.kind),
                e.speed,
                source,
                network,
                e.stress,
                e.capacity,
                e.network_size,
                e.last_stress_applied,
                e.last_capacity_provided,
                e.motor_speed,
                u8::from(e.crank_backwards),
                e.flow_score,
                u8::from(e.pressing.running),
                e.pressing.running_ticks,
                u8::from(e.pressing.mode == PressMode::Belt),
                u8::from(e.mixing.running),
                e.mixing.running_ticks,
                e.material,
            ));
        }
        out
    }
    /// The entities `save` wrote. Each attaches again on its first tick,
    /// as a loaded block entity does (`NeedsSpeedUpdate` aside).
    pub fn load(text: &str) -> Kinetics {
        let mut k = Kinetics::new();
        let mut lines = text.lines();
        if lines.next() != Some("create-kinetics 1") {
            return k;
        }
        for line in lines {
            let f: Vec<&str> = line.split(' ').collect();
            // 14 fields before the water wheel and the press, 18 before the
            // mixer, 20 before the wheel's wood, 21 since.
            if ![14, 18, 20, 21].contains(&f.len()) {
                continue;
            }
            let num = |i: usize| f[i].parse::<f32>().ok();
            let int = |i: usize| f[i].parse::<i32>().ok();
            let (Some(x), Some(y), Some(z)) = (int(0), int(1), int(2)) else {
                continue;
            };
            let Some(kind) = parse_kind_key(f[3]) else {
                continue;
            };
            let mut e = Kinetic::new(kind);
            e.speed = num(4).unwrap_or(0.0);
            e.source = parse_pos(f[5]);
            e.network = f[6].parse().ok();
            e.stress = num(7).unwrap_or(0.0);
            e.capacity = num(8).unwrap_or(0.0);
            e.network_size = int(9).unwrap_or(0);
            e.last_stress_applied = num(10).unwrap_or(0.0);
            e.last_capacity_provided = num(11).unwrap_or(0.0);
            e.motor_speed = int(12).unwrap_or(MOTOR_DEFAULT_SPEED);
            e.crank_backwards = f[13] == "1";
            if f.len() >= 18 {
                e.flow_score = int(14).unwrap_or(0);
                e.pressing.running = f[15] == "1";
                e.pressing.running_ticks = int(16).unwrap_or(0);
                e.pressing.prev_running_ticks = e.pressing.running_ticks;
                e.pressing.mode = if f[17] == "1" {
                    PressMode::Belt
                } else {
                    PressMode::World
                };
            }
            if f.len() >= 21 {
                e.material = f[20].parse().unwrap_or(0);
            }
            if f.len() >= 20 {
                // MechanicalMixerBlockEntity.read: `Running` and `Ticks`.
                e.mixing.running = f[18] == "1";
                e.mixing.running_ticks = int(19).unwrap_or(0);
            }
            e.over_stressed = e.network.is_some() && e.capacity < e.stress;
            e.update_speed = false;
            k.entities.insert([x, y, z], e);
            // Clients learn of loaded entities on the first tick.
            k.dirty.insert([x, y, z]);
        }
        k
    }
}

fn kind_key(k: Kind) -> String {
    let state = match k {
        Kind::Shaft(a)
        | Kind::Cogwheel(a)
        | Kind::LargeCogwheel(a)
        | Kind::Gearbox(a)
        | Kind::RopePulley(a)
        | Kind::LargeWaterWheel(a) => a.name(),
        Kind::CreativeMotor(f)
        | Kind::HandCrank(f)
        | Kind::MechanicalPress(f)
        | Kind::WaterWheel(f)
        | Kind::EncasedFan(f)
        | Kind::MechanicalPump(f)
        | Kind::MechanicalBearing(f)
        | Kind::WindmillBearing(f)
        | Kind::Drill(f) => f.name(),
        Kind::Millstone | Kind::MechanicalMixer | Kind::MechanicalArm => "",
        Kind::Deployer(f, _)
        | Kind::MechanicalPiston(f, ..)
        | Kind::Saw(f, _)
        | Kind::Gauge(_, f, _) => f.name(),
        Kind::ElevatorPulley(f) => f.name(),
        Kind::GantryShaft(f) | Kind::GantryCarriage(f, _) => f.name(),
        Kind::Belt(b) => {
            let face = b.block().map_or("floor", |(_, face)| face);
            return format!("{}[{};{face}]", k.block(), b.facing.name());
        }
    };
    format!("{}[{}]", k.block(), state)
}

fn parse_kind_key(s: &str) -> Option<Kind> {
    let (block, rest) = s.split_once('[')?;
    let state = rest.strip_suffix(']')?;
    let (first, second) = state.split_once(';').unwrap_or((state, ""));
    Kind::from_block(block, |key| match key {
        "axis" | "facing" => Some(first),
        "face" => Some(second),
        _ => None,
    })
}

fn parse_pos(s: &str) -> Option<Pos> {
    let mut it = s.split(',').map(|v| v.parse::<i32>().ok());
    Some([it.next()??, it.next()??, it.next()??])
}

fn is_large_to_large_gear(from: Kind, to: Kind, diff: Pos) -> bool {
    if !from.is_large_cog() || !to.is_large_cog() {
        return false;
    }
    let (fa, ta) = (from.rotation_axis(), to.rotation_axis());
    if fa == ta {
        return false;
    }
    for axis in Axis::ALL {
        let d = axis.choose(diff);
        if axis == fa || axis == ta {
            if d == 0 {
                return false;
            }
        } else if d != 0 {
            return false;
        }
    }
    true
}

fn is_large_to_small_cog(from: Kind, to: Kind, diff: Pos) -> bool {
    let axis_from = from.rotation_axis();
    if axis_from != to.rotation_axis() {
        return false;
    }
    if axis_from.choose(diff) != 0 {
        return false;
    }
    Axis::ALL
        .iter()
        .filter(|&&a| a != axis_from)
        .all(|&a| a.choose(diff).abs() == 1)
}

/// `KineticBlockEntityRenderer.getAngleForBe` in degrees: the angle a
/// rotating part shows at `time` ticks (`AnimationTickHolder.getRenderTime`).
pub fn render_angle(kind: Kind, pos: Pos, speed: f32, time: f32) -> f32 {
    (time * speed * 3.0 / 10.0 + rotation_offset(kind, kind.rotation_axis(), pos)) % 360.0
}

// ---- The client's view, over the mod's message channel -----------------

/// What a client knows of one kinetic block entity: what it draws and
/// what the goggles show.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub kind: Kind,
    /// `getSpeed`: zero while overstressed.
    pub speed: f32,
    /// Where its source is (`DirectionalShaftHalvesBlockEntity.getSourceFacing`).
    pub source: Option<Dir>,
    /// Its network's stress and capacity.
    pub stress: f32,
    pub capacity: f32,
    /// By kind: a press's or a mixer's running ticks (-1 at rest), a
    /// creative motor's speed setting (its scroll value); -1 otherwise.
    pub aux: i16,
    /// Whether a press reaches a depot.
    pub press_belt: bool,
}

impl View {
    pub fn over_stressed(&self) -> bool {
        self.capacity < self.stress
    }
}

/// Bytes of one entry.
const ENTRY: usize = 30;
/// Entries in one message, under the 30 000 byte cap.
pub const ENTRIES_PER_MESSAGE: usize = 1000;

fn kind_code(k: Kind) -> (u8, u8) {
    let axis = |a: Axis| a as u8;
    let dir = |d: Dir| d as u8;
    match k {
        Kind::Shaft(a) => (0, axis(a)),
        Kind::Cogwheel(a) => (1, axis(a)),
        Kind::LargeCogwheel(a) => (2, axis(a)),
        Kind::Gearbox(a) => (3, axis(a)),
        Kind::CreativeMotor(d) => (4, dir(d)),
        Kind::HandCrank(d) => (5, dir(d)),
        Kind::MechanicalPress(d) => (6, dir(d)),
        Kind::Millstone => (7, 0),
        Kind::WaterWheel(d) => (8, dir(d)),
        Kind::EncasedFan(d) => (9, dir(d)),
        Kind::MechanicalPump(d) => (12, dir(d)),
        Kind::MechanicalMixer => (10, 0),
        Kind::Belt(b) => {
            let slope = Slope::ALL.iter().position(|s| *s == b.slope).unwrap_or(0) as u8;
            let part = Part::ALL.iter().position(|p| *p == b.part).unwrap_or(0) as u8;
            (11, dir(b.facing) | slope << 3 | part << 6)
        }
        Kind::MechanicalArm => (14, 0),
        Kind::Deployer(d, along) => (13, dir(d) | u8::from(along) << 3),
        Kind::MechanicalPiston(d, along, sticky) => {
            (15, dir(d) | u8::from(along) << 3 | u8::from(sticky) << 4)
        }
        Kind::MechanicalBearing(d) => (16, dir(d)),
        Kind::WindmillBearing(d) => (17, dir(d)),
        Kind::RopePulley(a) => (18, axis(a)),
        Kind::GantryShaft(d) => (19, dir(d)),
        Kind::GantryCarriage(d, along) => (20, dir(d) | u8::from(along) << 3),
        Kind::Drill(d) => (21, dir(d)),
        Kind::Saw(d, along) => (22, dir(d) | u8::from(along) << 3),
        Kind::ElevatorPulley(d) => (23, dir(d)),
        Kind::LargeWaterWheel(a) => (24, axis(a)),
        Kind::Gauge(stress, d, along) => {
            (25, dir(d) | u8::from(along) << 3 | u8::from(stress) << 4)
        }
    }
}

fn kind_from_code(tag: u8, state: u8) -> Option<Kind> {
    let axis = Axis::ALL.get(usize::from(state)).copied();
    let dir = Dir::ALL.get(usize::from(state)).copied();
    Some(match tag {
        0 => Kind::Shaft(axis?),
        1 => Kind::Cogwheel(axis?),
        2 => Kind::LargeCogwheel(axis?),
        3 => Kind::Gearbox(axis?),
        4 => Kind::CreativeMotor(dir?),
        5 => Kind::HandCrank(dir?),
        6 => Kind::MechanicalPress(dir?),
        7 => Kind::Millstone,
        8 => Kind::WaterWheel(dir?),
        9 => Kind::EncasedFan(dir?),
        12 => Kind::MechanicalPump(dir?),
        10 => Kind::MechanicalMixer,
        11 => Kind::Belt(BeltState {
            facing: Dir::ALL.get(usize::from(state & 7)).copied()?,
            slope: Slope::ALL.get(usize::from(state >> 3 & 7)).copied()?,
            part: Part::ALL[usize::from(state >> 6)],
        }),
        14 => Kind::MechanicalArm,
        13 => Kind::Deployer(
            Dir::ALL.get(usize::from(state & 7)).copied()?,
            state & 8 != 0,
        ),
        15 => Kind::MechanicalPiston(
            Dir::ALL.get(usize::from(state & 7)).copied()?,
            state & 8 != 0,
            state & 16 != 0,
        ),
        16 => Kind::MechanicalBearing(dir?),
        17 => Kind::WindmillBearing(dir?),
        18 => Kind::RopePulley(axis?),
        19 => Kind::GantryShaft(dir?),
        20 => Kind::GantryCarriage(
            Dir::ALL.get(usize::from(state & 7)).copied()?,
            state & 8 != 0,
        ),
        21 => Kind::Drill(dir?),
        23 => Kind::ElevatorPulley(dir?),
        24 => Kind::LargeWaterWheel(axis?),
        25 => Kind::Gauge(
            state & 16 != 0,
            Dir::ALL.get(usize::from(state & 7)).copied()?,
            state & 8 != 0,
        ),
        22 => Kind::Saw(
            Dir::ALL.get(usize::from(state & 7)).copied()?,
            state & 8 != 0,
        ),
        _ => return None,
    })
}

/// One update for clients: the dimension's index (0 overworld, 1 nether,
/// 2 end), whether it replaces everything the client knows there, and the
/// entities (none: gone).
pub fn encode_update(dim: u8, full: bool, entries: &[(Pos, Option<View>)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + entries.len() * ENTRY);
    out.extend_from_slice(b"K3");
    out.push(dim);
    out.push(u8::from(full));
    for (pos, view) in entries {
        for v in pos {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let ((tag, state), speed, source, stress, capacity, press) = match view {
            Some(v) => (
                kind_code(v.kind),
                v.speed,
                v.source.map_or(255, |d| d as u8),
                v.stress,
                v.capacity,
                (v.aux, v.press_belt),
            ),
            None => ((255, 0), 0.0, 255, 0.0, 0.0, (-1, false)),
        };
        out.push(tag);
        out.push(state);
        out.extend_from_slice(&speed.to_le_bytes());
        out.push(source);
        out.extend_from_slice(&stress.to_le_bytes());
        out.extend_from_slice(&capacity.to_le_bytes());
        out.extend_from_slice(&press.0.to_le_bytes());
        out.push(u8::from(press.1));
    }
    out
}

pub type Update = (u8, bool, Vec<(Pos, Option<View>)>);

pub fn decode_update(data: &[u8]) -> Option<Update> {
    let rest = data.strip_prefix(b"K3")?;
    let (&dim, rest) = rest.split_first()?;
    let (&full, rest) = rest.split_first()?;
    if rest.len() % ENTRY != 0 {
        return None;
    }
    let int = |b: &[u8]| i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let float = |b: &[u8]| f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let entries = rest
        .as_chunks::<ENTRY>()
        .0
        .iter()
        .map(|c| {
            let pos = [int(&c[0..4]), int(&c[4..8]), int(&c[8..12])];
            let view = kind_from_code(c[12], c[13]).map(|kind| View {
                kind,
                speed: float(&c[14..18]),
                source: Dir::ALL.get(usize::from(c[18])).copied(),
                stress: float(&c[19..23]),
                capacity: float(&c[23..27]),
                aux: i16::from_le_bytes([c[27], c[28]]),
                press_belt: c[29] != 0,
            });
            (pos, view)
        })
        .collect();
    Some((dim, full != 0, entries))
}

/// What a client knows of the turning blocks: one dimension's at a time,
/// the one its player is in.
#[derive(Debug, Default)]
pub struct ClientView {
    pub dim: u8,
    pub blocks: HashMap<Pos, View>,
    /// Hand cranks' own angles (`HandCrankBlockEntity.independentAngle`,
    /// `chasingAngularVelocity`) and the render time they were stepped to.
    pub cranks: HashMap<Pos, (f32, f32, f32)>,
}

impl ClientView {
    pub fn apply(&mut self, data: &[u8]) -> bool {
        let Some((dim, full, entries)) = decode_update(data) else {
            return false;
        };
        if full || dim != self.dim {
            self.blocks.clear();
            self.dim = dim;
        }
        for (pos, view) in entries {
            match view {
                Some(v) => {
                    self.blocks.insert(pos, v);
                }
                None => {
                    self.blocks.remove(&pos);
                }
            }
        }
        self.cranks.retain(|p, _| self.blocks.contains_key(p));
        true
    }
    /// The hand crank at `pos`'s handle angle in degrees at render time
    /// `time`: `HandCrankBlockEntity.tick` chases the network's speed a
    /// quarter of the way each tick, and `getIndependentAngle` adds the
    /// partial tick.
    pub fn crank_angle(&mut self, pos: Pos, speed: f32, time: f32) -> f32 {
        let (angle, velocity, at) = self.cranks.entry(pos).or_insert((0.0, 0.0, time.floor()));
        let target = convert_to_angular(speed);
        // Whole ticks since the last frame, at most a second's worth.
        let ticks = (time.floor() - *at).clamp(0.0, 20.0) as u32;
        for _ in 0..ticks {
            *velocity += (target - *velocity) / 4.0;
            *angle = (*angle + *velocity) % 360.0;
        }
        *at = time.floor().max(*at);
        *angle + time.fract() * *velocity
    }
}

/// `KineticBlockEntity.convertToAngular`: degrees per tick at `speed` rpm.
pub fn convert_to_angular(speed: f32) -> f32 {
    speed * 3.0 / 10.0
}

/// `KineticBlockEntityRenderer.getRotationOffsetForPosition` for a part
/// turning about `axis` at `pos` (the cogs' half-tooth offset).
pub fn rotation_offset(kind: Kind, axis: Axis, pos: Pos) -> f32 {
    let sum = Axis::ALL
        .iter()
        .map(|&a| if a == axis { 0 } else { a.choose(pos) })
        .sum::<i32>();
    if sum % 2 == 0 {
        22.5
    } else if kind.is_large_cog() {
        11.25
    } else {
        0.0
    }
}

/// `GearboxRenderer`: the angle in degrees of the gearbox's shaft half
/// facing `side`, turned by where its source is.
pub fn gearbox_shaft_angle(pos: Pos, view: &View, side: Dir, time: f32) -> f32 {
    let mut angle = (time * view.speed * 3.0 / 10.0) % 360.0;
    if view.speed != 0.0
        && let Some(source) = view.source
    {
        if source.axis() == side.axis() {
            angle *= if source == side { 1.0 } else { -1.0 };
        } else if source.positive() == side.positive() {
            angle *= -1.0;
        }
    }
    angle + rotation_offset(view.kind, side.axis(), pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct World {
        blocks: HashMap<Pos, Kind>,
        destroyed: Vec<Pos>,
    }

    impl Level for World {
        fn kind(&self, pos: Pos) -> Option<Kind> {
            self.blocks.get(&pos).copied()
        }
        fn is_loaded(&self, _pos: Pos) -> bool {
            true
        }
        fn destroy_block(&mut self, pos: Pos) {
            self.blocks.remove(&pos);
            self.destroyed.push(pos);
        }
    }

    struct Rig {
        world: World,
        k: Kinetics,
    }

    impl Rig {
        fn new() -> Self {
            Rig {
                world: World::default(),
                k: Kinetics::new(),
            }
        }
        fn place(&mut self, pos: Pos, kind: Kind) {
            self.world.blocks.insert(pos, kind);
            self.k.on_changed(&mut self.world, pos, Some(kind));
        }
        fn remove(&mut self, pos: Pos) {
            self.world.blocks.remove(&pos);
            self.k.on_changed(&mut self.world, pos, None);
        }
        fn ticks(&mut self, n: usize) {
            for _ in 0..n {
                self.k.tick(&mut self.world);
            }
        }
        fn speed(&self, pos: Pos) -> f32 {
            self.k.speed_of(&self.world, pos)
        }
    }

    #[test]
    fn nearest_direction_matches_vanilla() {
        assert_eq!(Dir::nearest([1, 0, 0]), Dir::East);
        assert_eq!(Dir::nearest([0, -1, 0]), Dir::Down);
        // Ties go to the first direction in vanilla's order.
        assert_eq!(Dir::nearest([1, 1, 0]), Dir::Up);
        assert_eq!(Dir::nearest([0, 0, 0]), Dir::North);
    }

    #[test]
    fn network_ids_are_block_pos_longs() {
        assert_eq!(pos_as_long([0, 0, 0]), 0);
        assert_eq!(pos_as_long([1, 2, 3]), (1i64 << 38) | (3 << 12) | 2);
        assert_eq!(pos_as_long([-1, -1, -1]), -1);
    }

    #[test]
    fn motor_turns_a_line_of_shafts() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::CreativeMotor(Dir::East));
        for x in 1..=4 {
            r.place([x, 64, 0], Kind::Shaft(Axis::X));
        }
        r.ticks(2);
        assert_eq!(r.speed([0, 64, 0]), 16.0);
        for x in 1..=4 {
            assert_eq!(r.speed([x, 64, 0]), 16.0, "shaft {x}");
        }
        // A motor facing west turns the other way.
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::CreativeMotor(Dir::West));
        r.place([-1, 64, 0], Kind::Shaft(Axis::X));
        r.ticks(2);
        assert_eq!(r.speed([-1, 64, 0]), -16.0);
    }

    #[test]
    fn a_belt_turns_with_its_pulleys() {
        use super::super::create_belt::{BeltState, Part, Slope};
        let belt = |part, slope| {
            Kind::Belt(BeltState {
                facing: Dir::South,
                slope,
                part,
            })
        };
        let mut r = Rig::new();
        // Motor -> shaft -> belt start at z 0, belt along +z to z 3, shaft out.
        r.place([-2, 64, 0], Kind::CreativeMotor(Dir::East));
        r.place([-1, 64, 0], Kind::Shaft(Axis::X));
        r.place([0, 64, 0], belt(Part::Start, Slope::Horizontal));
        r.place([0, 64, 1], belt(Part::Middle, Slope::Horizontal));
        r.place([0, 64, 2], belt(Part::Middle, Slope::Horizontal));
        r.place([0, 64, 3], belt(Part::End, Slope::Horizontal));
        r.place([1, 64, 3], Kind::Shaft(Axis::X));
        // A parallel belt beside it isn't driven.
        r.place([1, 64, 1], belt(Part::Middle, Slope::Horizontal));
        r.ticks(3);
        for z in 0..4 {
            assert_eq!(r.speed([0, 64, z]), 16.0, "segment {z}");
        }
        assert_eq!(r.speed([1, 64, 3]), 16.0);
        assert_eq!(r.speed([1, 64, 1]), 0.0);
        // A sloped belt reaches its diagonal neighbours.
        let mut r = Rig::new();
        r.place([-2, 64, 0], Kind::CreativeMotor(Dir::East));
        r.place([-1, 64, 0], Kind::Shaft(Axis::X));
        r.place([0, 64, 0], belt(Part::Start, Slope::Upward));
        r.place([0, 65, 1], belt(Part::Middle, Slope::Upward));
        r.place([0, 66, 2], belt(Part::End, Slope::Upward));
        r.ticks(3);
        assert_eq!(r.speed([0, 66, 2]), 16.0);
        // Saved and loaded with its state.
        let saved = r.k.save();
        let loaded = Kinetics::load(&saved);
        assert_eq!(
            loaded.get([0, 65, 1]).map(|e| e.kind),
            Some(belt(Part::Middle, Slope::Upward))
        );
        // And sent to clients.
        let k = belt(Part::Pulley, Slope::Downward);
        let (tag, state) = kind_code(k);
        assert_eq!(kind_from_code(tag, state), Some(k));
    }

    #[test]
    fn shafts_off_axis_do_not_connect() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::CreativeMotor(Dir::East));
        r.place([1, 64, 0], Kind::Shaft(Axis::Z));
        r.ticks(2);
        assert_eq!(r.speed([1, 64, 0]), 0.0);
    }

    #[test]
    fn meshing_cogwheels_reverse() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::CreativeMotor(Dir::East));
        r.place([1, 64, 0], Kind::Cogwheel(Axis::X));
        r.place([1, 65, 0], Kind::Cogwheel(Axis::X));
        r.place([1, 66, 0], Kind::Cogwheel(Axis::X));
        r.ticks(2);
        assert_eq!(r.speed([1, 64, 0]), 16.0);
        assert_eq!(r.speed([1, 65, 0]), -16.0);
        assert_eq!(r.speed([1, 66, 0]), 16.0);
    }

    #[test]
    fn large_cogwheel_halves_and_doubles() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::CreativeMotor(Dir::East));
        r.place([1, 64, 0], Kind::LargeCogwheel(Axis::X));
        // A small cog diagonally beside a large one (same axis).
        r.place([1, 65, 1], Kind::Cogwheel(Axis::X));
        r.ticks(2);
        assert_eq!(r.speed([1, 64, 0]), 16.0);
        assert_eq!(r.speed([1, 65, 1]), -32.0);
    }

    #[test]
    fn gearbox_turns_the_sides() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::CreativeMotor(Dir::East));
        r.place([1, 64, 0], Kind::Gearbox(Axis::Y));
        r.place([2, 64, 0], Kind::Shaft(Axis::X));
        r.place([1, 64, 1], Kind::Shaft(Axis::Z));
        r.place([1, 64, -1], Kind::Shaft(Axis::Z));
        r.ticks(2);
        assert_eq!(r.speed([1, 64, 0]), 16.0);
        // Straight through: the same way. Sideways: by axis direction.
        assert_eq!(r.speed([2, 64, 0]), -16.0);
        assert_eq!(r.speed([1, 64, 1]), 16.0);
        assert_eq!(r.speed([1, 64, -1]), -16.0);
    }

    #[test]
    fn breaking_a_shaft_stops_what_it_drove() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::CreativeMotor(Dir::East));
        for x in 1..=4 {
            r.place([x, 64, 0], Kind::Shaft(Axis::X));
        }
        r.ticks(2);
        r.remove([2, 64, 0]);
        assert_eq!(r.speed([1, 64, 0]), 16.0);
        assert_eq!(r.speed([3, 64, 0]), 0.0);
        assert_eq!(r.speed([4, 64, 0]), 0.0);
        // Putting it back turns them again.
        r.place([2, 64, 0], Kind::Shaft(Axis::X));
        r.ticks(2);
        assert_eq!(r.speed([4, 64, 0]), 16.0);
    }

    #[test]
    fn joining_opposite_lines_pops_the_joint() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::CreativeMotor(Dir::East));
        r.place([1, 64, 0], Kind::Shaft(Axis::X));
        r.place([4, 64, 0], Kind::CreativeMotor(Dir::West));
        r.place([3, 64, 0], Kind::Shaft(Axis::X));
        r.ticks(2);
        assert_eq!((r.speed([1, 64, 0]), r.speed([3, 64, 0])), (16.0, -16.0));
        // The shaft joining them would have to turn both ways: it pops.
        r.place([2, 64, 0], Kind::Shaft(Axis::X));
        r.ticks(1);
        assert_eq!(r.world.destroyed, vec![[2, 64, 0]]);
        assert_eq!((r.speed([1, 64, 0]), r.speed([3, 64, 0])), (16.0, -16.0));
    }

    #[test]
    fn opposite_motor_placed_on_a_running_line_follows_it() {
        // CreativeMotorBlockEntity.initialize compares signed speeds, so a
        // motor facing back into a running line is driven by it.
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::CreativeMotor(Dir::East));
        r.place([1, 64, 0], Kind::Shaft(Axis::X));
        r.ticks(2);
        r.place([2, 64, 0], Kind::CreativeMotor(Dir::West));
        r.ticks(3);
        assert!(r.world.destroyed.is_empty());
        assert_eq!(r.speed([2, 64, 0]), 16.0);
    }

    #[test]
    fn faster_motor_takes_over() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::CreativeMotor(Dir::East));
        r.place([1, 64, 0], Kind::Shaft(Axis::X));
        r.place([2, 64, 0], Kind::Shaft(Axis::X));
        r.ticks(2);
        assert!(r.k.set_motor_speed(&mut r.world, [0, 64, 0], 64));
        r.ticks(1);
        assert_eq!(r.speed([2, 64, 0]), 64.0);
        assert!(r.world.destroyed.is_empty());
    }

    #[test]
    fn scrolling_a_motor_through_zero_breaks_nothing() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::CreativeMotor(Dir::East));
        r.place([1, 64, 0], Kind::Shaft(Axis::X));
        r.place([2, 64, 0], Kind::EncasedFan(Dir::East));
        r.ticks(2);
        let mut v = 16;
        while v > -16 {
            v = motor_scroll_step(v, false);
            assert!(r.k.set_motor_speed(&mut r.world, [0, 64, 0], v));
            r.ticks(1);
            assert!(r.world.destroyed.is_empty(), "at {v}");
            assert_eq!(r.speed([2, 64, 0]), v as f32, "at {v}");
        }
    }

    #[test]
    fn a_motor_jumping_to_the_other_way_breaks_nothing() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::CreativeMotor(Dir::East));
        r.place([1, 64, 0], Kind::Shaft(Axis::X));
        r.place([2, 64, 0], Kind::EncasedFan(Dir::East));
        r.ticks(2);
        for v in [-64, 48, -12, 256] {
            assert!(r.k.set_motor_speed(&mut r.world, [0, 64, 0], v));
            r.ticks(1);
            assert!(r.world.destroyed.is_empty(), "at {v}");
            assert_eq!(r.speed([2, 64, 0]), v as f32, "at {v}");
        }
    }

    #[test]
    fn hand_crank_turns_for_ten_ticks() {
        let mut r = Rig::new();
        // Crank facing up: its shaft points down, into the shaft below it.
        r.place([0, 65, 0], Kind::HandCrank(Dir::Up));
        r.place([0, 64, 0], Kind::Shaft(Axis::Y));
        r.ticks(2);
        assert_eq!(r.speed([0, 64, 0]), 0.0);
        assert!(r.k.turn_crank(&mut r.world, [0, 65, 0], false));
        r.ticks(1);
        assert_eq!(r.speed([0, 64, 0]), 32.0);
        r.ticks(10);
        assert_eq!(r.speed([0, 65, 0]), 0.0);
        assert_eq!(r.speed([0, 64, 0]), 0.0);
    }

    #[test]
    fn presses_overstress_a_hand_crank() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::HandCrank(Dir::West));
        r.place([1, 64, 0], Kind::MechanicalPress(Dir::East));
        r.ticks(2);
        r.k.turn_crank(&mut r.world, [0, 64, 0], false);
        r.ticks(2);
        // One press: 8 su x 32 rpm = 256 of 8 x 32 = 256 capacity.
        let (speed, stress, capacity, over) = r.k.stats(&r.world, [1, 64, 0]).unwrap();
        assert_eq!(
            (speed.abs(), stress, capacity, over),
            (32.0, 256.0, 256.0, false)
        );
        // A second press tips it over: everything stops.
        r.place([2, 64, 0], Kind::MechanicalPress(Dir::East));
        r.k.turn_crank(&mut r.world, [0, 64, 0], false);
        r.ticks(2);
        let (speed, stress, capacity, over) = r.k.stats(&r.world, [1, 64, 0]).unwrap();
        assert_eq!((speed, stress, capacity, over), (0.0, 512.0, 256.0, true));
        assert_eq!(r.speed([2, 64, 0]), 0.0);
    }

    #[test]
    fn creative_motor_carries_presses() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::CreativeMotor(Dir::East));
        for x in 1..=8 {
            r.place([x, 64, 0], Kind::MechanicalPress(Dir::East));
        }
        r.ticks(2);
        let (speed, stress, capacity, over) = r.k.stats(&r.world, [8, 64, 0]).unwrap();
        assert_eq!(
            (speed, stress, capacity, over),
            (16.0, 8.0 * 8.0 * 16.0, 16384.0 * 16.0, false)
        );
    }

    #[test]
    fn save_and_load_keep_the_network_turning() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::CreativeMotor(Dir::East));
        r.place([1, 64, 0], Kind::Shaft(Axis::X));
        r.place([2, 64, 0], Kind::MechanicalPress(Dir::East));
        r.ticks(3);
        let text = r.k.save();
        let mut loaded = Rig {
            world: World {
                blocks: r.world.blocks.clone(),
                destroyed: Vec::new(),
            },
            k: Kinetics::load(&text),
        };
        assert_eq!(loaded.k.len(), 3);
        loaded.ticks(3);
        assert_eq!(loaded.speed([1, 64, 0]), 16.0);
        assert_eq!(loaded.speed([2, 64, 0]), 16.0);
        let (_, stress, _, over) = loaded.k.stats(&loaded.world, [2, 64, 0]).unwrap();
        assert_eq!((stress, over), (8.0 * 16.0, false));
        // Breaking the motor after a load still stops the line.
        loaded.remove([0, 64, 0]);
        loaded.ticks(1);
        assert_eq!(loaded.speed([2, 64, 0]), 0.0);
    }

    #[test]
    fn render_angle_offsets_alternate() {
        // Cogs on alternating positions are offset half a tooth.
        let a = render_angle(Kind::Cogwheel(Axis::Y), [0, 64, 0], 0.0, 0.0);
        let b = render_angle(Kind::Cogwheel(Axis::Y), [1, 64, 0], 0.0, 0.0);
        assert_eq!((a, b), (22.5, 0.0));
        assert_eq!(
            render_angle(Kind::LargeCogwheel(Axis::Y), [1, 64, 0], 0.0, 0.0),
            11.25
        );
        // 16 rpm for 20 ticks: 96 degrees (3/10 degree per rpm-tick).
        assert_eq!(
            render_angle(Kind::Shaft(Axis::Y), [1, 64, 0], 16.0, 20.0),
            96.0
        );
    }

    #[test]
    fn kinds_parse_from_block_states() {
        let k = Kind::from_block("create:shaft", |p| (p == "axis").then_some("z"));
        assert_eq!(k, Some(Kind::Shaft(Axis::Z)));
        let k = Kind::from_block("create:hand_crank", |p| (p == "facing").then_some("up"));
        assert_eq!(k, Some(Kind::HandCrank(Dir::Up)));
        assert_eq!(Kind::from_block("minecraft:stone", |_| None), None);
        assert_eq!(
            parse_kind_key(&kind_key(Kind::Gearbox(Axis::X))),
            Some(Kind::Gearbox(Axis::X))
        );
    }

    #[test]
    fn updates_round_trip_to_the_client_view() {
        let view = |kind, speed| View {
            kind,
            speed,
            source: Some(Dir::West),
            stress: 128.0,
            capacity: 256.0,
            aux: -1,
            press_belt: false,
        };
        let entries = vec![
            ([1, -64, 3], Some(view(Kind::Shaft(Axis::Z), 16.0))),
            (
                [-7, 70, 2],
                Some(View {
                    source: None,
                    ..view(Kind::HandCrank(Dir::Down), -32.0)
                }),
            ),
            ([0, 0, 0], None),
        ];
        let data = encode_update(1, false, &entries);
        assert_eq!(data.len(), 4 + 3 * ENTRY);
        assert_eq!(decode_update(&data).unwrap(), (1, false, entries));
        let mut client = ClientView::default();
        client
            .blocks
            .insert([0, 0, 0], view(Kind::Cogwheel(Axis::X), 8.0));
        assert!(client.apply(&encode_update(0, false, &[([0, 0, 0], None)])));
        assert!(client.blocks.is_empty());
        assert!(client.apply(&data));
        assert_eq!((client.dim, client.blocks.len()), (1, 2));
        // A full update forgets what it doesn't list.
        assert!(client.apply(&encode_update(1, true, &[])));
        assert!(client.blocks.is_empty());
        assert!(!client.apply(b"nonsense"));
    }

    #[test]
    fn crank_handle_chases_the_network_speed() {
        let mut client = ClientView::default();
        let p = [0, 64, 0];
        assert_eq!(client.crank_angle(p, 32.0, 100.0), 0.0);
        // One tick: a quarter of 9.6 degrees a tick.
        assert!((client.crank_angle(p, 32.0, 101.0) - 2.4).abs() < 1e-4);
        // Long after, it turns at the full 9.6 degrees a tick.
        for t in 102..200 {
            client.crank_angle(p, 32.0, t as f32);
        }
        let a = client.crank_angle(p, 32.0, 200.0);
        let b = client.crank_angle(p, 32.0, 201.0);
        assert!(((b - a).rem_euclid(360.0) - 9.6).abs() < 1e-3);
    }

    #[test]
    fn gearbox_halves_turn_by_their_source() {
        let v = View {
            kind: Kind::Gearbox(Axis::Y),
            speed: 16.0,
            source: Some(Dir::West),
            stress: 0.0,
            capacity: 0.0,
            aux: -1,
            press_belt: false,
        };
        // Time 20 at 16 rpm: 96 degrees, before the half-tooth offsets.
        let p = [1, 64, 1];
        assert_eq!(gearbox_shaft_angle(p, &v, Dir::West, 20.0), 96.0);
        assert_eq!(gearbox_shaft_angle(p, &v, Dir::East, 20.0), -96.0);
        // Sideways: by axis direction, as the kinetics turn them.
        assert_eq!(gearbox_shaft_angle(p, &v, Dir::North, 20.0), -96.0);
        assert_eq!(gearbox_shaft_angle(p, &v, Dir::South, 20.0), 96.0);
        // On an even position the half-tooth offset adds on.
        assert_eq!(
            gearbox_shaft_angle([0, 64, 0], &v, Dir::North, 20.0),
            -96.0 + 22.5
        );
    }

    #[test]
    fn a_millstone_turns_from_below_and_loads_the_network() {
        let mut r = Rig::new();
        r.place([0, 63, 0], Kind::CreativeMotor(Dir::Up));
        r.place([0, 64, 0], Kind::Millstone);
        // A shaft beside it doesn't reach it.
        r.place([1, 64, 0], Kind::Shaft(Axis::X));
        r.ticks(2);
        assert_eq!(r.speed([0, 64, 0]), 16.0);
        assert_eq!(r.speed([1, 64, 0]), 0.0);
        let (_, stress, _, _) = r.k.stats(&r.world, [0, 64, 0]).unwrap();
        assert_eq!(stress, 4.0 * 16.0);
    }

    #[test]
    fn a_water_wheel_turns_its_axle_by_its_flow() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::WaterWheel(Dir::East));
        r.place([1, 64, 0], Kind::Shaft(Axis::X));
        r.ticks(2);
        assert_eq!(r.speed([1, 64, 0]), 0.0);
        r.k.set_flow_score(&mut r.world, [0, 64, 0], 2);
        r.ticks(1);
        assert_eq!(r.speed([1, 64, 0]), 8.0);
        let (_, _, capacity, _) = r.k.stats(&r.world, [1, 64, 0]).unwrap();
        assert_eq!(capacity, 32.0 * 8.0);
        r.k.set_flow_score(&mut r.world, [0, 64, 0], 0);
        r.ticks(1);
        assert_eq!(r.speed([1, 64, 0]), 0.0);
    }

    #[test]
    fn a_running_press_is_saved() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::MechanicalPress(Dir::East));
        r.ticks(1);
        r.k.pressing_mut([0, 64, 0]).unwrap().start(PressMode::Belt);
        r.k.pressing_mut([0, 64, 0]).unwrap().running_ticks = 60;
        let loaded = Kinetics::load(&r.k.save());
        let p = loaded.get([0, 64, 0]).unwrap().pressing;
        assert!(p.running && p.running_ticks == 60 && p.mode == PressMode::Belt);
        let v = loaded.view(&r.world, [0, 64, 0]).unwrap();
        assert_eq!((v.aux, v.press_belt), (60, true));
    }

    #[test]
    fn a_fan_turns_from_behind_and_loads_the_network() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::CreativeMotor(Dir::East));
        // The fan blows east: its shaft is on its west side.
        r.place([1, 64, 0], Kind::EncasedFan(Dir::East));
        r.ticks(2);
        assert_eq!(r.speed([1, 64, 0]), 16.0);
        let (_, stress, _, _) = r.k.stats(&r.world, [1, 64, 0]).unwrap();
        assert_eq!(stress, 2.0 * 16.0);
        // Facing the motor, its shaft points away: it doesn't turn.
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::CreativeMotor(Dir::East));
        r.place([1, 64, 0], Kind::EncasedFan(Dir::West));
        r.ticks(2);
        assert_eq!(r.speed([1, 64, 0]), 0.0);
    }

    #[test]
    fn a_mixer_is_driven_as_a_cogwheel() {
        let mut r = Rig::new();
        r.place([0, 63, 0], Kind::CreativeMotor(Dir::Up));
        r.place([0, 64, 0], Kind::Cogwheel(Axis::Y));
        r.place([1, 64, 0], Kind::MechanicalMixer);
        r.ticks(2);
        assert_eq!(r.speed([1, 64, 0]), -16.0);
        let (_, stress, _, _) = r.k.stats(&r.world, [1, 64, 0]).unwrap();
        assert_eq!(stress, 4.0 * 16.0);
        // No shaft reaches it from above.
        let mut r = Rig::new();
        r.place([0, 65, 0], Kind::CreativeMotor(Dir::Down));
        r.place([0, 64, 0], Kind::MechanicalMixer);
        r.ticks(2);
        assert_eq!(r.speed([0, 64, 0]), 0.0);
    }

    #[test]
    fn a_pump_is_driven_as_a_cogwheel_about_its_facing() {
        let mut r = Rig::new();
        r.place([0, 63, 0], Kind::CreativeMotor(Dir::Up));
        r.place([0, 64, 0], Kind::Cogwheel(Axis::Y));
        r.place([1, 64, 0], Kind::MechanicalPump(Dir::Up));
        r.ticks(2);
        assert_eq!(r.speed([1, 64, 0]), -16.0);
        let (_, stress, _, _) = r.k.stats(&r.world, [1, 64, 0]).unwrap();
        assert_eq!(stress, 4.0 * 16.0);
        // No shaft: a motor at its back doesn't turn it.
        let mut r = Rig::new();
        r.place([0, 63, 0], Kind::CreativeMotor(Dir::Up));
        r.place([0, 64, 0], Kind::MechanicalPump(Dir::Up));
        r.ticks(2);
        assert_eq!(r.speed([0, 64, 0]), 0.0);
        // The wrench turns it round, and it is saved.
        assert_eq!(
            Kind::MechanicalPump(Dir::Up).wrenched(Dir::North),
            Kind::MechanicalPump(Dir::Down)
        );
        assert_eq!(
            parse_kind_key(&kind_key(Kind::MechanicalPump(Dir::West))),
            Some(Kind::MechanicalPump(Dir::West))
        );
        assert_eq!(
            kind_from_code(12, Dir::East as u8),
            Some(Kind::MechanicalPump(Dir::East))
        );
    }

    #[test]
    fn an_arm_is_driven_as_a_cogwheel_and_a_deployer_by_its_shaft() {
        let mut r = Rig::new();
        r.place([0, 63, 0], Kind::CreativeMotor(Dir::Up));
        r.place([0, 64, 0], Kind::Cogwheel(Axis::Y));
        r.place([1, 64, 0], Kind::MechanicalArm);
        r.ticks(2);
        assert_eq!(r.speed([1, 64, 0]), -16.0);
        let (_, stress, _, _) = r.k.stats(&r.world, [1, 64, 0]).unwrap();
        assert_eq!(stress, 2.0 * 16.0);
        // A deployer facing down, its shaft along Z (not along first).
        let mut r = Rig::new();
        r.place([0, 64, -1], Kind::CreativeMotor(Dir::South));
        r.place([0, 64, 0], Kind::Deployer(Dir::Down, false));
        r.ticks(2);
        assert_eq!(r.speed([0, 64, 0]), 16.0);
        let (_, stress, _, _) = r.k.stats(&r.world, [0, 64, 0]).unwrap();
        assert_eq!(stress, 4.0 * 16.0);
        // Along first, its shaft is along X: the motor misses it.
        let mut r = Rig::new();
        r.place([0, 64, -1], Kind::CreativeMotor(Dir::South));
        r.place([0, 64, 0], Kind::Deployer(Dir::Down, true));
        r.ticks(2);
        assert_eq!(r.speed([0, 64, 0]), 0.0);
        for k in [
            Kind::MechanicalArm,
            Kind::Deployer(Dir::West, true),
            Kind::Deployer(Dir::Up, false),
            Kind::MechanicalPiston(Dir::North, true, true),
            Kind::MechanicalPiston(Dir::Down, false, false),
            Kind::MechanicalBearing(Dir::West),
            Kind::WindmillBearing(Dir::Up),
            Kind::RopePulley(Axis::Z),
            Kind::GantryShaft(Dir::South),
            Kind::GantryCarriage(Dir::Up, true),
            Kind::Drill(Dir::North),
            Kind::Saw(Dir::East, true),
            Kind::Saw(Dir::Up, false),
            Kind::Gauge(true, Dir::North, true),
            Kind::Gauge(false, Dir::Up, false),
            Kind::ElevatorPulley(Dir::East),
            Kind::LargeWaterWheel(Axis::Z),
        ] {
            assert_eq!(parse_kind_key(&kind_key(k)), Some(k));
            let (tag, state) = kind_code(k);
            assert_eq!(kind_from_code(tag, state), Some(k));
        }
    }

    #[test]
    fn a_running_mixer_is_saved_and_shown() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::MechanicalMixer);
        r.place([1, 64, 0], Kind::CreativeMotor(Dir::East));
        r.ticks(1);
        r.k.mixing_mut([0, 64, 0]).unwrap().start(0);
        r.k.mixing_mut([0, 64, 0]).unwrap().running_ticks = 12;
        let loaded = Kinetics::load(&r.k.save());
        let m = loaded.get([0, 64, 0]).unwrap().mixing;
        assert!(m.running && m.running_ticks == 12);
        assert_eq!(loaded.view(&r.world, [0, 64, 0]).unwrap().aux, 12);
        // The motor shows its setting.
        assert_eq!(loaded.view(&r.world, [1, 64, 0]).unwrap().aux, 16);
        assert_eq!(
            parse_kind_key(&kind_key(Kind::EncasedFan(Dir::Up))),
            Some(Kind::EncasedFan(Dir::Up))
        );
        assert_eq!(
            parse_kind_key(&kind_key(Kind::MechanicalMixer)),
            Some(Kind::MechanicalMixer)
        );
    }

    #[test]
    fn the_wrench_turns_blocks_as_create_does() {
        // A shaft along X, clicked on top (Y): turned about Y, along Z.
        assert_eq!(Kind::Shaft(Axis::X).wrenched(Dir::Up), Kind::Shaft(Axis::Z));
        assert_eq!(
            Kind::Shaft(Axis::Z).wrenched(Dir::Down),
            Kind::Shaft(Axis::X)
        );
        // Clicked on its end: it stays.
        assert_eq!(
            Kind::Cogwheel(Axis::X).wrenched(Dir::East),
            Kind::Cogwheel(Axis::X)
        );
        assert_eq!(
            Kind::Gearbox(Axis::Y).wrenched(Dir::North),
            Kind::Gearbox(Axis::X)
        );
        // Directional blocks: a quarter turn about the clicked face.
        assert_eq!(
            Kind::CreativeMotor(Dir::North).wrenched(Dir::Up),
            Kind::CreativeMotor(Dir::East)
        );
        assert_eq!(
            Kind::EncasedFan(Dir::Up).wrenched(Dir::East),
            Kind::EncasedFan(Dir::North)
        );
        assert_eq!(
            Kind::EncasedFan(Dir::Up).wrenched(Dir::Down),
            Kind::EncasedFan(Dir::Up)
        );
        assert_eq!(
            Kind::HandCrank(Dir::Up).wrenched(Dir::North),
            Kind::HandCrank(Dir::East)
        );
        // The press turns only from above or below.
        assert_eq!(
            Kind::MechanicalPress(Dir::North).wrenched(Dir::Up),
            Kind::MechanicalPress(Dir::East)
        );
        assert_eq!(
            Kind::MechanicalPress(Dir::North).wrenched(Dir::East),
            Kind::MechanicalPress(Dir::North)
        );
        assert_eq!(
            Kind::MechanicalMixer.wrenched(Dir::Up),
            Kind::MechanicalMixer
        );
        assert_eq!(Kind::Shaft(Axis::Y).state_property(), Some(("axis", "y")));
        // Four turns about one axis come back.
        let mut d = Dir::North;
        for _ in 0..4 {
            d = d.clockwise(Axis::X);
        }
        assert_eq!(d, Dir::North);
    }

    #[test]
    fn a_wrenched_shaft_leaves_its_line() {
        let mut r = Rig::new();
        r.place([0, 64, 0], Kind::CreativeMotor(Dir::East));
        r.place([1, 64, 0], Kind::Shaft(Axis::X));
        r.ticks(2);
        assert_eq!(r.speed([1, 64, 0]), 16.0);
        let turned = Kind::Shaft(Axis::X).wrenched(Dir::Up);
        r.place([1, 64, 0], turned);
        r.ticks(2);
        assert_eq!(r.speed([1, 64, 0]), 0.0);
    }

    #[test]
    fn the_motor_value_box_sits_on_its_sides() {
        // A motor facing north (shaft out of its north face): the box on
        // its east side, 12.5/16 of the way out, a pixel back.
        let c = motor_value_box(Dir::North, Dir::East).unwrap();
        let want = [12.5 / 16.0, 0.5, 0.5 + 1.0 / 16.0];
        for k in 0..3 {
            assert!((c[k] - want[k]).abs() < 1e-5, "{c:?}");
        }
        assert!(hits_motor_value_box(
            Dir::North,
            Dir::East,
            [1.0, 0.5, 0.55]
        ));
        assert!(!hits_motor_value_box(
            Dir::North,
            Dir::East,
            [1.0, 0.95, 0.55]
        ));
        // None on its front, back, or (lying down) underneath.
        assert_eq!(motor_value_box(Dir::North, Dir::North), None);
        assert_eq!(motor_value_box(Dir::North, Dir::South), None);
        assert_eq!(motor_value_box(Dir::North, Dir::Down), None);
        assert!(motor_value_box(Dir::Up, Dir::West).is_some());
        let top = motor_value_box(Dir::North, Dir::Up).unwrap();
        assert!((top[1] - 12.5 / 16.0).abs() < 1e-5);
    }

    #[test]
    fn the_motor_scrolls_in_growing_steps() {
        assert_eq!(motor_scroll_step(16, true), 20);
        assert_eq!(motor_scroll_step(1, true), 2);
        assert_eq!(motor_scroll_step(4, false), 3);
        assert_eq!(motor_scroll_step(3, true), 4);
        // Never zero: 1 down goes to -1.
        assert_eq!(motor_scroll_step(1, false), -1);
        assert_eq!(motor_scroll_step(-1, true), 1);
        assert_eq!(motor_scroll_step(32, true), 48);
        assert_eq!(motor_scroll_step(128, true), 192);
        assert_eq!(motor_scroll_step(250, true), 256);
        assert_eq!(motor_scroll_step(-256, false), -256);
    }

    #[test]
    fn water_wheel_woods() {
        assert_eq!(wheel_wood_of("minecraft:cherry_planks"), Some(7));
        assert_eq!(wheel_wood_of("minecraft:stone"), None);
        assert_eq!(wheel_wood_of("create:spruce_planks"), None);
        assert_eq!(
            wheel_sprite("block/spruce_planks", 7),
            "block/cherry_planks"
        );
        assert_eq!(
            wheel_sprite("block/spruce_log_top", 10),
            "block/crimson_stem_top"
        );
        assert_eq!(wheel_sprite("block/spruce_log", 9), "block/bamboo_block");
        assert_eq!(wheel_sprite("create:block/other", 3), "create:block/other");
        let mut k = Kinetics::new();
        k.entities
            .insert([0, 0, 0], Kinetic::new(Kind::WaterWheel(Dir::East)));
        assert!(k.set_wheel_material([0, 0, 0], 2));
        assert!(!k.set_wheel_material([0, 0, 0], 2));
        let loaded = Kinetics::load(&k.save());
        assert_eq!(loaded.entities[&[0, 0, 0]].material, 2);
    }

    #[test]
    fn large_wheel_structure_ring() {
        let ring = large_wheel_structure([0, 0, 0], Axis::Z);
        assert_eq!(ring.len(), 8);
        // The sides face the wheel.
        assert!(ring.contains(&([1, 0, 0], Dir::West)));
        assert!(ring.contains(&([0, -1, 0], Dir::Up)));
        // Every corner of the 3 by 3 is there, none along the axis.
        for x in -1..=1 {
            for y in -1..=1 {
                if (x, y) != (0, 0) {
                    assert!(ring.iter().any(|(p, _)| *p == [x, y, 0]), "{x} {y}");
                }
            }
        }
    }
}
