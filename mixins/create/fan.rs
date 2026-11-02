// Create's encased fan, ported from Create 6.0.10 (MIT, The Create Team):
// AirCurrent (its length, segments, bounds and the type of processing along
// it), IAirCurrentSource.getMaxDistance, FanProcessing's timers and the four
// fan processing types' catalysts. Injected as `crate::game::create_fan`; no
// Horizon types, tested with `cargo test`.

use super::create_kinetics::{Axis, Dir, Pos};

/// `CKinetics.fanPushDistance` and `fanPullDistance` defaults.
pub const FAN_PUSH_DISTANCE: f32 = 20.0;
pub const FAN_PULL_DISTANCE: f32 = 20.0;
/// `CKinetics.fanRotationArgmax` default.
pub const FAN_ROTATION_ARGMAX: f32 = 256.0;
/// `CKinetics.fanBlockCheckRate` default: ticks between rebuilds.
pub const FAN_BLOCK_CHECK_RATE: i32 = 30;
/// `CKinetics.fanProcessingTime` default.
pub const FAN_PROCESSING_TIME: i32 = 150;

/// `FanProcessingType`: the four of `AllFanProcessingTypes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FanType {
    Splashing,
    Haunting,
    Smoking,
    Blasting,
}

/// A fluid a catalyst can be (`AllFluidTags.FAN_PROCESSING_CATALYSTS_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fluid {
    Water,
    Lava,
}

impl FanType {
    /// `FanProcessingTypeRegistry.SORTED_TYPES`: by priority, highest first.
    pub const SORTED: [FanType; 4] = [
        FanType::Splashing,
        FanType::Haunting,
        FanType::Smoking,
        FanType::Blasting,
    ];
    /// The order of `SORTED` (only the tests read it).
    #[allow(dead_code)]
    pub fn priority(self) -> i32 {
        match self {
            FanType::Blasting => 100,
            FanType::Smoking => 200,
            FanType::Haunting => 300,
            FanType::Splashing => 400,
        }
    }
    /// Its registry id.
    pub fn id(self) -> &'static str {
        match self {
            FanType::Splashing => "create:splashing",
            FanType::Haunting => "create:haunting",
            FanType::Smoking => "create:smoking",
            FanType::Blasting => "create:blasting",
        }
    }
    /// `isValidAt` for the block `block` with state properties `prop`, and
    /// the fluid in it. The block tags are Create's generated
    /// `fan_processing_catalysts/*` (blaze burners included, though they
    /// aren't ported yet).
    pub fn valid_at<'a>(
        self,
        block: &str,
        prop: &dyn Fn(&str) -> Option<&'a str>,
        fluid: Option<Fluid>,
    ) -> bool {
        let campfire = matches!(block, "minecraft:campfire" | "minecraft:soul_campfire");
        let unlit_campfire = campfire && prop("lit") == Some("false");
        match self {
            FanType::Blasting => {
                if fluid == Some(Fluid::Lava) {
                    return true;
                }
                if block == "create:blaze_burner" {
                    return prop("heat_level")
                        .is_none_or(|h| matches!(h, "fading" | "kindled" | "seething"));
                }
                false
            }
            FanType::Haunting => {
                if !matches!(
                    block,
                    "create:lit_blaze_burner" | "minecraft:soul_fire" | "minecraft:soul_campfire"
                ) {
                    return false;
                }
                if unlit_campfire {
                    return false;
                }
                prop("flame_type").is_none_or(|f| f == "soul")
            }
            FanType::Smoking => {
                if !matches!(
                    block,
                    "create:blaze_burner"
                        | "create:lit_blaze_burner"
                        | "minecraft:fire"
                        | "minecraft:campfire"
                ) {
                    return false;
                }
                if unlit_campfire {
                    return false;
                }
                if prop("flame_type").is_some_and(|f| f != "regular") {
                    return false;
                }
                prop("heat_level").is_none_or(|h| h == "smouldering")
            }
            FanType::Splashing => fluid == Some(Fluid::Water),
        }
    }
}

