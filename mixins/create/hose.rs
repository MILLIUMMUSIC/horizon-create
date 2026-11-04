// Create's hose pulley: `FluidManipulationBehaviour`, `FluidDrainingBehaviour`,
// `FluidFillingBehaviour` and `HosePulleyFluidHandler`. No Horizon types.

use super::create_fluids::{FluidHandler, FluidStack, FluidTank};
use super::create_kinetics::{Dir, Pos};
use std::collections::HashSet;

/// `SEARCHED_PER_TICK`.
const SEARCHED_PER_TICK: usize = 1024;
const VALIDATION_TIMER_MIN: i32 = 160;
/// `CFluids.hosePulleyRange`.
const MAX_RANGE: i32 = 128;
/// `CFluids.hosePulleyBlockThreshold`.
pub const MAX_BLOCKS: usize = 10000;
/// `HosePulleyBlockEntity.internalTank`'s capacity.
pub const TANK: i32 = 1500;

/// A block as these behaviours see it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cell {
    /// Its `WATERLOGGED`, if it has one.
    pub waterlogged: Option<bool>,
    /// A `LiquidBlock`: its `LEVEL`.
    pub liquid_level: Option<i32>,
    /// `FluidState.getType`, made still.
    pub fluid: Option<String>,
    /// Its fluid state flows (not a source).
    pub flowing: bool,
    /// Its collision shape is empty.
    pub no_collision: bool,
    /// `canBeReplacedByFluid`.
    pub replaceable_by_fluid: bool,
}

/// What these behaviours ask of the world.
pub trait HoseWorld {
    fn loaded(&self, pos: Pos) -> bool;
    fn cell(&self, pos: Pos) -> Cell;
    /// `setBlock` to air.
    fn set_air(&mut self, pos: Pos);
    /// `setBlock` with `WATERLOGGED`, as `updatePostWaterlogging` has it.
    fn set_waterlogged(&mut self, pos: Pos, on: bool);
    /// `replaceBlock` and the fluid's source.
    fn place_source(&mut self, pos: Pos, fluid: &str);
    /// `playEffect`.
    fn effect(&mut self, pos: Pos, fluid: &str, fill_sound: bool);
    /// `dimensionType.ultraWarm`.
    fn ultra_warm(&self) -> bool;
}

/// `BottomlessFluidMode.ALLOW_BY_TAG`: `create:bottomless/allow`.
pub fn bottomless(fluid: &str) -> bool {
    fluid == "minecraft:water" || fluid == "minecraft:lava"
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    pos: Pos,
    distance: i32,
}

fn dist_sq(a: Pos, b: Pos) -> i64 {
    (0..3).map(|k| i64::from(a[k] - b[k]).pow(2)).sum()
}

/// `comparePositions`: true when `a` goes before `b`: highest, furthest first.
fn before(a: &Entry, b: &Entry, root: Pos) -> bool {
    if a.pos[1] != b.pos[1] {
        return a.pos[1] > b.pos[1];
    }
    if a.distance != b.distance {
        return a.distance > b.distance;
    }
    dist_sq(a.pos, root) > dist_sq(b.pos, root)
}

/// The first of `queue` by `before`, or by its opposite (`descending`).
fn first_index(queue: &[Entry], root: Pos, opposite: bool) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, e) in queue.iter().enumerate() {
        match best {
            Some(b)
                if !(if opposite {
                    before(&queue[b], e, root)
                } else {
                    before(e, &queue[b], root)
                }) => {}
            _ => best = Some(i),
        }
    }
    best
}

