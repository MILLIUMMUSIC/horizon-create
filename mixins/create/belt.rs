// Create's mechanical belt, injected as `crate::game::create_belt`: the
// belt's states (`BeltBlock`: slope, part, facing), its chain
// (`nextSegmentPosition`, `initBelt`, `getBeltChain`), its movement
// (`BeltBlockEntity`, `BeltHelper`), the items on it (`BeltInventory`,
// `TransportedItemStack`, `ItemHandlerBeltSegment`, the belt's
// `DirectBeltInputBehaviour`), the belt funnels it passes
// (`BeltFunnelInteractionHandler`) and the belt connector
// (`BeltConnectorItem`). No Horizon types: the world comes in through
// `BeltLevel` and `BeltWorld`, item stacks through `Stack`.

use super::create_fan::FanType;
use super::create_kinetics::{Axis, Dir, Pos};

/// `BeltSlope`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slope {
    Horizontal,
    Upward,
    Downward,
    Vertical,
    Sideways,
}

/// `BeltPart`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Start,
    Middle,
    End,
    Pulley,
}

impl Slope {
    pub const ALL: [Slope; 5] = [
        Slope::Horizontal,
        Slope::Upward,
        Slope::Downward,
        Slope::Vertical,
        Slope::Sideways,
    ];
}

impl Part {
    pub const ALL: [Part; 4] = [Part::Start, Part::Middle, Part::End, Part::Pulley];
}

/// The blocks a belt is made of here. Horizon has no template with a
/// belt's state properties, so each part is a block of its own on a
/// grindstone's (`face` and `facing`): its `face` is the slope (floor
/// horizontal, wall upward, ceiling downward); vertical and sideways belts
/// have no block yet.
pub const BELT_BLOCKS: [(&str, Part); 4] = [
    ("create:belt", Part::Start),
    ("create:hz_belt_middle", Part::Middle),
    ("create:hz_belt_end", Part::End),
    ("create:hz_belt_pulley", Part::Pulley),
];

/// A belt segment's block state (`BeltBlock`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BeltState {
    /// `HORIZONTAL_FACING`.
    pub facing: Dir,
    pub slope: Slope,
    pub part: Part,
}

impl BeltState {
    /// The belt segment of block `name` with its properties.
    pub fn from_block<'a>(name: &str, prop: impl Fn(&str) -> Option<&'a str>) -> Option<BeltState> {
        let part = BELT_BLOCKS.iter().find(|(n, _)| *n == name)?.1;
        let facing = Dir::parse(prop("facing")?)?;
        if facing.axis() == Axis::Y {
            return None;
        }
        let slope = match prop("face")? {
            "floor" => Slope::Horizontal,
            "wall" => Slope::Upward,
            "ceiling" => Slope::Downward,
            _ => return None,
        };
        Some(BeltState {
            facing,
            slope,
            part,
        })
    }
    /// Its block and the `face` property that holds its slope; none for
    /// the slopes that have no block here.
    pub fn block(self) -> Option<(&'static str, &'static str)> {
        let face = match self.slope {
            Slope::Horizontal => "floor",
            Slope::Upward => "wall",
            Slope::Downward => "ceiling",
            Slope::Vertical | Slope::Sideways => return None,
        };
        let name = BELT_BLOCKS.iter().find(|(_, p)| *p == self.part)?.0;
        Some((name, face))
    }
    /// `getRotationAxis`.
    pub fn rotation_axis(self) -> Axis {
        if self.slope == Slope::Sideways {
            return Axis::Y;
        }
        self.facing.clockwise(Axis::Y).axis()
    }
    /// `BeltBlockEntity.hasPulley`.
    pub fn has_pulley(self) -> bool {
        self.part != Part::Middle
    }
    /// `hasShaftTowards`.
    pub fn has_shaft_towards(self, face: Dir) -> bool {
        face.axis() == self.rotation_axis() && self.has_pulley()
    }
    /// `canTransportObjects`.
    pub fn can_transport_objects(self) -> bool {
        !matches!(self.slope, Slope::Vertical | Slope::Sideways)
    }
    /// `BeltBlockEntity.canPropagateDiagonally`.
    pub fn propagates_diagonally(self) -> bool {
        matches!(self.slope, Slope::Upward | Slope::Downward)
    }
    /// `nextSegmentPosition`.
    pub fn next_segment(self, pos: Pos, forward: bool) -> Option<Pos> {
        let offset = if forward { 1 } else { -1 };
        if self.part == Part::End && forward || self.part == Part::Start && !forward {
            return None;
        }
        if self.slope == Slope::Vertical {
            let up = if self.facing.positive() {
                offset
            } else {
                -offset
            };
            return Some([pos[0], pos[1] + up, pos[2]]);
        }
        let n = self.facing.normal();
        let mut p = [pos[0] + n[0] * offset, pos[1], pos[2] + n[2] * offset];
        if !matches!(self.slope, Slope::Horizontal | Slope::Sideways) {
            p[1] += if self.slope == Slope::Upward {
                offset
            } else {
                -offset
            };
        }
        Some(p)
    }
    /// Whether `other` at `to` follows or precedes this segment at `from`
    /// in its chain, as `propagateRotationTo` asks of their controllers:
    /// consecutive segments always share one.
    pub fn same_chain(self, from: Pos, other: BeltState, to: Pos) -> bool {
        (self.next_segment(from, true) == Some(to) && other.next_segment(to, false) == Some(from))
            || (self.next_segment(from, false) == Some(to)
                && other.next_segment(to, true) == Some(from))
    }
    /// The verticality of `getPositionForOffset`.
    fn verticality(self) -> i32 {
        match self.slope {
            Slope::Downward => -1,
            Slope::Upward => 1,
            _ => 0,
        }
    }
    /// `BeltBlockEntity.getBeltMovementSpeed`'s sign along the chain:
    /// `getDirectionAwareBeltMovementSpeed` at `speed` rpm.
    pub fn direction_aware_speed(self, speed: f32) -> f32 {
        let mut offset = if self.facing.positive() { 1.0 } else { -1.0 };
        if self.facing.axis() == Axis::X {
            offset *= -1.0;
        }
        speed / 480.0 * offset
    }
    /// `getMovementFacing`.
    pub fn movement_facing(self, speed: f32) -> Dir {
        let axis = self.facing.axis();
        let negative = (speed / 480.0 < 0.0) ^ (axis == Axis::X);
        let positive = Dir::positive_of(axis);
        if negative {
            positive.opposite()
        } else {
            positive
        }
    }
    /// `getMovementDirection(firstHalf, ignoreHalves)`.
    pub fn movement_direction(self, speed: f32, first_half: bool, ignore_halves: bool) -> Pos {
        if speed == 0.0 {
            return [0, 0, 0];
        }
        let axis = self.facing.axis();
        let mut movement_facing = if axis == Axis::X {
            Dir::positive_of(axis).opposite()
        } else {
            Dir::positive_of(axis)
        };
        if speed < 0.0 {
            movement_facing = movement_facing.opposite();
        }
        let m = movement_facing.normal();
        let not_horizontal = self.slope != Slope::Horizontal;
        let slope_before_half = (self.part == Part::End) == self.facing.positive();
        let on_slope = not_horizontal
            && (self.part == Part::Middle || slope_before_half == first_half || ignore_halves);
        if !on_slope {
            return m;
        }
        let up_slope = if movement_facing == self.facing {
            Slope::Upward
        } else {
            Slope::Downward
        };
        [m[0], if self.slope == up_slope { 1 } else { -1 }, m[2]]
    }
}

/// `BeltHelper.getPositionForOffset` from the controller at `pos`.
pub fn position_for_offset(pos: Pos, state: BeltState, length: i32, offset: i32) -> Pos {
    let n = state.facing.normal();
    let v = state.verticality();
    [
        pos[0] + offset * n[0],
        pos[1] + offset.clamp(0, (length - 1).max(0)) * v,
        pos[2] + offset * n[2],
    ]
}

