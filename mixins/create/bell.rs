// The rules of the peculiar and haunted bells (`AbstractBellBlock`, vanilla's
// `BellBlock`, `HauntedBellBlockEntity`, `HauntedBellPulser`, `SoulPulseEffect`),
// with no Horizon types.

use super::create_kinetics::{Axis, Dir, Pos};

/// `BellAttachType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attachment {
    Floor,
    Ceiling,
    SingleWall,
    DoubleWall,
}

impl Attachment {
    pub fn parse(s: &str) -> Option<Attachment> {
        match s {
            "floor" => Some(Attachment::Floor),
            "ceiling" => Some(Attachment::Ceiling),
            "single_wall" => Some(Attachment::SingleWall),
            "double_wall" => Some(Attachment::DoubleWall),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Attachment::Floor => "floor",
            Attachment::Ceiling => "ceiling",
            Attachment::SingleWall => "single_wall",
            Attachment::DoubleWall => "double_wall",
        }
    }
    fn on_wall(self) -> bool {
        matches!(self, Attachment::SingleWall | Attachment::DoubleWall)
    }
}

/// `getConnectedDirection`: from the block to what it hangs on is its opposite.
fn connected(attachment: Attachment, facing: Dir) -> Dir {
    match attachment {
        Attachment::Floor => Dir::Up,
        Attachment::Ceiling => Dir::Down,
        _ => facing.opposite(),
    }
}

/// `canSurvive`: `sturdy(d)` says the block towards `d` holds it on its near face.
pub fn can_survive(attachment: Attachment, facing: Dir, sturdy: impl Fn(Dir) -> bool) -> bool {
    sturdy(connected(attachment, facing).opposite())
}

/// `getStateForPlacement`: a click on the side `clicked` of its place (towards
/// the clicked block `clicked.opposite()`), `horizontal` the placer's facing.
pub fn placement(
    clicked: Dir,
    horizontal: Dir,
    sturdy: impl Fn(Dir) -> bool,
) -> Option<(Attachment, Dir)> {
    let survives = |a, f| can_survive(a, f, &sturdy);
    if clicked.axis() == Axis::Y {
        let a = if clicked == Dir::Down {
            Attachment::Ceiling
        } else {
            Attachment::Floor
        };
        return survives(a, horizontal).then_some((a, horizontal));
    }
    let facing = clicked.opposite();
    let double = sturdy(clicked) && sturdy(clicked.opposite());
    let a = if double {
        Attachment::DoubleWall
    } else {
        Attachment::SingleWall
    };
    if survives(a, facing) {
        return Some((a, facing));
    }
    let a = if sturdy(Dir::Down) {
        Attachment::Floor
    } else {
        Attachment::Ceiling
    };
    survives(a, facing).then_some((a, facing))
}

/// What `updateShape` makes of a change on `side`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Keep,
    Gone,
    Now(Attachment, Dir),
}

/// `updateShape`: `sturdy` as for `can_survive`.
pub fn update_shape(
    attachment: Attachment,
    facing: Dir,
    side: Dir,
    sturdy: impl Fn(Dir) -> bool,
) -> Shape {
    let towards = connected(attachment, facing).opposite();
    if towards == side
        && !can_survive(attachment, facing, &sturdy)
        && attachment != Attachment::DoubleWall
    {
        return Shape::Gone;
    }
    if side.axis() == facing.axis() {
        if attachment == Attachment::DoubleWall && !sturdy(side) {
            return Shape::Now(Attachment::SingleWall, side.opposite());
        }
        if attachment == Attachment::SingleWall && towards.opposite() == side && sturdy(side) {
            return Shape::Now(Attachment::DoubleWall, facing);
        }
    }
    Shape::Keep
}

/// `canRingFrom`: the side hit and the height on the block.
pub fn can_ring_from(attachment: Attachment, facing: Dir, hit: Dir, height: f64) -> bool {
    if hit.axis() == Axis::Y || height > 0.8124 {
        return false;
    }
    if attachment.on_wall() {
        facing.axis() != hit.axis()
    } else {
        facing.axis() == hit.axis()
    }
}

/// `neighborChanged`'s direction to ring in.
pub fn powered_ring_direction(attachment: Attachment, facing: Dir) -> Dir {
    if attachment.on_wall() {
        facing.clockwise(Axis::Y)
    } else {
        facing
    }
}

pub const RING_DURATION: i32 = 74;
pub const RECHARGE_TICKS: i32 = 65;
pub const EFFECT_TICKS: i32 = 20;
/// `HauntedBellBlockEntity.DISTANCE`.
pub const BELL_DISTANCE: i32 = 10;
/// `HauntedBellPulser.DISTANCE`.
pub const HELD_DISTANCE: i32 = 3;
pub const WARMUP_TICKS: i32 = 10;
pub const HELD_RECHARGE_TICKS: u64 = 8;

/// `BellRenderer.getSwingAngle`, in radians.
pub fn swing_angle(time: f32) -> f32 {
    let t = time / 1.5;
    1.2 * (t / std::f32::consts::PI).sin() / (2.5 + t / 3.0)
}

