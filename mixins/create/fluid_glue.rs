// Horizon's side of Create's fluids, injected as
// `crate::game::create_fluid_glue`: each dimension's pipes and pumps
// (`create_fluids::Fluids`), the fluid tanks' contents (kept here and saved
// beside the world: the tank's template has no block entity), what an open
// pipe end sees and changes in the world, buckets on tanks, and the tanks'
// fluid drawn for clients (`create:fluids`). Every fluid holder is reached
// through `FluidAccess` (`Access` below), the one place an engine
// `block_capability` hook would take over. The rules are in fluids.rs.

use super::Game;
use super::create_fluids::{
    BasinTanks, FluidAccess, FluidHandler, FluidStack, FluidWorld, Fluids, PipeBlock, PipeStream, Space,
    SpaceChange, TANK_CAPACITY, VanillaTarget, basin_fluid_boxes, bucket_fluid, filled_bucket, tank_fluid_box,
};
use super::create_tank::{TankPart, Tanks};
use horizon_client::containers::{BlockEntity, ModBlockEntity};
use horizon_world::nbt::{JavaString, Tag};
use super::create_kinetics::{Axis, Dir, Pos};
use horizon_client::items::ItemStack;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

/// Tanks' contents for clients (format F1).
pub(super) const FLUIDS: &str = "create:fluids";
pub(super) const PIPE: &str = "create:fluid_pipe";
pub(super) const PUMP: &str = "create:mechanical_pump";
pub(super) const GLASS: &str = "create:glass_fluid_pipe";
pub(super) const ENCASED: &str = "create:encased_fluid_pipe";
pub(super) const VALVE: &str = "create:fluid_valve";
pub(super) const TANK: &str = "create:fluid_tank";
pub(super) const CREATIVE_TANK: &str = "create:creative_fluid_tank";
/// Ticks between full updates to clients and between saves.
const SNAPSHOT_EVERY: u64 = 100;
const SAVE_EVERY: u64 = 1200;
/// `FluidTankRenderer`'s view distance (`BlockEntityRenderer`'s default).
const VIEW_DISTANCE: f64 = 64.0;
/// The default biome's water colour (the plains' `water_color`).
const WATER_TINT: u32 = 0x3F76E4;

#[derive(Default)]
struct State {
    dims: HashMap<String, Fluids>,
    tanks: HashMap<String, Tanks>,
    loaded: HashSet<String>,
    root: Option<PathBuf>,
    /// Basins' tanks as their last tick saw them (input tanks, then output
    /// tanks: `BasinRenderer`'s order), by dimension.
    basins: HashMap<String, HashMap<Pos, Vec<FluidStack>>>,
    /// Spouts' tanks and processing ticks as their last tick saw them.
    spouts: HashMap<String, HashMap<Pos, (FluidStack, i32)>>,
    /// Item drains' tanks.
    drains: HashMap<String, HashMap<Pos, FluidStack>>,
    /// What clients were last told, by dimension.
    shown: HashMap<String, HashMap<Pos, Shown>>,
    ticks: u64,
}

/// A fluid holder as clients draw it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Shown {
    Tank(ShownTank),
    Basin(Vec<FluidStack>),
    /// A spout's fluid and `processingTicks`.
    Spout(FluidStack, i32),
    /// An item drain's fluid.
    Drain(FluidStack),
    /// A glass pipe's flows.
    Glass(Vec<PipeStream>),
}

/// A tank block as clients see it: its controller and, at the controller,
/// the multiblock's size, windows and fluid.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ShownTank {
    controller: Pos,
    width: i32,
    height: i32,
    window: bool,
    creative: bool,
    fluid: FluidStack,
}

impl ShownTank {
    fn of(pos: Pos, part: &TankPart) -> ShownTank {
        let fluid = if part.tank.fluid.is_empty() {
            FluidStack::empty()
        } else {
            part.tank.fluid.with_amount(part.amount())
        };
        ShownTank {
            controller: part.controller_of(pos),
            width: part.width,
            height: part.height,
            window: part.window,
            creative: part.creative,
            fluid,
        }
    }
    /// The tank's capacity (`getTotalTankSize` buckets of 8).
    fn capacity(&self) -> i32 {
        self.width * self.width * self.height * TANK_CAPACITY
    }
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// Block changes heard (`native_block_changed`), for their level's next
/// tick: their own lock, as our own tick changes blocks while holding the
/// state. Whether the block was one of ours.
static PENDING: Mutex<Vec<(String, Pos, bool)>> = Mutex::new(Vec::new());
/// Tanks whose comparators read them again after this tick.
static COMPARATORS: Mutex<Vec<Pos>> = Mutex::new(Vec::new());
/// Whether any pipe exists, so other blocks' changes are only noted then.
static HAS_PIPES: AtomicBool = AtomicBool::new(false);
/// Clients' tanks and basins: dimension code and contents by position.
static CLIENT: Mutex<Option<(u8, HashMap<Pos, Shown>)>> = Mutex::new(None);

fn state() -> MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn pending() -> MutexGuard<'static, Vec<(String, Pos, bool)>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Blocks of ours outside the kinetics (the pump is a kinetic block).
pub(super) fn is_ours(name: &str) -> bool {
    name == PIPE || name == GLASS || name == ENCASED || is_tank_block(name)
}

pub(super) fn is_tank_block(name: &str) -> bool {
    name == TANK || name == CREATIVE_TANK
}

fn dim_code(id: &str) -> u8 {
    match id {
        "minecraft:the_nether" => 1,
        "minecraft:the_end" => 2,
        _ => 0,
    }
}

fn in_world(game: &Game<'_>, pos: Pos) -> bool {
    let w = &game.world;
    w.loaded(pos[0], pos[2]) && pos[1] >= w.shape.min_y && pos[1] < w.shape.max_y()
}

fn block_state<'a>(game: &'a Game<'_>, pos: Pos) -> Option<&'a horizon_world::chunk::BlockState> {
    if !in_world(game, pos) {
        return None;
    }
    game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2]))
}

fn block_name(game: &Game<'_>, pos: Pos) -> Option<String> {
    block_state(game, pos).map(|s| s.name.clone())
}

/// The pipe or pump at `pos` as `FluidTransportBehaviour` sees its state.
fn pipe_block(game: &Game<'_>, pos: Pos) -> Option<PipeBlock> {
    let s = block_state(game, pos)?;
    match s.name.as_str() {
        // FluidPipeBlock's six sides (its own state properties).
        PIPE => Some(PipeBlock::Pipe(pipe_sides(&s.properties))),
        PUMP => s.properties.get("facing").and_then(|f| Dir::parse(f)).map(PipeBlock::Pump),
        ENCASED => Some(PipeBlock::Encased(encased_sides(&s.properties))),
        GLASS => glass_axis(&s.properties).map(PipeBlock::Glass),
        VALVE => valve_axis(&s.properties).map(|a| PipeBlock::Valve(a, s.properties.get("enabled").is_some_and(|v| v == "true"))),
        _ => None,
    }
}

fn glass_axis(props: &horizon_world::block_state_properties::BlockProperties) -> Option<Axis> {
    props.get("axis").and_then(|a| Axis::parse(a))
}

/// `FluidValveBlock.getPipeAxis`.
fn valve_axis(props: &horizon_world::block_state_properties::BlockProperties) -> Option<Axis> {
    let facing = props.get("facing").and_then(|f| Dir::parse(f))?;
    let along = props.get("axis_along_first").is_some_and(|v| v == "true");
    Some(super::create_fluids::valve_pipe_axis(facing, along))
}

/// An encased pipe state's open sides, by `Dir`.
fn encased_sides(props: &horizon_world::block_state_properties::BlockProperties) -> [bool; 6] {
    Dir::ALL.map(|d| props.get(d.name()).is_some_and(|v| v != "false"))
}

/// `state` of a glass pipe or valve with its ends as its neighbours make
/// them, or of an encased pipe with its sides open as `open` (its drains
/// from its neighbours).
pub(super) fn with_attachments(
    level: &super::GameLevel<'_>,
    state: horizon_world::registry::BlockStateId,
    pos: Pos,
) -> Option<horizon_world::registry::BlockStateId> {
    let s = level.registry.state(state)?;
    let (glass, axis) = match s.name.as_str() {
        GLASS => (true, glass_axis(&s.properties)?),
        VALVE => (false, valve_axis(&s.properties)?),
        ENCASED => {
            let open = encased_sides(&s.properties);
            let mut next = state;
            for d in Dir::ALL {
                let side = super::create_fluids::encased_side(open[d as usize], d, pipe_neighbour(level, pos, d));
                next = level.native_state_with_property(next, d.name(), side)?;
            }
            return Some(next);
        }
        _ => return None,
    };
    let mut next = state;
    for (property, d) in [("end_negative", Dir::positive_of(axis).opposite()), ("end_positive", Dir::positive_of(axis))] {
        let end = super::create_fluids::straight_end(glass, axis, d, pipe_neighbour(level, pos, d));
        next = level.native_state_with_property(next, property, end.name())?;
    }
    Some(next)
}