/// `FanProcessingType.getAt`: the first type, by priority, a block is a
/// catalyst of.
pub fn fan_type_at<'a>(
    block: &str,
    prop: &dyn Fn(&str) -> Option<&'a str>,
    fluid: Option<Fluid>,
) -> Option<FanType> {
    FanType::SORTED
        .into_iter()
        .find(|t| t.valid_at(block, prop, fluid))
}

/// What a block does to the air (`AirCurrent.getFlowLimit`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Obstacle {
    /// `FAN_TRANSPARENT` or no collision shape: the air goes through.
    Pass,
    /// A full block (`Shapes.block()`): the air stops before it.
    Full,
    /// Any other shape: the air reaches into it (`findMaxDepth`).
    Partial,
}

/// What an air current needs of the world.
pub trait FanLevel {
    fn is_loaded(&self, pos: Pos) -> bool;
    fn obstacle(&self, pos: Pos) -> Obstacle;
    /// `VoxelShape.min(axis, first, second)` of the block's collision shape
    /// (`+inf` for an empty column), `first` and `second` on the next axes
    /// cyclically; and `max` (`-inf` for an empty column).
    fn shape_min(&self, pos: Pos, axis: Axis, first: f64, second: f64) -> f64;
    fn shape_max(&self, pos: Pos, axis: Axis, first: f64, second: f64) -> f64;
    /// `FanProcessingType.getAt`.
    fn fan_type(&self, pos: Pos) -> Option<FanType>;
}

fn offset(p: Pos, d: Dir, n: i32) -> Pos {
    let v = d.normal();
    [p[0] + v[0] * n, p[1] + v[1] * n, p[2] + v[2] * n]
}

/// `IAirCurrentSource.getMaxDistance`.
pub fn max_distance(speed: f32) -> f32 {
    let factor = (speed.abs() / FAN_ROTATION_ARGMAX).min(1.0);
    let lerp = |to: f32| 3.0 + factor * (to - 3.0);
    if speed > 0.0 {
        lerp(FAN_PUSH_DISTANCE)
    } else {
        lerp(FAN_PULL_DISTANCE)
    }
}

/// `AirCurrent.DEPTH_TEST_COORDINATES`.
const DEPTH_TEST: [[f64; 2]; 5] = [
    [0.25, 0.25],
    [0.25, 0.75],
    [0.5, 0.5],
    [0.75, 0.25],
    [0.75, 0.75],
];

/// `AirCurrent.findMaxDepth`: how far the air reaches into the shape at
/// `pos` going `d`; infinite when it has a hole.
fn find_max_depth(level: &dyn FanLevel, pos: Pos, d: Dir) -> f64 {
    let axis = d.axis();
    let mut max_depth: f64 = 0.0;
    for [a, b] in DEPTH_TEST {
        let depth = if d.positive() {
            let min = level.shape_min(pos, axis, a, b);
            if min == f64::INFINITY {
                return f64::INFINITY;
            }
            min
        } else {
            let max = level.shape_max(pos, axis, a, b);
            if max == f64::NEG_INFINITY {
                return f64::INFINITY;
            }
            1.0 - max
        };
        max_depth = max_depth.max(depth);
    }
    max_depth
}

/// `AirCurrent.getFlowLimit`.
pub fn flow_limit(level: &dyn FanLevel, start: Pos, max: f32, facing: Dir) -> f32 {
    let mut i = 0;
    while (i as f32) < max {
        let pos = offset(start, facing, i + 1);
        if !level.is_loaded(pos) {
            return i as f32;
        }
        match level.obstacle(pos) {
            Obstacle::Pass => {}
            Obstacle::Full => return i as f32,
            Obstacle::Partial => {
                let depth = find_max_depth(level, pos, facing);
                if depth != f64::INFINITY {
                    return ((f64::from(i) + depth + 1.0 / 32.0) as f32).min(max);
                }
            }
        }
        i += 1;
    }
    max
}

/// `AirCurrent.AirCurrentSegment`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub kind: Option<FanType>,
    pub start: i32,
    pub end: i32,
}

/// `AirCurrent`, on the server.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AirCurrent {
    pub direction: Option<Dir>,
    pub pushing: bool,
    pub max_distance: f32,
    pub segments: Vec<Segment>,
    /// `bounds`: min x, y, z, max x, y, z.
    pub bounds: [f64; 6],
}

