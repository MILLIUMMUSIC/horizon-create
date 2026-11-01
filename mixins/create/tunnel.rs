// Create's belt tunnels, injected as `crate::game::create_tunnel`: the
// andesite and brass tunnel's states (`BeltTunnelBlock`'s horizontal axis
// and shape, chosen from its neighbours by `getTunnelState`), their sides
// and flaps (`BeltTunnelBlockEntity.updateTunnelConnections`), connected
// brass tunnels (`BrassTunnelBlockEntity.getAdjacent`) and the brass
// tunnel's distribution (`tick`, `gatherValidOutputs`, `insertIntoTunnel`,
// `distribute`) in its Split and Round robin modes. What an item does when
// it enters a tunnel along the belt is in belt.rs (`check_for_tunnels`).
// No Horizon types.

use super::create_belt::Stack;
use super::create_kinetics::{Axis, Dir, Pos};

/// `brassTunnelTimer`'s default, in ticks.
pub const BRASS_TUNNEL_TIMER: i32 = 10;

/// `Iterate.horizontalDirections` (`Direction.from2DDataValue` order).
pub const HORIZONTAL: [Dir; 4] = [Dir::South, Dir::West, Dir::North, Dir::East];

/// `BeltTunnelBlock.Shape`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    Straight,
    Window,
    Closed,
    TLeft,
    TRight,
    Cross,
}

impl Shape {
    pub const ALL: [Shape; 6] = [
        Shape::Straight,
        Shape::Window,
        Shape::Closed,
        Shape::TLeft,
        Shape::TRight,
        Shape::Cross,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Shape::Straight => "straight",
            Shape::Window => "window",
            Shape::Closed => "closed",
            Shape::TLeft => "t_left",
            Shape::TRight => "t_right",
            Shape::Cross => "cross",
        }
    }
    /// `hasWindow`.
    pub fn has_window(self) -> bool {
        matches!(self, Shape::Window | Shape::Closed)
    }
    /// `isStraight`.
    pub fn is_straight(self) -> bool {
        self.has_window() || self == Shape::Straight
    }
    /// `isJunction`.
    pub fn is_junction(self) -> bool {
        matches!(self, Shape::Cross | Shape::TLeft | Shape::TRight)
    }
}

/// The tunnels' blocks here: a chain's template (its `axis` holds the
/// horizontal axis) for each `shape`, which it has no property for. The
/// item places the straight one, which the native mod turns to the shape
/// and axis its neighbours give it.
pub const ANDESITE_TUNNEL: &str = "create:andesite_tunnel";
pub const BRASS_TUNNEL: &str = "create:brass_tunnel";

/// The block of a tunnel of this kind and shape.
pub fn block_name(brass: bool, shape: Shape) -> String {
    let kind = if brass { "brass" } else { "andesite" };
    if shape == Shape::Straight {
        format!("create:{kind}_tunnel")
    } else {
        format!("create:hz_{kind}_tunnel_{}", shape.name())
    }
}

/// A tunnel's block state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TunnelState {
    pub brass: bool,
    /// `HORIZONTAL_AXIS`: the belt's.
    pub axis: Axis,
    pub shape: Shape,
}

impl TunnelState {
    pub fn from_block<'a>(
        name: &str,
        prop: impl Fn(&str) -> Option<&'a str>,
    ) -> Option<TunnelState> {
        let (brass, shape) = parse_name(name)?;
        // A chain placed upright (before the native mod turns it) counts
        // as along X, as its model shows.
        let axis = match prop("axis") {
            Some("z") => Axis::Z,
            _ => Axis::X,
        };
        Some(TunnelState { brass, axis, shape })
    }
    pub fn block(self) -> String {
        block_name(self.brass, self.shape)
    }
    /// The sides items leave by (`sides`), each with whether it has a flap
    /// (`flaps`: not where another tunnel or an extended belt funnel
    /// facing it closes it).
    pub fn sides(self, pos: Pos, level: &dyn TunnelLevel) -> Vec<(Dir, bool)> {
        let mut out = Vec::new();
        for d in HORIZONTAL {
            if d.axis() != self.axis {
                if self.shape.is_straight() {
                    continue;
                }
                let positive = d.positive() ^ (d.axis() == Axis::Z);
                if positive && self.shape == Shape::TLeft {
                    continue;
                }
                if !positive && self.shape == Shape::TRight {
                    continue;
                }
            }
            let next = offset(pos, d);
            let occluded =
                level.tunnel(next).is_some() || level.extended_funnel(next) == Some(d.opposite());
            out.push((d, !occluded));
        }
        out
    }
    /// `flaps.keySet()`.
    pub fn flaps(self, pos: Pos, level: &dyn TunnelLevel) -> Vec<Dir> {
        self.sides(pos, level)
            .into_iter()
            .filter(|(_, f)| *f)
            .map(|(d, _)| d)
            .collect()
    }
    /// `onWrenched`: a straight tunnel's window opens or closes.
    pub fn wrenched(self) -> Option<TunnelState> {
        let shape = match self.shape {
            Shape::Window => Shape::Closed,
            Shape::Closed => Shape::Window,
            _ => return None,
        };
        Some(TunnelState { shape, ..self })
    }
}