/// `search`: on from the frontier; false when a chunk isn't loaded.
#[allow(clippy::too_many_arguments)]
fn search(
    world: &dyn HoseWorld,
    root: Pos,
    fluid: &mut Option<String>,
    frontier: &mut Vec<Entry>,
    visited: &mut HashSet<Pos>,
    add: &mut dyn FnMut(Pos, i32),
    downward: bool,
) -> bool {
    let range = i64::from(MAX_RANGE) * i64::from(MAX_RANGE);
    let mut i = 0;
    while i < SEARCHED_PER_TICK
        && !frontier.is_empty()
        && (visited.len() <= MAX_BLOCKS || !fluid.as_deref().is_some_and(bottomless))
    {
        i += 1;
        let entry = frontier.remove(0);
        if !visited.insert(entry.pos) {
            continue;
        }
        if !world.loaded(entry.pos) {
            return false;
        }
        let Some(current) = world.cell(entry.pos).fluid else {
            continue;
        };
        if fluid.is_none() {
            *fluid = Some(current.clone());
        }
        if fluid.as_deref() != Some(current.as_str()) {
            continue;
        }
        add(entry.pos, entry.distance);
        for side in Dir::ALL {
            if !downward && side == Dir::Down {
                continue;
            }
            let n = side.normal();
            let next = [
                entry.pos[0] + n[0],
                entry.pos[1] + n[1],
                entry.pos[2] + n[2],
            ];
            if !world.loaded(next) {
                return false;
            }
            if visited.contains(&next) || dist_sq(next, root) > range {
                continue;
            }
            let cell = world.cell(next);
            if cell.fluid.is_none() {
                continue;
            }
            if cell.flowing && side == Dir::Up && !(next[0] == root[0] && next[2] == root[2]) {
                continue;
            }
            frontier.push(Entry {
                pos: next,
                distance: entry.distance + 1,
            });
        }
    }
    true
}

/// `canPullFluidsFrom`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pull {
    None,
    Source,
    Flowing,
}

fn can_pull(c: &Cell) -> Pull {
    if c.waterlogged == Some(true) {
        return Pull::Source;
    }
    if let Some(level) = c.liquid_level {
        return if level == 0 {
            Pull::Source
        } else {
            Pull::Flowing
        };
    }
    if c.fluid.is_some() && c.no_collision {
        Pull::Source
    } else {
        Pull::None
    }
}

/// `FluidDrainingBehaviour`.
#[derive(Clone, Debug, Default)]
pub struct Drainer {
    root: Option<Pos>,
    pub infinite: bool,
    counterpart_acted: bool,
    fluid: Option<String>,
    frontier: Vec<Entry>,
    visited: HashSet<Pos>,
    revalidate_in: i32,
    validation_set: HashSet<Pos>,
    queue: Vec<Entry>,
    is_valid: bool,
    validation_frontier: Vec<Entry>,
    validation_visited: HashSet<Pos>,
    new_validation_set: HashSet<Pos>,
}

