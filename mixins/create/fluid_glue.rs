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
    BasinTanks, FluidAccess, FluidHandler, FluidStack, FluidTank, FluidWorld, Fluids, PipeBlock, Space,
    SpaceChange, TANK_CAPACITY, VanillaTarget, basin_fluid_boxes, bucket_fluid, filled_bucket, load_tanks,
    save_tanks, tank_fluid_box,
};
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
pub(super) const TANK: &str = "create:fluid_tank";
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
    tanks: HashMap<String, HashMap<Pos, FluidTank>>,
    loaded: HashSet<String>,
    root: Option<PathBuf>,
    /// Basins' tanks as their last tick saw them (input tanks, then output
    /// tanks: `BasinRenderer`'s order), by dimension.
    basins: HashMap<String, HashMap<Pos, Vec<FluidStack>>>,
    /// What clients were last told, by dimension.
    shown: HashMap<String, HashMap<Pos, Shown>>,
    ticks: u64,
}

/// A fluid holder as clients draw it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Shown {
    Tank(FluidStack),
    Basin(Vec<FluidStack>),
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// Block changes heard (`native_block_changed`), for their level's next
/// tick: their own lock, as our own tick changes blocks while holding the
/// state. Whether the block was one of ours.
static PENDING: Mutex<Vec<(String, Pos, bool)>> = Mutex::new(Vec::new());
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
    name == PIPE || name == TANK
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
        // Straight pipes on a chain's template: open at both ends of its axis.
        PIPE => s.properties.get("axis").and_then(|a| Axis::parse(a)).map(PipeBlock::straight),
        PUMP => s.properties.get("facing").and_then(|f| Dir::parse(f)).map(PipeBlock::Pump),
        _ => None,
    }
}

fn is_tank(game: &Game<'_>, pos: Pos) -> bool {
    block_name(game, pos).as_deref() == Some(TANK)
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
        let tanks = s.tanks.get(dimension).map(save_tanks).unwrap_or_default();
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
    tanks: &'a mut HashMap<Pos, FluidTank>,
}

impl Access<'_, '_> {
    /// The tank at `pos` (a new, empty one if it was never filled).
    fn tank(&mut self, pos: Pos) -> Option<&mut FluidTank> {
        if !is_tank(self.game, pos) {
            return None;
        }
        Some(self.tanks.entry(pos).or_insert_with(|| FluidTank::new(TANK_CAPACITY)))
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
        if let Some(t) = self.tank(pos) {
            return Some(op(t));
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
    fn has_handler(&self, pos: Pos, _: Dir) -> bool {
        // FluidTankBlockEntity's and BasinBlockEntity's capabilities, on
        // every side.
        is_tank(self.game, pos) || is_basin(self.game, pos)
    }
    fn handler_tanks(&self, pos: Pos, _: Dir) -> Vec<FluidStack> {
        if is_tank(self.game, pos) {
            return vec![self.tanks.get(&pos).map(|t| t.fluid.clone()).unwrap_or_default()];
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
                s.tanks.insert(dimension.to_owned(), load_tanks(&text));
            }
        }
        s.ticks += 1;
        let ticks = s.ticks;
        let State {
            dims, tanks, shown, basins, ..
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
            if !is_tank(world.game, pos) {
                world.tanks.remove(&pos);
            }
            fluids.block_changed(&mut world, pos);
        }
        for &(pos, _, speed) in pumps {
            fluids.pump_speed(&mut world, pos, speed);
        }
        fluids.tick(&mut world);
        HAS_PIPES.store(
            dims.values().any(|f| !f.transports.is_empty()),
            Ordering::Relaxed,
        );
        let basins = basins.entry(dimension.to_owned()).or_default();
        let shown = shown.entry(dimension.to_owned()).or_default();
        message = updates(game, dimension, tanks, basins, shown, ticks);
        if ticks % SAVE_EVERY == 0 {
            save(s);
        }
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

/// The tanks and basins whose contents changed since clients were told
/// (all of them every `SNAPSHOT_EVERY` ticks), as one F1 message.
fn updates(
    game: &Game<'_>,
    dimension: &str,
    tanks: &mut HashMap<Pos, FluidTank>,
    basins: &mut HashMap<Pos, Vec<FluidStack>>,
    shown: &mut HashMap<Pos, Shown>,
    ticks: u64,
) -> Option<Vec<u8>> {
    // Tanks that are no tank any more lose their contents.
    tanks.retain(|p, _| !in_world(game, *p) || is_tank(game, *p));
    basins.retain(|p, _| !in_world(game, *p) || is_basin(game, *p));
    let now: HashMap<Pos, Shown> = tanks
        .iter()
        .filter(|(_, t)| !t.fluid.is_empty())
        .map(|(p, t)| (*p, Shown::Tank(t.fluid.clone())))
        .chain(basins.iter().map(|(p, b)| (*p, Shown::Basin(b.clone()))))
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

/// F1: the dimension, whether it replaces all, then per holder its position,
/// kind (0 gone, 1 tank, 2 basin) and fluids (amount, id).
fn encode(dim: u8, full: bool, entries: &[(Pos, Option<Shown>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"F1");
    out.push(dim);
    out.push(u8::from(full));
    for (pos, shown) in entries {
        for v in pos {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let (kind, fluids): (u8, &[FluidStack]) = match shown {
            None => (0, &[]),
            Some(Shown::Tank(f)) => (1, std::slice::from_ref(f)),
            Some(Shown::Basin(b)) => (2, b),
        };
        out.push(kind);
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
    let rest = data.strip_prefix(b"F1")?;
    let (&dim, rest) = rest.split_first()?;
    let (&full, mut rest) = rest.split_first()?;
    let int = |b: &[u8]| i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let mut entries = Vec::new();
    while !rest.is_empty() {
        let head = rest.get(..14)?;
        let pos = [int(&head[0..4]), int(&head[4..8]), int(&head[8..12])];
        let (kind, n) = (head[12], usize::from(head[13]));
        rest = &rest[14..];
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
            1 => Some(Shown::Tank(fluids.into_iter().next()?)),
            2 => Some(Shown::Basin(fluids)),
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
    if !is_tank(game, pos) && !is_basin(game, pos) {
        return false;
    }
    let Some(held) = game.held().map(|s| s.item.to_string()) else {
        return false;
    };
    let full = bucket_fluid(&held);
    if full.is_none() && held != "minecraft:bucket" {
        return false;
    }
    let creative = game.native_acting_player().is_some_and(|p| p.game_mode == 1);
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
    for (pos, shown) in holders {
        let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
        if (corner + glam::DVec3::splat(0.5)).distance_squared(camera) > VIEW_DISTANCE * VIEW_DISTANCE {
            continue;
        }
        let boxes: Vec<(FluidStack, [f32; 6])> = match shown {
            Shown::Tank(fluid) => {
                let fill = fluid.amount as f32 / TANK_CAPACITY as f32;
                tank_fluid_box(fill, 1.0, 1.0).map(|b| (fluid, b)).into_iter().collect()
            }
            Shown::Basin(stacks) => basin_fluid_boxes(&stacks),
        };
        let light = game.world.light(pos[0], pos[1], pos[2]);
        let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
        let origin = (corner - camera).as_vec3();
        for (fluid, b) in boxes {
            draw_fluid_box(game, &fluid.fluid, b, origin, packed, out);
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
    for (corners, shade) in box_faces(b) {
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
