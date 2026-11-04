// Create's `BoilerData` (a fluid tank with steam engines on it) and the rules of
// `SteamEngineBlockEntity`, `PoweredShaftBlockEntity` and `SteamEngineValueBox`.
// No Horizon types.

use super::create_crafter::Pointing;
use super::create_fluids::SmartFace as Face;
use super::create_kinetics::{Axis, Dir};

/// `BoilerData.SAMPLE_RATE`.
pub const SAMPLE_RATE: i32 = 5;
const WATER_SUPPLY_PER_LEVEL: i32 = 10;
const PASSIVE_ENGINE_EFFICIENCY: f32 = 1.0 / 8.0;
/// `BlockStressValues.getCapacity(STEAM_ENGINE)`.
pub const ENGINE_CAPACITY: f32 = 1024.0;

/// `BoilerData`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Boiler {
    pub gathered_supply: i32,
    pub supply_over_time: [f32; 10],
    pub ticks_until_next_sample: i32,
    pub current_index: usize,
    pub passive_heat: bool,
    pub active_heat: i32,
    pub water_supply: f32,
    pub attached_engines: i32,
    pub attached_whistles: i32,
}

impl Boiler {
    /// `isActive`.
    pub fn is_active(&self) -> bool {
        self.attached_engines > 0 || self.attached_whistles > 0
    }
    /// `tick` on the server, after `updateTemperature`.
    pub fn tick(&mut self, capacity: i32, creative: bool) {
        if !self.is_active() {
            return;
        }
        self.ticks_until_next_sample -= 1;
        if self.ticks_until_next_sample > 0 || capacity == 0 {
            return;
        }
        self.ticks_until_next_sample = SAMPLE_RATE;
        let sample = self.gathered_supply as f32 / SAMPLE_RATE as f32;
        self.supply_over_time[self.current_index] = sample;
        self.water_supply = self.water_supply.max(sample);
        self.current_index = (self.current_index + 1) % self.supply_over_time.len();
        self.gathered_supply = 0;
        if self.current_index == 0 {
            self.water_supply = self.supply_over_time.iter().fold(0.0, |a, b| b.max(a));
        }
        if creative {
            self.water_supply = (WATER_SUPPLY_PER_LEVEL * 20) as f32;
        }
    }
    /// `BoilerFluidHandler.fill`.
    pub fn fill(&mut self, amount: i32, simulate: bool) -> i32 {
        if !simulate {
            self.gathered_supply += amount;
        }
        amount
    }
    /// `getMaxHeatLevelForBoilerSize`.
    pub fn max_for_size(&self, size: i32) -> i32 {
        (size / 4).min(18)
    }
    /// `getMaxHeatLevelForWaterSupply`.
    pub fn max_for_water(&self) -> i32 {
        (self.water_supply.ceil() as i32 / WATER_SUPPLY_PER_LEVEL).min(18)
    }
    /// `isPassive(boilerSize)`.
    pub fn is_passive(&self, size: i32) -> bool {
        self.passive_heat && self.max_for_size(size) > 0 && self.max_for_water() > 0
    }
    /// `getActualHeat`.
    pub fn actual_heat(&self, size: i32) -> i32 {
        self.active_heat
            .min(self.max_for_water().min(self.max_for_size(size)))
    }
    /// `getEngineEfficiency`.
    pub fn engine_efficiency(&self, size: i32) -> f32 {
        if self.attached_engines == 0 {
            return 0.0;
        }
        if self.is_passive(size) {
            return PASSIVE_ENGINE_EFFICIENCY / self.attached_engines as f32;
        }
        if self.active_heat == 0 {
            return 0.0;
        }
        let actual = self.actual_heat(size);
        if self.attached_engines <= actual {
            1.0
        } else {
            actual as f32 / self.attached_engines as f32
        }
    }
    /// `read`: where the gauge's dial goes.
    pub fn gauge_target(&self, size: i32) -> f32 {
        let for_size = self.max_for_size(size);
        if self.is_passive(size) {
            1.0 / 8.0
        } else if for_size == 0 {
            0.0
        } else {
            self.actual_heat(size) as f32 / for_size as f32
        }
    }
    /// `updateTemperature`, from the heat of the blocks under it
    /// (`BoilerHeater.findHeat`).
    pub fn update_temperature(&mut self, heats: impl IntoIterator<Item = i32>) {
        self.passive_heat = false;
        self.active_heat = 0;
        for heat in heats {
            if heat == 0 {
                self.passive_heat = true;
            } else if heat > 0 {
                self.active_heat += heat;
            }
        }
        self.passive_heat &= self.active_heat == 0;
    }
    /// `clear`.
    pub fn clear(&mut self) {
        self.water_supply = 0.0;
        self.active_heat = 0;
        self.passive_heat = false;
        self.attached_engines = 0;
        self.supply_over_time = [0.0; 10];
    }
    /// `addToGoggleTooltip`'s `totalSU`.
    pub fn total_capacity(&self, size: i32) -> f32 {
        let level = self.actual_heat(size);
        self.engine_efficiency(size)
            * 16.0
            * level.max(self.attached_engines) as f32
            * ENGINE_CAPACITY
    }
    /// `getHeatLevelTextComponent`.
    pub fn status(&self, size: i32) -> String {
        let level = self.actual_heat(size);
        if self.is_passive(size) {
            "Passive".to_owned()
        } else if level == 0 {
            "Idle".to_owned()
        } else if level == 18 {
            "Max".to_owned()
        } else {
            format!("Lvl {level}")
        }
    }
    /// `read`: what was saved, the samples all at the supply.
    pub fn restored(
        water_supply: f32,
        active_heat: i32,
        passive_heat: bool,
        engines: i32,
        whistles: i32,
    ) -> Boiler {
        Boiler {
            water_supply,
            active_heat,
            passive_heat,
            attached_engines: engines,
            attached_whistles: whistles,
            supply_over_time: [water_supply.trunc(); 10],
            ..Boiler::default()
        }
    }
}