impl Drainer {
    pub fn counterpart_acted(&mut self) {
        self.counterpart_acted = true;
    }
    /// `reset`.
    pub fn reset(&mut self) {
        self.revalidate_in = VALIDATION_TIMER_MIN;
        self.frontier.clear();
        self.visited.clear();
        self.infinite = false;
        self.fluid = None;
        self.root = None;
        self.queue.clear();
        self.validation_set.clear();
        self.new_validation_set.clear();
        self.validation_frontier.clear();
        self.validation_visited.clear();
    }
    fn soft_reset(&mut self, root: Pos) {
        self.queue.clear();
        self.validation_set.clear();
        self.new_validation_set.clear();
        self.validation_frontier.clear();
        self.validation_visited.clear();
        self.visited.clear();
        self.infinite = false;
        self.revalidate_in = VALIDATION_TIMER_MIN;
        self.frontier.push(Entry {
            pos: root,
            distance: 0,
        });
    }
    fn rebuild(&mut self, root: Pos) {
        self.reset();
        self.root = Some(root);
        if self.is_valid {
            self.frontier.push(Entry {
                pos: root,
                distance: 0,
            });
        }
    }
    /// `checkValid`: `own` is the block entity's position.
    fn check_valid(world: &dyn HoseWorld, root: Pos, own: Pos) -> bool {
        let mut current = root;
        let mut timeout = 1000;
        while timeout > 0 && root != own {
            timeout -= 1;
            match can_pull(&world.cell(current)) {
                Pull::Flowing => {
                    for d in Dir::ALL {
                        let n = d.normal();
                        if can_pull(&world.cell([
                            current[0] + n[0],
                            current[1] + n[1],
                            current[2] + n[2],
                        ])) == Pull::Source
                        {
                            return true;
                        }
                    }
                    current[1] += 1;
                }
                Pull::Source => return true,
                Pull::None => break,
            }
        }
        false
    }
    /// `tick`.
    pub fn tick(&mut self, world: &dyn HoseWorld, own: Pos) {
        if let Some(root) = self.root {
            self.is_valid = Self::check_valid(world, root, own);
        }
        if !self.frontier.is_empty() {
            self.continue_search(world);
            return;
        }
        if !self.validation_frontier.is_empty() {
            self.continue_validation(world);
            return;
        }
        if self.revalidate_in > 0 {
            self.revalidate_in -= 1;
        }
    }
    fn continue_search(&mut self, world: &dyn HoseWorld) {
        let Some(root) = self.root else {
            self.frontier.clear();
            return;
        };
        let mut found: Vec<Entry> = Vec::new();
        let ok = search(
            world,
            root,
            &mut self.fluid,
            &mut self.frontier,
            &mut self.visited,
            &mut |pos, distance| found.push(Entry { pos, distance }),
            false,
        );
        for e in found {
            self.queue.push(e);
            self.validation_set.insert(e.pos);
        }
        if !ok {
            self.frontier.clear();
            self.visited.clear();
        }
        if self.visited.len() >= MAX_BLOCKS
            && self.fluid.as_deref().is_some_and(bottomless)
            && !self.queue.is_empty()
        {
            self.infinite = true;
            let first = first_index(&self.queue, root, false).map(|i| self.queue[i].pos);
            self.frontier.clear();
            self.visited.clear();
            self.queue.clear();
            if let Some(pos) = first {
                self.queue.push(Entry { pos, distance: 0 });
            }
            return;
        }
        if self.frontier.is_empty() {
            self.visited.clear();
        }
    }
    fn continue_validation(&mut self, world: &dyn HoseWorld) {
        let Some(root) = self.root else {
            self.validation_frontier.clear();
            return;
        };
        let mut found = Vec::new();
        let ok = search(
            world,
            root,
            &mut self.fluid,
            &mut self.validation_frontier,
            &mut self.validation_visited,
            &mut |pos, _| found.push(pos),
            false,
        );
        self.new_validation_set.extend(found);
        if !ok {
            self.validation_frontier.clear();
            self.validation_visited.clear();
            self.revalidate_in = VALIDATION_TIMER_MIN * 2;
            return;
        }
        if self.validation_visited.len() >= MAX_BLOCKS
            && self.fluid.as_deref().is_some_and(bottomless)
        {
            if !self.infinite {
                self.reset();
            }
            self.validation_frontier.clear();
            self.revalidate_in = VALIDATION_TIMER_MIN * 2;
            return;
        }
        if !self.validation_frontier.is_empty() {
            return;
        }
        if self.infinite {
            self.reset();
            return;
        }
        self.validation_set = std::mem::take(&mut self.new_validation_set);
        self.validation_visited.clear();
    }
    /// `getDrainableFluid`.
    pub fn drainable(&mut self, world: &mut dyn HoseWorld, root: Pos, own: Pos) -> Option<String> {
        if self.fluid.is_none()
            || !self.frontier.is_empty()
            || !self.pull_next(world, root, own, true)
        {
            return None;
        }
        self.fluid.clone()
    }
    /// `pullNext`.
    pub fn pull_next(
        &mut self,
        world: &mut dyn HoseWorld,
        root: Pos,
        own: Pos,
        simulate: bool,
    ) -> bool {
        if !self.frontier.is_empty() {
            return false;
        }
        if self.root != Some(root) {
            self.rebuild(root);
            return false;
        }
        if self.counterpart_acted {
            self.counterpart_acted = false;
            self.soft_reset(root);
            return false;
        }
        if !self.queue.is_empty() && !self.is_valid {
            self.rebuild(root);
            return false;
        }
        if self.validation_frontier.is_empty()
            && !self.queue.is_empty()
            && !simulate
            && self.revalidate_in == 0
        {
            self.validation_frontier.clear();
            self.validation_visited.clear();
            self.new_validation_set.clear();
            self.validation_frontier.push(Entry {
                pos: root,
                distance: 0,
            });
            self.revalidate_in = VALIDATION_TIMER_MIN;
        }
        if self.infinite {
            if let Some(f) = self.fluid.clone() {
                world.effect(root, &f, true);
            }
            return true;
        }
        while let Some(i) = first_index(&self.queue, root, false) {
            let current = self.queue[i].pos;
            let cell = world.cell(current);
            let mut emptied_air = false;
            let mut unlog = false;
            let mut fluid: Option<String> = None;
            if cell.waterlogged == Some(true) {
                unlog = true;
                fluid = Some("minecraft:water".to_owned());
            } else if let Some(level) = cell.liquid_level {
                emptied_air = true;
                if level == 0 {
                    fluid = cell.fluid.clone();
                } else {
                    world.set_air(current);
                    self.queue.remove(i);
                    if self.queue.is_empty() {
                        self.is_valid = Self::check_valid(world, root, own);
                        self.reset();
                    }
                    continue;
                }
            } else if cell.fluid.is_some() && cell.no_collision {
                fluid = cell.fluid.clone();
                emptied_air = true;
            }
            if self.fluid.is_none() {
                self.fluid = fluid.clone();
            }
            if self.fluid != fluid || fluid.is_none() {
                self.queue.remove(i);
                if self.queue.is_empty() {
                    self.is_valid = Self::check_valid(world, root, own);
                    self.reset();
                }
                continue;
            }
            if simulate {
                return true;
            }
            let name = fluid.unwrap_or_default();
            world.effect(current, &name, true);
            if unlog {
                world.set_waterlogged(current, false);
            } else if emptied_air {
                world.set_air(current);
            }
            self.queue.remove(i);
            if self.queue.is_empty() {
                self.is_valid = Self::check_valid(world, root, own);
                self.reset();
            } else if !self.validation_set.contains(&current) {
                self.reset();
            }
            return true;
        }
        if self.root.is_none() {
            return false;
        }
        if self.is_valid {
            self.rebuild(root);
        }
        false
    }
}