/// `BeltHelper.getVectorForOffset`: the world position of `offset` along
/// the belt of the controller at `pos`.
pub fn vector_for_offset(pos: Pos, state: BeltState, length: i32, offset: f32) -> [f64; 3] {
    let mut vertical = state.verticality() as f32;
    if offset < 0.5 {
        vertical = 0.0;
    }
    let vertical = vertical * (offset.min(length as f32 - 0.5) - 0.5);
    let n = state.facing.normal();
    let mut horizontal = [n[0] as f32 * (offset - 0.5), n[2] as f32 * (offset - 0.5)];
    if state.slope == Slope::Vertical {
        horizontal = [0.0, 0.0];
    }
    [
        f64::from(pos[0]) + 0.5 + f64::from(horizontal[0]),
        f64::from(pos[1]) + 0.5 + f64::from(vertical),
        f64::from(pos[2]) + 0.5 + f64::from(horizontal[1]),
    ]
}

/// What the belt needs of the world's blocks.
pub trait BeltLevel {
    fn belt(&self, pos: Pos) -> Option<BeltState>;
    fn is_loaded(&self, pos: Pos) -> bool;
}

/// A belt's chain as `initBelt` sets it on each segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chain {
    /// The controller (its start) and every segment from it, in order.
    Belt { controller: Pos, segments: Vec<Pos> },
    /// A segment isn't loaded yet: try again later.
    Unloaded,
    /// `world.destroyBlock(pos, true)`: the broken belt at this position.
    Broken(Pos),
}

/// `BeltBlock.initBelt` for the segment at `pos`: its controller, found
/// going back, and the chain from it (`getBeltChain`).
pub fn find_chain(level: &dyn BeltLevel, pos: Pos) -> Chain {
    let mut current = pos;
    let mut limit = 1000;
    while limit > 0 {
        limit -= 1;
        let Some(state) = level.belt(current) else {
            return Chain::Broken(pos);
        };
        let Some(next) = state.next_segment(current, false) else {
            break;
        };
        if !level.is_loaded(next) {
            return Chain::Unloaded;
        }
        current = next;
    }
    let segments = belt_chain(level, current);
    if segments.len() < 2 {
        return Chain::Broken(current);
    }
    Chain::Belt {
        controller: current,
        segments,
    }
}

/// `BeltBlock.getBeltChain`.
pub fn belt_chain(level: &dyn BeltLevel, controller: Pos) -> Vec<Pos> {
    let mut out = Vec::new();
    let mut current = Some(controller);
    let mut limit = 1000;
    while let Some(p) = current {
        if limit == 0 {
            break;
        }
        limit -= 1;
        let Some(state) = level.belt(p) else {
            break;
        };
        out.push(p);
        current = state.next_segment(p, true);
    }
    out
}

/// What `BeltBlock.onRemove` does to the rest of a broken belt: each
/// segment next to the gone one at `pos` becomes a shaft (it had a pulley)
/// or air, which breaks the chain on in turn.
pub fn on_removed(level: &dyn BeltLevel, state: BeltState, pos: Pos) -> Vec<(Pos, Option<Axis>)> {
    let mut out = Vec::new();
    for forward in [true, false] {
        let Some(p) = state.next_segment(pos, forward) else {
            continue;
        };
        let Some(s) = level.belt(p) else {
            continue;
        };
        out.push((p, s.has_pulley().then(|| s.rotation_axis())));
    }
    out
}

// ---- Item stacks ------------------------------------------------------------

/// The item stacks the belt carries: what it needs of them.
pub trait Stack: Clone + PartialEq {
    fn count(&self) -> u32;
    fn set_count(&mut self, count: u32);
    /// `getMaxStackSize`.
    fn max_stack(&self) -> u32;
}

/// `TransportedItemStack`.
#[derive(Clone, Debug, PartialEq)]
pub struct Transported<S> {
    pub stack: S,
    pub belt_position: f32,
    pub side_offset: f32,
    pub angle: i32,
    pub inserted_at: i32,
    pub inserted_from: Dir,
    pub locked: bool,
    pub locked_externally: bool,
    pub prev_belt_position: f32,
    pub prev_side_offset: f32,
    pub processed_by: Option<FanType>,
    pub processing_time: i32,
}

impl<S: Stack> Transported<S> {
    /// A new one at angle `angle` (Create rolls 0..360, or 180 for items
    /// that stand upright).
    pub fn new(stack: S, angle: i32) -> Self {
        let side = target_side_offset(angle);
        Transported {
            stack,
            belt_position: 0.0,
            side_offset: side,
            angle,
            inserted_at: 0,
            inserted_from: Dir::Up,
            locked: false,
            locked_externally: false,
            prev_belt_position: 0.0,
            prev_side_offset: side,
            processed_by: None,
            processing_time: 0,
        }
    }
    /// `getTargetSideOffset`.
    pub fn target_side_offset(&self) -> f32 {
        target_side_offset(self.angle)
    }
    /// `getSimilar`: the same place and processing, a new angle.
    pub fn similar(&self, stack: S, angle: i32) -> Self {
        let mut t = Transported::new(stack, angle);
        t.belt_position = self.belt_position;
        t.inserted_at = self.inserted_at;
        t.inserted_from = self.inserted_from;
        t.prev_belt_position = self.prev_belt_position;
        t.prev_side_offset = self.prev_side_offset;
        t.processed_by = self.processed_by;
        t.processing_time = self.processing_time;
        t
    }
    /// `copy`: as `getSimilar`, with its angle and side offset; not locked.
    pub fn copied(&self) -> Self {
        let mut t = self.similar(self.stack.clone(), self.angle);
        t.side_offset = self.side_offset;
        t
    }
}

fn target_side_offset(angle: i32) -> f32 {
    (angle - 180) as f32 / (360.0 * 3.0)
}

/// `BeltProcessingBehaviour.ProcessingResult`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessingResult {
    Pass,
    Hold,
    /// Nothing here removes an item yet (Create's deployers and saws do).
    #[allow(dead_code)]
    Remove,
}

/// `TransportedResult`: what processing turned an item into. None of
/// `held` and no outputs removes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Conversion<S> {
    pub outputs: Vec<S>,
    pub held: Option<S>,
    /// The outputs stay held where they are (`BeltPressingCallbacks` sets
    /// `copy.locked` on its results: they wait under the press for the
    /// cycle's end, and the rest of the stack waits behind them).
    pub locked: bool,
}

/// `BeltFunnelBlock.Shape`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FunnelShape {
    Retracted,
    Extended,
    Pushing,
    Pulling,
}

/// A belt funnel above a segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeltFunnel {
    pub facing: Dir,
    pub shape: FunnelShape,
    pub powered: bool,
}

/// `BeltInventory.Ending`, with its margin (Create's `FUNNEL` ending is
/// commented out there, so it isn't here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Unresolved,
    Eject,
    Insert,
    Blocked,
}

impl Ending {
    fn margin(self) -> f32 {
        match self {
            Ending::Unresolved | Ending::Eject => 0.0,
            Ending::Insert => 0.25,
            Ending::Blocked => 0.45,
        }
    }
}

/// What a belt's items need of the world around it.
pub trait BeltWorld<S> {
    /// A press (`BeltProcessingBehaviour`) two above `segment`'s position
    /// at `pos`: `handleReceivedItem`, none when there is none.
    fn received(
        &mut self,
        pos: Pos,
        item: &Transported<S>,
    ) -> Option<(ProcessingResult, Option<Conversion<S>>)>;
    /// `handleHeldItem` for an item held at `pos`; none: no processing there.
    fn held(
        &mut self,
        pos: Pos,
        item: &Transported<S>,
    ) -> Option<(ProcessingResult, Option<Conversion<S>>)>;
    /// `BeltProcessingBehaviour.isBlocked`: something with collision above
    /// `pos` (not a funnel).
    fn processing_blocked(&self, pos: Pos) -> bool;
    /// A belt funnel at `pos` (above a segment).
    fn funnel(&self, pos: Pos) -> Option<BeltFunnel>;
    /// The funnel at `pos` inserting `stack` into the inventory behind it:
    /// what is left.
    fn funnel_insert(&mut self, pos: Pos, stack: &S) -> Option<S>;
    /// A `DirectBeltInputBehaviour` at `pos` (a belt, depot, funnel, chute).
    fn has_direct_input(&self, pos: Pos) -> bool;
    /// `canInsertFromSide` and `handleInsertion` at `pos` from `side`: what
    /// is left; none when it can't take anything.
    fn direct_insert(&mut self, pos: Pos, item: &Transported<S>, side: Dir) -> Option<Option<S>>;
    /// `hasBlockSolidSide(pos, face)`.
    fn solid_side(&self, pos: Pos, face: Dir) -> bool;
    /// An item entity at `at` moving at `motion`.
    fn eject(&mut self, stack: S, at: [f64; 3], motion: [f64; 3]);
}