/// `BoilerHeaters`: the heat of a block under a boiler (-1 none, 0 passive),
/// with its name, its `heat_level` and its `lit`.
pub fn heater_heat(name: &str, heat_level: Option<&str>, lit: Option<&str>) -> i32 {
    match name {
        // BoilerHeaters.blazeBurner.
        "create:blaze_burner" => match heat_level {
            None | Some("none") => -1,
            Some("seething") => 2,
            Some("fading" | "kindled") => 1,
            Some(_) => 0,
        },
        // The tag `create:passive_boiler_heaters`, through isNotUnheated.
        "create:lit_blaze_burner"
        | "minecraft:magma_block"
        | "minecraft:lava"
        | "minecraft:fire"
        | "minecraft:soul_fire" => 0,
        "minecraft:campfire" | "minecraft:soul_campfire" => {
            if lit == Some("false") {
                -1
            } else {
                0
            }
        }
        _ => -1,
    }
}

/// `PoweredShaftBlockEntity.getSpeedModifier`.
pub fn speed_modifier(efficiency: f32) -> i32 {
    1 + if efficiency >= 1.0 {
        3
    } else {
        ((efficiency * 4.0).floor() as i32).min(2)
    }
}

/// `getGeneratedSpeed`.
pub fn shaft_speed(direction: i32, efficiency: f32) -> f32 {
    if efficiency * ENGINE_CAPACITY > 0.0 {
        (direction * 16 * speed_modifier(efficiency)) as f32
    } else {
        0.0
    }
}

/// `calculateAddedStressCapacity`.
pub fn shaft_capacity(efficiency: f32) -> f32 {
    efficiency * ENGINE_CAPACITY / speed_modifier(efficiency) as f32
}

/// `getConnectedDirection`: the way out of what it is attached to.
pub fn connected(face: Face, facing: Dir) -> Dir {
    match face {
        Face::Floor => Dir::Up,
        Face::Ceiling => Dir::Down,
        Face::Wall => facing,
    }
}

/// `FaceAttachedHorizontalDirectionalBlock.getStateForPlacement`: the first of
/// the `looks` (`getNearestLookingDirections`) with a tank that way.
pub fn placement(
    looks: &[Dir],
    horizontal: Dir,
    tank: impl Fn(Dir) -> bool,
) -> Option<(Face, Dir)> {
    let look = looks.iter().copied().find(|d| tank(*d))?;
    Some(match look {
        Dir::Up => (Face::Ceiling, horizontal),
        Dir::Down => (Face::Floor, horizontal),
        d => (Face::Wall, d.opposite()),
    })
}