/// `FluidFillingBehaviour.SpaceType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Space {
    Fillable,
    Filled,
    Blocking,
}

fn space_at(c: &Cell, fluid: &str) -> Space {
    if let Some(on) = c.waterlogged {
        return if fluid != "minecraft:water" {
            Space::Blocking
        } else if on {
            Space::Filled
        } else {
            Space::Fillable
        };
    }
    if let Some(level) = c.liquid_level {
        return if level != 0 {
            Space::Fillable
        } else if c.fluid.as_deref() == Some(fluid) {
            Space::Filled
        } else {
            Space::Blocking
        };
    }
    if c.fluid.is_some() && c.no_collision {
        return if c.fluid.as_deref() == Some(fluid) {
            Space::Filled
        } else {
            Space::Blocking
        };
    }
    if c.replaceable_by_fluid {
        Space::Fillable
    } else {
        Space::Blocking
    }
}

/// `FluidFillingBehaviour`.
#[derive(Clone, Debug, Default)]
pub struct Filler {
    root: Option<Pos>,
    pub infinite: bool,
    counterpart_acted: bool,
    visited: HashSet<Pos>,
    revalidate_in: i32,
    queue: Vec<Entry>,
    infinity_frontier: Vec<Entry>,
    infinity_visited: HashSet<Pos>,
}