impl AirCurrent {
    /// `AirCurrent.rebuild` for the fan at `start` facing `facing`, turning
    /// at `speed`.
    pub fn rebuild(&mut self, level: &dyn FanLevel, start: Pos, facing: Dir, speed: f32) {
        if speed == 0.0 {
            self.max_distance = 0.0;
            self.segments.clear();
            self.bounds = [0.0; 6];
            return;
        }
        self.direction = Some(facing);
        // getAirFlowDirection: convertToDirection(speed, facing) > 0.
        let directed = if facing.positive() { speed } else { -speed };
        self.pushing = directed > 0.0;
        let max = max_distance(speed);
        self.max_distance = flow_limit(level, start, max, facing);

        self.segments.clear();
        let mut current: Option<Segment> = None;
        let mut kind = None;
        let limit = self.limit();
        let (search_start, search_end, step, to_offset) = if self.pushing {
            (1, limit, 1, -1)
        } else {
            (limit, 1, -1, 0)
        };
        let mut i = search_start;
        while i * step <= search_end * step {
            if let Some(t) = level.fan_type(offset(start, facing, i)) {
                kind = Some(t);
            }
            match &mut current {
                None => {
                    current = Some(Segment {
                        kind,
                        start: i + to_offset,
                        end: 0,
                    })
                }
                Some(c) if c.kind != kind => {
                    c.end = i + to_offset;
                    self.segments.push(*c);
                    *c = Segment {
                        kind,
                        start: i + to_offset,
                        end: 0,
                    };
                }
                Some(_) => {}
            }
            i += step;
        }
        if let Some(mut c) = current {
            c.end = search_end + step + to_offset;
            self.segments.push(c);
        }

        // The bounding box, from the fan's face along the current.
        if self.max_distance < 0.25 {
            self.bounds = [0.0; 6];
        } else {
            let front = offset(start, facing, 1).map(f64::from);
            let mut b = [
                front[0],
                front[1],
                front[2],
                front[0] + 1.0,
                front[1] + 1.0,
                front[2] + 1.0,
            ];
            let factor = f64::from(self.max_distance - 1.0);
            let scale = facing.normal().map(|v| f64::from(v) * factor);
            for k in 0..3 {
                let s = scale[k];
                if factor > 0.0 {
                    // AABB.expandTowards
                    if s < 0.0 {
                        b[k] += s;
                    } else {
                        b[k + 3] += s;
                    }
                } else {
                    // AABB.contract, then move
                    if s < 0.0 {
                        b[k] -= s;
                    } else if s > 0.0 {
                        b[k + 3] -= s;
                    }
                    b[k] += s;
                    b[k + 3] += s;
                }
            }
            self.bounds = b;
        }
    }
    /// `getLimit`: the blocks the current reaches into.
    pub fn limit(&self) -> i32 {
        if self.max_distance.fract() == 0.0 {
            self.max_distance as i32
        } else {
            self.max_distance as i32 + 1
        }
    }
    /// `getTypeAt`: the processing at `offset` blocks from the fan's face.
    pub fn type_at(&self, offset: f32) -> Option<FanType> {
        if offset < 0.0 || offset > self.max_distance {
            return None;
        }
        if self.pushing {
            self.segments
                .iter()
                .find(|s| offset <= s.end as f32)
                .and_then(|s| s.kind)
        } else {
            self.segments
                .iter()
                .find(|s| offset >= s.end as f32)
                .and_then(|s| s.kind)
        }
    }
    /// `findAffectedHandlers`: where a depot in the current (or, beside a
    /// horizontal current, one below it) is processed, and the type of the
    /// segment there (a catalyst in the depot's own block wins).
    pub fn handler_spots(&self, start: Pos) -> Vec<(Pos, Option<FanType>)> {
        let Some(d) = self.direction else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for i in 1..=self.limit() {
            let kind = self.type_at((i - 1) as f32);
            for below in [0, 1] {
                let p = offset(start, d, i);
                out.push(([p[0], p[1] - below, p[2]], kind));
                if d.axis() == Axis::Y {
                    break;
                }
            }
        }
        out
    }
    /// Whether a box (an item's) touches the current's bounds.
    pub fn touches(&self, b: [f64; 6]) -> bool {
        let a = self.bounds;
        a[0] < b[3] && a[3] > b[0] && a[1] < b[4] && a[4] > b[1] && a[2] < b[5] && a[5] > b[2]
    }
}