fn parse_name(name: &str) -> Option<(bool, Shape)> {
    let rest = name.strip_prefix("create:")?;
    let (brass, rest) = if let Some(r) = rest.strip_prefix("hz_") {
        if let Some(s) = r.strip_prefix("brass_tunnel_") {
            (true, s)
        } else {
            (false, r.strip_prefix("andesite_tunnel_")?)
        }
    } else {
        return match rest {
            "andesite_tunnel" => Some((false, Shape::Straight)),
            "brass_tunnel" => Some((true, Shape::Straight)),
            _ => None,
        };
    };
    let shape = Shape::ALL
        .into_iter()
        .find(|s| s.name() == rest && *s != Shape::Straight)?;
    Some((brass, shape))
}

pub fn offset(pos: Pos, d: Dir) -> Pos {
    let n = d.normal();
    [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]]
}

fn below(pos: Pos) -> Pos {
    [pos[0], pos[1] - 1, pos[2]]
}

/// What tunnels need to know of the blocks around them.
pub trait TunnelLevel {
    /// The tunnel at `pos`.
    fn tunnel(&self, pos: Pos) -> Option<TunnelState>;
    /// The horizontal belt segment at `pos` (`BeltSlope.HORIZONTAL`): the
    /// axis of its `HORIZONTAL_FACING`.
    fn flat_belt_axis(&self, pos: Pos) -> Option<Axis>;
    /// Any belt segment at `pos`: the axis it runs along.
    fn belt_axis(&self, pos: Pos) -> Option<Axis>;
    /// A `DirectBeltInputBehaviour` at `pos`.
    fn has_direct_input(&self, pos: Pos) -> bool;
    /// Its `canInsertFromSide(side)`.
    fn can_insert_from_side(&self, pos: Pos, side: Dir) -> bool;
    /// An extended belt funnel at `pos`: its facing.
    fn extended_funnel(&self, pos: Pos) -> Option<Dir>;
}

/// `canSurvive`/`isValidPositionForPlacement`: on a horizontal belt
/// (Create also wants the belt's casing, which belts here don't have).
pub fn can_survive(pos: Pos, level: &dyn TunnelLevel) -> bool {
    level.flat_belt_axis(below(pos)).is_some()
}

/// `hasValidOutput`: what is at `side` of the segment at `segment`.
fn has_valid_output(level: &dyn TunnelLevel, segment: Pos, side: Dir) -> bool {
    let p = offset(segment, side);
    if let Some(axis) = level.belt_axis(p) {
        return axis == side.axis();
    }
    level.has_direct_input(p) && level.can_insert_from_side(p, side)
}

/// `canHaveWindow`: tunnels (or extended funnels facing in) both ways along
/// it, not two funnels.
fn can_have_window(level: &dyn TunnelLevel, pos: Pos, axis: Axis) -> bool {
    let fw = Dir::positive_of(axis);
    let one = offset(pos, fw);
    let two = offset(pos, fw.opposite());
    let funnel1 = level.extended_funnel(one) == Some(fw.opposite());
    let funnel2 = level.extended_funnel(two) == Some(fw);
    let valid1 = level.tunnel(one).is_some() || funnel1;
    let valid2 = level.tunnel(two).is_some() || funnel2;
    valid1 && valid2 && !(funnel1 && funnel2)
}

/// `getTunnelState`: the shape and axis a tunnel at `pos` takes.
pub fn tunnel_state(level: &dyn TunnelLevel, pos: Pos, brass: bool) -> TunnelState {
    let segment = below(pos);
    let axis = level.belt_axis(segment).unwrap_or(Axis::X);
    let left = Dir::positive_of(axis).clockwise(Axis::Y);
    let on_left = has_valid_output(level, segment, left);
    let on_right = has_valid_output(level, segment, left.opposite());
    let mut shape = match (on_left, on_right) {
        (true, true) => Shape::Cross,
        (true, false) => Shape::TLeft,
        (false, true) => Shape::TRight,
        _ => Shape::Straight,
    };
    if shape == Shape::Straight && can_have_window(level, pos, axis) {
        shape = Shape::Window;
    }
    TunnelState { brass, axis, shape }
}

