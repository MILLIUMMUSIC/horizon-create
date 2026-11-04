// Create's fluid tank as a multiblock, ported from Create 6.0.10 (MIT, The
// Create Team): FluidTankBlockEntity's multiblock fields and windows,
// CreativeFluidTankBlockEntity's tank, and ConnectivityHandler's formMulti
// and splitMulti with their fluid handling (vault.rs has the same search
// without fluids). No Horizon types: every tank block's part lives in
// `Tanks`, which the glue keeps beside the world (the tank's template has no
// block entity) and tells of tanks placed and removed.

use super::create_fluids::{FluidHandler, FluidStack, FluidTank, TANK_CAPACITY};
use super::create_kinetics::Pos;
use std::collections::{HashMap, HashSet};

/// `FluidTankBlockEntity.MAX_SIZE`: the widest base.
pub const MAX_SIZE: i32 = 3;
/// `CFluids.fluidTankMaxHeight`'s default.
pub const MAX_HEIGHT: i32 = 32;

/// `FluidTankBlock.Shape`: where a block of a tank shows a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Plain,
    Window,
    WindowNw,
    WindowSw,
    WindowNe,
    WindowSe,
}

impl Shape {
    /// `getSerializedName`.
    pub fn name(self) -> &'static str {
        match self {
            Shape::Plain => "plain",
            Shape::Window => "window",
            Shape::WindowNw => "window_nw",
            Shape::WindowSw => "window_sw",
            Shape::WindowNe => "window_ne",
            Shape::WindowSe => "window_se",
        }
    }
}

/// `FluidTankBlock`'s `TOP`, `BOTTOM` and `SHAPE`, as a block of a tank
/// looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub top: bool,
    pub bottom: bool,
    pub shape: Shape,
}

impl Look {
    /// `FluidTankGenerator.getModel`: the model's name under
    /// `block/fluid_tank/block_` (`single`, `top`, `middle`, `bottom`, then
    /// the window's shape unless plain).
    pub fn model(self) -> String {
        let part = match (self.top, self.bottom) {
            (true, true) => "single",
            (true, false) => "top",
            (false, true) => "bottom",
            (false, false) => "middle",
        };
        match self.shape {
            Shape::Plain => part.to_owned(),
            s => format!("{part}_{}", s.name()),
        }
    }
}

/// `setWindows`: the shape of the block `x`, `z` across a tank `width`
/// wide from its controller.
pub fn window_shape(window: bool, width: i32, x: i32, z: i32) -> Shape {
    if !window {
        return Shape::Plain;
    }
    match width {
        // SIZE 1: every tank has a window.
        1 => Shape::Window,
        // SIZE 2: every tank has a corner window.
        2 => match (x == 0, z == 0) {
            (true, true) => Shape::WindowNw,
            (true, false) => Shape::WindowSw,
            (false, true) => Shape::WindowNe,
            (false, false) => Shape::WindowSe,
        },
        // SIZE 3: tanks in the centre of each side have a window.
        3 if (x.abs() - z.abs()).abs() == 1 => Shape::Window,
        _ => Shape::Plain,
    }
}

/// One tank block's `FluidTankBlockEntity` fields.
#[derive(Clone, Debug, PartialEq)]
pub struct TankPart {
    /// `controller`: none when this block is its own controller.
    pub controller: Option<Pos>,
    pub width: i32,
    pub height: i32,
    /// `window` (the controller's counts).
    pub window: bool,
    /// `tankInventory`: only a controller's holds anything.
    pub tank: FluidTank,
    /// A `CreativeFluidTankBlockEntity` (another block entity type: the two
    /// kinds never join).
    pub creative: bool,
    /// `updateConnectivity`: look for a multiblock on the next tick.
    pub update_connectivity: bool,
    /// `boiler`: a controller's.
    pub boiler: super::create_boiler::Boiler,
}