/// `VecHelper.alignedDistanceToFace`: how far `p` is from the face `face`
/// of the block at `block`, along the face's axis.
pub fn aligned_distance_to_face(p: [f64; 3], block: Pos, face: Dir) -> f64 {
    let k = face.axis() as usize;
    let plane = f64::from(block[k]) + if face.positive() { 1.0 } else { 0.0 };
    (p[k] - plane).abs()
}

/// `AirCurrent.tickAffectedEntities`' push on an entity at `distance` from
/// the fan's centre, moving at `motion`: its new motion (a sneaking entity
/// is pushed an eighth as hard).
pub fn push(
    motion: [f64; 3],
    flow: Dir,
    speed: f32,
    distance: f64,
    max_distance: f32,
    sneaking: bool,
) -> [f64; 3] {
    let n = flow.normal();
    let sneak_modifier = if sneaking { 4096.0 } else { 512.0 };
    let acceleration =
        (f64::from(speed.abs()) / sneak_modifier / (distance / f64::from(max_distance))) as f32;
    let max = 5.0;
    let mut out = motion;
    for k in 0..3 {
        let into = (f64::from(n[k] as f32 * acceleration) - motion[k]).clamp(-max, max);
        out[k] += into / 8.0;
    }
    out
}

/// `NozzleBlockEntity.tick`'s push on an entity `diff` from the nozzle's
/// centre (`range` the fan's `getMaxDistance`; `pushing` out or drawing
/// in; items pushed a quarter as hard): the motion it gains, if any.
pub fn nozzle_push(diff: [f64; 3], range: f32, pushing: bool, item: bool) -> Option<[f64; 3]> {
    let distance = (diff[0] * diff[0] + diff[1] * diff[1] + diff[2] * diff[2]).sqrt();
    let range = f64::from(range);
    if range == 0.0 || distance > range || distance == 0.0 {
        return None;
    }
    if !pushing && distance < 1.5 {
        return None;
    }
    let factor = if item { 1.0 / 128.0 } else { 1.0 / 32.0 };
    let scale = (range - distance) * if pushing { 1.0 } else { -1.0 } / distance * factor;
    Some(diff.map(|d| d * scale))
}

/// What `FanProcessingType.affectEntity` does to an entity in the current.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntityEffect {
    /// `igniteForSeconds` (not for fire-immune entities).
    pub fire_seconds: i32,
    /// `hurt` by this much, and the vanilla damage type standing in for
    /// Create's own (`fan_lava`, `fan_fire`); for splashing, only the kinds
    /// in `hurt_kinds`.
    pub damage: Option<(f32, &'static str)>,
    pub hurt_kinds: &'static [&'static str],
    /// Haunting: blindness (30 ticks) and slowness II (20 ticks).
    pub blind_and_slow: bool,
    /// Splashing: fire put out.
    pub extinguish: bool,
    /// Haunting: horses turn into skeleton horses.
    pub haunts_horses: bool,
}

/// `affectEntity` of each type.
pub fn entity_effect(kind: FanType) -> EntityEffect {
    let none = EntityEffect {
        fire_seconds: 0,
        damage: None,
        hurt_kinds: &[],
        blind_and_slow: false,
        extinguish: false,
        haunts_horses: false,
    };
    match kind {
        FanType::Blasting => EntityEffect {
            fire_seconds: 10,
            damage: Some((4.0, "minecraft:lava")),
            ..none
        },
        FanType::Smoking => EntityEffect {
            fire_seconds: 2,
            damage: Some((2.0, "minecraft:in_fire")),
            ..none
        },
        FanType::Haunting => EntityEffect {
            blind_and_slow: true,
            haunts_horses: true,
            ..none
        },
        FanType::Splashing => EntityEffect {
            damage: Some((2.0, "minecraft:drown")),
            hurt_kinds: &[
                "minecraft:enderman",
                "minecraft:snow_golem",
                "minecraft:blaze",
            ],
            extinguish: true,
            ..none
        },
    }
}