/// `FluidValveBlockEntity.tick`: the pointer of the valve at `pos` a tick on,
/// `ENABLED` following it. Its first tick takes the pointer from the state.
pub(super) fn valve_tick(game: &mut Game<'_>, pos: Pos, speed: f32) {
    let enabled = block_state(game, pos).is_some_and(|s| s.properties.get("enabled").is_some_and(|v| v == "true"));
    let dimension = game.dim.id().to_owned();
    let pointer = {
        let mut map = valve_pointers().lock().unwrap_or_else(|e| e.into_inner());
        let entry = map.entry((dimension, pos)).or_insert(if enabled { 1.0 } else { 0.0 });
        *entry = super::create_fluids::valve_pointer(*entry, speed);
        *entry
    };
    if enabled && pointer == 0.0 {
        game.native_set_block_property(pos, "enabled", "false");
    } else if !enabled && pointer == 1.0 {
        game.native_set_block_property(pos, "enabled", "true");
    }
}

/// Valves' pointers on the server, by dimension and position.
fn valve_pointers() -> &'static Mutex<HashMap<(String, Pos), f32>> {
    static POINTERS: std::sync::OnceLock<Mutex<HashMap<(String, Pos), f32>>> = std::sync::OnceLock::new();
    POINTERS.get_or_init(Default::default)
}

/// Clients' valve pointers (this tick's and the last).
static CLIENT_VALVES: Mutex<Option<HashMap<Pos, (f32, f32)>>> = Mutex::new(None);

/// `FluidValveRenderer`: the pointer drawn, `0` closed to `1` open.
pub(super) fn client_valve_pointer(pos: Pos, enabled: bool, partial: f32) -> f32 {
    let mut guard = CLIENT_VALVES.lock().unwrap_or_else(|e| e.into_inner());
    let start = if enabled { 1.0 } else { 0.0 };
    let (last, now) = *guard.get_or_insert_with(HashMap::new).entry(pos).or_insert((start, start));
    last + (now - last) * partial
}

/// The clients' pointers a tick on, by the speeds they are told.
pub(super) fn valves_client_tick() {
    // The drawing holds the client's lock while it asks for a pointer, so the
    // speeds are read with this lock free.
    let positions: Vec<Pos> = {
        let guard = CLIENT_VALVES.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().map_or_else(Vec::new, |m| m.keys().copied().collect())
    };
    let speeds: Vec<(Pos, Option<f32>)> = positions.into_iter().map(|p| (p, super::create_glue::client_speed(p))).collect();
    let mut guard = CLIENT_VALVES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(map) = guard.as_mut() else {
        return;
    };
    for (pos, speed) in speeds {
        match speed {
            Some(speed) => {
                if let Some((last, now)) = map.get_mut(&pos) {
                    *last = *now;
                    *now = super::create_fluids::valve_pointer(*now, speed);
                }
            }
            None => {
                map.remove(&pos);
            }
        }
    }
}

/// `FluidPipeBlock.onWrenched`, `GlassFluidPipeBlock.onWrenched`,
/// `EncasedPipeBlock.onWrenched`: `true` when `pos` was one of them. A pipe
/// clicked along its axis passes.
pub(super) fn wrench_pipe(game: &mut Game<'_>, pos: Pos, face: Dir) -> Option<bool> {
    let block = pipe_block(game, pos)?;
    let local = game.pick().filter(|h| h.pos == pos).map_or([0.5; 3], |h| {
        [0, 1, 2].map(|k| h.point[k] - f64::from(pos[k]))
    });
    let registry = game.pack.assets.registry;
    let next = match block {
        PipeBlock::Pipe(_) => {
            let axis = super::create_fluids::pipe_wrench_axis(block.straight_axis(), face, local);
            let Some(axis) = axis else {
                return Some(false);
            };
            let base = registry.default_state(GLASS)?;
            game.with_level(|_, level, _| {
                let s = level.native_state_with_property(base, "axis", axis.name())?;
                with_attachments(level, s, pos)
            })
        }
        PipeBlock::Glass(axis) => {
            let side = Dir::positive_of(axis);
            let mut open = [false; 6];
            open[side as usize] = true;
            open[side.opposite() as usize] = true;
            regular_pipe(game, pos, open, side)
        }
        PipeBlock::Encased(open) => {
            let first = Dir::ALL.into_iter().find(|d| open[*d as usize]).unwrap_or(Dir::Up);
            regular_pipe(game, pos, open, first)
        }
        _ => return None,
    };
    if let Some(next) = next {
        let dimension = game.dim.id().to_owned();
        game.native_set_block(&dimension, pos, next, super::native_hooks::flags::UPDATE_ALL);
    }
    Some(true)
}

/// `FluidPipeBlock.updateBlockState` from `open`, the state a pipe at `pos`
/// goes back to.
fn regular_pipe(game: &mut Game<'_>, pos: Pos, open: [bool; 6], preferred: Dir) -> Option<horizon_world::registry::BlockStateId> {
    let base = game.pack.assets.registry.default_state(PIPE)?;
    game.with_level(|_, level, _| {
        let open = super::create_fluids::update_pipe(open, preferred, None, |d| {
            super::create_fluids::pipe_connects(pipe_neighbour(level, pos, d), d)
        });
        with_sides(level, base, pos, open)
    })
}

/// `EncasedPipeBlock.handleEncasing` and `AxisPipeBlock.useItemOn`: copper
/// casing on a pipe or a glass pipe, not used up.
pub(super) fn try_encase(game: &mut Game<'_>, pos: Pos) -> bool {
    if game.held().is_none_or(|s| &*s.item != "create:copper_casing")
        || game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft)
    {
        return false;
    }
    let open = match pipe_block(game, pos) {
        Some(PipeBlock::Pipe(open)) => open,
        Some(b @ PipeBlock::Glass(_)) => Dir::ALL.map(|d| b.open(d)),
        _ => return false,
    };
    let Some(base) = game.pack.assets.registry.default_state(ENCASED) else {
        return false;
    };
    let next = game.with_level(|_, level, _| {
        let mut s = base;
        for d in Dir::ALL {
            s = level.native_state_with_property(s, d.name(), if open[d as usize] { "true" } else { "false" })?;
        }
        with_attachments(level, s, pos)
    });
    if let Some(next) = next {
        let dimension = game.dim.id().to_owned();
        game.native_set_block(&dimension, pos, next, super::native_hooks::flags::UPDATE_ALL);
        // EncasableBlock.playEncaseSound: copper's place sound.
        let at = pos.map(|v| f64::from(v) + 0.5);
        game.with_level(|_, level, _| {
            level.native_play_sound("minecraft:block.copper.place", at, super::create_sounds::BLOCKS, 1.0, 0.8);
        });
    }
    true
}

/// A valve turned by the wrench: its facing and shaft set, its ends again.
pub(super) fn set_valve(game: &mut Game<'_>, pos: Pos, facing: Dir, along: bool) {
    let id = game.world.block(pos[0], pos[1], pos[2]);
    let next = game.with_level(|_, level, _| {
        let s = level.native_state_with_property(id, "facing", facing.name())?;
        let s = level.native_state_with_property(s, "axis_along_first", if along { "true" } else { "false" })?;
        with_attachments(level, s, pos)
    });
    if let Some(next) = next {
        let dimension = game.dim.id().to_owned();
        game.native_set_block(&dimension, pos, next, super::native_hooks::flags::UPDATE_ALL);
    }
}

/// A pipe state's open sides, by `Dir`.
pub(super) fn pipe_sides(props: &horizon_world::block_state_properties::BlockProperties) -> [bool; 6] {
    Dir::ALL.map(|d| props.get(d.name()).is_some_and(|v| v != "closed"))
}

/// The pipe state `state` at `pos` with its sides opened as `open`, each
/// with what it draws there (`getRenderedRimAttachment`).
fn with_sides(
    level: &super::GameLevel<'_>,
    state: horizon_world::registry::BlockStateId,
    pos: Pos,
    open: [bool; 6],
) -> Option<horizon_world::registry::BlockStateId> {
    let mut s = state;
    for d in Dir::ALL {
        let side = super::create_fluids::pipe_attachment(open, d, pipe_neighbour(level, pos, d));
        s = level.native_state_with_property(s, d.name(), side.name())?;
    }
    Some(s)
}