impl Filler {
    pub fn counterpart_acted(&mut self) {
        self.counterpart_acted = true;
    }
    /// `reset`.
    pub fn reset(&mut self) {
        self.revalidate_in = VALIDATION_TIMER_MIN;
        self.visited.clear();
        self.infinite = false;
        self.queue.clear();
        self.infinity_frontier.clear();
        self.infinity_visited.clear();
    }
    fn soft_reset(&mut self, root: Pos) {
        self.visited.clear();
        self.queue.clear();
        self.queue.push(Entry {
            pos: root,
            distance: 0,
        });
        self.infinite = false;
        self.revalidate_in = VALIDATION_TIMER_MIN;
    }
    /// `tick`.
    pub fn tick(&mut self, world: &dyn HoseWorld) {
        if !self.infinity_frontier.is_empty()
            && let Some(root) = self.root
            && let Some(fluid) = world.cell(root).fluid
        {
            self.continue_validation(world, root, fluid);
        }
        if self.revalidate_in > 0 {
            self.revalidate_in -= 1;
        }
    }
    fn continue_validation(&mut self, world: &dyn HoseWorld, root: Pos, fluid: String) {
        let mut fluid = Some(fluid);
        let mut found = Vec::new();
        let ok = search(
            world,
            root,
            &mut fluid,
            &mut self.infinity_frontier,
            &mut self.infinity_visited,
            &mut |pos, distance| found.push(Entry { pos, distance }),
            true,
        );
        self.infinity_frontier.extend(found);
        if !ok {
            self.infinity_frontier.clear();
            self.infinity_visited.clear();
            self.revalidate_in = VALIDATION_TIMER_MIN * 2;
            return;
        }
        if self.infinity_visited.len() >= MAX_BLOCKS {
            if !self.infinite {
                self.reset();
                self.infinite = true;
            }
            self.infinity_frontier.clear();
            self.revalidate_in = VALIDATION_TIMER_MIN * 2;
            return;
        }
        if !self.infinity_frontier.is_empty() {
            return;
        }
        if self.infinite {
            self.reset();
            return;
        }
        self.infinity_visited.clear();
    }
    /// `tryDeposit`.
    pub fn try_deposit(
        &mut self,
        world: &mut dyn HoseWorld,
        fluid: &str,
        root: Pos,
        simulate: bool,
    ) -> bool {
        if self.root != Some(root) {
            self.reset();
            self.root = Some(root);
            self.queue.push(Entry {
                pos: root,
                distance: 0,
            });
            return false;
        }
        if self.counterpart_acted {
            self.counterpart_acted = false;
            self.soft_reset(root);
            return false;
        }
        if self.revalidate_in == 0 {
            self.visited.clear();
            self.infinity_frontier.clear();
            self.infinity_visited.clear();
            self.infinity_frontier.push(Entry {
                pos: root,
                distance: 0,
            });
            self.revalidate_in = VALIDATION_TIMER_MIN;
            self.soft_reset(root);
        }
        let evaporate = world.ultra_warm() && fluid == "minecraft:water";
        if self.infinite || evaporate {
            if world.cell(root).fluid.as_deref() != Some(fluid) && !evaporate {
                return false;
            }
            if !simulate {
                world.effect(root, fluid, false);
            }
            return true;
        }
        let range = i64::from(MAX_RANGE) * i64::from(MAX_RANGE);
        let mut success = false;
        let mut i = 0;
        while !success && i < SEARCHED_PER_TICK {
            i += 1;
            let Some(first) = first_index(&self.queue, root, true) else {
                break;
            };
            let entry = self.queue[first];
            if self.visited.contains(&entry.pos) {
                self.queue.remove(first);
                continue;
            }
            if !simulate {
                self.visited.insert(entry.pos);
            }
            if self.visited.len() >= MAX_BLOCKS {
                self.infinite = true;
                self.visited.clear();
                self.queue.clear();
                return false;
            }
            let cell = world.cell(entry.pos);
            let space = space_at(&cell, fluid);
            if space == Space::Blocking {
                // The entry stays: visited by now it goes next round, and
                // simulating there is nothing more to find.
                if simulate {
                    return false;
                }
                continue;
            }
            if space == Space::Fillable {
                success = true;
                if !simulate {
                    world.effect(entry.pos, fluid, false);
                    if cell.waterlogged.is_some() && fluid == "minecraft:water" {
                        world.set_waterlogged(entry.pos, true);
                    } else {
                        world.place_source(entry.pos, fluid);
                    }
                }
            }
            if simulate && success {
                return true;
            }
            self.visited.insert(entry.pos);
            self.queue.remove(first);
            for side in Dir::ALL {
                if side == Dir::Up {
                    continue;
                }
                let n = side.normal();
                let next = [
                    entry.pos[0] + n[0],
                    entry.pos[1] + n[1],
                    entry.pos[2] + n[2],
                ];
                if self.visited.contains(&next) || dist_sq(next, root) > range {
                    continue;
                }
                if space_at(&world.cell(next), fluid) != Space::Blocking {
                    self.queue.push(Entry {
                        pos: next,
                        distance: entry.distance + 1,
                    });
                }
            }
        }
        success
    }
}

/// `HosePulleyBlockEntity.tick`'s step: the offset a tick on, and whether it
/// still moves; `can_go(y_below)` says the block that far below can be replaced.
pub fn offset_tick(offset: f32, speed: f32, can_go: impl Fn(i32) -> bool) -> (f32, bool) {
    let mut moving = true;
    let mut next = offset + speed / 512.0;
    if next < 0.0 {
        next = 0.0;
        moving = false;
    }
    if !can_go(next.ceil() as i32) {
        next = next.trunc();
        moving = false;
    }
    if speed == 0.0 {
        moving = false;
    }
    (next, moving)
}