/// A horse's `CreateHaunting` progress for one more tick in a haunting
/// current: the new progress, whether it plays the soul sound (every 10),
/// and whether it turns now (at 100).
pub fn haunt_horse(progress: i32) -> (i32, bool, bool) {
    if progress < 100 {
        return (progress + 1, progress % 10 == 0, false);
    }
    (progress, false, true)
}

/// `FanProcessing.decrementProcessingTime`'s starting time for a stack of
/// `count`: longer for each 16 items.
pub fn processing_time(count: u32) -> i32 {
    let modifier = (count.max(1) as i32 - 1) / 16 + 1;
    FAN_PROCESSING_TIME * modifier + 1
}

/// An item entity's `CreateData.Processing`: the type it is being
/// processed by and the ticks left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Processing {
    pub kind: FanType,
    pub time: i32,
}

/// `FanProcessing.canProcess(ItemEntity, type)`: `can` is the type's own
/// `canProcess` of the stack.
pub fn entity_can_process(
    state: Option<Processing>,
    kind: FanType,
    can: impl FnOnce() -> bool,
) -> bool {
    match state {
        Some(p) if p.kind == kind => {
            if p.time >= 0 {
                true
            } else if p.time == -1 {
                false
            } else {
                can()
            }
        }
        _ => can(),
    }
}

/// `FanProcessing.decrementProcessingTime`: the ticks left after this one;
/// the item is processed at 0.
pub fn entity_decrement(state: &mut Option<Processing>, kind: FanType, count: u32) -> i32 {
    let p = match state {
        Some(p) if p.kind == kind => p,
        _ => state.insert(Processing {
            kind,
            time: processing_time(count),
        }),
    };
    p.time -= 1;
    p.time
}