impl TankPart {
    pub fn new(creative: bool) -> Self {
        TankPart {
            controller: None,
            width: 1,
            height: 1,
            window: true,
            tank: FluidTank::new(TANK_CAPACITY),
            creative,
            update_connectivity: false,
            boiler: Default::default(),
        }
    }
    /// `getTotalTankSize`.
    pub fn total_size(&self) -> i32 {
        self.width * self.width * self.height
    }
    /// `isController`.
    pub fn is_controller(&self, pos: Pos) -> bool {
        self.controller.is_none_or(|c| c == pos)
    }
    /// `getController`.
    pub fn controller_of(&self, pos: Pos) -> Pos {
        if self.is_controller(pos) {
            pos
        } else {
            self.controller.unwrap_or(pos)
        }
    }
    /// `getFluidAmount` (`CreativeSmartFluidTank`'s: full whenever it
    /// holds a fluid).
    pub fn amount(&self) -> i32 {
        if self.creative && !self.tank.fluid.is_empty() {
            self.tank.capacity
        } else {
            self.tank.amount()
        }
    }
    /// `CreativeSmartFluidTank.setContainedFluid`.
    pub fn set_contained_fluid(&mut self, fluid: &FluidStack) {
        self.tank.fluid = if fluid.is_empty() {
            FluidStack::empty()
        } else {
            fluid.with_amount(self.tank.capacity)
        };
    }
    /// `applyFluidTankSize`: `blocks` blocks' worth, the overflow drained.
    fn apply_size(&mut self, blocks: i32) {
        self.tank.capacity = blocks * TANK_CAPACITY;
        let overflow = self.tank.amount() - self.tank.capacity;
        if overflow > 0 {
            self.tank.drain_any(overflow, false);
        }
        if self.creative && !self.tank.fluid.is_empty() {
            self.tank.fluid.amount = self.tank.capacity;
        }
    }
}

/// A controller's tank as its fluid handler (`handlerForCapability`): the
/// creative tank fills without keeping anything and drains without losing
/// anything.
pub struct TankHandler<'a> {
    pub tank: &'a mut FluidTank,
    pub creative: bool,
    /// `handlerForCapability`: a boiler's `BoilerFluidHandler` instead, when
    /// it is active.
    pub boiler: Option<&'a mut super::create_boiler::Boiler>,
}

impl FluidHandler for TankHandler<'_> {
    fn fill(&mut self, resource: &FluidStack, simulate: bool) -> i32 {
        if let Some(b) = self.boiler.as_mut() {
            return if resource.fluid == super::create_fluids::WATER && !resource.is_empty() {
                b.fill(resource.amount, simulate)
            } else {
                0
            };
        }
        if self.creative {
            return if resource.is_empty() {
                0
            } else {
                resource.amount
            };
        }
        self.tank.fill(resource, simulate)
    }
    fn drain(&mut self, resource: &FluidStack, simulate: bool) -> FluidStack {
        if self.boiler.is_some() {
            return FluidStack::empty();
        }
        self.tank.drain(resource, simulate || self.creative)
    }
    fn drain_any(&mut self, max: i32, simulate: bool) -> FluidStack {
        if self.boiler.is_some() {
            return FluidStack::empty();
        }
        self.tank.drain_any(max, simulate || self.creative)
    }
}

/// The tanks of a level.
#[derive(Clone, Debug, Default)]
pub struct Tanks {
    pub parts: HashMap<Pos, TankPart>,
    /// Blocks whose look or contents changed, for clients.
    pub changed: HashSet<Pos>,
}

/// `SearchCache`: each position's controller, as first found.
#[derive(Default)]
struct SearchCache {
    controllers: HashMap<Pos, Option<Pos>>,
}

impl SearchCache {
    fn put(&mut self, pos: Pos, controller: Pos) {
        self.controllers.insert(pos, Some(controller));
    }
}

fn at(origin: Pos, x: i32, y: i32, z: i32) -> Pos {
    [origin[0] + x, origin[1] + y, origin[2] + z]
}

impl Tanks {
    /// `getControllerBE`'s position for the block at `pos`.
    pub fn controller(&self, pos: Pos) -> Option<Pos> {
        let c = self.parts.get(&pos)?.controller_of(pos);
        self.parts.contains_key(&c).then_some(c)
    }

    /// The block at `pos`'s controller's part.
    pub fn controller_part(&self, pos: Pos) -> Option<&TankPart> {
        self.parts.get(&self.controller(pos)?)
    }