/// What the pipe at `pos` sees on side `d` (`FluidPipeBlock.canConnectTo`).
pub(super) fn pipe_neighbour(level: &super::GameLevel<'_>, pos: Pos, d: Dir) -> super::create_fluids::PipeNeighbour {
    use super::create_fluids::PipeNeighbour as N;
    let n = d.normal();
    let p = [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]];
    let Some(s) = level.registry.state(level.world.block(p[0], p[1], p[2])) else {
        return N::Other;
    };
    match s.name.as_str() {
        PIPE => N::Pipe,
        PUMP => s.properties.get("facing").and_then(|f| Dir::parse(f)).map_or(N::Other, N::Pump),
        ENCASED => N::Encased(encased_sides(&s.properties)[d.opposite() as usize]),
        GLASS => glass_axis(&s.properties).map_or(N::Other, N::Glass),
        VALVE => valve_axis(&s.properties).map_or(N::Other, N::Valve),
        n if is_tank_block(n) || n == super::create_machines::BASIN => N::Holder,
        "minecraft:cauldron" | "minecraft:lava_cauldron" | "minecraft:water_cauldron" => N::Vanilla,
        _ if s.properties.contains_key("honey_level") => N::Vanilla,
        _ => N::Other,
    }
}

/// `FluidPipeBlock.getStateForPlacement`: its sides towards what it
/// connects to, else straight along the placer's look.
pub(super) fn pipe_placement(
    level: &super::GameLevel<'_>,
    state: horizon_world::registry::BlockStateId,
    pos: Pos,
    looking: Dir,
) -> Option<horizon_world::registry::BlockStateId> {
    let open = super::create_fluids::update_pipe([false; 6], looking, None, |d| {
        super::create_fluids::pipe_connects(pipe_neighbour(level, pos, d), d)
    });
    with_sides(level, state, pos, open)
}

/// `FluidPipeBlock.updateShape`: a neighbour on `direction` changed; the
/// side opposite it is left alone.
pub(super) fn pipe_update_shape(
    level: &super::GameLevel<'_>,
    state: horizon_world::registry::BlockStateId,
    pos: Pos,
    direction: Dir,
) -> Option<horizon_world::registry::BlockStateId> {
    let s = level.registry.state(state)?;
    if s.name == GLASS || s.name == ENCASED || s.name == VALVE {
        return with_attachments(level, state, pos);
    }
    if s.name != PIPE {
        return None;
    }
    let open = super::create_fluids::update_pipe(pipe_sides(&s.properties), direction, Some(direction.opposite()), |d| {
        super::create_fluids::pipe_connects(pipe_neighbour(level, pos, d), d)
    });
    with_sides(level, state, pos, open)
}

fn is_tank(game: &Game<'_>, pos: Pos) -> bool {
    block_name(game, pos).is_some_and(|n| is_tank_block(&n))
}

fn is_creative_tank(game: &Game<'_>, pos: Pos) -> bool {
    block_name(game, pos).as_deref() == Some(CREATIVE_TANK)
}

/// The tank blocks changed at `pos`: placed (`onPlace`), removed
/// (`onRemove`) or swapped for the other kind.
fn tank_changed(game: &Game<'_>, tanks: &mut Tanks, pos: Pos) {
    let creative = is_creative_tank(game, pos);
    let now = is_tank(game, pos);
    let had = tanks.parts.get(&pos).map(|p| p.creative);
    if had.is_some() && (!now || had != Some(creative)) {
        tanks.remove(pos);
    }
    if now && !tanks.parts.contains_key(&pos) {
        tanks.place(pos, creative);
    }
}

fn is_basin(game: &Game<'_>, pos: Pos) -> bool {
    block_name(game, pos).as_deref() == Some(super::create_machines::BASIN)
}

// ---- The basin's tanks, in its block entity -------------------------------------

const BASIN_TANK_KEYS: [&str; 4] = ["Output0", "Output1", "Input0", "Input1"];

/// The basin's four tanks, from its block entity's data.
pub(super) fn basin_tanks(be: &ModBlockEntity) -> BasinTanks {
    let mut t = BasinTanks::default();
    let mut stacks = Vec::new();
    for key in BASIN_TANK_KEYS {
        let fluid = match be.nbt.get(&JavaString::from(format!("{key}Fluid").as_str())) {
            Some(Tag::String(s)) => s.to_string_lossy(),
            _ => String::new(),
        };
        let amount = match be.nbt.get(&JavaString::from(format!("{key}Amount").as_str())) {
            Some(Tag::Int(n)) => *n,
            _ => 0,
        };
        stacks.push(FluidStack { fluid, amount });
    }
    t.set_stacks(&stacks);
    t
}

pub(super) fn set_basin_tanks(be: &mut ModBlockEntity, t: &BasinTanks) {
    for (key, stack) in BASIN_TANK_KEYS.into_iter().zip(t.stacks()) {
        let fluid = if stack.is_empty() { String::new() } else { stack.fluid };
        be.nbt.insert(
            JavaString::from(format!("{key}Fluid").as_str()),
            Tag::String(JavaString::from(fluid.as_str())),
        );
        be.nbt
            .insert(JavaString::from(format!("{key}Amount").as_str()), Tag::Int(stack.amount.max(0)));
    }
}

/// Whether the basin's tanks changed since its last tick looked
/// (`SmartFluidTankBehaviour.whenFluidUpdates`).
pub(super) fn basin_fluids_changed(be: &mut ModBlockEntity) -> bool {
    let now: String = basin_tanks(be)
        .stacks()
        .iter()
        .map(|s| format!("{}={};", s.fluid, s.amount))
        .collect();
    let key = JavaString::from("FluidsSeen");
    let seen = match be.nbt.get(&key) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    };
    if seen == now {
        return false;
    }
    be.nbt.insert(key, Tag::String(JavaString::from(now.as_str())));
    true
}

fn save_path(root: &Path, dimension: &str) -> PathBuf {
    root.join("data")
        .join(format!("create_fluids_{}.txt", dimension.replace(':', "_")))
}

fn save(s: &State) {
    let Some(root) = &s.root else {
        return;
    };
    let _ = std::fs::create_dir_all(root.join("data"));
    let dims: HashSet<&String> = s.dims.keys().chain(s.tanks.keys()).collect();
    for dimension in dims {
        let pipes = s.dims.get(dimension).map(Fluids::save).unwrap_or_default();
        let tanks = s.tanks.get(dimension).map(Tanks::save).unwrap_or_default();
        let text = format!("{pipes}\n{tanks}\n");
        if let Err(e) = std::fs::write(save_path(root, dimension), text) {
            log::warn!("Create: can't save the fluids of {dimension}: {e}");
        }
    }
}

// ---- The world, as the fluid rules see it ---------------------------------------

/// The level in place, with its tanks: `FluidAccess` for every fluid holder
/// (tanks here; another mod's would answer through an engine capability
/// hook), and what open pipe ends see.
struct Access<'a, 'g> {
    game: &'a mut Game<'g>,
    dimension: &'a str,
    tanks: &'a mut Tanks,
}

impl Access<'_, '_> {
    /// Whether `pos` is a tank, its part made if the native mod never saw
    /// it placed (looking for a multiblock on its next tick).
    fn tank(&mut self, pos: Pos) -> bool {
        if !is_tank(self.game, pos) {
            return false;
        }
        if !self.tanks.parts.contains_key(&pos) {
            let mut part = TankPart::new(is_creative_tank(self.game, pos));
            part.update_connectivity = true;
            self.tanks.parts.insert(pos, part);
        }
        true
    }

    fn set(&mut self, pos: Pos, state: Option<horizon_world::registry::BlockStateId>) {
        if let Some(state) = state {
            self.game
                .native_set_block(self.dimension, pos, state, super::native_hooks::flags::UPDATE_ALL);
        }
    }
}