/// The belt a controller drives, as its inventory sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Belt {
    pub controller: Pos,
    /// The controller's state.
    pub state: BeltState,
    pub length: i32,
    /// The controller's `getSpeed`.
    pub speed: f32,
}

impl Belt {
    pub fn position_for_offset(&self, offset: i32) -> Pos {
        position_for_offset(self.controller, self.state, self.length, offset)
    }
    pub fn vector_for_offset(&self, offset: f32) -> [f64; 3] {
        vector_for_offset(self.controller, self.state, self.length, offset)
    }
    pub fn movement_speed(&self) -> f32 {
        self.state.direction_aware_speed(self.speed)
    }
    pub fn movement_facing(&self) -> Dir {
        self.state.movement_facing(self.speed)
    }
}

/// `BeltInventory`: the items on one belt, kept by its controller.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BeltInventory<S> {
    /// The first is furthest along.
    pub items: Vec<Transported<S>>,
    pub to_insert: Vec<Transported<S>>,
    /// Indices into `items`, taken out at the next tick.
    pub to_remove: Vec<usize>,
    pub movement_positive: bool,
    /// `notifyUpdate`: clients need the items again.
    pub dirty: bool,
}

impl<S: Stack> BeltInventory<S> {
    pub fn new() -> Self {
        BeltInventory {
            items: Vec::new(),
            to_insert: Vec::new(),
            to_remove: Vec::new(),
            movement_positive: false,
            dirty: false,
        }
    }

    fn flush(&mut self) {
        if self.to_insert.is_empty() && self.to_remove.is_empty() {
            return;
        }
        let mut remove = std::mem::take(&mut self.to_remove);
        remove.sort_unstable();
        remove.dedup();
        for i in remove.into_iter().rev() {
            if i < self.items.len() {
                self.items.remove(i);
            }
        }
        for t in std::mem::take(&mut self.to_insert) {
            self.insert(t);
        }
        self.dirty = true;
    }

    /// `insert`: kept in order along the movement.
    fn insert(&mut self, new: Transported<S>) {
        // compareTo: the one further along is "less".
        let index = self
            .items
            .iter()
            .position(|s| (s.belt_position < new.belt_position) == self.movement_positive)
            .unwrap_or(self.items.len());
        self.items.insert(index, new);
    }

    /// `tick`.
    pub fn tick(&mut self, belt: &Belt, world: &mut dyn BeltWorld<S>) {
        self.flush();
        if belt.speed == 0.0 {
            return;
        }
        let speed = belt.movement_speed();
        if self.movement_positive != (speed > 0.0) {
            self.movement_positive = !self.movement_positive;
            self.items.reverse();
            self.dirty = true;
        }
        let positive = self.movement_positive;
        let movement_facing = belt.movement_facing();
        let horizontal = belt.state.slope == Slope::Horizontal;
        let spacing = 1.0f32;
        let mut ending = Ending::Unresolved;
        let mut front: Option<f32> = None;
        let mut i = 0;
        while i < self.items.len() {
            let stack_in_front = front;
            {
                let item = &mut self.items[i];
                item.prev_belt_position = item.belt_position;
                item.prev_side_offset = item.side_offset;
            }
            if self.items[i].stack.count() == 0 {
                self.items.remove(i);
                continue;
            }
            let mut movement = speed;
            if self.items[i].locked_externally {
                self.items[i].locked_externally = false;
                front = Some(self.items[i].belt_position);
                i += 1;
                continue;
            }
            let mut no_movement = false;
            let current = self.items[i].belt_position;
            if let Some(f) = stack_in_front {
                let diff = f - current;
                if diff.abs() <= spacing {
                    no_movement = true;
                }
                movement = if positive {
                    movement.min(diff - spacing)
                } else {
                    movement.max(diff + spacing)
                };
            }
            let mut diff_to_end = if positive {
                belt.length as f32 - current
            } else {
                -current
            };
            if diff_to_end.abs() < movement.abs() + 1.0 {
                if ending == Ending::Unresolved {
                    ending = self.resolve_ending(belt, world);
                }
                diff_to_end += if positive {
                    -ending.margin()
                } else {
                    ending.margin()
                };
            }
            let limited = if positive {
                movement.min(diff_to_end)
            } else {
                movement.max(diff_to_end)
            };
            let next_offset = self.items[i].belt_position + limited;

            if horizontal {
                let before = self.items[i].stack.clone();
                match self.handle_processing(belt, world, i, next_offset, no_movement) {
                    Step::Removed => {
                        self.dirty = true;
                        continue;
                    }
                    Step::Converted => {
                        // Its results come in at the next tick.
                        self.dirty = true;
                        continue;
                    }
                    Step::Kept => {}
                }
                if self.items[i].stack != before {
                    self.dirty = true;
                }
                if self.items[i].locked {
                    front = Some(self.items[i].belt_position);
                    i += 1;
                    continue;
                }
            }

            if self.check_for_funnels(belt, world, i, next_offset) {
                if self.items[i].stack.count() == 0 {
                    self.items.remove(i);
                    continue;
                }
                front = Some(self.items[i].belt_position);
                i += 1;
                continue;
            }
            if no_movement {
                front = Some(self.items[i].belt_position);
                i += 1;
                continue;
            }

            // Apply movement.
            {
                let item = &mut self.items[i];
                item.belt_position += limited;
                let to_middle = item.target_side_offset() - item.side_offset;
                item.side_offset +=
                    (to_middle * limited.abs() * 6.0).clamp(-to_middle.abs(), to_middle.abs());
            }
            if limited == movement {
                front = Some(self.items[i].belt_position);
                i += 1;
                continue;
            }

            // The end is reached.
            let next = belt.position_for_offset(if positive { belt.length } else { -1 });
            match ending {
                Ending::Insert => {
                    let item = self.items[i].clone();
                    match world.direct_insert(next, &item, movement_facing) {
                        Some(left) if left.as_ref() != Some(&item.stack) => {
                            self.dirty = true;
                            match left {
                                None => {
                                    self.items.remove(i);
                                    continue;
                                }
                                Some(l) => self.items[i].stack = l,
                            }
                        }
                        _ => {}
                    }
                }
                Ending::Eject => {
                    let item = self.items.remove(i);
                    self.eject(belt, world, item);
                    self.dirty = true;
                    continue;
                }
                _ => {}
            }
            front = Some(self.items[i].belt_position);
            i += 1;
        }
    }