/// `FanProcessing.applyProcessing(TransportedItemStack, ...)`'s timer on a
/// depot's held item (`processedBy`, `processingTime`): whether to process
/// it now. `can` is the type's `canProcess` of the stack.
pub fn transported_step(
    processed_by: &mut Option<FanType>,
    time: &mut i32,
    kind: FanType,
    count: u32,
    can: impl FnOnce() -> bool,
) -> bool {
    if *processed_by != Some(kind) {
        *processed_by = Some(kind);
        *time = processing_time(count);
        if !can() {
            *time = -1;
        }
        return false;
    }
    if *time == -1 {
        return false;
    }
    let was = *time;
    *time -= 1;
    was <= 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct World {
        /// Blocks: full, or a slab-like partial shape (its bottom half).
        full: Vec<Pos>,
        slabs: Vec<Pos>,
        catalysts: HashMap<Pos, FanType>,
    }

    impl FanLevel for World {
        fn is_loaded(&self, _: Pos) -> bool {
            true
        }
        fn obstacle(&self, pos: Pos) -> Obstacle {
            if self.full.contains(&pos) {
                Obstacle::Full
            } else if self.slabs.contains(&pos) {
                Obstacle::Partial
            } else {
                Obstacle::Pass
            }
        }
        fn shape_min(&self, _: Pos, axis: Axis, _: f64, _: f64) -> f64 {
            // A bottom slab: along X and Z every column is filled from 0;
            // along Y it starts at 0.
            let _ = axis;
            0.0
        }
        fn shape_max(&self, _: Pos, axis: Axis, _: f64, _: f64) -> f64 {
            if axis == Axis::Y { 0.5 } else { 1.0 }
        }
        fn fan_type(&self, pos: Pos) -> Option<FanType> {
            self.catalysts.get(&pos).copied()
        }
    }

    #[test]
    fn distance_grows_with_speed() {
        assert_eq!(max_distance(0.0), 3.0);
        assert_eq!(max_distance(256.0), 20.0);
        assert_eq!(max_distance(-512.0), 20.0);
        assert!((max_distance(128.0) - 11.5).abs() < 1e-5);
    }

    #[test]
    fn a_full_block_stops_the_air_a_slab_lets_it_in() {
        let mut w = World::default();
        w.full.push([5, 0, 0]);
        // Blocks 1..4 in front are free: it stops at the fifth.
        assert_eq!(flow_limit(&w, [0, 0, 0], 20.0, Dir::East), 4.0);
        // Blowing down onto a bottom slab two below: half a block into it.
        w.slabs.push([0, -2, 0]);
        let limit = flow_limit(&w, [0, 0, 0], 20.0, Dir::Down);
        assert!((limit - (1.0 + 0.5 + 1.0 / 32.0)).abs() < 1e-5, "{limit}");
    }

    #[test]
    fn segments_carry_the_catalyst_downwind() {
        let mut w = World::default();
        w.full.push([0, 0, 9]);
        w.catalysts.insert([0, 0, 3], FanType::Splashing);
        let mut a = AirCurrent::default();
        // Facing south (+z) and turning positive: pushing.
        a.rebuild(&w, [0, 0, 0], Dir::South, 256.0);
        assert!(a.pushing);
        assert_eq!(a.max_distance, 8.0);
        // Air passes 1, 2 plain, then water at 3 colours the rest.
        assert_eq!(a.type_at(1.0), None);
        assert_eq!(a.type_at(2.5), Some(FanType::Splashing));
        assert_eq!(a.type_at(7.9), Some(FanType::Splashing));
        assert_eq!(a.type_at(8.5), None);
        // Its box runs from the fan's face for 8 blocks.
        assert_eq!(a.bounds, [0.0, 0.0, 1.0, 1.0, 1.0, 9.0]);
        // Pulling (turned the other way), the catalyst colours the air on
        // its way to the fan.
        a.rebuild(&w, [0, 0, 0], Dir::South, -256.0);
        assert!(!a.pushing);
        assert_eq!(a.type_at(1.0), Some(FanType::Splashing));
        assert_eq!(a.type_at(5.0), None);
    }

    #[test]
    fn a_stopped_fan_has_no_current() {
        let w = World::default();
        let mut a = AirCurrent::default();
        a.rebuild(&w, [0, 0, 0], Dir::Up, 32.0);
        assert!(a.max_distance > 3.0);
        a.rebuild(&w, [0, 0, 0], Dir::Up, 0.0);
        assert_eq!(
            (a.max_distance, a.segments.len(), a.bounds),
            (0.0, 0, [0.0; 6])
        );
        assert!(a.handler_spots([0, 0, 0]).iter().all(|(_, k)| k.is_none()));
    }

    #[test]
    fn depots_beside_a_horizontal_current_count_too() {
        let mut w = World::default();
        w.full.push([4, 0, 0]);
        w.full.push([0, 4, 0]);
        let mut a = AirCurrent::default();
        a.rebuild(&w, [0, 0, 0], Dir::East, 1.0);
        // 3 blocks to a wall: each at the current's height and below.
        let spots = a.handler_spots([0, 0, 0]);
        assert_eq!(spots.len(), 6);
        assert_eq!(spots[1].0, [1, -1, 0]);
        a.rebuild(&w, [0, 0, 0], Dir::Up, 1.0);
        assert_eq!(a.handler_spots([0, 0, 0]).len(), 3);
    }

    #[test]
    fn catalysts_by_priority() {
        let none = |_: &str| None;
        let lit = |k: &str| (k == "lit").then_some("true");
        let unlit = |k: &str| (k == "lit").then_some("false");
        assert_eq!(
            fan_type_at("minecraft:water", &none, Some(Fluid::Water)),
            Some(FanType::Splashing)
        );
        assert_eq!(
            fan_type_at("minecraft:lava", &none, Some(Fluid::Lava)),
            Some(FanType::Blasting)
        );
        assert_eq!(
            fan_type_at("minecraft:fire", &none, None),
            Some(FanType::Smoking)
        );
        assert_eq!(
            fan_type_at("minecraft:soul_fire", &none, None),
            Some(FanType::Haunting)
        );
        assert_eq!(
            fan_type_at("minecraft:campfire", &lit, None),
            Some(FanType::Smoking)
        );
        assert_eq!(fan_type_at("minecraft:campfire", &unlit, None), None);
        assert_eq!(
            fan_type_at("minecraft:soul_campfire", &lit, None),
            Some(FanType::Haunting)
        );
        assert_eq!(fan_type_at("minecraft:stone", &none, None), None);
        // A seething blaze burner blasts; a smouldering one smokes.
        let seething = |k: &str| (k == "heat_level").then_some("seething");
        let smouldering = |k: &str| (k == "heat_level").then_some("smouldering");
        assert_eq!(
            fan_type_at("create:blaze_burner", &seething, None),
            Some(FanType::Blasting)
        );
        assert_eq!(
            fan_type_at("create:blaze_burner", &smouldering, None),
            Some(FanType::Smoking)
        );
        // SORTED is by priority, highest first.
        assert!(
            FanType::SORTED
                .windows(2)
                .all(|w| w[0].priority() > w[1].priority())
        );
    }

    #[test]
    fn items_wait_their_time() {
        assert_eq!(processing_time(1), 151);
        assert_eq!(processing_time(16), 151);
        assert_eq!(processing_time(17), 301);
        // An item entity: 151 ticks, processed when the count reaches 0.
        let mut state = None;
        let mut ticks = 0;
        loop {
            ticks += 1;
            if entity_decrement(&mut state, FanType::Splashing, 1) == 0 {
                break;
            }
        }
        assert_eq!(ticks, 151);
        // Another type starts over.
        assert_eq!(entity_decrement(&mut state, FanType::Smoking, 1), 150);
        assert!(entity_can_process(state, FanType::Smoking, || false));
        assert!(!entity_can_process(
            Some(Processing {
                kind: FanType::Smoking,
                time: -1
            }),
            FanType::Smoking,
            || true
        ));
        // On a depot: the first tick sets the timer, then 152 more.
        let (mut by, mut time) = (None, 0);
        let mut ticks = 0;
        while !transported_step(&mut by, &mut time, FanType::Haunting, 1, || true) {
            ticks += 1;
            assert!(ticks < 1000);
        }
        assert_eq!(ticks, 152);
        // Nothing to do with it: it waits for another type.
        let (mut by, mut time) = (None, 0);
        for _ in 0..500 {
            assert!(!transported_step(
                &mut by,
                &mut time,
                FanType::Haunting,
                1,
                || false
            ));
        }
    }

    #[test]
    fn nozzles_push_out_and_draw_in() {
        let v = nozzle_push([2.0, 0.0, 0.0], 10.0, true, false).unwrap();
        assert!((v[0] - 8.0 / 32.0).abs() < 1e-9);
        let v = nozzle_push([2.0, 0.0, 0.0], 10.0, false, true).unwrap();
        assert!((v[0] + 8.0 / 128.0).abs() < 1e-9);
        assert_eq!(nozzle_push([1.0, 0.0, 0.0], 10.0, false, false), None);
        assert_eq!(nozzle_push([11.0, 0.0, 0.0], 10.0, true, false), None);
    }

    #[test]
    fn entities_in_the_current() {
        let b = entity_effect(FanType::Blasting);
        assert_eq!(
            (b.fire_seconds, b.damage),
            (10, Some((4.0, "minecraft:lava")))
        );
        let s = entity_effect(FanType::Splashing);
        assert!(s.extinguish && s.hurt_kinds.contains(&"minecraft:blaze"));
        assert!(entity_effect(FanType::Haunting).haunts_horses);
        let mut p = 0;
        let mut sounds = 0;
        loop {
            let (next, sound, turns) = haunt_horse(p);
            sounds += i32::from(sound);
            if turns {
                break;
            }
            p = next;
        }
        assert_eq!((p, sounds), (100, 10));
    }

    #[test]
    fn the_air_pushes_items_along() {
        let v = push([0.0; 3], Dir::East, 256.0, 2.0, 20.0, false);
        let sneaking = push([0.0; 3], Dir::East, 256.0, 2.0, 20.0, true);
        assert!(sneaking[0] > 0.0 && sneaking[0] < v[0]);
        // 256 / 512 / (2 / 20) = 5 a tick, an eighth of it at once.
        assert!((v[0] - 5.0 / 8.0).abs() < 1e-6);
        assert_eq!((v[1], v[2]), (0.0, 0.0));
        assert_eq!(
            aligned_distance_to_face([3.5, 0.5, 0.5], [0, 0, 0], Dir::East),
            2.5
        );
        assert_eq!(
            aligned_distance_to_face([0.5, -2.0, 0.5], [0, 0, 0], Dir::Down),
            2.0
        );
    }
}