impl Access<'_, '_> {
    /// `op` on the fluid holder at `pos`: a tank, or a basin's tanks (kept in
    /// its block entity, written back unless simulating).
    fn with_handler<R>(&mut self, pos: Pos, simulate: bool, op: impl FnOnce(&mut dyn FluidHandler) -> R) -> Option<R> {
        if self.tank(pos) {
            let c = self.tanks.controller(pos)?;
            let mut h = self.tanks.handler(pos)?;
            let r = op(&mut h);
            if !simulate {
                self.tanks.changed.insert(c);
            }
            return Some(r);
        }
        if super::create_spout_glue::is_drain(self.game, pos) {
            return self.game.native_with_mod_block_entity(pos, |be| {
                let mut t = super::create_spout_glue::drain_tank(be);
                let r = op(&mut super::create_spout::DrainTank(&mut t));
                if !simulate {
                    super::create_spout_glue::set_spout_tank(be, &t);
                }
                r
            });
        }
        if super::create_spout_glue::is_spout(self.game, pos) {
            return self.game.native_with_mod_block_entity(pos, |be| {
                let mut t = super::create_spout_glue::spout_tank(be);
                let r = op(&mut t);
                if !simulate {
                    super::create_spout_glue::set_spout_tank(be, &t);
                }
                r
            });
        }
        if !is_basin(self.game, pos) {
            return None;
        }
        self.game.native_with_mod_block_entity(pos, |be| {
            let mut t = basin_tanks(be);
            let r = op(&mut t);
            if !simulate {
                set_basin_tanks(be, &t);
            }
            r
        })
    }
}

impl FluidAccess for Access<'_, '_> {
    fn has_handler(&self, pos: Pos, side: Dir) -> bool {
        // FluidTankBlockEntity's and BasinBlockEntity's capabilities, on
        // every side; SpoutBlockEntity's on all but its bottom.
        is_tank(self.game, pos)
            || is_basin(self.game, pos)
            || (side != Dir::Down && super::create_spout_glue::is_spout(self.game, pos))
            || (side != Dir::Up && super::create_spout_glue::is_drain(self.game, pos))
    }
    fn handler_tanks(&self, pos: Pos, side: Dir) -> Vec<FluidStack> {
        if side != Dir::Up && super::create_spout_glue::is_drain(self.game, pos) {
            return match self.game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
                Some(BlockEntity::Mod(m)) => vec![super::create_spout_glue::drain_tank(m).fluid],
                _ => Vec::new(),
            };
        }
        if side != Dir::Down && super::create_spout_glue::is_spout(self.game, pos) {
            return match self.game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
                Some(BlockEntity::Mod(m)) => vec![super::create_spout_glue::spout_tank(m).fluid],
                _ => Vec::new(),
            };
        }
        if is_tank(self.game, pos) {
            let fluid = self.tanks.controller_part(pos).map(|p| {
                if p.tank.fluid.is_empty() {
                    FluidStack::empty()
                } else {
                    p.tank.fluid.with_amount(p.amount())
                }
            });
            return vec![fluid.unwrap_or_default()];
        }
        if !is_basin(self.game, pos) {
            return Vec::new();
        }
        match self.game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
            Some(BlockEntity::Mod(m)) => basin_tanks(m).stacks(),
            _ => Vec::new(),
        }
    }
    fn fill(&mut self, pos: Pos, _: Dir, resource: &FluidStack, simulate: bool) -> i32 {
        self.with_handler(pos, simulate, |h| h.fill(resource, simulate)).unwrap_or(0)
    }
    fn drain(&mut self, pos: Pos, _: Dir, resource: &FluidStack, simulate: bool) -> FluidStack {
        self.with_handler(pos, simulate, |h| h.drain(resource, simulate)).unwrap_or_default()
    }
    fn drain_any(&mut self, pos: Pos, _: Dir, max: i32, simulate: bool) -> FluidStack {
        self.with_handler(pos, simulate, |h| h.drain_any(max, simulate)).unwrap_or_default()
    }
}

impl FluidWorld for Access<'_, '_> {
    fn is_loaded(&self, pos: Pos) -> bool {
        in_world(self.game, pos)
    }
    fn pipe_block(&self, pos: Pos) -> Option<PipeBlock> {
        pipe_block(self.game, pos)
    }
    fn space(&self, pos: Pos, face: Dir) -> Space {
        let game = &*self.game;
        let Some(state) = block_state(game, pos) else {
            return Space::default();
        };
        let id = game.world.block(pos[0], pos[1], pos[2]);
        let facts = game.res.facts().get(id);
        let fluid = horizon_world::fluid::FluidState::from_block(state).ok();
        let kind = fluid.map(|f| f.kind());
        let prop = |k: &str| state.properties.get(k).map(String::as_str);
        let vanilla = if let Some(level) = prop("honey_level").and_then(|l| l.parse().ok()) {
            Some(VanillaTarget::Honey(level))
        } else {
            match state.name.as_str() {
                "minecraft:cauldron" => Some(VanillaTarget::Cauldron),
                "minecraft:lava_cauldron" => Some(VanillaTarget::LavaCauldron),
                "minecraft:water_cauldron" => Some(VanillaTarget::WaterCauldron(prop("level") == Some("3"))),
                _ => None,
            }
        };
        Space {
            fluid: kind
                .filter(|k| *k != horizon_world::fluid::FluidKind::Empty)
                .map(|k| k.name().to_owned()),
            source: fluid.is_some_and(|f| f.is_source()),
            waterloggable: state.properties.contains_key("waterlogged"),
            replaceable: game.world.is_air(id) || facts.replaceable,
            breakable: facts.hardness >= 0.0,
            // BlockHelper.hasBlockSolidSide: a full face toward the pipe.
            solid_side: facts.sturdy_full & (1 << face as u8) != 0,
            fan_transparent: game.block_in_tag("create:fan_transparent", &state.name),
            vanilla,
            ultra_warm: self.dimension == "minecraft:the_nether",
        }
    }
    fn change_space(&mut self, pos: Pos, change: SpaceChange) {
        let registry = self.game.pack.assets.registry;
        let here = self.game.world.block(pos[0], pos[1], pos[2]);
        let state = match change {
            SpaceChange::Waterlogged(on) => {
                self.game.with_props(here, &[("waterlogged", if on { "true" } else { "false" })])
            }
            // The fluid's block at LEVEL 14, as `removeFluidFromSpace` leaves it.
            SpaceChange::Lowered(fluid) => registry
                .default_state(&fluid)
                .and_then(|s| self.game.with_props(s, &[("level", "14")])),
            SpaceChange::Source(fluid) => registry.default_state(&fluid),
            SpaceChange::EmptyCauldron => registry.default_state("minecraft:cauldron"),
            SpaceChange::EmptyHive => self.game.with_props(here, &[("honey_level", "0")]),
            SpaceChange::Block(name) => registry.default_state(name),
        };
        self.set(pos, state);
    }
    fn pipe_collision(&mut self, pos: Pos, block: Option<&'static str>) {
        // BlockHelper.destroyBlock: the pipe drops, then the reaction's block.
        let [x, y, z] = pos;
        let block = block.unwrap_or("minecraft:air");
        self.game.native_run_command(&format!(
            "execute in {} run setblock {x} {y} {z} {block} destroy",
            self.dimension
        ));
    }
}

// ---- Server -------------------------------------------------------------

/// `native_block_changed` (through the glue): `ours` when the old or new
/// state is one of Create's blocks.
pub(super) fn block_changed(dimension: &str, pos: Pos, ours: bool) {
    if !ours && !HAS_PIPES.load(Ordering::Relaxed) {
        return;
    }
    let mut p = pending();
    // A dimension nobody ticks mustn't grow it for ever.
    if p.len() > 100_000 {
        p.clear();
    }
    p.push((dimension.to_owned(), pos, ours));
}