    /// `handleBeltProcessingAndCheckIfRemoved` on item `i`.
    fn handle_processing(
        &mut self,
        belt: &Belt,
        world: &mut dyn BeltWorld<S>,
        i: usize,
        next_offset: f32,
        no_movement: bool,
    ) -> Step {
        let positive = self.movement_positive;
        let item = self.items[i].clone();
        let segment = item.belt_position as i32;
        if item.locked {
            let pos = belt.position_for_offset(segment);
            match world.held(pos, &item) {
                None => {
                    self.items[i].locked = false;
                    self.dirty = true;
                    Step::Kept
                }
                Some((result, conversion)) => {
                    if let Some(c) = conversion {
                        self.convert(i, segment, c);
                        return Step::Converted;
                    }
                    match result {
                        ProcessingResult::Remove => {
                            self.items.remove(i);
                            Step::Removed
                        }
                        ProcessingResult::Hold => Step::Kept,
                        ProcessingResult::Pass => {
                            self.items[i].locked = false;
                            self.dirty = true;
                            Step::Kept
                        }
                    }
                }
            }
        } else {
            if no_movement {
                return Step::Kept;
            }
            if !(item.belt_position > 0.5 || positive) {
                return Step::Kept;
            }
            let first = (item.belt_position + if positive { 0.5 } else { -0.5 }) as i32;
            let step = if positive { 1 } else { -1 };
            let mut segment = first;
            while if positive {
                segment as f32 + 0.5 <= next_offset
            } else {
                segment as f32 + 0.5 >= next_offset
            } {
                let pos = belt.position_for_offset(segment);
                if !world.processing_blocked(pos)
                    && let Some((result, conversion)) = world.received(pos, &self.items[i])
                {
                    if let Some(c) = conversion {
                        self.convert(i, segment, c);
                        return Step::Converted;
                    }
                    match result {
                        ProcessingResult::Remove => {
                            self.items.remove(i);
                            return Step::Removed;
                        }
                        ProcessingResult::Hold => {
                            let item = &mut self.items[i];
                            item.belt_position = segment as f32
                                + 0.5
                                + if positive { 1.0 / 512.0 } else { -1.0 / 512.0 };
                            item.locked = true;
                            self.dirty = true;
                            return Step::Kept;
                        }
                        ProcessingResult::Pass => {}
                    }
                }
                segment += step;
            }
            Step::Kept
        }
    }

    /// `applyToEachWithin`'s result on item `i` at `segment`
    /// (`handleProcessingOnItem`): it goes, its outputs where it was and
    /// what stays held at the segment's middle come in at the next tick.
    fn convert(&mut self, i: usize, segment: i32, c: Conversion<S>) {
        let positive = self.movement_positive;
        let item = self.items.remove(i);
        if let Some(h) = c.held {
            let mut held = item.copied();
            held.stack = h;
            held.belt_position =
                segment as f32 + 0.5 - if positive { 1.0 / 512.0 } else { -1.0 / 512.0 };
            self.to_insert.push(held);
        }
        for (k, out) in c.outputs.into_iter().enumerate() {
            let angle = (item.angle + 37 * (k as i32 + 1)).rem_euclid(360);
            let mut t = item.similar(out, angle);
            t.locked = c.locked;
            self.to_insert.push(t);
        }
    }

    /// `BeltFunnelInteractionHandler.checkForFunnels` for item `i`.
    fn check_for_funnels(
        &mut self,
        belt: &Belt,
        world: &mut dyn BeltWorld<S>,
        i: usize,
        next_offset: f32,
    ) -> bool {
        let positive = self.movement_positive;
        let step = if positive { 1 } else { -1 };
        let mut next_segment =
            (self.items[i].belt_position.floor() as i32).clamp(0, (belt.length - 1).max(0));
        let movement_facing = belt.movement_facing();
        loop {
            let segment = next_segment;
            let within = if positive {
                segment as f32 <= next_offset
            } else {
                segment as f32 + 1.0 >= next_offset
            };
            if !within {
                break;
            }
            next_segment += step;
            let funnel_pos = {
                let p = belt.position_for_offset(segment);
                [p[0], p[1] + 1, p[2]]
            };
            let Some(funnel) = world.funnel(funnel_pos) else {
                continue;
            };
            let blocking = funnel.facing == movement_facing.opposite();
            if funnel.facing == movement_facing || funnel.shape == FunnelShape::Pushing {
                continue;
            }
            let mut entry = segment as f32 + 0.5;
            if funnel.shape == FunnelShape::Extended {
                entry += 0.499 * if positive { -1.0 } else { 1.0 };
            }
            let crossed = next_offset > entry && positive || next_offset < entry && !positive;
            if !crossed {
                return false;
            }
            if blocking {
                self.items[i].belt_position = entry;
            }
            if funnel.powered {
                if blocking {
                    return true;
                }
                continue;
            }
            let stack = self.items[i].stack.clone();
            let left = world.funnel_insert(funnel_pos, &stack);
            if left.as_ref() == Some(&stack) {
                if blocking {
                    return true;
                }
                continue;
            }
            match left {
                Some(l) => self.items[i].stack = l,
                None => self.items[i].stack.set_count(0),
            }
            self.dirty = true;
            if blocking || self.items[i].stack.count() == 0 {
                return true;
            }
        }
        false
    }

    /// `resolveEnding`.
    fn resolve_ending(&self, belt: &Belt, world: &dyn BeltWorld<S>) -> Ending {
        let next = belt.position_for_offset(if self.movement_positive {
            belt.length
        } else {
            -1
        });
        if world.has_direct_input(next) {
            return Ending::Insert;
        }
        if world.solid_side(next, belt.movement_facing().opposite()) {
            return Ending::Blocked;
        }
        Ending::Eject
    }

    /// `eject`: an item entity off the belt's end, with the belt's speed.
    pub fn eject(&self, belt: &Belt, world: &mut dyn BeltWorld<S>, item: Transported<S>) {
        let at = belt.vector_for_offset(item.belt_position);
        let speed = (belt.speed / 480.0).abs().max(1.0 / 8.0);
        let d = belt.state.movement_direction(belt.speed, true, true);
        let motion = [
            f64::from(d[0] as f32 * speed),
            f64::from(d[1] as f32 * speed + 1.0 / 8.0),
            f64::from(d[2] as f32 * speed),
        ];
        let len = (motion[0] * motion[0] + motion[1] * motion[1] + motion[2] * motion[2]).sqrt();
        let nudge = |k: usize| {
            if len > 0.0 {
                motion[k] / len * 0.001
            } else {
                0.0
            }
        };
        let at = [
            at[0] + nudge(0),
            at[1] + nudge(1) + 6.0 / 16.0,
            at[2] + nudge(2),
        ];
        world.eject(item.stack, at, motion);
    }

    /// `ejectAll`.
    pub fn eject_all(&mut self, belt: &Belt, world: &mut dyn BeltWorld<S>) {
        self.flush();
        for item in std::mem::take(&mut self.items) {
            self.eject(belt, world, item);
        }
        self.dirty = true;
    }

    /// `canInsertAtFromSide`.
    pub fn can_insert_at_from_side(&self, belt: &Belt, segment: i32, side: Dir) -> bool {
        let facing = belt.movement_facing();
        let mut segment_pos = segment as f32;
        if facing == side.opposite() {
            return false;
        }
        if facing != side {
            segment_pos += 0.5;
        } else if !self.movement_positive {
            segment_pos += 1.0;
        }
        let blocking = |s: &Transported<S>| {
            s.inserted_at == segment
                && s.inserted_from == side
                && if self.movement_positive {
                    s.belt_position <= segment_pos + 1.0
                } else {
                    s.belt_position >= segment_pos - 1.0
                }
        };
        !self.items.iter().any(blocking) && !self.to_insert.iter().any(blocking)
    }

    /// `ItemHandlerBeltSegment.insertItem` at `segment` (from above: a
    /// dropped item, a funnel or chute's `insertItem`): what is left.
    pub fn insert_at(
        &mut self,
        belt: &Belt,
        segment: i32,
        stack: S,
        angle: i32,
        simulate: bool,
    ) -> Option<S> {
        if !self.can_insert_at_from_side(belt, segment, Dir::Up) {
            return Some(stack);
        }
        let max = stack.max_stack();
        let mut left = None;
        let mut stack = stack;
        if stack.count() > max {
            let mut l = stack.clone();
            l.set_count(stack.count() - max);
            stack.set_count(max);
            left = Some(l);
        }
        if !simulate {
            let mut t = Transported::new(stack, angle);
            t.inserted_at = segment;
            t.belt_position =
                segment as f32 + 0.5 + if self.movement_positive { -1.0 } else { 1.0 } / 16.0;
            t.prev_belt_position = t.belt_position;
            self.to_insert.push(t);
            self.dirty = true;
        }
        left
    }