    pub fn controller_part_mut(&mut self, pos: Pos) -> Option<&mut TankPart> {
        let c = self.controller(pos)?;
        self.parts.get_mut(&c)
    }

    /// The fluid handler at `pos` (`handlerForCapability`: the
    /// controller's tank).
    pub fn handler(&mut self, pos: Pos) -> Option<TankHandler<'_>> {
        let part = self.controller_part_mut(pos)?;
        let creative = part.creative;
        if creative && !part.tank.fluid.is_empty() {
            part.tank.fluid.amount = part.tank.capacity;
        }
        let active = part.boiler.is_active();
        Some(TankHandler {
            tank: &mut part.tank,
            creative,
            boiler: active.then_some(&mut part.boiler),
        })
    }

    /// How the block at `pos` looks (`notifyMultiUpdated` and `setWindows`).
    #[allow(dead_code)]
    pub fn look(&self, pos: Pos) -> Option<Look> {
        let part = self.parts.get(&pos)?;
        let c = part.controller_of(pos);
        let (width, height, window) = match self.parts.get(&c) {
            Some(con) => (con.width, con.height, con.window),
            None => (part.width, part.height, part.window),
        };
        Some(Look {
            bottom: c[1] == pos[1],
            top: c[1] + height - 1 == pos[1],
            shape: window_shape(window, width, pos[0] - c[0], pos[2] - c[2]),
        })
    }

    fn mark(&mut self, controller: Pos) {
        let Some(part) = self.parts.get(&controller) else {
            return;
        };
        let (w, h) = (part.width, part.height);
        for y in 0..h {
            for x in 0..w {
                for z in 0..w {
                    self.changed.insert(at(controller, x, y, z));
                }
            }
        }
    }

    /// `FluidTankBlock.onPlace`: a new tank block, looking for a multiblock
    /// at once (`updateConnectivity`).
    pub fn place(&mut self, pos: Pos, creative: bool) {
        self.parts.insert(pos, TankPart::new(creative));
        self.changed.insert(pos);
        self.form_multi(pos);
    }

    /// `FluidTankBlock.onRemove`: the block entity goes, then its
    /// multiblock splits (`splitMulti`). The part removed.
    pub fn remove(&mut self, pos: Pos) -> Option<TankPart> {
        let removed = self.parts.remove(&pos)?;
        self.changed.insert(pos);
        self.split(pos, Some(&removed), None);
        Some(removed)
    }

    /// Every tank block's tick: those that lost their multiblock look for a
    /// new one (`updateConnectivity`, controllers only).
    pub fn tick(&mut self) {
        let mut due: Vec<Pos> = self
            .parts
            .iter()
            .filter(|(_, p)| p.update_connectivity)
            .map(|(p, _)| *p)
            .collect();
        due.sort();
        for pos in due {
            let Some(part) = self.parts.get_mut(&pos) else {
                continue;
            };
            if !part.update_connectivity {
                continue;
            }
            part.update_connectivity = false;
            if part.is_controller(pos) {
                self.form_multi(pos);
            }
        }
    }

    /// `toggleWindows` on the block at `pos` (the wrench).
    pub fn toggle_windows(&mut self, pos: Pos) {
        let Some(c) = self.controller(pos) else {
            return;
        };
        if let Some(part) = self.parts.get_mut(&c) {
            if part.boiler.is_active() {
                return;
            }
            part.window = !part.window;
        }
        self.mark(c);
    }

    /// `ConnectivityHandler.formMulti` from the controller at `pos`.
    pub fn form_multi(&mut self, pos: Pos) {
        let mut cache = SearchCache::default();
        self.form_multi_from(&mut cache, vec![pos]);
    }

    fn get_or_cache(&self, cache: &mut SearchCache, creative: bool, pos: Pos) -> Option<Pos> {
        if let Some(c) = cache.controllers.get(&pos) {
            return *c;
        }
        let found = self
            .parts
            .get(&pos)
            .filter(|p| p.creative == creative)
            .map(|p| p.controller_of(pos))
            .filter(|c| self.parts.get(c).is_some_and(|p| p.creative == creative));
        cache.controllers.insert(pos, found);
        found
    }

    fn form_multi_from(&mut self, cache: &mut SearchCache, mut frontier: Vec<Pos>) {
        let Some(first) = frontier.first().and_then(|p| self.parts.get(p)) else {
            return;
        };
        let creative = first.creative;
        // A vertical multiblock: the search isn't bounded below in Y.
        let mut min_x = i64::MAX;
        let min_y = i64::from(i32::MIN);
        let mut min_z = i64::MAX;
        for p in &frontier {
            min_x = min_x.min(i64::from(p[0]));
            min_z = min_z.min(i64::from(p[2]));
        }
        min_x -= i64::from(MAX_SIZE);
        min_z -= i64::from(MAX_SIZE);
        let mut queue: Vec<(i32, Pos)> = Vec::new();
        let mut visited: HashSet<Pos> = HashSet::new();
        while !frontier.is_empty() {
            let part = frontier.remove(0);
            if !visited.insert(part) {
                continue;
            }
            let amount = self.try_to_form(part, cache, true);
            if amount > 1 {
                queue.push((amount, part));
            }
            for axis in 0..3 {
                let mut next = part;
                next[axis] -= 1;
                if i64::from(next[0]) <= min_x
                    || i64::from(next[1]) <= min_y
                    || i64::from(next[2]) <= min_z
                {
                    continue;
                }
                if visited.contains(&next)
                    || !self
                        .parts
                        .get(&next)
                        .is_some_and(|p| p.creative == creative)
                {
                    continue;
                }
                frontier.push(next);
            }
        }
        visited.clear();
        // The creation queue: a PriorityQueue, largest amount first.
        queue.sort_by_key(|(amount, _)| std::cmp::Reverse(*amount));
        for (_, part) in queue {
            if !visited.insert(part) {
                continue;
            }
            self.try_to_form(part, cache, false);
        }
    }

    /// `tryToFormNewMulti`: the biggest multiblock `pos` (a controller) can
    /// start, made when not simulating; its block count.
    fn try_to_form(&mut self, pos: Pos, cache: &mut SearchCache, simulate: bool) -> i32 {
        let Some(part) = self.parts.get(&pos) else {
            return 0;
        };
        if !part.is_controller(pos) {
            return 0;
        }
        let (width, height) = (part.width, part.height);
        let mut best_width = 1;
        let mut best_amount = -1;
        for w in 1..=MAX_SIZE {
            let amount = self.try_width(pos, w, cache, true);
            if amount < best_amount {
                continue;
            }
            best_width = w;
            best_amount = amount;
        }
        if !simulate {
            if width == best_width && width * width * height == best_amount {
                return best_amount;
            }
            self.split(pos, None, Some(cache));
            if let Some(p) = self.parts.get_mut(&pos) {
                p.apply_size(best_amount);
            }
            self.try_width(pos, best_width, cache, false);
            if let Some(p) = self.parts.get_mut(&pos) {
                p.update_connectivity = false;
                p.width = best_width;
                p.height = best_amount / best_width / best_width;
            }
            self.mark(pos);
        }
        best_amount
    }

    /// `tryToFormNewMultiOfWidth`.
    fn try_width(
        &mut self,
        origin: Pos,
        width: i32,
        cache: &mut SearchCache,
        simulate: bool,
    ) -> i32 {
        let Some(be) = self.parts.get(&origin) else {
            return 0;
        };
        let creative = be.creative;
        let fluid = be.tank.fluid.clone();
        let mut amount = 0;
        let mut height = 0;
        'search: for y in 0..MAX_HEIGHT {
            for x in 0..width {
                for z in 0..width {
                    let pos = at(origin, x, y, z);
                    let Some(con_pos) = self.get_or_cache(cache, creative, pos) else {
                        break 'search;
                    };
                    let Some(controller) = self.parts.get(&con_pos) else {
                        break 'search;
                    };
                    let other = controller.width;
                    if other > width {
                        break 'search;
                    }
                    if other == width && controller.height == MAX_HEIGHT {
                        break 'search;
                    }
                    if con_pos != origin {
                        let [cx, _, cz] = con_pos;
                        let [ox, _, oz] = origin;
                        if cx < ox || cz < oz || cx + other > ox + width || cz + other > oz + width
                        {
                            break 'search;
                        }
                    }
                    let other_fluid = &controller.tank.fluid;
                    if !fluid.is_empty() && !other_fluid.is_empty() && !fluid.same(other_fluid) {
                        break 'search;
                    }
                }
            }
            amount += width * width;
            height += 1;
        }
        if simulate {
            return amount;
        }
        // `getExtraData`: the window, on if any joined tank had one
        // (`modifyExtraData`).
        let mut window = self.parts.get(&origin).is_some_and(|p| p.window);
        for y in 0..height {
            for x in 0..width {
                for z in 0..width {
                    let pos = at(origin, x, y, z);
                    if pos == origin {
                        continue;
                    }
                    let Some(part) = self.parts.get_mut(&pos) else {
                        continue;
                    };
                    if part.creative != creative {
                        continue;
                    }
                    window |= part.window;
                    let moved = std::mem::replace(&mut part.tank.fluid, FluidStack::empty());
                    if !moved.is_empty()
                        && let Some(be) = self.parts.get_mut(&origin)
                    {
                        if be.creative && be.tank.fluid.is_empty() {
                            be.set_contained_fluid(&moved);
                        } else if !be.creative {
                            be.tank.fill(&moved, false);
                        }
                    }
                    self.split(pos, None, Some(cache));
                    if let Some(p) = self.parts.get_mut(&pos) {
                        p.controller = Some(origin);
                        p.update_connectivity = false;
                        p.height = height;
                        p.width = width;
                    }
                    cache.put(pos, origin);
                    self.changed.insert(pos);
                }
            }
        }
        if let Some(be) = self.parts.get_mut(&origin) {
            be.window = window;
        }
        amount
    }

    /// `ConnectivityHandler.splitMultiAndInvalidate` (no reconnecting) of
    /// the multiblock the block at `pos` belongs to: `removed` when that
    /// block was just taken out of the level (its controller then is
    /// itself, and keeps none of the fluid).
    fn split(&mut self, pos: Pos, removed: Option<&TankPart>, mut cache: Option<&mut SearchCache>) {
        let (origin, be) = match removed {
            Some(r) if r.is_controller(pos) => (pos, r.clone()),
            Some(r) => {
                let c = r.controller_of(pos);
                match self.parts.get(&c) {
                    Some(p) => (c, p.clone()),
                    None => return,
                }
            }
            None => {
                let Some(part) = self.parts.get(&pos) else {
                    return;
                };
                let c = part.controller_of(pos);
                match self.parts.get(&c) {
                    Some(p) => (c, p.clone()),
                    None => return,
                }
            }
        };
        let (width, height) = (be.width, be.height);
        if width == 1 && height == 1 {
            return;
        }
        let be_removed = removed.is_some_and(|r| r.is_controller(pos));
        // The fluid handed out: what the controller holds beyond one
        // block's worth (all of it when the controller went).
        let mut to_distribute = be.tank.fluid.clone();
        let max_capacity = TANK_CAPACITY;
        if !to_distribute.is_empty() && !be_removed {
            to_distribute.amount -= max_capacity;
        }
        if let Some(p) = self.parts.get_mut(&origin) {
            p.apply_size(1);
        }
        for y in 0..height {
            for x in 0..width {
                for z in 0..width {
                    let p = at(origin, x, y, z);
                    let Some(part) = self.parts.get(&p) else {
                        continue;
                    };
                    if part.creative != be.creative || part.controller_of(p) != origin {
                        continue;
                    }
                    let window = be.window;
                    self.remove_controller(p);
                    if let Some(part) = self.parts.get_mut(&p) {
                        part.window = window;
                        if !to_distribute.is_empty() && to_distribute.amount > 0 && p != origin {
                            if part.creative {
                                if part.tank.fluid.is_empty() {
                                    part.set_contained_fluid(&to_distribute);
                                }
                            } else {
                                let split = max_capacity.min(to_distribute.amount);
                                part.tank.fill(&to_distribute.with_amount(split), false);
                                to_distribute.amount -= split;
                            }
                        }
                    }
                    if let Some(c) = cache.as_deref_mut() {
                        c.put(p, p);
                    }
                }
            }
        }
    }

    /// `removeController(true)`: the block on its own again, looking for a
    /// multiblock on its next tick.
    fn remove_controller(&mut self, pos: Pos) {
        if let Some(p) = self.parts.get_mut(&pos) {
            p.update_connectivity = true;
            p.controller = None;
            p.width = 1;
            p.height = 1;
            p.boiler.clear();
            self.changed.insert(pos);
        }
    }

    /// Saved one per line: `tank x y z cx cy cz width height window
    /// creative amount fluid` (the fluid `-` when empty).
    pub fn save(&self) -> String {
        let mut lines: Vec<String> = self
            .parts
            .iter()
            .map(|(p, t)| {
                let c = t.controller_of(*p);
                let (amount, fluid) = if t.tank.fluid.is_empty() {
                    (0, "-")
                } else {
                    (t.tank.fluid.amount, t.tank.fluid.fluid.as_str())
                };
                format!(
                    "tank {} {} {} {} {} {} {} {} {} {} {} {}",
                    p[0],
                    p[1],
                    p[2],
                    c[0],
                    c[1],
                    c[2],
                    t.width,
                    t.height,
                    u8::from(t.window),
                    u8::from(t.creative),
                    amount,
                    fluid
                )
            })
            .collect();
        // BoilerData.write.
        lines.extend(
            self.parts
                .iter()
                .filter(|(_, t)| t.boiler.is_active())
                .map(|(p, t)| {
                    format!(
                        "boiler {} {} {} {} {} {} {} {}",
                        p[0],
                        p[1],
                        p[2],
                        t.boiler.water_supply,
                        t.boiler.active_heat,
                        u8::from(t.boiler.passive_heat),
                        t.boiler.attached_engines,
                        t.boiler.attached_whistles
                    )
                }),
        );
        lines.sort();
        lines.join("\n")
    }

    /// What `save` wrote; also the single tanks saved before multiblocks
    /// (`tank x y z amount fluid`), each on its own and looking for a
    /// multiblock on its first tick.
    pub fn load(text: &str) -> Tanks {
        let mut tanks = Tanks::default();
        for line in text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.first() != Some(&"tank") {
                continue;
            }
            let n: Vec<i32> = parts[1..parts.len().saturating_sub(1)]
                .iter()
                .filter_map(|s| s.parse().ok())
                .collect();
            let fluid = parts.last().copied().unwrap_or("-");
            match (parts.len(), n.as_slice()) {
                (6, &[x, y, z, amount]) => {
                    let mut t = TankPart::new(false);
                    t.tank.fluid = FluidStack::new(fluid, amount.min(TANK_CAPACITY));
                    t.update_connectivity = true;
                    tanks.parts.insert([x, y, z], t);
                }
                (13, &[x, y, z, cx, cy, cz, width, height, window, creative, amount]) => {
                    let pos = [x, y, z];
                    let c = [cx, cy, cz];
                    let mut t = TankPart::new(creative != 0);
                    t.controller = (c != pos).then_some(c);
                    t.width = width.clamp(1, MAX_SIZE);
                    t.height = height.clamp(1, MAX_HEIGHT);
                    t.window = window != 0;
                    if c == pos {
                        t.tank.capacity = t.width * t.width * t.height * TANK_CAPACITY;
                        if fluid != "-" && amount > 0 {
                            t.tank.fluid = FluidStack::new(fluid, amount.min(t.tank.capacity));
                        }
                    }
                    tanks.parts.insert(pos, t);
                }
                _ => {}
            }
        }
        for line in text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let ["boiler", x, y, z, supply, heat, passive, engines, whistles] = parts.as_slice()
            else {
                continue;
            };
            let (Ok(x), Ok(y), Ok(z)) = (x.parse(), y.parse(), z.parse()) else {
                continue;
            };
            if let Some(t) = tanks.parts.get_mut(&[x, y, z]) {
                t.boiler = super::create_boiler::Boiler::restored(
                    supply.parse().unwrap_or(0.0),
                    heat.parse().unwrap_or(0),
                    *passive == "1",
                    engines.parse().unwrap_or(0),
                    whistles.parse().unwrap_or(0),
                );
            }
        }
        tanks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn water(amount: i32) -> FluidStack {
        FluidStack::new("minecraft:water", amount)
    }

    #[test]
    fn a_column_joins_into_one_tank() {
        let mut t = Tanks::default();
        t.place([0, 0, 0], false);
        t.place([0, 1, 0], false);
        t.tick();
        t.place([0, 2, 0], false);
        t.tick();
        let bottom = &t.parts[&[0, 0, 0]];
        assert!(bottom.is_controller([0, 0, 0]));
        assert_eq!((bottom.width, bottom.height), (1, 3));
        assert_eq!(bottom.tank.capacity, 3 * TANK_CAPACITY);
        assert_eq!(t.controller([0, 2, 0]), Some([0, 0, 0]));
        assert_eq!(
            t.look([0, 1, 0]),
            Some(Look {
                top: false,
                bottom: false,
                shape: Shape::Window
            })
        );
        assert_eq!(
            t.look([0, 2, 0]).map(|l| l.model()),
            Some("top_window".to_owned())
        );
    }

    #[test]
    fn a_square_base_makes_a_wide_tank_with_corner_windows() {
        let mut t = Tanks::default();
        for (x, z) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            t.place([x, 5, z], false);
            t.tick();
        }
        let c = &t.parts[&[0, 5, 0]];
        assert_eq!((c.width, c.height), (2, 1));
        assert_eq!(t.look([0, 5, 0]).unwrap().shape, Shape::WindowNw);
        assert_eq!(t.look([1, 5, 0]).unwrap().shape, Shape::WindowNe);
        assert_eq!(t.look([0, 5, 1]).unwrap().shape, Shape::WindowSw);
        assert_eq!(t.look([1, 5, 1]).unwrap().shape, Shape::WindowSe);
        assert_eq!(t.look([1, 5, 1]).unwrap().model(), "single_window_se");
    }

    #[test]
    fn a_three_wide_tank_has_windows_mid_side() {
        assert_eq!(window_shape(true, 3, 1, 0), Shape::Window);
        assert_eq!(window_shape(true, 3, 0, 0), Shape::Plain);
        assert_eq!(window_shape(true, 3, 1, 1), Shape::Plain);
        assert_eq!(window_shape(false, 1, 0, 0), Shape::Plain);
    }

    #[test]
    fn fluids_gather_in_the_controller_and_spread_when_split() {
        let mut t = Tanks::default();
        t.place([0, 0, 0], false);
        t.parts.get_mut(&[0, 0, 0]).unwrap().tank.fluid = water(5000);
        t.place([0, 1, 0], false);
        t.parts.get_mut(&[0, 1, 0]).unwrap().tank.fluid = water(0);
        t.form_multi([0, 1, 0]);
        // The block above joined the one below (the search walks down).
        assert_eq!(t.parts[&[0, 0, 0]].height, 2);
        t.handler([0, 1, 0]).unwrap().fill(&water(11000), false);
        assert_eq!(t.parts[&[0, 0, 0]].tank.fluid.amount, 16000);
        // Breaking the top keeps a block's worth below.
        t.remove([0, 1, 0]);
        let bottom = &t.parts[&[0, 0, 0]];
        assert_eq!((bottom.width, bottom.height), (1, 1));
        assert_eq!(bottom.tank.fluid.amount, TANK_CAPACITY);
    }

    #[test]
    fn breaking_the_controller_hands_its_fluid_to_the_rest() {
        let mut t = Tanks::default();
        for y in 0..3 {
            t.place([0, y, 0], false);
            t.tick();
        }
        t.handler([0, 0, 0]).unwrap().fill(&water(20000), false);
        t.remove([0, 0, 0]);
        assert_eq!(t.parts[&[0, 1, 0]].tank.fluid.amount, 8000);
        assert_eq!(t.parts[&[0, 2, 0]].tank.fluid.amount, 8000);
        // Their next tick joins them again, fluids together.
        t.tick();
        let c = &t.parts[&[0, 1, 0]];
        assert_eq!((c.height, c.tank.fluid.amount), (2, 16000));
    }

    #[test]
    fn different_fluids_and_creative_tanks_stay_apart() {
        let mut t = Tanks::default();
        t.place([0, 0, 0], false);
        t.parts.get_mut(&[0, 0, 0]).unwrap().tank.fluid = water(1000);
        t.parts.insert([0, 1, 0], TankPart::new(false));
        t.parts.get_mut(&[0, 1, 0]).unwrap().tank.fluid = FluidStack::new("minecraft:lava", 1000);
        t.form_multi([0, 1, 0]);
        assert_eq!(t.parts[&[0, 0, 0]].height, 1);
        t.place([5, 0, 0], true);
        t.place([5, 1, 0], false);
        assert_eq!(t.parts[&[5, 0, 0]].height, 1);
    }

    #[test]
    fn the_creative_tank_never_empties() {
        let mut t = Tanks::default();
        t.place([0, 0, 0], true);
        t.parts
            .get_mut(&[0, 0, 0])
            .unwrap()
            .set_contained_fluid(&water(1));
        let mut h = t.handler([0, 0, 0]).unwrap();
        assert_eq!(h.drain_any(1000, false).amount, 1000);
        assert_eq!(h.fill(&FluidStack::new("minecraft:lava", 500), false), 500);
        assert_eq!(t.parts[&[0, 0, 0]].amount(), TANK_CAPACITY);
        assert!(t.parts[&[0, 0, 0]].tank.fluid.same(&water(1)));
    }

    #[test]
    fn the_wrench_toggles_the_windows_of_the_whole_tank() {
        let mut t = Tanks::default();
        t.place([0, 0, 0], false);
        t.place([0, 1, 0], false);
        t.tick();
        t.toggle_windows([0, 1, 0]);
        assert_eq!(t.look([0, 1, 0]).unwrap().shape, Shape::Plain);
        assert_eq!(t.look([0, 0, 0]).unwrap().model(), "bottom");
    }

    #[test]
    fn a_boiler_takes_water_for_its_supply_and_gives_nothing() {
        let mut tanks = Tanks::default();
        tanks.place([0, 0, 0], false);
        use super::super::create_fluids::WATER;
        tanks.parts.get_mut(&[0, 0, 0]).unwrap().tank.fluid = FluidStack::new(WATER, 500);
        tanks
            .parts
            .get_mut(&[0, 0, 0])
            .unwrap()
            .boiler
            .attached_engines = 1;
        let mut h = tanks.handler([0, 0, 0]).unwrap();
        assert_eq!(h.fill(&FluidStack::new(WATER, 100), true), 100);
        assert_eq!(h.fill(&FluidStack::new(WATER, 100), false), 100);
        assert_eq!(h.fill(&FluidStack::new("minecraft:lava", 100), false), 0);
        assert!(h.drain_any(100, false).is_empty());
        let part = &tanks.parts[&[0, 0, 0]];
        assert_eq!(part.boiler.gathered_supply, 100);
        assert_eq!(part.tank.fluid.amount, 500);
        // The wrench leaves its windows alone, and a save keeps it.
        let before = part.window;
        tanks.toggle_windows([0, 0, 0]);
        assert_eq!(tanks.parts[&[0, 0, 0]].window, before);
        let loaded = Tanks::load(&tanks.save());
        assert_eq!(loaded.parts[&[0, 0, 0]].boiler.attached_engines, 1);
    }

    #[test]
    fn tanks_round_trip_and_old_saves_load() {
        let mut t = Tanks::default();
        t.place([0, 0, 0], false);
        t.place([0, 1, 0], false);
        t.tick();
        t.handler([0, 0, 0]).unwrap().fill(&water(9000), false);
        let back = Tanks::load(&t.save());
        assert_eq!(back.parts, t.parts);
        let old = Tanks::load("tank 1 2 3 500 minecraft:lava");
        let p = &old.parts[&[1, 2, 3]];
        assert_eq!(p.tank.fluid.amount, 500);
        assert!(p.update_connectivity);
    }
}