/// After the kinetics' tick of `dimension`: pumps' speeds, block changes,
/// then every pipe and pump ticks; tanks' changes go to the players there.
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str, pumps: &[(Pos, Dir, f32)]) {
    if game.dim.id() != dimension {
        return;
    }
    let mut changes = Vec::new();
    pending().retain(|(d, p, ours)| {
        if d == dimension {
            changes.push((*p, *ours));
            false
        } else {
            true
        }
    });
    let message;
    {
        let mut guard = state();
        let s = guard.get_or_insert_with(State::default);
        if s.root.is_none() {
            s.root = Some(game.save.root.clone());
        }
        if s.loaded.insert(dimension.to_owned()) {
            let saved = s
                .root
                .as_ref()
                .and_then(|r| std::fs::read_to_string(save_path(r, dimension)).ok());
            if let Some(text) = saved {
                s.dims.insert(dimension.to_owned(), Fluids::load(&text));
                s.tanks.insert(dimension.to_owned(), Tanks::load(&text));
            }
        }
        s.ticks += 1;
        let ticks = s.ticks;
        let State {
            dims, tanks, shown, basins, spouts, drains, ..
        } = s;
        let fluids = dims.entry(dimension.to_owned()).or_default();
        let tanks = tanks.entry(dimension.to_owned()).or_default();
        let mut world = Access {
            game: &mut *game,
            dimension,
            tanks: &mut *tanks,
        };
        for (pos, ours) in changes {
            if !ours {
                // validateNeighbourChange: liquids changing don't count.
                let liquid = block_name(world.game, pos)
                    .is_some_and(|n| n == "minecraft:water" || n == "minecraft:lava");
                if liquid {
                    continue;
                }
            }
            if ours && in_world(world.game, pos) {
                tank_changed(world.game, world.tanks, pos);
            }
            fluids.block_changed(&mut world, pos);
        }
        world.tanks.tick();
        for &(pos, _, speed) in pumps {
            fluids.pump_speed(&mut world, pos, speed);
        }
        fluids.tick(&mut world);
        HAS_PIPES.store(
            dims.values().any(|f| !f.transports.is_empty()),
            Ordering::Relaxed,
        );
        let basins = basins.entry(dimension.to_owned()).or_default();
        let spouts = spouts.entry(dimension.to_owned()).or_default();
        let drains = drains.entry(dimension.to_owned()).or_default();
        let shown = shown.entry(dimension.to_owned()).or_default();
        let streams = dims.get(dimension).map_or_else(Vec::new, Fluids::glass_streams);
        message = updates(game, dimension, tanks, basins, spouts, drains, streams, shown, ticks);
        if ticks % SAVE_EVERY == 0 {
            save(s);
        }
    }
    let changed: Vec<Pos> = std::mem::take(&mut *COMPARATORS.lock().unwrap_or_else(|e| e.into_inner()));
    for p in changed {
        game.native_update_output_neighbors(dimension, p);
    }
    let Some(message) = message else {
        return;
    };
    for player in game.native_server_players() {
        if player.dimension != dimension {
            continue;
        }
        if let Err(e) = game.native_net_send_to_player(player.uuid, FLUIDS, &message) {
            log::debug!("Create: fluids to {}: {e}", player.name);
        }
    }
}

/// A basin's block entity tick: its tanks, for clients
/// (`BasinBlockEntity.sendData`).
pub(super) fn note_basin(dimension: &str, pos: Pos, tanks: &BasinTanks) {
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    let stacks: Vec<FluidStack> = tanks.input.iter().chain(tanks.output.iter()).map(|t| t.fluid.clone()).collect();
    let basins = s.basins.entry(dimension.to_owned()).or_default();
    if stacks.iter().all(FluidStack::is_empty) {
        basins.remove(&pos);
    } else {
        basins.insert(pos, stacks);
    }
}

/// A spout's block entity tick: its tank and processing ticks, for clients
/// (`SpoutBlockEntity.notifyUpdate`).
pub(super) fn note_spout(dimension: &str, pos: Pos, fluid: &FluidStack, ticks: i32) {
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    let spouts = s.spouts.entry(dimension.to_owned()).or_default();
    // onFluidStackChanged: comparators beside it read it again.
    if spouts.get(&pos).map_or(0, |(f, _)| f.amount.max(0) * i32::from(!f.is_empty())) != fluid.amount.max(0) * i32::from(!fluid.is_empty()) {
        COMPARATORS.lock().unwrap_or_else(|e| e.into_inner()).push(pos);
    }
    if fluid.is_empty() && ticks < 0 {
        spouts.remove(&pos);
    } else {
        spouts.insert(pos, (fluid.clone(), ticks));
    }
}

/// An item drain's block entity tick: its tank, for clients.
pub(super) fn note_drain(dimension: &str, pos: Pos, fluid: &FluidStack) {
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    let drains = s.drains.entry(dimension.to_owned()).or_default();
    if drains.get(&pos).map_or(0, |f| f.amount) != if fluid.is_empty() { 0 } else { fluid.amount } {
        COMPARATORS.lock().unwrap_or_else(|e| e.into_inner()).push(pos);
    }
    if fluid.is_empty() {
        drains.remove(&pos);
    } else {
        drains.insert(pos, fluid.clone());
    }
}

/// The tanks and basins whose contents changed since clients were told
/// (all of them every `SNAPSHOT_EVERY` ticks), as one F1 message.
fn updates(
    game: &Game<'_>,
    dimension: &str,
    tanks: &mut Tanks,
    basins: &mut HashMap<Pos, Vec<FluidStack>>,
    spouts: &mut HashMap<Pos, (FluidStack, i32)>,
    drains: &mut HashMap<Pos, FluidStack>,
    streams: Vec<(Pos, Vec<PipeStream>)>,
    shown: &mut HashMap<Pos, Shown>,
    ticks: u64,
) -> Option<Vec<u8>> {
    // Tanks that are no tank any more go (a change the level didn't tell).
    let gone: Vec<Pos> = tanks
        .parts
        .keys()
        .filter(|p| in_world(game, **p) && !is_tank(game, **p))
        .copied()
        .collect();
    for p in gone {
        tanks.remove(p);
    }
    // Level.updateNeighbourForOutputSignal (`onFluidStackChanged`): the
    // comparators beside a changed tank read it again, once the state is
    // let go (they ask `tank_analog`).
    COMPARATORS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .extend(tanks.changed.drain().filter(|p| in_world(game, *p)));
    basins.retain(|p, _| !in_world(game, *p) || is_basin(game, *p));
    spouts.retain(|p, _| !in_world(game, *p) || super::create_spout_glue::is_spout(game, *p));
    drains.retain(|p, _| !in_world(game, *p) || super::create_spout_glue::is_drain(game, *p));
    let now: HashMap<Pos, Shown> = tanks
        .parts
        .iter()
        .map(|(p, t)| (*p, Shown::Tank(ShownTank::of(*p, t))))
        .chain(basins.iter().map(|(p, b)| (*p, Shown::Basin(b.clone()))))
        .chain(spouts.iter().map(|(p, (f, t))| (*p, Shown::Spout(f.clone(), *t))))
        .chain(drains.iter().map(|(p, f)| (*p, Shown::Drain(f.clone()))))
        .chain(streams.into_iter().map(|(p, s)| (p, Shown::Glass(s))))
        .collect();
    let full = ticks % SNAPSHOT_EVERY == 0;
    let mut entries: Vec<(Pos, Option<Shown>)> = now
        .iter()
        .filter(|(p, v)| full || shown.get(*p) != Some(*v))
        .map(|(p, v)| (*p, Some(v.clone())))
        .collect();
    entries.extend(shown.keys().filter(|p| !now.contains_key(*p)).map(|p| (*p, None)));
    *shown = now;
    if entries.is_empty() && !full {
        return None;
    }
    entries.truncate(1000);
    Some(encode(dim_code(dimension), full, &entries))
}