    /// `BeltBlockEntity.isOccupied` from `side` at `segment`.
    pub fn is_occupied(&self, belt: &Belt, segment: i32, side: Dir) -> bool {
        belt.speed == 0.0
            || belt.movement_facing() == side.opposite()
            || !self.can_insert_at_from_side(belt, segment, side)
    }

    /// `BeltBlockEntity.tryInsertingFromSide`: another belt, a chute or a
    /// funnel hands `item` to segment `segment` from `side` (the way it
    /// moves); what is left. `from_belt`: the block behind the segment is a
    /// belt (a smoother hand-over).
    pub fn insert_from_side(
        &mut self,
        belt: &Belt,
        segment_state: BeltState,
        segment: i32,
        item: &Transported<S>,
        side: Dir,
        from_belt: bool,
    ) -> Option<S> {
        if !segment_state.can_transport_objects() || self.is_occupied(belt, segment, side) {
            return Some(item.stack.clone());
        }
        let mut t = item.copied();
        let speed = belt.movement_speed();
        t.belt_position = segment as f32 + 0.5 - speed.signum() / 16.0;
        let facing = belt.movement_facing();
        if side.axis() != Axis::Y {
            if facing != side {
                t.side_offset = if side.positive() { 0.675 } else { -0.675 };
                if side.axis() == Axis::X {
                    t.side_offset *= -1.0;
                }
            } else {
                let extra = if t.prev_belt_position != 0.0 && from_belt {
                    0.26
                } else {
                    0.0
                };
                t.belt_position = if speed > 0.0 {
                    segment as f32 - extra
                } else {
                    segment as f32 + 1.0 + extra
                };
            }
        }
        t.prev_side_offset = t.side_offset;
        t.inserted_at = segment;
        t.inserted_from = side;
        t.prev_belt_position = t.belt_position;
        t.locked = false;
        self.to_insert.push(t);
        self.dirty = true;
        None
    }

    /// `getStackAtOffset`: the item over segment `offset`.
    pub fn stack_at_offset(&self, offset: i32) -> Option<usize> {
        let (min, max) = (offset as f32, offset as f32 + 1.0);
        (0..self.items.len()).find(|&i| {
            let s = &self.items[i];
            !self.to_remove.contains(&i) && s.belt_position <= max && s.belt_position > min
        })
    }

    /// `ItemHandlerBeltSegment.extractItem`: up to `amount` of the item over
    /// `offset`.
    pub fn extract_at(&mut self, offset: i32, amount: u32, simulate: bool) -> Option<S> {
        let i = self.stack_at_offset(offset)?;
        let n = amount.min(self.items[i].stack.count());
        if n == 0 {
            return None;
        }
        let mut out = self.items[i].stack.clone();
        out.set_count(n);
        if !simulate {
            let left = self.items[i].stack.count() - n;
            self.items[i].stack.set_count(left);
            if left == 0 {
                self.to_remove.push(i);
            }
            self.dirty = true;
        }
        Some(out)
    }

    /// `applyToEachWithin(position, maxDistance, f)`: `f` may turn each item
    /// near `position` into others.
    pub fn apply_to_each_within(
        &mut self,
        position: f32,
        max_distance: f32,
        mut f: impl FnMut(&mut Transported<S>) -> Option<Conversion<S>>,
    ) {
        let positive = self.movement_positive;
        for i in 0..self.items.len() {
            if self.to_remove.contains(&i) {
                continue;
            }
            if (position - self.items[i].belt_position).abs() >= max_distance {
                continue;
            }
            let before = self.items[i].clone();
            let Some(c) = f(&mut self.items[i]) else {
                if self.items[i] != before {
                    self.dirty = true;
                }
                continue;
            };
            let item = self.items[i].clone();
            if let Some(h) = c.held {
                let mut held = item.copied();
                held.stack = h;
                held.belt_position =
                    position.trunc() + 0.5 - if positive { 1.0 / 512.0 } else { -1.0 / 512.0 };
                self.to_insert.push(held);
            }
            for (k, out) in c.outputs.into_iter().enumerate() {
                let angle = (item.angle + 37 * (k as i32 + 1)).rem_euclid(360);
                let mut t = item.similar(out, angle);
                t.locked = c.locked;
                self.to_insert.push(t);
            }
            self.to_remove.push(i);
            self.dirty = true;
        }
    }

    /// Everything on it, removed (the belt was broken, a player took them).
    pub fn take_within(&mut self, position: f32, max_distance: f32) -> Vec<S> {
        let mut out = Vec::new();
        self.apply_to_each_within(position, max_distance, |t| {
            out.push(t.stack.clone());
            Some(Conversion {
                outputs: Vec::new(),
                held: None,
                locked: false,
            })
        });
        out
    }
}

enum Step {
    Kept,
    Removed,
    Converted,
}

// ---- The belt connector -------------------------------------------------------

/// `maxBeltLength`'s default.
pub const MAX_BELT_LENGTH: i32 = 20;

/// What the connector needs of the world.
pub trait ConnectorLevel {
    fn is_loaded(&self, pos: Pos) -> bool;
    /// The axis of a shaft at `pos` (`ShaftBlock.isShaft`).
    fn shaft_axis(&self, pos: Pos) -> Option<Axis>;
    /// A kinetic block entity's `getTheoreticalSpeed`; none without one.
    fn theoretical_speed(&self, pos: Pos) -> Option<f32>;
    /// `canBeReplaced`.
    fn replaceable(&self, pos: Pos) -> bool;
}

fn closer_than(a: Pos, b: Pos, distance: i32) -> bool {
    let d: i64 = (0..3).map(|k| i64::from(a[k] - b[k]).pow(2)).sum();
    d < i64::from(distance) * i64::from(distance)
}

/// `BeltConnectorItem.canConnect`.
pub fn can_connect(level: &dyn ConnectorLevel, first: Pos, second: Pos) -> bool {
    if !level.is_loaded(first) || !level.is_loaded(second) {
        return false;
    }
    if !closer_than(second, first, MAX_BELT_LENGTH) {
        return false;
    }
    let diff = [
        second[0] - first[0],
        second[1] - first[1],
        second[2] - first[2],
    ];
    let Some(axis) = level.shaft_axis(first) else {
        return false;
    };
    let [x, y, z] = diff;
    let sames = i32::from(x.abs() == y.abs())
        + i32::from(y.abs() == z.abs())
        + i32::from(z.abs() == x.abs());
    if axis.choose(diff) != 0 || sames != 1 {
        return false;
    }
    if level.shaft_axis(second) != Some(axis) {
        return false;
    }
    if axis == Axis::Y && x != 0 && z != 0 {
        return false;
    }
    let (Some(s1), Some(s2)) = (
        level.theoretical_speed(first),
        level.theoretical_speed(second),
    ) else {
        return false;
    };
    if s1.signum() != s2.signum() && s1 != 0.0 && s2 != 0.0 {
        return false;
    }
    let step = diff.map(i32::signum);
    let mut current = [first[0] + step[0], first[1] + step[1], first[2] + step[2]];
    let mut limit = 1000;
    while current != second && limit > 0 {
        limit -= 1;
        if level.shaft_axis(current) != Some(axis) && !level.replaceable(current) {
            return false;
        }
        current = [
            current[0] + step[0],
            current[1] + step[1],
            current[2] + step[2],
        ];
    }
    true
}

/// `getSlopeBetween`.
pub fn slope_between(start: Pos, end: Pos) -> Slope {
    let diff = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
    if diff[1] != 0 {
        if diff[0] != 0 || diff[2] != 0 {
            return if diff[1] > 0 {
                Slope::Upward
            } else {
                Slope::Downward
            };
        }
        return Slope::Vertical;
    }
    Slope::Horizontal
}

/// `getFacingFromTo`.
pub fn facing_from_to(start: Pos, end: Pos) -> Dir {
    let axis = if start[0] == end[0] { Axis::Z } else { Axis::X };
    let diff = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
    let positive = if diff[0] == 0 && diff[2] == 0 {
        diff[1] > 0
    } else {
        axis.choose([diff[0], 0, diff[2]]) > 0
    };
    if positive {
        Dir::positive_of(axis)
    } else {
        Dir::positive_of(axis).opposite()
    }
}