/// `HosePulleyFluidHandler`: what the block entity and its behaviours make of
/// a pipe's fill and drain.
pub struct Handler<'a> {
    pub tank: &'a mut FluidTank,
    pub filler: &'a mut Filler,
    pub drainer: &'a mut Drainer,
    pub world: &'a mut dyn HoseWorld,
    pub root: Pos,
    pub own: Pos,
    /// `() -> !isMoving`.
    pub still: bool,
}

impl FluidHandler for Handler<'_> {
    fn fill(&mut self, resource: &FluidStack, simulate: bool) -> i32 {
        if !self.tank.fluid.is_empty() && !self.tank.fluid.same(resource) {
            return 0;
        }
        if resource.is_empty() || !bottomless(&resource.fluid) {
            return 0;
        }
        let mut diff = resource.amount;
        let after = diff + self.tank.fluid.amount;
        let mut remaining = resource.clone();
        let mut deposited = false;
        if self.still
            && after >= 1000
            && self
                .filler
                .try_deposit(self.world, &resource.fluid, self.root, simulate)
        {
            self.drainer.counterpart_acted();
            remaining.amount -= 1000;
            diff -= 1000;
            deposited = true;
        }
        if simulate {
            return if diff <= 0 {
                resource.amount
            } else {
                self.tank.fill(&remaining, true)
            };
        }
        if diff <= 0 {
            self.tank.drain_any(-diff, false);
            return resource.amount;
        }
        self.tank.fill(&remaining, false) + if deposited { 1000 } else { 0 }
    }
    fn drain(&mut self, resource: &FluidStack, simulate: bool) -> FluidStack {
        drain_internal(self, resource.amount, Some(resource), simulate)
    }
    fn drain_any(&mut self, max: i32, simulate: bool) -> FluidStack {
        drain_internal(self, max, None, simulate)
    }
}