/// F2: the dimension, whether it replaces all, then per holder its position,
/// kind (0 gone, 1 tank, 2 basin), a tank's controller, width, height and
/// flags (1 window, 2 creative), and fluids (amount, id).
fn encode(dim: u8, full: bool, entries: &[(Pos, Option<Shown>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"F2");
    out.push(dim);
    out.push(u8::from(full));
    for (pos, shown) in entries {
        for v in pos {
            out.extend_from_slice(&v.to_le_bytes());
        }
        // A glass pipe's flows, each sent as side (with inbound on top),
        // progress and then its fluid among the fluids.
        if let Some(Shown::Glass(streams)) = shown {
            out.push(5);
            out.push(streams.len().min(6) as u8);
            for s in streams.iter().take(6) {
                out.push(s.side as u8 | u8::from(s.inbound) << 3);
                out.push(s.progress);
                let id = s.fluid.fluid.as_bytes();
                let len = id.len().min(255);
                out.push(len as u8);
                out.extend_from_slice(&id[..len]);
            }
            continue;
        }
        let (kind, fluids): (u8, &[FluidStack]) = match shown {
            None | Some(Shown::Glass(_)) => (0, &[]),
            Some(Shown::Tank(t)) => (1, std::slice::from_ref(&t.fluid)),
            Some(Shown::Basin(b)) => (2, b),
            Some(Shown::Spout(f, _)) => (3, std::slice::from_ref(f)),
            Some(Shown::Drain(f)) => (4, std::slice::from_ref(f)),
        };
        out.push(kind);
        if let Some(Shown::Spout(_, ticks)) = shown {
            out.extend_from_slice(&ticks.to_le_bytes());
        }
        if let Some(Shown::Tank(t)) = shown {
            for v in t.controller {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.push(t.width.clamp(0, 255) as u8);
            out.push(t.height.clamp(0, 255) as u8);
            out.push(u8::from(t.window) | u8::from(t.creative) << 1);
        }
        out.push(fluids.len().min(255) as u8);
        for fluid in fluids.iter().take(255) {
            let amount = if fluid.is_empty() { 0 } else { fluid.amount };
            out.extend_from_slice(&amount.to_le_bytes());
            let id = fluid.fluid.as_bytes();
            let len = id.len().min(255);
            out.push(len as u8);
            out.extend_from_slice(&id[..len]);
        }
    }
    out
}

type Decoded = (u8, bool, Vec<(Pos, Option<Shown>)>);

fn decode(data: &[u8]) -> Option<Decoded> {
    let rest = data.strip_prefix(b"F2")?;
    let (&dim, rest) = rest.split_first()?;
    let (&full, mut rest) = rest.split_first()?;
    let int = |b: &[u8]| i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let mut entries = Vec::new();
    while !rest.is_empty() {
        let head = rest.get(..13)?;
        let pos = [int(&head[0..4]), int(&head[4..8]), int(&head[8..12])];
        let kind = head[12];
        rest = &rest[13..];
        if kind == 5 {
            let (&n, mut after) = rest.split_first()?;
            let mut streams = Vec::new();
            for _ in 0..n {
                let h = after.get(..3)?;
                let len = usize::from(h[2]);
                let id = after.get(3..3 + len)?;
                streams.push(PipeStream {
                    side: Dir::ALL.get(usize::from(h[0] & 7)).copied()?,
                    inbound: h[0] & 8 != 0,
                    progress: h[1],
                    fluid: FluidStack {
                        fluid: String::from_utf8(id.to_vec()).ok()?,
                        amount: 1,
                    },
                });
                after = &after[3 + len..];
            }
            rest = after;
            entries.push((pos, Some(Shown::Glass(streams))));
            continue;
        }
        let mut tank = None;
        if kind == 1 {
            let t = rest.get(..15)?;
            tank = Some(ShownTank {
                controller: [int(&t[0..4]), int(&t[4..8]), int(&t[8..12])],
                width: i32::from(t[12]),
                height: i32::from(t[13]),
                window: t[14] & 1 != 0,
                creative: t[14] & 2 != 0,
                fluid: FluidStack::empty(),
            });
            rest = &rest[15..];
        }
        let mut ticks = -1;
        if kind == 3 {
            ticks = int(rest.get(..4)?);
            rest = &rest[4..];
        }
        let (&n, after) = rest.split_first()?;
        let n = usize::from(n);
        rest = after;
        let mut fluids = Vec::new();
        for _ in 0..n {
            let h = rest.get(..5)?;
            let amount = int(&h[0..4]);
            let len = usize::from(h[4]);
            let id = rest.get(5..5 + len)?;
            fluids.push(FluidStack {
                fluid: String::from_utf8(id.to_vec()).ok()?,
                amount,
            });
            rest = &rest[5 + len..];
        }
        let shown = match kind {
            1 => {
                let mut t = tank?;
                t.fluid = fluids.into_iter().next().unwrap_or_default();
                Some(Shown::Tank(t))
            }
            2 => Some(Shown::Basin(fluids)),
            3 => Some(Shown::Spout(fluids.into_iter().next().unwrap_or_default(), ticks)),
            4 => Some(Shown::Drain(fluids.into_iter().next().unwrap_or_default())),
            _ => None,
        };
        entries.push((pos, shown));
    }
    Some((dim, full != 0, entries))
}

/// `FluidTankBlock.useItemOn` and `BasinBlock.useItemOn` with a bucket
/// (`FluidHelper.tryEmptyItemIntoBE` / `tryFillItemFromBE`), through the
/// block's fluid handler: a full bucket empties into it, an empty one takes
/// a bucket's worth out. True when it took the click.
pub(super) fn right_click(game: &mut Game<'_>, pos: Pos) -> bool {
    let tank = is_tank(game, pos);
    if !tank && !is_basin(game, pos) {
        return false;
    }
    let Some(held) = game.held().map(|s| s.item.to_string()) else {
        return false;
    };
    let full = bucket_fluid(&held);
    if full.is_none() && held != "minecraft:bucket" {
        return false;
    }
    // The acting player's mode, or the window's own player's (no player is
    // swapped in for the host's clicks).
    let creative = game.native_acting_player().map_or(game.mode_id(), |p| p.game_mode) == 1;
    // FluidTankBlock.useItemOn: in survival only the creative tank takes
    // buckets ("in Survival Mode Fluids cannot be added or taken manually").
    let creative_tank = is_creative_tank(game, pos);
    if tank && !creative && !creative_tank {
        return false;
    }
    let dimension = game.dim.id().to_owned();
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    let tanks = s.tanks.entry(dimension.clone()).or_default();
    let mut access = Access {
        game: &mut *game,
        dimension: &dimension,
        tanks,
    };
    let side = Dir::Up;
    let result = if let Some(fluid) = full {
        let stack = FluidStack::new(fluid, 1000);
        if access.fill(pos, side, &stack, true) != 1000 {
            return true;
        }
        access.fill(pos, side, &stack, false);
        // CreativeSmartFluidTank.setContainedFluid: the bucket's fluid, for good.
        if creative_tank && let Some(p) = access.tanks.controller_part_mut(pos) {
            p.set_contained_fluid(&stack);
        }
        Some("minecraft:bucket")
    } else {
        // The first tank holding a bucket's worth of a fluid a bucket takes.
        let found = access
            .handler_tanks(pos, side)
            .into_iter()
            .find(|f| f.amount >= 1000 && filled_bucket(&f.fluid).is_some());
        let Some(f) = found else {
            return true;
        };
        if access.drain(pos, side, &f.with_amount(1000), false).amount != 1000 {
            return true;
        }
        if creative_tank && let Some(p) = access.tanks.controller_part_mut(pos) {
            p.set_contained_fluid(&FluidStack::empty());
        }
        filled_bucket(&f.fluid)
    };
    drop(guard);
    if creative {
        return true;
    }
    if let Some(item) = result {
        let hand = game.held_mut();
        match hand {
            Some(s) if s.count > 1 => {
                s.count -= 1;
                game.give_or_drop(ItemStack::new(item, 1));
            }
            _ => *hand = Some(ItemStack::new(item, 1)),
        }
    }
    true
}

/// `FluidTankBlock.getAnalogOutputSignal`: the controller's fill as a
/// comparator level (`ComparatorUtil.fractionToRedstoneLevel`).
pub(super) fn tank_analog(dimension: &str, pos: Pos) -> i32 {
    let guard = state();
    let Some(part) = guard
        .as_ref()
        .and_then(|s| s.tanks.get(dimension))
        .and_then(|t| t.controller_part(pos))
    else {
        return 0;
    };
    let frac = if part.tank.capacity > 0 {
        f64::from(part.amount()) / f64::from(part.tank.capacity)
    } else {
        0.0
    };
    (frac * 14.0 + if frac > 0.0 { 1.0 } else { 0.0 }).clamp(0.0, 15.0).floor() as i32
}

/// `FluidTankBlock.onWrenched`: the wrench toggles the whole tank's windows
/// (`toggleWindows`; boilers keep theirs shut). True when it took the click.
pub(super) fn wrench(game: &mut Game<'_>, pos: Pos) -> bool {
    if !is_tank(game, pos) {
        return false;
    }
    let dimension = game.dim.id().to_owned();
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    let tanks = s.tanks.entry(dimension).or_default();
    if !tanks.parts.contains_key(&pos) {
        tank_changed(game, tanks, pos);
    }
    tanks.toggle_windows(pos);
    true
}

/// `native_world_lifecycle`: saved when a world closes, emptied when one
/// opens.
pub(super) fn world_phase(closing: bool) {
    let mut guard = state();
    if closing && let Some(s) = guard.as_ref() {
        save(s);
    }
    *guard = None;
    pending().clear();
    HAS_PIPES.store(false, Ordering::Relaxed);
    *CLIENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Periodic save, with the glue's.
pub(super) fn save_all() {
    if let Some(s) = state().as_ref() {
        save(s);
    }
}

// ---- Client -------------------------------------------------------------

/// `native_net_from_server` on `create:fluids`.
/// What this client was last told the tank or basin at `pos` holds, with
/// each tank's capacity (`IFluidHandler` for the goggles): none for
/// another block.
pub(super) fn client_fluids(dimension: u8, pos: Pos) -> Option<(Vec<(FluidStack, i32)>, usize)> {
    let guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    let (dim, shown) = guard.as_ref()?;
    if *dim != dimension {
        return None;
    }
    Some(match shown.get(&pos)? {
        // The controller's tank (`FluidTankBlockEntity.addToGoggleTooltip`);
        // the creative tank shows nothing.
        Shown::Tank(t) => {
            let c = match shown.get(&t.controller) {
                Some(Shown::Tank(c)) => c,
                _ => t,
            };
            if c.creative {
                return None;
            }
            (vec![(c.fluid.clone(), c.capacity())], 1)
        }
        // Two input and two output tanks.
        Shown::Basin(fs) => (fs.iter().map(|f| (f.clone(), super::create_fluids::BASIN_TANK)).collect(), 4),
        Shown::Spout(f, _) => (vec![(f.clone(), super::create_spout::SPOUT_CAPACITY)], 1),
        Shown::Drain(f) => (vec![(f.clone(), super::create_spout::DRAIN_CAPACITY)], 1),
        // A pipe holds nothing for the goggles.
        Shown::Glass(_) => return None,
    })
}

/// What this client was last told the spout at `pos` holds, and its
/// processing ticks.
pub(super) fn client_spout(game: &Game<'_>, pos: Pos) -> Option<(FluidStack, i32)> {
    let guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    let (dim, shown) = guard.as_ref()?;
    if *dim != dim_code(game.dim.id()) {
        return None;
    }
    match shown.get(&pos)? {
        Shown::Spout(f, t) => Some((f.clone(), *t)),
        _ => None,
    }
}

pub(super) fn from_server(data: &[u8]) {
    let Some((dim, full, entries)) = decode(data) else {
        log::debug!("Create: a bad fluids message");
        return;
    };
    let mut guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    let c = guard.get_or_insert_with(|| (dim, HashMap::new()));
    if full || c.0 != dim {
        *c = (dim, HashMap::new());
    }
    for (pos, shown) in entries {
        match shown {
            Some(v) => c.1.insert(pos, v),
            None => c.1.remove(&pos),
        };
    }
}

/// A fluid's still texture (`FluidType.getStillTexture`) and tint.
/// The atlas keeps vanilla's sprites without their namespace
/// (`block/water_still`) and mods' with it.
fn fluid_sprite(fluid: &str) -> (String, u32) {
    match fluid {
        "minecraft:water" => ("block/water_still".to_owned(), WATER_TINT),
        "minecraft:lava" => ("block/lava_still".to_owned(), 0xFFFFFF),
        f => {
            let (ns, name) = f.split_once(':').unwrap_or(("minecraft", f));
            (format!("{ns}:fluid/{name}_still"), 0xFFFFFF)
        }
    }
}

/// `FluidTankRenderer` and `BasinRenderer.renderFluids`: each tank's fluid
/// as a box as high as it is full, each basin's fluids side by side.
pub(super) fn draw(game: &Game<'_>, camera: glam::DVec3, out: &mut Vec<horizon_client::mesher::Vertex>) {
    let holders: Vec<(Pos, Shown)> = {
        let guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
        match guard.as_ref() {
            Some((dim, t)) if *dim == dim_code(game.dim.id()) => t.iter().map(|(p, v)| (*p, v.clone())).collect(),
            _ => return,
        }
    };
    let tanks: HashMap<Pos, ShownTank> = holders
        .iter()
        .filter_map(|(p, v)| match v {
            Shown::Tank(t) => Some((*p, t.clone())),
            Shown::Basin(_) | Shown::Spout(..) | Shown::Drain(_) | Shown::Glass(_) => None,
        })
        .collect();
    for (pos, shown) in holders {
        let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
        if (corner + glam::DVec3::splat(0.5)).distance_squared(camera) > VIEW_DISTANCE * VIEW_DISTANCE {
            continue;
        }
        let packed = packed_light(game, pos);
        let origin = (corner - camera).as_vec3();
        let boxes: Vec<(FluidStack, [f32; 6])> = match shown {
            Shown::Tank(t) => {
                draw_tank_block(game, &tanks, pos, &t, origin, out);
                // FluidTankRenderer: the controller draws the fluid, through
                // its windows only.
                if t.controller != pos || !t.window || t.fluid.is_empty() {
                    continue;
                }
                let fill = t.fluid.amount as f32 / t.capacity().max(1) as f32;
                if let Some(b) = tank_fluid_box(fill, t.width as f32, t.height as f32) {
                    draw_tiled_fluid_box(game, &t.fluid.fluid, b, pos, camera, out);
                }
                continue;
            }
            Shown::Basin(stacks) => basin_fluid_boxes(&stacks),
            // SpoutRenderer: the tank, then the stream below while it spouts.
            Shown::Spout(fluid, ticks) => {
                let level = fluid.amount as f32 / super::create_spout::SPOUT_CAPACITY as f32;
                let tank = if fluid.is_empty() { None } else { super::create_spout::tank_box(level) };
                let stream = super::create_spout_glue::client_stream(pos, ticks, !fluid.is_empty());
                tank.into_iter().chain(stream).map(|b| (fluid.clone(), b)).collect()
            }
            // TransparentStraightPipeRenderer: FluidRenderer.renderFluidStream
            // for each flow.
            Shown::Glass(streams) => {
                streams.iter().map(|s| (s.fluid.clone(), super::create_fluids::pipe_stream_box(s))).collect()
            }
            // ItemDrainRenderer.renderFluid. Not yet: the stream over an
            // item being emptied.
            Shown::Drain(fluid) => {
                let level = fluid.amount as f32 / super::create_spout::DRAIN_CAPACITY as f32;
                super::create_spout::drain_fluid_box(level).into_iter().map(|b| (fluid.clone(), b)).collect()
            }
        };
        for (fluid, b) in boxes {
            draw_fluid_box(game, &fluid.fluid, b, origin, packed, out, [true; 6]);
        }
    }
}

fn packed_light(game: &Game<'_>, pos: Pos) -> u32 {
    let light = game.world.light(pos[0], pos[1], pos[2]);
    (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8)
}

/// Model holders of the tank's looks, by name.
static TANK_MODELS: Mutex<Option<HashMap<String, Option<horizon_world::registry::BlockStateId>>>> = Mutex::new(None);

/// The tank block's model as `FluidTankGenerator` picks it (`TOP`,
/// `BOTTOM`, `SHAPE` from its controller), held by a hidden
/// `create:hz_tank_*` block: the tank's own blockstate is blank, as no
/// template has those properties.
fn draw_tank_block(
    game: &Game<'_>,
    tanks: &HashMap<Pos, ShownTank>,
    pos: Pos,
    t: &ShownTank,
    origin: glam::Vec3,
    out: &mut Vec<horizon_client::mesher::Vertex>,
) {
    let c = tanks.get(&t.controller).unwrap_or(t);
    let look = super::create_tank::Look {
        bottom: t.controller[1] == pos[1],
        top: t.controller[1] + c.height - 1 == pos[1],
        shape: super::create_tank::window_shape(c.window, c.width, pos[0] - t.controller[0], pos[2] - t.controller[2]),
    };
    let prefix = if t.creative { "hz_creative_tank_" } else { "hz_tank_" };
    let name = format!("create:{prefix}{}", look.model());
    let state = {
        let mut guard = TANK_MODELS.lock().unwrap_or_else(|e| e.into_inner());
        *guard
            .get_or_insert_with(HashMap::new)
            .entry(name)
            .or_insert_with_key(|n| game.pack.assets.registry.default_state(n))
    };
    let Some(state) = state else {
        return;
    };
    // FluidTankCTBehaviour.connectsTo: the same kind of tank, one multiblock.
    let connected = |other: Pos| {
        tanks
            .get(&other)
            .is_some_and(|o| o.controller == t.controller && o.creative == t.creative)
    };
    // FluidTankModel.standard / creative: the side, top and inner shifts.
    let (side, top, inner) = if t.creative {
        ("create:block/creative_fluid_tank", "create:block/creative_casing", "create:block/creative_casing")
    } else {
        ("create:block/fluid_tank", "create:block/fluid_tank_top", "create:block/fluid_tank_inner")
    };
    draw_ct_model(
        game,
        state,
        pos,
        glam::Mat4::from_translation(origin),
        // FluidTankModel's CullData: the quads culled toward a joined block
        // beside it are left out (none above or below).
        |cull| {
            let n = cull.normal();
            cull.axis() != Axis::Y && connected([pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]])
        },
        // FluidTankCTBehaviour.getShift: the inner texture on Y faces, else
        // the side's on horizontal faces and the top's on vertical ones.
        |facing, name| {
            let original = if facing.axis() == Axis::Y && name == inner {
                inner
            } else if facing.axis() == Axis::Y {
                top
            } else {
                side
            };
            let (up, right) = super::create_ct::default_frame(facing);
            (name == original).then(|| (format!("{original}_connected"), up, right))
        },
        connected,
        out,
    );
}

/// `CTModel`: the model of `state` drawn at `m` (each face lit by the
/// block it faces, as the chunk mesh lights it), each quad culled toward a
/// direction `culled` names left out, and each face whose sprite `shift`
/// answers for (its connected sprite and the face's up and right) moved onto
/// the tile of that `RECTANGLE` sheet its neighbours (`connected`) pick.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_ct_model(
    game: &Game<'_>,
    state: horizon_world::registry::BlockStateId,
    pos: Pos,
    m: glam::Mat4,
    culled: impl Fn(Dir) -> bool,
    shift: impl Fn(Dir, &str) -> Option<(String, Dir, Dir)>,
    connected: impl Fn(Pos) -> bool,
    out: &mut Vec<horizon_client::mesher::Vertex>,
) {
    use super::create_ct::{CtType, build_context, target_uv};
    use horizon_assets::model::Direction as D;
    let seed = horizon_core::random::position_seed(horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]));
    let Ok(models) = game.pack.render.model_catalog.collect_seeded(state, seed) else {
        return;
    };
    let dir = |d: D| match d {
        D::Down => Dir::Down,
        D::Up => Dir::Up,
        D::North => Dir::North,
        D::South => Dir::South,
        D::West => Dir::West,
        D::East => Dir::East,
    };
    let sprites = &game.gui.atlas.sprites;
    for (_, model) in &models {
        for face in &model.faces {
            let Some(name) = game.pack.layer_names.get(&face.layer) else {
                continue;
            };
            let Some(s) = sprites.get(name) else {
                continue;
            };
            if face.quad.cull.map(dir).is_some_and(&culled) {
                continue;
            }
            let facing = dir(face.quad.facing);
            let ct = shift(facing, name).and_then(|(target, up, right)| {
                let t = sprites.get(&target)?;
                let c = build_context(CtType::Rectangle, pos, facing, up, right, &connected);
                Some((t, CtType::Rectangle.index(&c)))
            });
            let shade: u32 = match face.quad.facing {
                D::Up => 255,
                D::Down => 127,
                D::North | D::South => 204,
                D::West | D::East => 153,
            };
            let color = 0xFF00_0000 | shade << 16 | shade << 8 | shade;
            let n = facing.normal();
            let light = packed_light(game, [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]]);
            for k in 0..4 {
                let p = face.quad.positions[k];
                let mut uv = face.quad.uv[k];
                let (mut u0, mut v0, mut du, mut dv) = (s.u0, s.v0, s.u1 - s.u0, s.v1 - s.v0);
                if let Some((t, index)) = ct {
                    uv = target_uv(uv, index, CtType::Rectangle);
                    (u0, v0, du, dv) = (t.u0, t.v0, t.u1 - t.u0, t.v1 - t.v0);
                }
                out.push(horizon_client::mesher::Vertex {
                    normal: [0; 4],
                    pos: m.transform_point3(glam::Vec3::from_array(p)).to_array(),
                    uv: [u0 + uv[0] * du, v0 + uv[1] * dv],
                    layer: 0,
                    color,
                    light,
                    glint_uv: [0.0, 0.0],
                });
            }
        }
    }
}