/// `createBelts`: the segments from `start` to `end` with their states
/// (`start_axis`: the first shaft's axis). Shafts on the way become
/// pulleys; a shaft along Y makes the belt sideways.
pub fn belts_between(level: &dyn ConnectorLevel, start: Pos, end: Pos) -> Vec<(Pos, BeltState)> {
    let mut slope = slope_between(start, end);
    let mut facing = facing_from_to(start, end);
    let diff = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
    if diff[0] == diff[2] {
        let axis = if level.shaft_axis(start) == Some(Axis::X) {
            Axis::Z
        } else {
            Axis::X
        };
        facing = if facing.positive() {
            Dir::positive_of(axis)
        } else {
            Dir::positive_of(axis).opposite()
        };
    }
    let mut positions = Vec::new();
    let mut current = start;
    let mut limit = 1000;
    loop {
        positions.push(current);
        if slope == Slope::Vertical {
            current[1] += if facing.positive() { 1 } else { -1 };
        } else {
            let n = facing.normal();
            current = [current[0] + n[0], current[1], current[2] + n[2]];
            if slope != Slope::Horizontal {
                current[1] += if slope == Slope::Upward { 1 } else { -1 };
            }
        }
        if current == end || limit <= 0 {
            break;
        }
        limit -= 1;
    }
    positions.push(end);
    let mut out = Vec::new();
    for pos in positions {
        let mut part = if pos == start {
            Part::Start
        } else if pos == end {
            Part::End
        } else {
            Part::Middle
        };
        let shaft = level.shaft_axis(pos);
        if part == Part::Middle && shaft.is_some() {
            part = Part::Pulley;
        }
        if shaft == Some(Axis::Y) {
            slope = Slope::Sideways;
        }
        out.push((
            pos,
            BeltState {
                facing,
                slope,
                part,
            },
        ));
    }
    out
}

// ---- Rendering ------------------------------------------------------------------