/// `updateTunnel` (and `updateShape`, which keeps a closed window closed):
/// the state the tunnel now at `old` should be.
pub fn updated_state(level: &dyn TunnelLevel, pos: Pos, old: TunnelState) -> TunnelState {
    let new = tunnel_state(level, pos, old.brass);
    if new.axis == old.axis && new.shape.has_window() && old.shape.has_window() {
        return old;
    }
    new
}

/// `BrassTunnelBlockEntity.getAdjacent`: the brass tunnel beside the one at
/// `pos` on its left or right, along the same axis.
pub fn adjacent(level: &dyn TunnelLevel, pos: Pos, left: bool) -> Option<Pos> {
    let state = level.tunnel(pos).filter(|t| t.brass)?;
    let base = Dir::positive_of(state.axis);
    let d = if left {
        base.clockwise(Axis::Y).opposite()
    } else {
        base.clockwise(Axis::Y)
    };
    let p = offset(pos, d);
    level
        .tunnel(p)
        .filter(|t| t.brass && t.axis == state.axis)?;
    Some(p)
}

/// `isConnected` (`determineIfConnected`): both have flaps.
pub fn is_connected(level: &dyn TunnelLevel, pos: Pos, left: bool) -> bool {
    let Some(state) = level.tunnel(pos) else {
        return false;
    };
    if state.flaps(pos, level).is_empty() {
        return false;
    }
    adjacent(level, pos, left).is_some_and(|p| {
        level
            .tunnel(p)
            .is_some_and(|t| !t.flaps(p, level).is_empty())
    })
}

/// `hasDistributionBehaviour`: a brass tunnel with flaps, connected to
/// another or with a flap to the side.
pub fn has_distribution(level: &dyn TunnelLevel, pos: Pos) -> bool {
    let Some(state) = level.tunnel(pos).filter(|t| t.brass) else {
        return false;
    };
    let flaps = state.flaps(pos, level);
    if flaps.is_empty() {
        return false;
    }
    if is_connected(level, pos, true) || is_connected(level, pos, false) {
        return true;
    }
    flaps.iter().any(|d| d.axis() != state.axis)
}

/// The brass tunnels connected to the one at `pos` (`getAdjacent` walked
/// both ways), itself first; none when the walk reaches an unloaded block.
pub fn group(
    level: &dyn TunnelLevel,
    pos: Pos,
    is_loaded: impl Fn(Pos) -> bool,
) -> Option<Vec<Pos>> {
    let mut out = vec![pos];
    for left in [true, false] {
        let mut at = pos;
        loop {
            if !is_loaded(at) {
                return None;
            }
            let Some(next) = adjacent(level, at, left) else {
                break;
            };
            if out.contains(&next) {
                break;
            }
            out.push(next);
            at = next;
        }
    }
    Some(out)
}

/// `BrassTunnelBlockEntity.SelectionMode`: the two ported here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionMode {
    #[default]
    Split,
    RoundRobin,
}

impl SelectionMode {
    pub fn next(self) -> SelectionMode {
        match self {
            SelectionMode::Split => SelectionMode::RoundRobin,
            SelectionMode::RoundRobin => SelectionMode::Split,
        }
    }
    /// `create.tunnel.selection_mode.*`'s English names.
    pub fn label(self) -> &'static str {
        match self {
            SelectionMode::Split => "Split",
            SelectionMode::RoundRobin => "Round Robin",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            SelectionMode::Split => "split",
            SelectionMode::RoundRobin => "round_robin",
        }
    }
    pub fn parse(s: &str) -> Option<SelectionMode> {
        match s {
            "split" => Some(SelectionMode::Split),
            "round_robin" => Some(SelectionMode::RoundRobin),
            _ => None,
        }
    }
}

/// What a brass tunnel's distribution needs of the world.
pub trait TunnelWorld<S>: TunnelLevel {
    fn is_loaded(&self, pos: Pos) -> bool;
    /// The belt segment at `pos`: its speed (`getSpeed`) and
    /// `getMovementFacing`.
    fn belt(&self, pos: Pos) -> Option<(f32, Dir)>;
    /// A pulling belt funnel at `pos` facing `facing`.
    fn pulling_funnel(&self, pos: Pos, facing: Dir) -> bool;
    /// `hasBlockSolidSide(pos, face)`.
    fn solid_side(&self, pos: Pos, face: Dir) -> bool;
    /// A `FilteringBehaviour` at `pos` (a brass funnel's): whether it lets
    /// `stack` through and whether it is empty; none when there is none.
    fn filter(&self, pos: Pos, stack: &S) -> Option<(bool, bool)>;
    /// `handleInsertion` of `stack` at `pos` from `side`: what is left.
    fn insert(&mut self, pos: Pos, side: Dir, stack: S, simulate: bool) -> Option<S>;
    /// The tunnel at `tunnel` throws `stack` off the belt's end towards
    /// `side` (an item entity).
    fn eject(&mut self, tunnel: Pos, side: Dir, stack: S);
}