/// `FluidRenderer.renderFluidBox` over a box many blocks wide and high
/// (`renderTiledFace`): the texture repeats each block, each block's piece
/// lit by its own cell, only the box's outer faces drawn.
fn draw_tiled_fluid_box(
    game: &Game<'_>,
    fluid: &str,
    b: [f32; 6],
    controller: Pos,
    camera: glam::DVec3,
    out: &mut Vec<horizon_client::mesher::Vertex>,
) {
    let [x0, y0, z0, x1, y1, z1] = b;
    let cells = |lo: f32, hi: f32| (lo.floor() as i32)..(hi.ceil() as i32);
    for cx in cells(x0, x1) {
        for cy in cells(y0, y1) {
            for cz in cells(z0, z1) {
                let (fx, fy, fz) = (cx as f32, cy as f32, cz as f32);
                let piece = [
                    x0.max(fx) - fx,
                    y0.max(fy) - fy,
                    z0.max(fz) - fz,
                    x1.min(fx + 1.0) - fx,
                    y1.min(fy + 1.0) - fy,
                    z1.min(fz + 1.0) - fz,
                ];
                if piece[0] >= piece[3] || piece[1] >= piece[4] || piece[2] >= piece[5] {
                    continue;
                }
                // Up, down, north, south, west, east: outer faces only.
                let outer = [
                    y1 <= fy + 1.0,
                    y0 >= fy,
                    z0 >= fz,
                    z1 <= fz + 1.0,
                    x0 >= fx,
                    x1 <= fx + 1.0,
                ];
                let cell = [controller[0] + cx, controller[1] + cy, controller[2] + cz];
                let corner = glam::DVec3::new(f64::from(cell[0]), f64::from(cell[1]), f64::from(cell[2]));
                let origin = (corner - camera).as_vec3();
                draw_fluid_box(game, fluid, piece, origin, packed_light(game, cell), out, outer);
            }
        }
    }
}