/// `BeltRenderer`'s partials for a segment, bottom first: (model, bottom,
/// diagonal); and its turn about Y (`horizontalAngle + 180` upward).
pub fn belt_partials(state: BeltState) -> (Vec<(&'static str, bool, bool)>, f32) {
    let downward = state.slope == Slope::Downward;
    let upward = state.slope == Slope::Upward;
    let diagonal = downward || upward;
    let mut start = state.part == Part::Start;
    let mut end = state.part == Part::End;
    if downward || state.slope == Slope::Vertical && state.facing.positive() {
        std::mem::swap(&mut start, &mut end);
    }
    let mut out = Vec::new();
    for bottom in [true, false] {
        let name = if diagonal {
            if start {
                "belt_diagonal_start"
            } else if end {
                "belt_diagonal_end"
            } else {
                "belt_diagonal_middle"
            }
        } else if bottom {
            if start {
                "belt_start_bottom"
            } else if end {
                "belt_end_bottom"
            } else {
                "belt_middle_bottom"
            }
        } else if start {
            "belt_start"
        } else if end {
            "belt_end"
        } else {
            "belt_middle"
        };
        out.push((name, bottom && !diagonal, diagonal));
        if diagonal {
            break;
        }
    }
    let angle = state.facing.horizontal_angle()
        + if upward { 180.0 } else { 0.0 }
        + if state.slope == Slope::Sideways {
            270.0
        } else {
            0.0
        };
    (out, angle)
}

/// `BeltRenderer`'s texture scroll, as a fraction of the scrolling
/// sprite's height, at `time` ticks and `speed` rpm.
pub fn belt_scroll(state: BeltState, speed: f32, time: f32, bottom: bool) -> f32 {
    let diagonal = matches!(state.slope, Slope::Upward | Slope::Downward);
    let downward = state.slope == Slope::Downward;
    let along_x = state.facing.axis() == Axis::X;
    let sideways = state.slope == Slope::Sideways;
    let time = time * if state.facing.positive() { 1.0 } else { -1.0 };
    let mut speed = speed;
    if diagonal && (downward ^ along_x)
        || !sideways && !diagonal && along_x
        || sideways && !state.facing.positive()
    {
        speed = -speed;
    }
    let mult = if diagonal { 3.0 / 8.0 } else { 0.5 };
    let scroll =
        f64::from(speed) * f64::from(time) / (31.5 * 16.0) + if bottom { 0.5 } else { 0.0 };
    ((scroll - scroll.floor()) as f32) * mult
}

/// `BeltRenderer.renderItem`: where an item at `offset` (and `side`)
/// along the belt of the controller at `pos` is drawn, and its tilt about
/// the belt's sideways axis in degrees.
pub fn item_place(
    pos: Pos,
    state: BeltState,
    length: i32,
    offset: f32,
    side: f32,
) -> ([f64; 3], f32) {
    let n = state.facing.normal();
    let verticality = state.verticality() as f32;
    let start = [
        f64::from(pos[0]) + 0.5 - 0.5 * f64::from(n[0]),
        f64::from(pos[1]) + 15.0 / 16.0,
        f64::from(pos[2]) + 0.5 - 0.5 * f64::from(n[2]),
    ];
    let vertical = if offset < 0.5 {
        0.0
    } else {
        verticality * (offset.min(length as f32 - 0.5) - 0.5)
    };
    let on_slope =
        state.slope != Slope::Horizontal && offset.clamp(0.5, length as f32 - 0.5) == offset;
    let tilt_forward =
        (downward(state) ^ state.facing.positive()) == (state.facing.axis() == Axis::Z);
    let angle = if on_slope {
        if tilt_forward { -45.0 } else { 45.0 }
    } else {
        0.0
    };
    let along_x = state.facing.clockwise(Axis::Y).axis() == Axis::X;
    let side = if along_x { side } else { -side };
    let mut p = [
        start[0] + f64::from(n[0] as f32 * offset),
        start[1] + f64::from(vertical),
        start[2] + f64::from(n[2] as f32 * offset),
    ];
    if along_x {
        p[0] += f64::from(side);
    } else {
        p[2] += f64::from(side);
    }
    if on_slope {
        p[1] += 1.0 / 8.0;
    }
    (p, angle)
}

fn downward(state: BeltState) -> bool {
    state.slope == Slope::Downward
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Clone, Debug, PartialEq)]
    struct Item(&'static str, u32);

    impl Stack for Item {
        fn count(&self) -> u32 {
            self.1
        }
        fn set_count(&mut self, count: u32) {
            self.1 = count;
        }
        fn max_stack(&self) -> u32 {
            64
        }
    }

    fn st(facing: Dir, slope: Slope, part: Part) -> BeltState {
        BeltState {
            facing,
            slope,
            part,
        }
    }

    #[derive(Default)]
    struct Blocks {
        belts: HashMap<Pos, BeltState>,
        shafts: HashMap<Pos, Axis>,
        solid: Vec<Pos>,
        inputs: Vec<Pos>,
        inserted: Vec<(Pos, Item, Dir)>,
        ejected: Vec<(Item, [f64; 3], [f64; 3])>,
        presses: Vec<Pos>,
        funnels: HashMap<Pos, BeltFunnel>,
        funnelled: Vec<Item>,
        /// The press's running ticks (`PressingBehaviour`), none at rest.
        cycle: Option<i32>,
    }

    const HALF: i32 = 60;

    impl Blocks {
        /// `PressingBehaviour.tick`: the cycle ends after twice its middle.
        fn tick_press(&mut self) {
            self.cycle = self.cycle.map(|t| t + 1).filter(|t| *t <= 2 * HALF);
        }
    }

    impl BeltLevel for Blocks {
        fn belt(&self, pos: Pos) -> Option<BeltState> {
            self.belts.get(&pos).copied()
        }
        fn is_loaded(&self, _: Pos) -> bool {
            true
        }
    }

    impl ConnectorLevel for Blocks {
        fn is_loaded(&self, _: Pos) -> bool {
            true
        }
        fn shaft_axis(&self, pos: Pos) -> Option<Axis> {
            self.shafts.get(&pos).copied()
        }
        fn theoretical_speed(&self, pos: Pos) -> Option<f32> {
            self.shafts.contains_key(&pos).then_some(0.0)
        }
        fn replaceable(&self, pos: Pos) -> bool {
            !self.solid.contains(&pos)
        }
    }

    impl BeltWorld<Item> for Blocks {
        fn received(
            &mut self,
            pos: Pos,
            item: &Transported<Item>,
        ) -> Option<(ProcessingResult, Option<Conversion<Item>>)> {
            if !self.presses.contains(&pos) {
                return None;
            }
            // BeltPressingCallbacks.onItemReceived.
            Some(if self.cycle.is_some() {
                (ProcessingResult::Hold, None)
            } else if item.stack.0 == "iron_ingot" {
                self.cycle = Some(0);
                (ProcessingResult::Hold, None)
            } else {
                (ProcessingResult::Pass, None)
            })
        }
        fn held(
            &mut self,
            pos: Pos,
            item: &Transported<Item>,
        ) -> Option<(ProcessingResult, Option<Conversion<Item>>)> {
            if !self.presses.contains(&pos) {
                return None;
            }
            // BeltPressingCallbacks.whenItemHeld: pressed at the cycle's
            // middle, one at a time; held until the cycle ends.
            match self.cycle {
                None => return Some((ProcessingResult::Pass, None)),
                Some(HALF) if item.stack.0 == "iron_ingot" => {}
                Some(_) => return Some((ProcessingResult::Hold, None)),
            }
            let mut left = item.stack.clone();
            left.1 -= 1;
            Some((
                ProcessingResult::Hold,
                Some(Conversion {
                    outputs: vec![Item("iron_sheet", 1)],
                    held: (left.1 > 0).then_some(left),
                    locked: true,
                }),
            ))
        }
        fn processing_blocked(&self, _: Pos) -> bool {
            false
        }
        fn funnel(&self, pos: Pos) -> Option<BeltFunnel> {
            self.funnels.get(&pos).copied()
        }
        fn funnel_insert(&mut self, _: Pos, stack: &Item) -> Option<Item> {
            self.funnelled.push(stack.clone());
            None
        }
        fn has_direct_input(&self, pos: Pos) -> bool {
            self.inputs.contains(&pos)
        }
        fn direct_insert(
            &mut self,
            pos: Pos,
            item: &Transported<Item>,
            side: Dir,
        ) -> Option<Option<Item>> {
            self.inserted.push((pos, item.stack.clone(), side));
            Some(None)
        }
        fn solid_side(&self, pos: Pos, _: Dir) -> bool {
            self.solid.contains(&pos)
        }
        fn eject(&mut self, stack: Item, at: [f64; 3], motion: [f64; 3]) {
            self.ejected.push((stack, at, motion));
        }
    }

    /// A flat belt of `n` segments from the origin towards `facing`.
    fn flat(blocks: &mut Blocks, facing: Dir, n: i32) -> Vec<Pos> {
        let d = facing.normal();
        (0..n)
            .map(|i| {
                let p = [d[0] * i, 0, d[2] * i];
                let part = if i == 0 {
                    Part::Start
                } else if i == n - 1 {
                    Part::End
                } else {
                    Part::Middle
                };
                blocks.belts.insert(p, st(facing, Slope::Horizontal, part));
                p
            })
            .collect()
    }

    #[test]
    fn blocks_round_trip() {
        for part in Part::ALL {
            for slope in [Slope::Horizontal, Slope::Upward, Slope::Downward] {
                let s = st(Dir::West, slope, part);
                let (name, face) = s.block().unwrap();
                let back = BeltState::from_block(name, |k| match k {
                    "facing" => Some("west"),
                    "face" => Some(face),
                    _ => None,
                });
                assert_eq!(back, Some(s));
            }
        }
        assert_eq!(st(Dir::North, Slope::Vertical, Part::Start).block(), None);
    }

    #[test]
    fn axes_and_shafts() {
        let s = st(Dir::North, Slope::Horizontal, Part::Start);
        assert_eq!(s.rotation_axis(), Axis::X);
        assert!(s.has_shaft_towards(Dir::East));
        assert!(!s.has_shaft_towards(Dir::North));
        assert!(!st(Dir::North, Slope::Horizontal, Part::Middle).has_shaft_towards(Dir::East));
        assert_eq!(
            st(Dir::East, Slope::Sideways, Part::End).rotation_axis(),
            Axis::Y
        );
    }

    #[test]
    fn chain_from_any_segment() {
        let mut b = Blocks::default();
        let segs = flat(&mut b, Dir::East, 4);
        let chain = find_chain(&b, segs[2]);
        assert_eq!(
            chain,
            Chain::Belt {
                controller: [0, 0, 0],
                segments: segs.clone()
            }
        );
        // Sloped: each segment one up.
        let mut s = Blocks::default();
        s.belts
            .insert([0, 0, 0], st(Dir::South, Slope::Upward, Part::Start));
        s.belts
            .insert([0, 1, 1], st(Dir::South, Slope::Upward, Part::Middle));
        s.belts
            .insert([0, 2, 2], st(Dir::South, Slope::Upward, Part::End));
        assert!(
            matches!(find_chain(&s, [0, 2, 2]), Chain::Belt { controller: [0, 0, 0], ref segments } if segments.len() == 3)
        );
        // A lone start is broken.
        let mut lone = Blocks::default();
        lone.belts
            .insert([0, 0, 0], st(Dir::South, Slope::Horizontal, Part::Start));
        assert_eq!(find_chain(&lone, [0, 0, 0]), Chain::Broken([0, 0, 0]));
        // Consecutive segments share a chain, parallel ones don't.
        let a = st(Dir::East, Slope::Horizontal, Part::Start);
        let m = st(Dir::East, Slope::Horizontal, Part::Middle);
        assert!(a.same_chain([0, 0, 0], m, [1, 0, 0]));
        assert!(!a.same_chain([0, 0, 0], a, [0, 0, 1]));
    }

    #[test]
    fn offsets_and_movement() {
        let s = st(Dir::South, Slope::Upward, Part::Start);
        assert_eq!(position_for_offset([0, 0, 0], s, 3, 2), [0, 2, 2]);
        assert_eq!(position_for_offset([0, 0, 0], s, 3, 3), [0, 2, 3]);
        assert_eq!(position_for_offset([0, 0, 0], s, 3, -1), [0, 0, -1]);
        let v = vector_for_offset([0, 0, 0], s, 3, 1.5);
        assert_eq!(v, [0.5, 1.5, 1.5]);
        // Positive speed along south moves positive; along east, negative.
        let south = st(Dir::South, Slope::Horizontal, Part::Start);
        assert!(south.direction_aware_speed(32.0) > 0.0);
        assert_eq!(south.movement_facing(32.0), Dir::South);
        let east = st(Dir::East, Slope::Horizontal, Part::Start);
        assert!(east.direction_aware_speed(32.0) < 0.0);
        assert_eq!(east.movement_facing(32.0), Dir::West);
        assert_eq!(east.movement_facing(-32.0), Dir::East);
    }

    fn belt(facing: Dir, n: i32, speed: f32) -> Belt {
        Belt {
            controller: [0, 0, 0],
            state: st(facing, Slope::Horizontal, Part::Start),
            length: n,
            speed,
        }
    }

    #[test]
    fn items_move_and_eject_off_the_end() {
        let mut b = Blocks::default();
        flat(&mut b, Dir::South, 3);
        let belt = belt(Dir::South, 3, 48.0);
        let mut inv: BeltInventory<Item> = BeltInventory::new();
        assert_eq!(inv.insert_at(&belt, 0, Item("stone", 3), 180, false), None);
        // 48 rpm: a tenth of a block a tick.
        for _ in 0..10 {
            inv.tick(&belt, &mut b);
        }
        assert_eq!(inv.items.len(), 1);
        let p = inv.items[0].belt_position;
        // Put in before it first moved: 1/16 past the middle.
        assert!((p - (0.5 + 1.0 / 16.0 + 1.0)).abs() < 1e-3, "{p}");
        for _ in 0..30 {
            inv.tick(&belt, &mut b);
        }
        assert!(inv.items.is_empty());
        assert_eq!(b.ejected.len(), 1);
        let (item, at, motion) = &b.ejected[0];
        assert_eq!(item, &Item("stone", 3));
        assert!(at[2] > 2.9 && motion[2] > 0.0);
    }

    #[test]
    fn items_keep_their_spacing_and_stop_at_a_wall() {
        let mut b = Blocks::default();
        flat(&mut b, Dir::South, 4);
        b.solid.push([0, 0, 4]);
        let belt = belt(Dir::South, 4, 96.0);
        let mut inv: BeltInventory<Item> = BeltInventory::new();
        inv.insert_at(&belt, 0, Item("a", 1), 180, false);
        for _ in 0..3 {
            inv.tick(&belt, &mut b);
        }
        // Still too close to the first.
        assert!(inv.insert_at(&belt, 0, Item("b", 1), 180, false).is_some());
        for _ in 0..3 {
            inv.tick(&belt, &mut b);
        }
        assert_eq!(inv.insert_at(&belt, 0, Item("b", 1), 180, false), None);
        for _ in 0..200 {
            inv.tick(&belt, &mut b);
        }
        assert!(b.ejected.is_empty());
        let front = inv.items[0].belt_position;
        let back = inv.items[1].belt_position;
        assert!((front - (4.0 - 0.45)).abs() < 1e-3, "{front}");
        assert!((front - back - 1.0).abs() < 1e-3, "{back}");
    }

    #[test]
    fn hands_over_to_what_takes_it() {
        let mut b = Blocks::default();
        flat(&mut b, Dir::South, 2);
        b.inputs.push([0, 0, 2]);
        let belt = belt(Dir::South, 2, 96.0);
        let mut inv: BeltInventory<Item> = BeltInventory::new();
        inv.insert_at(&belt, 1, Item("x", 2), 180, false);
        for _ in 0..20 {
            inv.tick(&belt, &mut b);
        }
        assert!(inv.items.is_empty());
        assert_eq!(b.inserted, vec![([0, 0, 2], Item("x", 2), Dir::South)]);
    }

    #[test]
    fn the_press_holds_and_converts() {
        // Both directions: the rest of the stack waits behind the locked
        // sheet, then comes back under the press for the next cycle.
        for (facing, speed) in [(Dir::South, 16.0), (Dir::South, -16.0)] {
            let mut b = Blocks::default();
            flat(&mut b, facing, 3);
            b.presses.push([0, 0, 1]);
            let belt = belt(facing, 3, speed);
            let mut inv: BeltInventory<Item> = BeltInventory::new();
            let start = if speed > 0.0 { 0 } else { 2 };
            inv.insert_at(&belt, start, Item("iron_ingot", 3), 180, false);
            let mut seen_locked = false;
            for _ in 0..3000 {
                inv.tick(&belt, &mut b);
                b.tick_press();
                seen_locked |= inv.items.iter().any(|t| t.locked);
            }
            assert!(seen_locked);
            // All three ingots pressed; the sheets went on and off the end.
            assert!(inv.items.is_empty(), "{:?}", inv.items);
            let count = |id: &str| {
                b.ejected
                    .iter()
                    .filter(|e| e.0.0 == id)
                    .map(|e| e.0.1)
                    .sum::<u32>()
            };
            assert_eq!(
                (count("iron_sheet"), count("iron_ingot")),
                (3, 0),
                "speed {speed}"
            );
        }
    }

    #[test]
    fn a_belt_funnel_takes_items() {
        let mut b = Blocks::default();
        flat(&mut b, Dir::South, 3);
        b.funnels.insert(
            [0, 1, 1],
            BeltFunnel {
                facing: Dir::North,
                shape: FunnelShape::Retracted,
                powered: false,
            },
        );
        let belt = belt(Dir::South, 3, 64.0);
        let mut inv: BeltInventory<Item> = BeltInventory::new();
        inv.insert_at(&belt, 0, Item("y", 5), 180, false);
        for _ in 0..40 {
            inv.tick(&belt, &mut b);
        }
        assert!(inv.items.is_empty());
        assert_eq!(b.funnelled, vec![Item("y", 5)]);
        assert!(b.ejected.is_empty());
    }

    #[test]
    fn segments_refuse_while_an_item_is_on_them() {
        let mut b = Blocks::default();
        flat(&mut b, Dir::South, 3);
        let belt = belt(Dir::South, 3, 32.0);
        let mut inv: BeltInventory<Item> = BeltInventory::new();
        assert_eq!(inv.insert_at(&belt, 1, Item("a", 1), 180, false), None);
        assert_eq!(
            inv.insert_at(&belt, 1, Item("b", 1), 180, false),
            Some(Item("b", 1))
        );
        inv.tick(&belt, &mut b);
        assert_eq!(inv.extract_at(1, 64, false), Some(Item("a", 1)));
        inv.tick(&belt, &mut b);
        assert!(inv.items.is_empty());
    }

    #[test]
    fn connector_rules() {
        let mut b = Blocks::default();
        b.shafts.insert([0, 0, 0], Axis::X);
        b.shafts.insert([0, 0, 4], Axis::X);
        assert!(can_connect(&b, [0, 0, 0], [0, 0, 4]));
        // Along the shafts' own axis: no.
        b.shafts.insert([4, 0, 0], Axis::X);
        assert!(!can_connect(&b, [0, 0, 0], [4, 0, 0]));
        // Something solid between.
        b.solid.push([0, 0, 2]);
        assert!(!can_connect(&b, [0, 0, 0], [0, 0, 4]));
        b.solid.clear();
        // A diagonal: one up per block.
        b.shafts.insert([0, 3, 3], Axis::X);
        assert!(can_connect(&b, [0, 0, 0], [0, 3, 3]));
        assert!(!can_connect(&b, [0, 0, 0], [0, 2, 3]));
        b.shafts.insert([0, 0, 2], Axis::X);
        let belts = belts_between(&b, [0, 0, 0], [0, 0, 4]);
        let parts: Vec<Part> = belts.iter().map(|(_, s)| s.part).collect();
        assert_eq!(
            parts,
            vec![
                Part::Start,
                Part::Middle,
                Part::Pulley,
                Part::Middle,
                Part::End
            ]
        );
        assert!(
            belts
                .iter()
                .all(|(_, s)| s.facing == Dir::South && s.slope == Slope::Horizontal)
        );
        let up = belts_between(&b, [0, 0, 0], [0, 3, 3]);
        assert_eq!(up.len(), 4);
        assert_eq!(up[1].0, [0, 1, 1]);
        assert!(up.iter().all(|(_, s)| s.slope == Slope::Upward));
        // Downwards the other way round.
        assert_eq!(facing_from_to([0, 3, 3], [0, 0, 0]), Dir::North);
        assert_eq!(slope_between([0, 3, 3], [0, 0, 0]), Slope::Downward);
    }

    #[test]
    fn renderer_partials() {
        let (parts, angle) = belt_partials(st(Dir::South, Slope::Horizontal, Part::Start));
        assert_eq!(
            parts,
            vec![
                ("belt_start_bottom", true, false),
                ("belt_start", false, false)
            ]
        );
        assert_eq!(angle, 0.0);
        let (parts, angle) = belt_partials(st(Dir::North, Slope::Downward, Part::Start));
        assert_eq!(parts, vec![("belt_diagonal_end", false, true)]);
        assert_eq!(angle, 180.0);
        let s = st(Dir::South, Slope::Horizontal, Part::Middle);
        assert_eq!(belt_scroll(s, 0.0, 10.0, true), 0.25);
        let (p, tilt) = item_place([0, 0, 0], s, 3, 1.0, 0.0);
        assert_eq!(p, [0.5, 15.0 / 16.0, 1.0]);
        assert_eq!(tilt, 0.0);
    }
}