/// `AbstractBellBlockEntity`'s ringing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ringing {
    pub ringing: bool,
    pub ticks: i32,
}

impl Ringing {
    pub fn ring(&mut self) {
        self.ringing = true;
        self.ticks = 0;
    }
    /// `HauntedBellBlockEntity.ring`'s check.
    pub fn haunted_can_ring(&self) -> bool {
        !(self.ringing && self.ticks < RECHARGE_TICKS)
    }
    pub fn tick(&mut self) {
        if self.ringing {
            self.ticks += 1;
        }
        if self.ticks >= RING_DURATION {
            self.ringing = false;
            self.ticks = 0;
        }
    }
}

/// `HauntedBellPulser`'s `WARMUP` entry: true when this tick of holding sends a
/// pulse. A new entry starts at `WARMUP_TICKS`.
pub fn held_pulse(warmup: &mut i32, game_time: u64) -> bool {
    let first = *warmup == 1;
    *warmup -= 1;
    if *warmup > 0 {
        return false;
    }
    first || !game_time.is_multiple_of(HELD_RECHARGE_TICKS)
}

const MAX_DISTANCE: i32 = 11;
const TICKS_PER_LAYER: i32 = 6;
const WAITING_TICKS: i32 = 100;

/// `SoulPulseEffect.genLayers`' layer `idx`: the offsets at that rounded distance
/// (with 0 in the first), mirrored.
pub fn layer(idx: i32) -> Vec<Pos> {
    let mut out = Vec::new();
    if !(0..MAX_DISTANCE).contains(&idx) {
        return out;
    }
    for x in 0..MAX_DISTANCE {
        for y in 0..MAX_DISTANCE {
            for z in 0..MAX_DISTANCE {
                let dist = (f64::from(x * x + y * y + z * z).sqrt().round() as i32).max(1);
                if dist - 1 != idx {
                    continue;
                }
                for sx in if x != 0 { vec![x, -x] } else { vec![x] } {
                    for sy in if y != 0 { vec![y, -y] } else { vec![y] } {
                        for sz in if z != 0 { vec![z, -z] } else { vec![z] } {
                            out.push([sx, sy, sz]);
                        }
                    }
                }
            }
        }
    }
    out
}

/// `SoulPulseEffect`, not overlapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pulse {
    pub pos: Pos,
    pub distance: i32,
    pub ticks: i32,
    pub added: Vec<Pos>,
}

impl Pulse {
    pub fn new(pos: Pos, distance: i32) -> Pulse {
        Pulse {
            pos,
            distance,
            ticks: TICKS_PER_LAYER * distance,
            added: Vec::new(),
        }
    }
    pub fn finished(&self) -> bool {
        self.ticks <= -WAITING_TICKS
    }
    fn spawns(&self, can_spawn: &mut dyn FnMut(Pos) -> bool) -> Vec<Pos> {
        let idx = self.distance - self.ticks / TICKS_PER_LAYER - 1;
        layer(idx)
            .into_iter()
            .map(|o| [o[0] + self.pos[0], o[1] + self.pos[1], o[2] + self.pos[2]])
            .filter(|p| can_spawn(*p))
            .collect()
    }
    /// `tick`: the places this layer lands on, skipping empty layers.
    pub fn tick(&mut self, can_spawn: &mut dyn FnMut(Pos) -> bool) -> Vec<Pos> {
        if self.finished() {
            return Vec::new();
        }
        self.ticks -= 1;
        if self.ticks < 0 || self.ticks % TICKS_PER_LAYER != 0 {
            return Vec::new();
        }
        let mut spawns = self.spawns(can_spawn);
        while spawns.is_empty() && self.ticks > 0 {
            self.ticks -= TICKS_PER_LAYER;
            spawns.extend(self.spawns(can_spawn));
        }
        spawns
    }
}

/// `SoulPulseEffectHandler`.
#[derive(Clone, Debug, Default)]
pub struct Pulses {
    pub pulses: Vec<Pulse>,
    occupied: std::collections::HashSet<Pos>,
}