/// `drainInternal`.
fn drain_internal(
    h: &mut Handler<'_>,
    max: i32,
    resource: Option<&FluidStack>,
    simulate: bool,
) -> FluidStack {
    if let Some(r) = resource
        && !h.tank.fluid.is_empty()
        && !h.tank.fluid.same(r)
    {
        return FluidStack::empty();
    }
    if h.tank.fluid.amount >= 1000 {
        return h.tank.drain_any(max, simulate);
    }
    let returned = h.drainer.drainable(h.world, h.root, h.own);
    if !h.still || !h.drainer.pull_next(h.world, h.root, h.own, simulate) {
        return h.tank.drain_any(max, simulate);
    }
    h.filler.counterpart_acted();
    let available = 1000 + h.tank.fluid.amount;
    let Some(name) = returned else {
        return h.tank.drain_any(max, simulate);
    };
    if !h.tank.fluid.is_empty() && h.tank.fluid.fluid != name {
        return h.tank.drain_any(max, simulate);
    }
    if resource.is_some_and(|r| r.fluid != name) {
        return FluidStack::empty();
    }
    let drained = max.min(available);
    if !simulate {
        h.tank.fluid = if available - drained > 0 {
            FluidStack::new(&name, available - drained)
        } else {
            FluidStack::empty()
        };
    }
    FluidStack::new(&name, drained)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct W {
        cells: HashMap<Pos, Cell>,
        hot: bool,
    }

    fn water() -> Cell {
        Cell {
            liquid_level: Some(0),
            fluid: Some("minecraft:water".into()),
            no_collision: true,
            replaceable_by_fluid: true,
            ..Cell::default()
        }
    }

    fn air() -> Cell {
        Cell {
            no_collision: true,
            replaceable_by_fluid: true,
            ..Cell::default()
        }
    }

    impl HoseWorld for W {
        fn loaded(&self, _: Pos) -> bool {
            true
        }
        fn cell(&self, pos: Pos) -> Cell {
            self.cells.get(&pos).cloned().unwrap_or_default()
        }
        fn set_air(&mut self, pos: Pos) {
            self.cells.insert(pos, air());
        }
        fn set_waterlogged(&mut self, pos: Pos, on: bool) {
            self.cells.entry(pos).or_default().waterlogged = Some(on);
        }
        fn place_source(&mut self, pos: Pos, fluid: &str) {
            let mut c = water();
            c.fluid = Some(fluid.to_owned());
            self.cells.insert(pos, c);
        }
        fn effect(&mut self, _: Pos, _: &str, _: bool) {}
        fn ultra_warm(&self) -> bool {
            self.hot
        }
    }

    const OWN: Pos = [0, 10, 0];

    #[test]
    fn it_drains_from_the_top_down_and_furthest_first() {
        let mut w = W::default();
        for (p, ()) in [([0, 5, 0], ()), ([1, 5, 0], ()), ([0, 4, 0], ())] {
            w.cells.insert(p, water());
        }
        let mut d = Drainer::default();
        let root = [0, 5, 0];
        d.tick(&w, OWN);
        assert!(!d.pull_next(&mut w, root, OWN, false));
        d.tick(&w, OWN);
        // The root's context is built with it valid only from the next tick.
        assert!(!d.pull_next(&mut w, root, OWN, false));
        for _ in 0..4 {
            d.tick(&w, OWN);
        }
        // The search doesn't go down from where it starts.
        assert!(d.pull_next(&mut w, root, OWN, false));
        assert_eq!(w.cell([1, 5, 0]), air());
        assert_eq!(w.cell([0, 5, 0]).fluid.as_deref(), Some("minecraft:water"));
        assert!(d.pull_next(&mut w, root, OWN, false));
        assert_eq!(w.cell([0, 5, 0]), air());
    }

    #[test]
    fn it_fills_from_the_bottom_up() {
        let mut w = W::default();
        let root = [0, 5, 0];
        w.cells.insert(root, air());
        w.cells.insert([0, 4, 0], air());
        let mut f = Filler::default();
        assert!(!f.try_deposit(&mut w, "minecraft:water", root, false));
        f.revalidate_in = 100;
        assert!(f.try_deposit(&mut w, "minecraft:water", root, false));
        assert_eq!(w.cell(root).fluid.as_deref(), Some("minecraft:water"));
        assert!(f.try_deposit(&mut w, "minecraft:water", root, false));
        assert_eq!(w.cell([0, 4, 0]).fluid.as_deref(), Some("minecraft:water"));
        // Nowhere left.
        assert!(!f.try_deposit(&mut w, "minecraft:water", root, false));
    }

    #[test]
    fn water_goes_up_in_steam_in_the_nether() {
        let mut w = W {
            hot: true,
            ..W::default()
        };
        let root = [0, 5, 0];
        w.cells.insert(root, air());
        let mut f = Filler::default();
        f.try_deposit(&mut w, "minecraft:water", root, false);
        f.revalidate_in = 100;
        assert!(f.try_deposit(&mut w, "minecraft:water", root, false));
        assert_eq!(w.cell(root), air());
    }

    #[test]
    fn the_hose_lets_itself_down_to_what_stops_it() {
        assert_eq!(offset_tick(0.0, 256.0, |_| true), (0.5, true));
        assert_eq!(offset_tick(0.9, 256.0, |d| d < 2), (1.0, false));
        assert_eq!(offset_tick(0.25, -256.0, |_| true), (0.0, false));
        assert_eq!(offset_tick(1.0, 0.0, |_| true), (1.0, false));
    }

    #[test]
    fn the_handler_takes_a_bucket_at_a_time_to_the_world() {
        let mut w = W::default();
        let root = [0, 5, 0];
        w.cells.insert(root, air());
        let mut tank = FluidTank::new(TANK);
        let (mut filler, mut drainer) = (Filler::default(), Drainer::default());
        filler.try_deposit(&mut w, "minecraft:water", root, false);
        filler.revalidate_in = 100;
        let mut h = Handler {
            tank: &mut tank,
            filler: &mut filler,
            drainer: &mut drainer,
            world: &mut w,
            root,
            own: OWN,
            still: true,
        };
        assert_eq!(h.fill(&FluidStack::new("minecraft:water", 400), false), 400);
        assert_eq!(h.tank.fluid.amount, 400);
        assert_eq!(h.fill(&FluidStack::new("minecraft:milk", 10), false), 0);
        assert_eq!(h.fill(&FluidStack::new("minecraft:water", 700), false), 700);
        assert_eq!(h.tank.fluid.amount, 100);
        assert_eq!(w.cell(root).fluid.as_deref(), Some("minecraft:water"));
    }
}