/// `isShaftValid`, for a shaft along `axis`.
pub fn shaft_valid(connected: Dir, axis: Axis) -> bool {
    axis != connected.axis()
}

/// `SteamEngineBlockEntity.tick`'s `conveyedSpeedLevel`, then what the shaft's
/// speed makes of it: the direction it gets and the engine's rotation
/// direction (0 clockwise, 1 counter-clockwise), turned round to agree.
pub fn conveyed_direction(
    efficiency: f32,
    target_axis: Axis,
    facing: Dir,
    rotation: i32,
    source_speed: Option<f32>,
) -> (i32, i32) {
    let mut level = if efficiency == 0.0 || target_axis == Axis::Y || facing.positive() {
        1
    } else {
        -1
    };
    if target_axis == Axis::Z {
        level = -level;
    }
    if rotation == 1 {
        level = -level;
    }
    let mut rotation = rotation;
    if let Some(speed) = source_speed
        && speed != 0.0
        && (speed > 0.0) != (level > 0)
    {
        rotation = 1 - rotation;
        level = -level;
    }
    (level, rotation)
}

/// `SteamEngineBlockEntity.getTargetAngle` from the shaft's angle in radians;
/// none when the shaft runs along the engine.
pub fn target_angle(angle: f32, axis: Axis, connected: Dir) -> Option<f32> {
    if axis == connected.axis() {
        return None;
    }
    let mut angle = angle;
    let horizontal = axis != Axis::Y;
    if horizontal && ((connected.axis() == Axis::X) ^ connected.positive()) {
        angle = -angle;
    }
    if axis == Axis::X && connected == Dir::Down {
        angle = -angle;
    }
    Some(angle)
}

/// `SteamEngineRenderer`: the piston's offset, and the linkage's turn.
pub fn piston_motion(angle: f32) -> (f32, f32) {
    let throw = 6.0f32 / 16.0;
    let rod = 14.0f32 / 16.0;
    let piston =
        throw * angle.sin() - (rod * rod - throw * throw * angle.cos() * angle.cos()).sqrt();
    let distance = ((piston - throw * angle.sin()).powi(2)).sqrt();
    let turn = (distance / rod).acos() * if angle.cos() >= 0.0 { 1.0 } else { -1.0 };
    (piston, turn)
}

/// `SteamEngineValueBox.isSideActive`: the roll of its slot on `side`, if that
/// side has one.
pub fn value_box_roll(connected: Dir, facing: Dir, side: Dir) -> Option<f32> {
    if connected.axis() == side.axis() {
        return None;
    }
    let mut roll = 0.0;
    for p in Pointing::ALL {
        if p.combined_direction(connected) == side {
            roll = p.x_rotation();
        }
    }
    if connected == Dir::Up {
        roll += 180.0;
    }
    let mut recessed = roll % 180.0 == 0.0;
    if connected.axis() == Axis::Y {
        recessed ^= facing.axis() == Axis::X;
    }
    (!recessed).then_some(roll)
}