/// `testFlapFilter` with no filter of its own (sided filters aren't
/// ported): an adjacent filter decides.
fn test_flap_filter<S>(world: &dyn TunnelWorld<S>, tunnel: Pos, side: Dir, stack: &S) -> bool {
    world
        .filter(offset(tunnel, side), stack)
        .is_none_or(|(ok, _)| ok)
}

/// `flapFilterEmpty`.
fn flap_filter_empty<S>(world: &dyn TunnelWorld<S>, tunnel: Pos, side: Dir, stack: &S) -> bool {
    world
        .filter(offset(tunnel, side), stack)
        .is_none_or(|(_, empty)| empty)
}

/// `insertIntoTunnel`: `stack` out of the tunnel at `tunnel` by `side`.
/// None: it can't go there (`null`); otherwise what is left.
pub fn insert_into_tunnel<S: Stack>(
    world: &mut dyn TunnelWorld<S>,
    tunnel: Pos,
    side: Dir,
    stack: S,
    simulate: bool,
) -> Option<Option<S>> {
    if stack.count() == 0 {
        return Some(None);
    }
    if !test_flap_filter(world, tunnel, side, &stack) {
        return None;
    }
    let segment = below(tunnel);
    let (_, movement) = world.belt(segment)?;
    let out = offset(segment, side);
    if world.has_direct_input(out) {
        if !world.can_insert_from_side(out, side) {
            return None;
        }
        return Some(world.insert(out, side, stack, simulate));
    }
    if side == movement && !world.solid_side(out, side.opposite()) {
        if !simulate {
            world.eject(tunnel, side, stack);
        }
        return Some(None);
    }
    None
}

/// `addValidOutputsOf`: the sides of the tunnel at `tunnel` items may
/// leave by. `own`: the distributing tunnel, whose sides come first.
fn add_valid_outputs<S>(
    world: &dyn TunnelWorld<S>,
    tunnel: Pos,
    own: bool,
    out: &mut Vec<(Pos, Dir)>,
) {
    let Some((speed, movement)) = world.belt(below(tunnel)) else {
        return;
    };
    let Some(state) = world.tunnel(tunnel) else {
        return;
    };
    let sides: Vec<Dir> = state
        .sides(tunnel, world)
        .into_iter()
        .map(|(d, _)| d)
        .collect();
    for side_pass in [true, false] {
        if !own && side_pass {
            continue;
        }
        for d in HORIZONTAL {
            if d == movement && speed == 0.0 {
                continue;
            }
            if own && side_pass == (d.axis() == movement.axis()) {
                continue;
            }
            if d == movement.opposite() || !sides.contains(&d) {
                continue;
            }
            let o = offset(below(tunnel), d);
            if world.pulling_funnel(offset(tunnel, d), d) {
                continue;
            }
            if !world.has_direct_input(o) {
                if d == movement && !world.solid_side(o, d.opposite()) {
                    out.push((tunnel, d));
                }
                continue;
            }
            if world.can_insert_from_side(o, d) {
                out.push((tunnel, d));
            }
        }
    }
}

/// `gatherValidOutputs` for the brass tunnel at `pos`.
pub fn gather_valid_outputs<S>(world: &dyn TunnelWorld<S>, pos: Pos) -> Option<Vec<(Pos, Dir)>> {
    let tunnels = group(world, pos, |p| world.is_loaded(p))?;
    let mut out = Vec::new();
    for (i, t) in tunnels.into_iter().enumerate() {
        add_valid_outputs(world, t, i == 0, &mut out);
    }
    Some(out)
}

/// A brass tunnel's own state (`BrassTunnelBlockEntity`): what it holds and
/// where it is in its distribution.
#[derive(Clone, Debug, PartialEq)]
pub struct BrassTunnel<S> {
    pub stack: Option<S>,
    pub entered_from: Option<Dir>,
    /// `distributionProgress`: -1 to look for targets.
    pub progress: i32,
    pub previous_index: usize,
    /// `distributionTargets`: filtered first, then the others.
    pub targets: [Vec<(Pos, Dir)>; 2],
    pub new_item_arrived: bool,
    pub mode: SelectionMode,
}

impl<S> Default for BrassTunnel<S> {
    fn default() -> Self {
        BrassTunnel {
            stack: None,
            entered_from: None,
            progress: 0,
            previous_index: 0,
            targets: [Vec::new(), Vec::new()],
            new_item_arrived: false,
            mode: SelectionMode::Split,
        }
    }
}