impl Pulses {
    pub fn add(&mut self, pulse: Pulse) {
        self.pulses.push(pulse);
    }
    /// `tick`: where souls appear now.
    pub fn tick(&mut self, can_spawn: &mut dyn FnMut(Pos) -> bool) -> Vec<Pos> {
        let mut out = Vec::new();
        for pulse in &mut self.pulses {
            for p in pulse.tick(can_spawn) {
                if self.occupied.insert(p) {
                    pulse.added.push(p);
                    out.push(p);
                }
            }
        }
        for pulse in self.pulses.iter().filter(|p| p.finished()) {
            for p in &pulse.added {
                self.occupied.remove(p);
            }
        }
        self.pulses.retain(|p| !p.finished());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placed_on_faces() {
        let all = |_: Dir| true;
        assert_eq!(
            placement(Dir::Up, Dir::East, all),
            Some((Attachment::Floor, Dir::East))
        );
        assert_eq!(
            placement(Dir::Down, Dir::East, all),
            Some((Attachment::Ceiling, Dir::East))
        );
        // On a side, facing away from what it hangs on.
        let wall = |d: Dir| d == Dir::South;
        assert_eq!(
            placement(Dir::North, Dir::East, wall),
            Some((Attachment::SingleWall, Dir::South))
        );
        // Between two walls.
        let both = |d: Dir| d.axis() == Axis::Z;
        assert_eq!(
            placement(Dir::North, Dir::East, both),
            Some((Attachment::DoubleWall, Dir::South))
        );
        // A floor with nothing under it fails.
        assert_eq!(placement(Dir::Up, Dir::East, |_| false), None);
    }

    #[test]
    fn hanging_on_its_block() {
        assert!(can_survive(Attachment::Floor, Dir::North, |d| d == Dir::Down));
        assert!(can_survive(Attachment::Ceiling, Dir::North, |d| d == Dir::Up));
        assert!(can_survive(Attachment::SingleWall, Dir::South, |d| d == Dir::South));
        assert!(!can_survive(Attachment::SingleWall, Dir::South, |d| d == Dir::North));
    }

    #[test]
    fn shape_follows_walls() {
        // A floor bell loses its floor.
        assert_eq!(
            update_shape(Attachment::Floor, Dir::North, Dir::Down, |_| false),
            Shape::Gone
        );
        // A second wall joins it; losing one leaves the other.
        let both = |d: Dir| d.axis() == Axis::Z;
        assert_eq!(
            update_shape(Attachment::SingleWall, Dir::South, Dir::North, both),
            Shape::Now(Attachment::DoubleWall, Dir::South)
        );
        let south = |d: Dir| d == Dir::South;
        assert_eq!(
            update_shape(Attachment::DoubleWall, Dir::South, Dir::North, south),
            Shape::Now(Attachment::SingleWall, Dir::South)
        );
        assert_eq!(
            update_shape(Attachment::Floor, Dir::North, Dir::East, |_| true),
            Shape::Keep
        );
    }

    #[test]
    fn rings_from_its_faces() {
        assert!(can_ring_from(
            Attachment::Floor,
            Dir::North,
            Dir::South,
            0.5
        ));
        assert!(!can_ring_from(
            Attachment::Floor,
            Dir::North,
            Dir::East,
            0.5
        ));
        assert!(!can_ring_from(
            Attachment::Floor,
            Dir::North,
            Dir::South,
            0.9
        ));
        assert!(!can_ring_from(Attachment::Floor, Dir::North, Dir::Up, 0.5));
        assert!(can_ring_from(
            Attachment::SingleWall,
            Dir::North,
            Dir::East,
            0.5
        ));
        assert!(!can_ring_from(
            Attachment::DoubleWall,
            Dir::North,
            Dir::South,
            0.5
        ));
    }

    #[test]
    fn haunted_recharges() {
        let mut r = Ringing::default();
        assert!(r.haunted_can_ring());
        r.ring();
        for _ in 0..64 {
            r.tick();
        }
        assert!(!r.haunted_can_ring());
        r.tick();
        assert!(r.haunted_can_ring());
        for _ in 0..9 {
            r.tick();
        }
        assert!(!r.ringing);
    }

    #[test]
    fn swing_settles() {
        assert_eq!(swing_angle(0.0), 0.0);
        assert!(swing_angle(5.0) > 0.0);
        assert!(swing_angle(74.0).abs() < swing_angle(5.0).abs());
    }

    #[test]
    fn held_warms_up_then_pulses() {
        let mut w = WARMUP_TICKS;
        let pulses: Vec<bool> = (1..=9).map(|t| held_pulse(&mut w, t)).collect();
        assert!(pulses.iter().all(|p| !p));
        // The first (tick 10 here), then all but each eighth.
        assert!(held_pulse(&mut w, 16));
        assert!(!held_pulse(&mut w, 16));
        assert!(held_pulse(&mut w, 17));
    }

    #[test]
    fn layers_are_spheres() {
        let first = layer(0);
        assert!(first.contains(&[0, 0, 0]));
        assert!(first.contains(&[-1, 0, 0]));
        assert!(!first.contains(&[1, 1, 1]));
        for (i, l) in (0..11).map(|i| (i, layer(i))) {
            assert!(!l.is_empty());
            for p in l {
                let d = f64::from(p[0] * p[0] + p[1] * p[1] + p[2] * p[2])
                    .sqrt()
                    .round() as i32;
                assert_eq!(d.max(1) - 1, i);
            }
        }
        assert!(layer(11).is_empty());
    }

    #[test]
    fn pulse_walks_out_and_does_not_overlap() {
        let mut all = Pulses::default();
        all.add(Pulse::new([0, 0, 0], 3));
        all.add(Pulse::new([0, 0, 0], 3));
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            for p in all.tick(&mut |p| p[1] == 0) {
                assert!(seen.insert(p));
            }
        }
        assert!(seen.contains(&[0, 0, 0]));
        assert!(seen.contains(&[3, 0, 0]));
        assert!(!seen.contains(&[4, 0, 0]));
        assert!(all.pulses.is_empty());
    }
}