/// `FluidRenderer.renderFluidBox`: the fluid's still texture on each face
/// of the box, tinted, shaded by the way each faces.
fn draw_fluid_box(
    game: &Game<'_>,
    fluid: &str,
    b: [f32; 6],
    origin: glam::Vec3,
    light: u32,
    out: &mut Vec<horizon_client::mesher::Vertex>,
    faces: [bool; 6],
) {
    let (sprite, tint) = fluid_sprite(fluid);
    let Some(s) = game.gui.atlas.sprites.get(&sprite) else {
        static WARNED: AtomicBool = AtomicBool::new(false);
        if !WARNED.swap(true, Ordering::Relaxed) {
            log::warn!("Create: no sprite {sprite} to draw {fluid} by");
        }
        return;
    };
    let (u0, v0, du, dv) = (s.u0, s.v0, s.u1 - s.u0, s.v1 - s.v0);
    for ((corners, shade), _) in box_faces(b).into_iter().zip(faces).filter(|(_, f)| *f) {
        let color = 0xFF00_0000 | shaded(tint, shade);
        for c in corners {
            // Each face's texture follows the block's own coordinates.
            let (u, v) = (c[3], c[4]);
            out.push(horizon_client::mesher::Vertex {
                normal: [0; 4],
                pos: (origin + glam::Vec3::new(c[0], c[1], c[2])).to_array(),
                uv: [u0 + u * du, v0 + v * dv],
                layer: 0,
                color,
                light,
                glint_uv: [0.0, 0.0],
            });
        }
    }
}

/// `tint` (0xRRGGBB) times `shade`, as the vertex's RGBA8 (red in the low
/// byte).
fn shaded(tint: u32, shade: u32) -> u32 {
    let ch = |shift: u32| (tint >> shift & 0xFF) * shade / 255;
    ch(16) | ch(8) << 8 | ch(0) << 16
}

/// The six faces of `[x0, y0, z0, x1, y1, z1]`, each four corners `[x, y,
/// z, u, v]` wound counter-clockwise seen from outside, with the shade
/// vanilla gives that face.
fn box_faces(b: [f32; 6]) -> Vec<([[f32; 5]; 4], u32)> {
    let [x0, y0, z0, x1, y1, z1] = b;
    vec![
        // Up and down: u along x, v along z.
        ([[x0, y1, z0, x0, z0], [x0, y1, z1, x0, z1], [x1, y1, z1, x1, z1], [x1, y1, z0, x1, z0]], 255),
        ([[x0, y0, z0, x0, z0], [x1, y0, z0, x1, z0], [x1, y0, z1, x1, z1], [x0, y0, z1, x0, z1]], 127),
        // North and south: u along x, v down y.
        ([[x0, y0, z0, x0, 1.0 - y0], [x0, y1, z0, x0, 1.0 - y1], [x1, y1, z0, x1, 1.0 - y1], [x1, y0, z0, x1, 1.0 - y0]], 204),
        ([[x0, y0, z1, x0, 1.0 - y0], [x1, y0, z1, x1, 1.0 - y0], [x1, y1, z1, x1, 1.0 - y1], [x0, y1, z1, x0, 1.0 - y1]], 204),
        // West and east: u along z.
        ([[x0, y0, z0, z0, 1.0 - y0], [x0, y0, z1, z1, 1.0 - y0], [x0, y1, z1, z1, 1.0 - y1], [x0, y1, z0, z0, 1.0 - y1]], 153),
        ([[x1, y0, z0, z0, 1.0 - y0], [x1, y1, z0, z0, 1.0 - y1], [x1, y1, z1, z1, 1.0 - y1], [x1, y0, z1, z1, 1.0 - y0]], 153),
    ]
}