impl<S: Stack> BrassTunnel<S> {
    /// `canTakeItems` (no synchronised outputs here).
    pub fn can_take(&self) -> bool {
        self.stack.as_ref().is_none_or(|s| s.count() == 0)
    }

    /// `setStackToDistribute`.
    pub fn set_stack(&mut self, stack: Option<S>, entered_from: Option<Dir>) {
        let stack = stack.filter(|s| s.count() > 0);
        if stack.is_some() {
            self.new_item_arrived = true;
        }
        self.stack = stack;
        self.entered_from = entered_from;
        self.progress = -1;
    }

    /// `tick` on the server for the tunnel at `pos`; true when its stack
    /// changed.
    pub fn tick(&mut self, pos: Pos, world: &mut dyn TunnelWorld<S>) -> bool {
        if self.progress > 0 {
            self.progress -= 1;
        }
        let Some((speed, _)) = world.belt(below(pos)) else {
            return false;
        };
        if speed == 0.0 {
            return false;
        }
        let Some(stack) = self.stack.clone().filter(|s| s.count() > 0) else {
            return false;
        };
        if self.progress == -1 {
            self.targets = [Vec::new(), Vec::new()];
            let Some(outputs) = gather_valid_outputs(world, pos) else {
                return false;
            };
            for (tunnel, side) in outputs {
                if insert_into_tunnel(world, tunnel, side, stack.clone(), true).is_none() {
                    continue;
                }
                let filtered = !flap_filter_empty(world, tunnel, side, &stack);
                self.targets[usize::from(!filtered)].push((tunnel, side));
            }
            if self.targets.iter().all(Vec::is_empty) {
                return false;
            }
            if self.new_item_arrived {
                self.new_item_arrived = false;
                self.progress = 2;
            } else {
                self.progress = BRASS_TUNNEL_TIMER;
                return false;
            }
        }
        if self.progress != 0 {
            return false;
        }
        let mut changed = false;
        for list in self.targets.clone() {
            if self.stack.is_none() {
                break;
            }
            let valid: Vec<(Pos, Dir)> = list
                .into_iter()
                .filter(|&(t, side)| !(t == pos && Some(side) == self.entered_from))
                .filter(|&(t, _)| world.tunnel(t).is_some_and(|s| s.brass))
                .collect();
            changed |= self.distribute(&valid, world);
            self.progress = -1;
        }
        changed
    }