/// `SteamEngineValueBox.getLocalOffset` for the roll `value_box_roll` found.
pub fn value_box_offset(roll: f32, horizontal_angle: f32, vertical_angle: f32) -> [f32; 3] {
    let around = |v: [f32; 3], degrees: f32, a: usize, b: usize| {
        // VecHelper.rotate about the centre: a' = a cos + b sin, b' = b cos - a sin.
        let (sin, cos) = degrees.to_radians().sin_cos();
        let mut out = [v[0] - 0.5, v[1] - 0.5, v[2] - 0.5];
        let (x, y) = (out[a], out[b]);
        out[a] = x * cos + y * sin;
        out[b] = y * cos - x * sin;
        [out[0] + 0.5, out[1] + 0.5, out[2] + 0.5]
    };
    let v = [8.0 / 16.0, 14.5 / 16.0, 9.0 / 16.0];
    // About Z (a y, b x), then Y (a x, b z), then X (a z, b y).
    let v = around(v, roll, 1, 0);
    let v = around(v, horizontal_angle, 0, 2);
    around(v, vertical_angle, 2, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active() -> Boiler {
        Boiler {
            attached_engines: 1,
            ..Boiler::default()
        }
    }

    #[test]
    fn water_supply_is_the_best_sample_of_the_last_ten() {
        let mut b = active();
        b.fill(50, false);
        assert_eq!(b.fill(50, true), 50);
        b.ticks_until_next_sample = 1;
        b.tick(8000, false);
        assert_eq!(b.water_supply, 10.0);
        assert_eq!(b.ticks_until_next_sample, SAMPLE_RATE);
        assert_eq!(b.max_for_water(), 1);
        // Its sample is still among the last ten at the end of the round...
        for _ in 0..9 * SAMPLE_RATE {
            b.tick(8000, false);
        }
        assert_eq!(b.current_index, 0);
        assert_eq!(b.water_supply, 10.0);
        // ...and gone at the end of the next.
        for _ in 0..10 * SAMPLE_RATE {
            b.tick(8000, false);
        }
        assert_eq!(b.water_supply, 0.0);
        b.supply_over_time = [0.0; 10];
        b.ticks_until_next_sample = 1;
        b.tick(8000, true);
        assert_eq!(b.water_supply, 200.0);
        assert_eq!(b.max_for_water(), 18);
    }

    #[test]
    fn heat_levels_and_efficiency() {
        let mut b = active();
        b.water_supply = 200.0;
        // A 1x1x4 tank's size gives level 1.
        assert_eq!(b.max_for_size(4), 1);
        assert_eq!(b.max_for_size(100), 18);
        b.update_temperature([2]);
        assert_eq!(b.active_heat, 2);
        assert_eq!(b.actual_heat(4), 1);
        assert_eq!(b.engine_efficiency(4), 1.0);
        assert_eq!(b.status(4), "Lvl 1");
        b.attached_engines = 2;
        assert_eq!(b.engine_efficiency(4), 0.5);
        b.update_temperature([0, -1]);
        assert!(b.passive_heat && b.active_heat == 0);
        assert_eq!(b.engine_efficiency(4), 1.0 / 8.0 / 2.0);
        assert_eq!(b.status(4), "Passive");
        // Active heat wins over passive.
        b.update_temperature([0, 1]);
        assert!(!b.passive_heat);
        b.update_temperature([-1]);
        assert_eq!(b.engine_efficiency(4), 0.0);
        assert_eq!(b.status(4), "Idle");
        // Too small a tank: no level, and no passive either.
        b.update_temperature([0]);
        assert_eq!(b.engine_efficiency(3), 0.0);
    }

    #[test]
    fn gauge_follows_the_level() {
        let mut b = active();
        b.water_supply = 200.0;
        b.update_temperature([1]);
        assert_eq!(b.gauge_target(8), 0.5);
        b.update_temperature([0]);
        assert_eq!(b.gauge_target(8), 0.125);
        assert_eq!(b.gauge_target(3), 0.0);
    }

    #[test]
    fn heaters() {
        assert_eq!(heater_heat("create:blaze_burner", Some("none"), None), -1);
        assert_eq!(
            heater_heat("create:blaze_burner", Some("smouldering"), None),
            0
        );
        assert_eq!(heater_heat("create:blaze_burner", Some("fading"), None), 1);
        assert_eq!(heater_heat("create:blaze_burner", Some("kindled"), None), 1);
        assert_eq!(
            heater_heat("create:blaze_burner", Some("seething"), None),
            2
        );
        assert_eq!(heater_heat("minecraft:magma_block", None, None), 0);
        assert_eq!(heater_heat("minecraft:campfire", None, Some("false")), -1);
        assert_eq!(heater_heat("minecraft:campfire", None, Some("true")), 0);
        assert_eq!(heater_heat("minecraft:stone", None, None), -1);
    }

    #[test]
    fn the_shaft_trades_speed_for_capacity() {
        assert_eq!(shaft_speed(1, 0.0), 0.0);
        assert_eq!(speed_modifier(1.0 / 8.0), 1);
        assert_eq!(shaft_speed(1, 1.0 / 8.0), 16.0);
        assert_eq!(shaft_capacity(1.0 / 8.0), 128.0);
        assert_eq!(speed_modifier(0.25), 2);
        assert_eq!(speed_modifier(0.5), 3);
        assert_eq!(speed_modifier(0.99), 3);
        assert_eq!(speed_modifier(1.0), 4);
        assert_eq!(shaft_speed(-1, 1.0), -64.0);
        assert_eq!(shaft_capacity(1.0), 256.0);
    }

    #[test]
    fn placement_finds_the_tank() {
        let looks = [
            Dir::Down,
            Dir::North,
            Dir::East,
            Dir::West,
            Dir::South,
            Dir::Up,
        ];
        assert_eq!(
            placement(&looks, Dir::North, |d| d == Dir::Down),
            Some((Face::Floor, Dir::North))
        );
        assert_eq!(
            placement(&looks, Dir::North, |d| d == Dir::Up),
            Some((Face::Ceiling, Dir::North))
        );
        assert_eq!(
            placement(&looks, Dir::North, |d| d == Dir::North),
            Some((Face::Wall, Dir::South))
        );
        assert_eq!(placement(&looks, Dir::North, |_| false), None);
        assert_eq!(connected(Face::Floor, Dir::North), Dir::Up);
        assert_eq!(connected(Face::Ceiling, Dir::North), Dir::Down);
        assert_eq!(connected(Face::Wall, Dir::East), Dir::East);
        assert!(shaft_valid(Dir::Up, Axis::X) && !shaft_valid(Dir::Up, Axis::Y));
    }

    #[test]
    fn value_box_is_on_the_engines_raised_sides() {
        // On a tank's top, the sides along the long way of its piston housing.
        assert_eq!(value_box_roll(Dir::Up, Dir::North, Dir::Up), None);
        let sides: Vec<Dir> = [Dir::North, Dir::South, Dir::West, Dir::East]
            .into_iter()
            .filter(|d| value_box_roll(Dir::Up, Dir::North, *d).is_some())
            .collect();
        assert_eq!(sides.len(), 2);
        assert_eq!(sides[0].axis(), sides[1].axis());
        // Unturned, the offset is the slot as written.
        let at = value_box_offset(0.0, 0.0, 0.0);
        assert!((at[1] - 14.5 / 16.0).abs() < 1e-5);
    }

    #[test]
    fn direction_agrees_with_the_shaft() {
        assert_eq!(
            conveyed_direction(1.0, Axis::X, Dir::North, 0, None),
            (-1, 0)
        );
        assert_eq!(
            conveyed_direction(1.0, Axis::X, Dir::South, 0, None),
            (1, 0)
        );
        assert_eq!(
            conveyed_direction(1.0, Axis::Z, Dir::East, 0, None),
            (-1, 0)
        );
        assert_eq!(
            conveyed_direction(1.0, Axis::Y, Dir::North, 0, None),
            (1, 0)
        );
        assert_eq!(
            conveyed_direction(0.0, Axis::X, Dir::North, 0, None),
            (1, 0)
        );
        assert_eq!(
            conveyed_direction(1.0, Axis::Y, Dir::North, 1, None),
            (-1, 1)
        );
        // A source turning the other way turns the engine's setting round.
        assert_eq!(
            conveyed_direction(1.0, Axis::Y, Dir::North, 0, Some(-32.0)),
            (-1, 1)
        );
        assert_eq!(
            conveyed_direction(1.0, Axis::Y, Dir::North, 0, Some(32.0)),
            (1, 0)
        );
    }

    #[test]
    fn piston_stays_on_its_crank() {
        let (down, _) = piston_motion(-std::f32::consts::FRAC_PI_2);
        let (up, _) = piston_motion(std::f32::consts::FRAC_PI_2);
        assert!((down + 20.0 / 16.0).abs() < 1e-5);
        assert!((up + 8.0 / 16.0).abs() < 1e-5);
        assert_eq!(target_angle(1.0, Axis::Y, Dir::Up), None);
        assert_eq!(target_angle(1.0, Axis::X, Dir::Up), Some(-1.0));
        assert_eq!(target_angle(1.0, Axis::Y, Dir::North), Some(1.0));
    }
}