    /// `distribute` over `targets` (Split: shared out as evenly as they
    /// take it; Round robin: the whole stack to the next one in turn).
    pub fn distribute(&mut self, targets: &[(Pos, Dir)], world: &mut dyn TunnelWorld<S>) -> bool {
        let n = targets.len();
        let Some(stack) = self.stack.clone() else {
            return false;
        };
        if n == 0 {
            return false;
        }
        let split = self.mode == SelectionMode::Split;
        let robin = self.mode == SelectionMode::RoundRobin;
        let with_count = |c: u32| {
            let mut s = stack.clone();
            s.set_count(c);
            s
        };
        // insertIntoTunnel's remainder count; none for `null`.
        let remainder =
            |world: &mut dyn TunnelWorld<S>, (t, side): (Pos, Dir), c: u32, simulate: bool| {
                if c == 0 {
                    return Some(0);
                }
                insert_into_tunnel(world, t, side, with_count(c), simulate)
                    .map(|r| r.map_or(0, |s| s.count()))
            };
        let mut distributed: Vec<((Pos, Dir), u32)> = Vec::new();
        let mut full: Vec<(Pos, Dir)> = Vec::new();
        let index_start = self.previous_index % n;
        let mut to_distribute = stack.count();
        for distribute_again in [true, false] {
            let mut this_cycle = to_distribute;
            let mut remaining = n;
            let mut leftovers = 0;
            for simulate in [true, false] {
                if remaining == 0 {
                    break;
                }
                leftovers = 0;
                let mut index = index_start;
                let stack_size = to_distribute;
                let split_size = stack_size / remaining as u32;
                let mut split_remainder = (stack_size % remaining as u32) as i32;
                let mut visited = 0;
                this_cycle = to_distribute;
                if !split && simulate {
                    continue;
                }
                while visited < n {
                    let pair = targets[index];
                    index = (index + 1) % n;
                    visited += 1;
                    if full.contains(&pair) {
                        if split && simulate {
                            remaining -= 1;
                        }
                        continue;
                    }
                    let count = if split {
                        split_size + u32::from(split_remainder > 0)
                    } else {
                        stack_size
                    };
                    let increased = distributed
                        .iter()
                        .find(|(p, _)| *p == pair)
                        .map(|(_, c)| *c);
                    let mut to_output = count + increased.unwrap_or(0);
                    let rem = remainder(world, pair, to_output, true);
                    let refused = match rem {
                        None => true,
                        Some(r) => {
                            r == if increased.is_some() {
                                count + 1
                            } else {
                                count
                            }
                        }
                    };
                    if refused {
                        if split && simulate {
                            remaining -= 1;
                        }
                        if !simulate {
                            full.push(pair);
                        }
                        if robin {
                            break;
                        }
                        continue;
                    }
                    let rem = rem.unwrap_or(0);
                    if rem > 0 && !simulate {
                        full.push(pair);
                    }
                    if !simulate {
                        to_output -= rem.min(to_output);
                        match distributed.iter_mut().find(|(p, _)| *p == pair) {
                            Some(e) => e.1 = to_output,
                            None => distributed.push((pair, to_output)),
                        }
                    }
                    leftovers += rem;
                    this_cycle = this_cycle.saturating_sub(count);
                    if this_cycle == 0 {
                        break;
                    }
                    split_remainder -= 1;
                    if !split {
                        break;
                    }
                }
            }
            to_distribute = this_cycle + leftovers;
            if leftovers == 0 && distribute_again {
                break;
            }
            if !split {
                break;
            }
        }
        let mut failed = 0;
        for (pair, c) in distributed {
            failed += remainder(world, pair, c, false).unwrap_or(c);
        }
        to_distribute += failed;
        let changed = to_distribute != stack.count();
        if to_distribute == 0 {
            self.stack = None;
            self.entered_from = None;
        } else {
            self.stack = Some(with_count(to_distribute));
        }
        self.previous_index = (self.previous_index + 1) % n;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Clone, Debug, PartialEq)]
    struct Item(u32);
    impl Stack for Item {
        fn count(&self) -> u32 {
            self.0
        }
        fn set_count(&mut self, count: u32) {
            self.0 = count;
        }
        fn max_stack(&self) -> u32 {
            64
        }
    }

    /// Belts along X at y = 0 moving east, tunnels above, depots or
    /// side belts beside them that each take up to `cap` items.
    #[derive(Default)]
    struct World {
        belts: HashMap<Pos, (Axis, f32, Dir)>,
        tunnels: HashMap<Pos, TunnelState>,
        inputs: HashMap<Pos, u32>,
        received: HashMap<Pos, u32>,
        ejected: Vec<(Pos, Dir, u32)>,
        solid: Vec<Pos>,
    }

    impl TunnelLevel for World {
        fn tunnel(&self, pos: Pos) -> Option<TunnelState> {
            self.tunnels.get(&pos).copied()
        }
        fn flat_belt_axis(&self, pos: Pos) -> Option<Axis> {
            self.belt_axis(pos)
        }
        fn belt_axis(&self, pos: Pos) -> Option<Axis> {
            self.belts.get(&pos).map(|b| b.0)
        }
        fn has_direct_input(&self, pos: Pos) -> bool {
            self.inputs.contains_key(&pos)
        }
        fn can_insert_from_side(&self, _: Pos, _: Dir) -> bool {
            true
        }
        fn extended_funnel(&self, _: Pos) -> Option<Dir> {
            None
        }
    }

    impl TunnelWorld<Item> for World {
        fn is_loaded(&self, _: Pos) -> bool {
            true
        }
        fn belt(&self, pos: Pos) -> Option<(f32, Dir)> {
            self.belts.get(&pos).map(|b| (b.1, b.2))
        }
        fn pulling_funnel(&self, _: Pos, _: Dir) -> bool {
            false
        }
        fn solid_side(&self, pos: Pos, _: Dir) -> bool {
            self.solid.contains(&pos)
        }
        fn filter(&self, _: Pos, _: &Item) -> Option<(bool, bool)> {
            None
        }
        fn insert(&mut self, pos: Pos, _: Dir, stack: Item, simulate: bool) -> Option<Item> {
            let cap = self.inputs[&pos];
            let have = self.received.get(&pos).copied().unwrap_or(0);
            let take = stack.0.min(cap - have);
            if !simulate {
                *self.received.entry(pos).or_default() += take;
            }
            (take < stack.0).then_some(Item(stack.0 - take))
        }
        fn eject(&mut self, tunnel: Pos, side: Dir, stack: Item) {
            self.ejected.push((tunnel, side, stack.0));
        }
    }

    fn belt_line(w: &mut World, xs: std::ops::Range<i32>, z: i32) {
        for x in xs {
            w.belts.insert([x, 0, z], (Axis::X, 32.0, Dir::East));
        }
    }

    #[test]
    fn names() {
        let t = TunnelState::from_block("create:hz_brass_tunnel_t_left", |k| {
            (k == "axis").then_some("z")
        })
        .unwrap();
        assert_eq!(
            t,
            TunnelState {
                brass: true,
                axis: Axis::Z,
                shape: Shape::TLeft
            }
        );
        assert_eq!(t.block(), "create:hz_brass_tunnel_t_left");
        let a = TunnelState::from_block(ANDESITE_TUNNEL, |_| Some("y")).unwrap();
        assert_eq!(
            (a.brass, a.axis, a.shape),
            (false, Axis::X, Shape::Straight)
        );
        assert_eq!(a.block(), ANDESITE_TUNNEL);
        assert!(TunnelState::from_block("create:hz_brass_tunnel_straight", |_| None).is_none());
        assert!(TunnelState::from_block("create:brass_funnel", |_| None).is_none());
    }

    #[test]
    fn shapes_from_neighbours() {
        let mut w = World::default();
        belt_line(&mut w, 0..3, 0);
        // Alone on a belt along X: straight, along X.
        let s = tunnel_state(&w, [1, 1, 0], false);
        assert_eq!((s.axis, s.shape), (Axis::X, Shape::Straight));
        // A belt along Z on its north side: its right (`left` is east's
        // clockwise turn, south).
        w.belts.insert([1, 0, -1], (Axis::Z, 32.0, Dir::North));
        assert_eq!(tunnel_state(&w, [1, 1, 0], false).shape, Shape::TRight);
        // A belt across it is no output.
        w.belts.insert([1, 0, 1], (Axis::X, 32.0, Dir::East));
        assert_eq!(tunnel_state(&w, [1, 1, 0], false).shape, Shape::TRight);
        // A depot on the south side too: a cross.
        w.belts.remove(&[1, 0, 1]);
        w.inputs.insert([1, 0, 1], 64);
        assert_eq!(tunnel_state(&w, [1, 1, 0], false).shape, Shape::Cross);
        w.belts.remove(&[1, 0, -1]);
        assert_eq!(tunnel_state(&w, [1, 1, 0], false).shape, Shape::TLeft);
        // Tunnels both ways along it: a window, which the wrench closes and
        // updates keep closed.
        let mut w = World::default();
        belt_line(&mut w, 0..3, 0);
        let straight = TunnelState {
            brass: false,
            axis: Axis::X,
            shape: Shape::Straight,
        };
        w.tunnels.insert([0, 1, 0], straight);
        w.tunnels.insert([2, 1, 0], straight);
        let mid = tunnel_state(&w, [1, 1, 0], false);
        assert_eq!(mid.shape, Shape::Window);
        let closed = mid.wrenched().unwrap();
        assert_eq!(closed.shape, Shape::Closed);
        assert_eq!(updated_state(&w, [1, 1, 0], closed), closed);
        assert!(straight.wrenched().is_none());
        // Sides and flaps: a straight tunnel only along its axis; a tunnel
        // beside it closes that flap.
        w.tunnels.insert([1, 1, 0], mid);
        assert_eq!(
            mid.sides([1, 1, 0], &w),
            vec![(Dir::West, false), (Dir::East, false)]
        );
        assert!(mid.flaps([1, 1, 0], &w).is_empty());
        let cross = TunnelState {
            shape: Shape::Cross,
            ..straight
        };
        assert_eq!(cross.flaps([5, 1, 5], &w), HORIZONTAL.to_vec());
        let t_left = TunnelState {
            shape: Shape::TLeft,
            ..straight
        };
        assert_eq!(
            t_left.flaps([5, 1, 5], &w),
            vec![Dir::South, Dir::West, Dir::East]
        );
        assert!(can_survive([1, 1, 0], &w));
        assert!(!can_survive([1, 2, 0], &w));
    }

    /// Brass tunnels over parallel belts along X (z = 0, 1, ...), each
    /// belt ending in a depot at x = 3.
    fn parallel(n: i32, mode: SelectionMode) -> (World, Vec<BrassTunnel<Item>>) {
        let mut w = World::default();
        for z in 0..n {
            belt_line(&mut w, 0..3, z);
            w.tunnels.insert(
                [1, 1, z],
                TunnelState {
                    brass: true,
                    axis: Axis::X,
                    shape: Shape::Straight,
                },
            );
            w.inputs.insert([2, 0, z], 64);
        }
        let tunnels = (0..n)
            .map(|_| BrassTunnel {
                mode,
                ..BrassTunnel::default()
            })
            .collect();
        (w, tunnels)
    }

    #[test]
    fn connected_tunnels_split() {
        let (mut w, mut t) = parallel(3, SelectionMode::Split);
        // Each tunnel's flaps face along the belt (no tunnel there), and
        // its neighbours connect it.
        assert!(is_connected(&w, [1, 1, 0], false));
        assert!(!is_connected(&w, [1, 1, 0], true));
        assert!(has_distribution(&w, [1, 1, 1]));
        assert_eq!(
            group(&w, [1, 1, 1], |_| true).unwrap(),
            vec![[1, 1, 1], [1, 1, 0], [1, 1, 2]]
        );
        // A lone straight brass tunnel doesn't distribute.
        let (w1, _) = parallel(1, SelectionMode::Split);
        assert!(!has_distribution(&w1, [1, 1, 0]));
        // Nine items into the middle one: three to each belt's next block.
        let outputs = gather_valid_outputs(&w, [1, 1, 1]).unwrap();
        assert_eq!(
            outputs,
            vec![
                ([1, 1, 1], Dir::East),
                ([1, 1, 0], Dir::East),
                ([1, 1, 2], Dir::East)
            ]
        );
        t[1].set_stack(Some(Item(9)), Some(Dir::West));
        let mut ticks = 0;
        while t[1].stack.is_some() && ticks < 20 {
            t[1].tick([1, 1, 1], &mut w);
            ticks += 1;
        }
        // Targets found at once, then two ticks (a new item).
        assert_eq!(ticks, 3);
        assert_eq!(w.received[&[2, 0, 0]], 3);
        assert_eq!(w.received[&[2, 0, 1]], 3);
        assert_eq!(w.received[&[2, 0, 2]], 3);
    }

    #[test]
    fn split_overflows_to_the_others() {
        let (mut w, mut t) = parallel(2, SelectionMode::Split);
        w.inputs.insert([2, 0, 0], 2);
        t[0].set_stack(Some(Item(10)), Some(Dir::West));
        let targets = gather_valid_outputs(&w, [1, 1, 0]).unwrap();
        t[0].distribute(&targets, &mut w);
        assert_eq!(w.received[&[2, 0, 0]], 2);
        assert_eq!(w.received[&[2, 0, 1]], 8);
        assert!(t[0].stack.is_none());
    }

    #[test]
    fn round_robin_takes_turns() {
        let (mut w, mut t) = parallel(2, SelectionMode::RoundRobin);
        let targets = gather_valid_outputs(&w, [1, 1, 0]).unwrap();
        for _ in 0..3 {
            t[0].set_stack(Some(Item(4)), Some(Dir::West));
            t[0].distribute(&targets, &mut w);
            assert!(t[0].stack.is_none());
        }
        assert_eq!(w.received[&[2, 0, 0]], 8);
        assert_eq!(w.received[&[2, 0, 1]], 4);
        // A full target: round robin waits for its turn to come round.
        w.inputs.insert([2, 0, 1], 4);
        t[0].set_stack(Some(Item(4)), Some(Dir::West));
        t[0].distribute(&targets, &mut w);
        assert_eq!(t[0].stack, Some(Item(4)));
        t[0].distribute(&targets, &mut w);
        assert!(t[0].stack.is_none());
        assert_eq!(w.received[&[2, 0, 0]], 12);
    }

    #[test]
    fn sides_first_and_off_the_end() {
        // A brass tunnel with a depot on its north side and nothing ahead:
        // its side, then the belt's end (thrown off), in that order.
        let mut w = World::default();
        belt_line(&mut w, 0..2, 0);
        w.inputs.insert([1, 0, -1], 64);
        w.tunnels.insert(
            [1, 1, 0],
            TunnelState {
                brass: true,
                axis: Axis::X,
                shape: Shape::TRight,
            },
        );
        assert!(has_distribution(&w, [1, 1, 0]));
        let outputs = gather_valid_outputs(&w, [1, 1, 0]).unwrap();
        assert_eq!(
            outputs,
            vec![([1, 1, 0], Dir::North), ([1, 1, 0], Dir::East)]
        );
        let mut t = BrassTunnel {
            mode: SelectionMode::Split,
            ..BrassTunnel::default()
        };
        t.set_stack(Some(Item(5)), Some(Dir::West));
        t.distribute(&outputs, &mut w);
        assert_eq!(w.received[&[1, 0, -1]], 3);
        assert_eq!(w.ejected, vec![([1, 1, 0], Dir::East, 2)]);
        // A solid block ahead: no output that way.
        w.solid.push([2, 0, 0]);
        assert_eq!(
            gather_valid_outputs(&w, [1, 1, 0]).unwrap(),
            vec![([1, 1, 0], Dir::North)]
        );
        // Stopped belts give nothing.
        w.belts.insert([1, 0, 0], (Axis::X, 0.0, Dir::East));
        t.set_stack(Some(Item(5)), Some(Dir::West));
        assert!(!t.tick([1, 1, 0], &mut w));
        assert_eq!(t.stack, Some(Item(5)));
    }
}
