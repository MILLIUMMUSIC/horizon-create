// Horizon's side of Create's logistics, injected as
// `crate::game::create_logistics`: the mechanical belt's items (one
// `BeltInventory` per controller, kept per dimension and saved beside the
// world: belts have no block entity here), what they meet (presses, depots,
// chutes, funnels, the next belt, walls), items dropped on belts, the belt
// connector, the chute (a mod block entity), andesite and brass funnels (no
// block entity: their timers and filters live here), belt tunnels (no block
// entity: a brass tunnel's stack and mode live here), and drawing what
// belts and chutes carry and funnels filter.
// The rules are in belt.rs, chute.rs, funnel.rs and tunnel.rs.

use super::Game;
use super::create_belt::{
    Belt, BeltFunnel, BeltInventory, BeltLevel, BeltState, BeltWorld, Chain, ConnectorLevel,
    Conversion, FunnelShape, ProcessingResult, Slope, Stack, Transported, TunnelAbove, belt_chain,
    belts_between, can_connect, find_chain, item_place, on_removed,
};
use super::create_chute::{Chute, ChuteAround, ChuteShape, ChuteWorld, chute_state, item_motion, max_pull_distance};
use super::create_funnel::{
    BeltBelow, EXTRACTION_TIMER, Filter, FunnelState, Mode, amount_to_extract, as_belt_funnel,
    collects_entity_at, extracted_item, filter_slot, overflow_area, tick_cooldown,
};
use super::create_tunnel::{
    BrassTunnel, SelectionMode, TunnelLevel, TunnelState, TunnelWorld, adjacent, can_survive,
    has_distribution, is_connected, updated_state,
};
use super::create_kinetics::{Axis, Dir, Kind, Pos};
use horizon_client::containers::ModBlockEntity;
use horizon_client::items::ItemStack;
use horizon_world::nbt::{Compound, JavaString, Tag};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

/// Belts' items for clients (`BeltBlockEntity.sendData`).
pub(super) const BELTS: &str = "create:belts";
pub(super) const CHUTE: &str = "create:chute";
const CONNECTOR: &str = "create:belt_connector";
const SHAFT: &str = "create:shaft";
/// `setDefaultPickUpDelay`.
const PICKUP_DELAY: u32 = 10;

struct Entry {
    inv: BeltInventory<ItemStack>,
    /// The belt as it was last ticked: where to eject its items when it
    /// breaks.
    belt: Belt,
}

/// A funnel's `FunnelBlockEntity` here: its extraction timer and, for a
/// brass one, its filter.
#[derive(Clone, Debug, Default)]
struct FunnelData {
    cooldown: i32,
    filter: Filter,
}

/// A tunnel's block entity here (a brass tunnel's stack and mode).
#[derive(Clone, Debug, Default)]
struct TunnelData {
    brass: BrassTunnel<ItemStack>,
    /// Placed since the last tick: its mode goes to the tunnels it joins
    /// (`updateTunnelConnections`).
    fresh: bool,
}

#[derive(Default)]
struct Logistics {
    belts: HashMap<(String, Pos), Entry>,
    /// Funnels by dimension, with their extraction timers and filters.
    funnels: HashMap<(String, Pos), FunnelData>,
    /// Tunnels by dimension.
    tunnels: HashMap<(String, Pos), TunnelData>,
    /// Dimensions read from their save.
    loaded: HashSet<String>,
    /// Belts clients were told hold items.
    shown: HashSet<(String, Pos)>,
    root: Option<PathBuf>,
    random: u64,
    /// A player's first shaft for the belt connector
    /// (`BELT_FIRST_SHAFT`), by player.
    first_shaft: HashMap<u128, (String, Pos)>,
    /// Funnels placed while sneaking, to turn into collecting ones.
    sneak_placed: Vec<(String, Pos)>,
    /// Chutes to look at on the next tick (`AbstractChuteBlock`'s scheduled
    /// ticks and `updateShape`), and chutes placed while sneaking.
    chute_updates: HashSet<(String, Pos)>,
    /// Belt segments the wrench took off (`BeltSlicer`): no chain to break.
    sliced: HashSet<(String, Pos)>,
    sneak_chutes: Vec<(String, Pos)>,
}

static STATE: Mutex<Option<Logistics>> = Mutex::new(None);
/// Each item's `getMaxStackSize`, read from the game once a world: its own
/// lock, as belts ask for it while the state is held.
static MAX_STACK: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

fn state() -> MutexGuard<'static, Option<Logistics>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn with_state<R>(f: impl FnOnce(&mut Logistics) -> R) -> R {
    let mut guard = state();
    f(guard.get_or_insert_with(Logistics::default))
}

impl Logistics {
    fn angle(&mut self) -> i32 {
        let mut x = self.random.max(0x9E37_79B9);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.random = x;
        (x % 360) as i32
    }
}

impl Stack for ItemStack {
    fn count(&self) -> u32 {
        self.count
    }
    fn set_count(&mut self, count: u32) {
        self.count = count;
    }
    fn max_stack(&self) -> u32 {
        let guard = MAX_STACK.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().and_then(|m| m.get(&*self.item).copied()).unwrap_or(64)
    }
}

fn block_name(game: &Game<'_>, pos: Pos) -> Option<String> {
    let w = &game.world;
    if !w.loaded(pos[0], pos[2]) || pos[1] < w.shape.min_y || pos[1] >= w.shape.max_y() {
        return None;
    }
    game.pack.assets.registry.state(w.block(pos[0], pos[1], pos[2])).map(|s| s.name.clone())
}

fn prop(game: &Game<'_>, pos: Pos, key: &str) -> Option<String> {
    let state = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2]))?;
    state.properties.get(key).cloned()
}

pub(super) fn is_belt(name: &str) -> bool {
    super::create_belt::is_belt_block(name)
}

/// Blocks of ours outside the kinetics, whose changes the glue reports.
pub(super) fn is_ours(name: &str) -> bool {
    is_belt(name)
        || is_chute_block(name)
        || super::create_funnel::is_funnel(name)
        || TunnelState::from_block(name, |_| None).is_some()
}

fn belt_at(game: &Game<'_>, pos: Pos) -> Option<BeltState> {
    let w = &game.world;
    if !w.loaded(pos[0], pos[2]) || pos[1] < w.shape.min_y || pos[1] >= w.shape.max_y() {
        return None;
    }
    let state = game.pack.assets.registry.state(w.block(pos[0], pos[1], pos[2]))?;
    BeltState::from_block(&state.name, |k| state.properties.get(k).map(String::as_str))
}

fn funnel_at(game: &Game<'_>, pos: Pos) -> Option<FunnelState> {
    let name = block_name(game, pos)?;
    let facing = prop(game, pos, "facing");
    FunnelState::from_block(&name, |k| if k == "facing" { facing.as_deref() } else { None })
}

fn tunnel_at(game: &Game<'_>, pos: Pos) -> Option<TunnelState> {
    let name = block_name(game, pos)?;
    let axis = prop(game, pos, "axis");
    TunnelState::from_block(&name, |k| if k == "axis" { axis.as_deref() } else { None })
}

/// The filter of the brass funnel at `pos` (an andesite funnel's isn't
/// active: none).
fn funnel_filter(game: &Game<'_>, pos: Pos) -> Option<Filter> {
    funnel_at(game, pos).filter(|f| f.brass)?;
    let dim = game.dim.id().to_owned();
    Some(with_state(|s| s.funnels.get(&(dim, pos)).map(|f| f.filter.clone()).unwrap_or_default()))
}

/// `FilteringBehaviour.test` of the funnel at `pos` for `stack`.
pub(super) fn funnel_accepts(game: &Game<'_>, pos: Pos, stack: &ItemStack) -> bool {
    funnel_filter(game, pos).is_none_or(|f| f.test(&stack.item))
}

/// The belt segment under the funnel at `pos`, as the funnel sees it.
fn belt_under(game: &Game<'_>, pos: Pos) -> Option<BeltBelow> {
    let below = [pos[0], pos[1] - 1, pos[2]];
    belt_at(game, below).filter(|b| b.can_transport_objects()).map(|b| {
        let speed = super::create_glue::with_kinetics(game, |k, level| k.speed_of(level, below));
        BeltBelow {
            facing: b.facing,
            movement: b.movement_facing(speed),
        }
    })
}

/// A belt segment's `getSpeed`.
fn belt_speed(game: &Game<'_>, pos: Pos) -> f32 {
    super::create_glue::with_kinetics(game, |k, level| k.speed_of(level, pos))
}

/// `DirectBeltInputBehaviour.canInsertFromSide` of what is at `pos`: a
/// belt that moves and doesn't come towards `side` (`canInsertFrom`), any
/// other input.
fn can_insert_from_side(game: &Game<'_>, pos: Pos, side: Dir) -> bool {
    if let Some(b) = belt_at(game, pos) {
        let speed = belt_speed(game, pos);
        return b.can_transport_objects() && speed != 0.0 && b.movement_facing(speed) != side.opposite();
    }
    has_direct_input(game, pos)
}

fn is_chute(game: &Game<'_>, pos: Pos) -> bool {
    chute_at(game, pos).is_some()
}

/// The chute's block for each shape (its block entity is `CHUTE` for all).
fn chute_block(shape: ChuteShape) -> &'static str {
    match shape {
        ChuteShape::Normal => CHUTE,
        ChuteShape::Window => "create:chute_window",
        ChuteShape::Intersection => "create:chute_intersection",
        ChuteShape::Encased => "create:chute_encased",
    }
}

pub(super) const SMART_CHUTE: &str = "create:smart_chute";

pub(super) fn is_chute_block(name: &str) -> bool {
    name == SMART_CHUTE || ChuteShape::ALL.iter().any(|s| chute_block(*s) == name)
}

fn is_smart_chute(game: &Game<'_>, pos: Pos) -> bool {
    block_name(game, pos).as_deref() == Some(SMART_CHUTE)
}

/// A smart chute's `FilteringBehaviour` (kept in its block entity's data),
/// and whether it is powered (`canActivate`, noted by its own tick): none
/// for another chute.
fn smart_filter(game: &Game<'_>, pos: Pos) -> Option<(Filter, bool)> {
    use horizon_client::containers::BlockEntity as B;
    if !is_smart_chute(game, pos) {
        return None;
    }
    let Some(B::Mod(m)) = game.world.block_entities.get(&(pos[0], pos[1], pos[2])) else {
        return Some((Filter::default(), false));
    };
    Some(read_smart_filter(m))
}

fn read_smart_filter(m: &ModBlockEntity) -> (Filter, bool) {
    let item = match m.nbt.get(&JavaString::from("Filter")) {
        Some(Tag::String(t)) => Some(t.to_string_lossy()).filter(|t| !t.is_empty()),
        _ => None,
    };
    let count = match m.nbt.get(&JavaString::from("FilterAmount")) {
        Some(Tag::Int(v)) => (*v).clamp(1, 64) as u32,
        _ => 64,
    };
    let powered = matches!(m.nbt.get(&JavaString::from("Powered")), Some(Tag::Byte(1)));
    (Filter { item, count }, powered)
}

/// `canAcceptItem` of the chute at `pos`: empty, and for a smart chute
/// unpowered and passing its filter.
fn chute_accepts(game: &Game<'_>, pos: Pos, stack: &ItemStack) -> bool {
    use horizon_client::containers::BlockEntity as B;
    let empty = match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(B::Mod(m)) => m.items.first().is_none_or(Option::is_none),
        _ => false,
    };
    empty && smart_filter(game, pos).is_none_or(|(f, powered)| !powered && f.test(&stack.item))
}

/// The facing and shape of the chute at `pos` (a template's `up`, before
/// it is set, stands for down).
fn chute_at(game: &Game<'_>, pos: Pos) -> Option<(Dir, ChuteShape)> {
    let name = block_name(game, pos)?;
    // A smart chute is always straight (`AbstractChuteBlock.getFacing`).
    if name == SMART_CHUTE {
        return Some((Dir::Down, ChuteShape::Normal));
    }
    let shape = ChuteShape::ALL.into_iter().find(|s| chute_block(*s) == name)?;
    let facing = prop(game, pos, "facing")
        .and_then(|f| Dir::ALL.into_iter().find(|d| d.name() == f))
        .filter(|d| *d != Dir::Up)
        .unwrap_or(Dir::Down);
    Some((facing, shape))
}

fn below(pos: Pos) -> Pos {
    [pos[0], pos[1] - 1, pos[2]]
}

fn above(pos: Pos) -> Pos {
    [pos[0], pos[1] + 1, pos[2]]
}

fn step(pos: Pos, d: Dir) -> Pos {
    let n = d.normal();
    [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]]
}

/// `getTargetChute`: where a chute facing `facing` sends its items.
fn chute_target(pos: Pos, facing: Dir) -> Pos {
    if facing == Dir::Down { below(pos) } else { step(below(pos), facing.opposite()) }
}

/// `getInputChutes`: the chute above facing down, and those above and
/// beside facing the way they lie.
fn input_chutes(game: &Game<'_>, pos: Pos) -> Vec<Pos> {
    let mut out = Vec::new();
    for d in [Dir::Up, Dir::North, Dir::South, Dir::West, Dir::East] {
        let facing = d.opposite();
        let at = if facing == Dir::Down { above(pos) } else { step(above(pos), facing) };
        if chute_at(game, at).is_some_and(|(f, _)| f == facing) {
            out.push(at);
        }
    }
    out
}

struct World<'a, 'g> {
    game: &'a Game<'g>,
}

impl BeltLevel for World<'_, '_> {
    fn belt(&self, pos: Pos) -> Option<BeltState> {
        belt_at(self.game, pos)
    }
    fn is_loaded(&self, pos: Pos) -> bool {
        self.game.world.loaded(pos[0], pos[2])
    }
}

impl TunnelLevel for World<'_, '_> {
    fn tunnel(&self, pos: Pos) -> Option<TunnelState> {
        tunnel_at(self.game, pos)
    }
    fn flat_belt_axis(&self, pos: Pos) -> Option<Axis> {
        belt_at(self.game, pos).filter(|b| b.slope == Slope::Horizontal).map(|b| b.facing.axis())
    }
    fn belt_axis(&self, pos: Pos) -> Option<Axis> {
        belt_at(self.game, pos).map(|b| b.facing.axis())
    }
    fn has_direct_input(&self, pos: Pos) -> bool {
        has_direct_input(self.game, pos)
    }
    fn can_insert_from_side(&self, pos: Pos, side: Dir) -> bool {
        can_insert_from_side(self.game, pos, side)
    }
    fn extended_funnel(&self, _: Pos) -> Option<Dir> {
        // Belt funnels here are never extended (`getShapeForPosition`'s
        // upright items aren't ported).
        None
    }
}

pub(super) fn core_dir(d: Dir) -> horizon_core::geometry::Direction {
    use horizon_core::geometry::Direction as D;
    match d {
        Dir::Down => D::Down,
        Dir::Up => D::Up,
        Dir::North => D::North,
        Dir::South => D::South,
        Dir::West => D::West,
        Dir::East => D::East,
    }
}

/// `isFaceSturdy` of the block at `pos` on its side `face`.
fn sturdy(game: &Game<'_>, pos: Pos, face: Dir) -> bool {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    game.res.facts().get(state).sturdy_full & (1 << face as u8) != 0
}

// ---- Inventories ------------------------------------------------------------

/// The slots others may take from the mod block entity `id`
/// (`native_block_entity_container`'s outputs).
fn mod_outputs(id: &str) -> Vec<usize> {
    let mut container = None;
    super::create_machines::block_entity_container(id, &mut container);
    container.map(|c| c.outputs).unwrap_or_default()
}

/// Whether there is an inventory at `pos` a funnel or chute can reach
/// (`Capabilities.ItemHandler.BLOCK`): chests and other containers, mod
/// block entities but chutes, belts.
fn has_inventory(game: &Game<'_>, pos: Pos) -> bool {
    use horizon_client::containers::BlockEntity as B;
    if belt_at(game, pos).is_some_and(BeltState::can_transport_objects) || super::create_arms::has_inventory(game, pos) {
        return true;
    }
    // A tunnel's item handler: the belt below it (`cap`).
    if tunnel_at(game, pos).is_some() {
        return belt_at(game, [pos[0], pos[1] - 1, pos[2]]).is_some_and(BeltState::can_transport_objects);
    }
    match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(B::Mod(m)) => m.id != CHUTE,
        Some(B::Chest(_) | B::Furnace(_) | B::Brewing(_) | B::Crafter(_)) => true,
        _ => false,
    }
}

/// `stack` into the inventory at `pos` through its side `face`: what is
/// left (all of it when there is none).
fn insert_into(game: &mut Game<'_>, pos: Pos, face: Dir, stack: ItemStack) -> Option<ItemStack> {
    // A whole vault, or a deployer's hand.
    if let Some(left) = super::create_arms::insert_into(game, pos, &stack) {
        return left;
    }
    if belt_at(game, pos).is_some() {
        return insert_on_belt_segment(game, pos, stack);
    }
    if let Some(t) = tunnel_at(game, pos) {
        return insert_into_tunnel_block(game, pos, t, stack);
    }
    game.native_insert_item(pos, core_dir(face), stack)
}

/// A tunnel's item handler: the belt below an andesite tunnel (or a brass one
/// that doesn't distribute), else `BrassTunnelItemHandler.insertItem`: the
/// whole stack to distribute when it can take one.
fn insert_into_tunnel_block(game: &mut Game<'_>, pos: Pos, t: TunnelState, stack: ItemStack) -> Option<ItemStack> {
    let below = [pos[0], pos[1] - 1, pos[2]];
    if !t.brass || !super::create_tunnel::has_distribution(&World { game }, pos) {
        if belt_at(game, below).is_none() {
            return Some(stack);
        }
        return insert_on_belt_segment(game, below, stack);
    }
    let dim = game.dim.id().to_owned();
    with_state(|s| {
        let t = s.tunnels.entry((dim, pos)).or_default();
        if !t.brass.can_take() {
            return Some(stack);
        }
        t.brass.set_stack(Some(stack), None);
        None
    })
}

/// `ItemHandlerBeltSegment.insertItem` on the belt segment at `pos`.
fn insert_on_belt_segment(game: &Game<'_>, pos: Pos, stack: ItemStack) -> Option<ItemStack> {
    let Chain::Belt { controller, segments } = find_chain(&World { game }, pos) else {
        return Some(stack);
    };
    let index = segments.iter().position(|p| *p == pos).unwrap_or(0) as i32;
    let dim = game.dim.id().to_owned();
    with_state(|s| {
        let angle = s.angle();
        let Some(e) = s.belts.get_mut(&(dim, controller)) else {
            return Some(stack);
        };
        let belt = e.belt;
        e.inv.insert_at(&belt, index, stack, angle, false)
    })
}

/// `ItemHelper.extract` (`UPTO`) of up to `max` of one item that passes
/// `test` from the inventory at `pos`: the first such item, gathered from
/// every slot holding it, up to a stack (chests and mod block entities'
/// output slots; vaults and deployers slot by slot).
fn extract_from(game: &mut Game<'_>, pos: Pos, max: u32, test: &dyn Fn(&ItemStack) -> bool) -> Option<ItemStack> {
    use horizon_client::containers::BlockEntity as B;
    if let Some(taken) = super::create_arms::extract_from(game, pos, max, test) {
        return taken;
    }
    // Through a tunnel: from the belt below it.
    if tunnel_at(game, pos).is_some() {
        return extract_from(game, [pos[0], pos[1] - 1, pos[2]], max, test);
    }
    // ItemHandlerBeltSegment.extractItem: the item over the segment.
    if belt_at(game, pos).is_some() {
        return with_belt_segment(game, pos, |inv, _, index| {
            inv.extract_at(index, max, true).filter(|s| test(s))?;
            inv.extract_at(index, max, false)
        })
        .flatten();
    }
    let key = (pos[0], pos[1], pos[2]);
    if let Some(B::Mod(m)) = game.world.block_entities.get(&key) {
        if m.id == CHUTE {
            return None;
        }
        let outputs = mod_outputs(&m.id);
        return game
            .native_with_mod_block_entity(pos, |be| {
                let mut slots: Vec<&mut Option<ItemStack>> = be
                    .items
                    .iter_mut()
                    .enumerate()
                    .filter(|(i, _)| outputs.contains(i))
                    .map(|(_, s)| s)
                    .collect();
                gather(&mut slots, max, test)
            })
            .flatten();
    }
    let Some(B::Chest(items)) = game.world.block_entities.get_mut(&key) else {
        return None;
    };
    let mut slots: Vec<&mut Option<ItemStack>> = items.iter_mut().collect();
    let out = gather(&mut slots, max, test)?;
    game.world.modified.insert((pos[0] >> 4, pos[2] >> 4));
    Some(out)
}

/// `ItemHelper.extract`'s gathering: the first stack passing `test`, then
/// more of the same item from later slots, up to `max` and a stack.
fn gather(slots: &mut [&mut Option<ItemStack>], max: u32, test: &dyn Fn(&ItemStack) -> bool) -> Option<ItemStack> {
    let first = slots.iter().position(|s| s.as_ref().is_some_and(test))?;
    let mut out = slots[first].clone()?;
    let cap = max.min(out.max_stack()).max(1);
    out.count = 0;
    for slot in slots.iter_mut().skip(first) {
        let Some(s) = slot.as_mut() else {
            continue;
        };
        let mut probe = s.clone();
        probe.count = out.count;
        if probe != out {
            continue;
        }
        let n = (cap - out.count).min(s.count);
        s.count -= n;
        out.count += n;
        if s.count == 0 {
            **slot = None;
        }
        if out.count >= cap {
            break;
        }
    }
    (out.count > 0).then_some(out)
}

// ---- Belts in the world -------------------------------------------------------

/// The world as one belt's items see it, with the other belts of its
/// dimension (taken out of the state while they tick).
struct BeltsWorld<'a, 'g> {
    game: &'a mut Game<'g>,
    others: &'a mut HashMap<Pos, Entry>,
    belt: Belt,
}

impl BeltWorld<ItemStack> for BeltsWorld<'_, '_> {
    fn received(
        &mut self,
        pos: Pos,
        item: &Transported<ItemStack>,
    ) -> Option<(ProcessingResult, Option<Conversion<ItemStack>>)> {
        let press = [pos[0], pos[1] + 2, pos[2]];
        let hold = super::create_machines::press_received(self.game, press, &item.stack)
            .or_else(|| super::create_arms::deployer_received(self.game, press, &item.stack))?;
        Some((if hold { ProcessingResult::Hold } else { ProcessingResult::Pass }, None))
    }
    fn held(
        &mut self,
        pos: Pos,
        item: &Transported<ItemStack>,
    ) -> Option<(ProcessingResult, Option<Conversion<ItemStack>>)> {
        let press = [pos[0], pos[1] + 2, pos[2]];
        // BeltPressingCallbacks lock their results; BeltDeployerCallbacks
        // set `locked` and then clear it again.
        let (hold, pressed, locked) = match super::create_machines::press_held(self.game, press, &item.stack) {
            Some((hold, pressed)) => (hold, pressed, true),
            None => {
                let (hold, pressed) = super::create_arms::deployer_held(self.game, press, &item.stack)?;
                (hold, pressed, false)
            }
        };
        let result = if hold { ProcessingResult::Hold } else { ProcessingResult::Pass };
        Some((result, pressed.map(|(outputs, held)| Conversion { outputs, held, locked })))
    }
    fn processing_blocked(&self, pos: Pos) -> bool {
        let above = [pos[0], pos[1] + 1, pos[2]];
        funnel_at(self.game, above).is_none() && !self.game.block_collision_shape(above).is_empty()
    }
    fn funnel(&self, pos: Pos) -> Option<BeltFunnel> {
        let f = funnel_at(self.game, pos)?;
        let below = BeltBelow {
            facing: self.belt.state.facing,
            movement: self.belt.movement_facing(),
        };
        as_belt_funnel(f, Some(below))
    }
    fn funnel_insert(&mut self, pos: Pos, stack: &ItemStack) -> Option<ItemStack> {
        let f = funnel_at(self.game, pos)?;
        if !funnel_accepts(self.game, pos, stack) {
            return Some(stack.clone());
        }
        let behind = f.behind(pos);
        if !has_inventory(self.game, behind) {
            return Some(stack.clone());
        }
        let left = insert_into(self.game, behind, f.facing, stack.clone());
        // BeltFunnelInteractionHandler: the flap swings in as it takes.
        if left.as_ref().is_none_or(|l| l.count < stack.count) {
            flap(self.game, pos, f.facing, true);
        }
        left
    }
    fn has_direct_input(&self, pos: Pos) -> bool {
        has_direct_input(self.game, pos)
    }
    fn direct_insert(
        &mut self,
        pos: Pos,
        item: &Transported<ItemStack>,
        side: Dir,
    ) -> Option<Option<ItemStack>> {
        if let Some(segment_state) = belt_at(self.game, pos) {
            let Chain::Belt { controller, segments } = find_chain(&World { game: self.game }, pos) else {
                return None;
            };
            let index = segments.iter().position(|p| *p == pos).unwrap_or(0) as i32;
            let e = self.others.get_mut(&controller)?;
            let belt = e.belt;
            return Some(e.inv.insert_from_side(&belt, segment_state, index, item, side, true));
        }
        direct_insert(self.game, pos, item.stack.clone())
    }
    fn solid_side(&self, pos: Pos, face: Dir) -> bool {
        sturdy(self.game, pos, face)
    }
    fn eject(&mut self, stack: ItemStack, at: [f64; 3], motion: [f64; 3]) {
        self.game.spawn_item(stack, at, motion, PICKUP_DELAY);
    }
    fn tunnel(&self, pos: Pos) -> Option<TunnelAbove> {
        let t = tunnel_at(self.game, pos)?;
        let level = World { game: self.game };
        let dim = self.game.dim.id().to_owned();
        let can_take = with_state(|s| s.tunnels.get(&(dim, pos)).is_none_or(|d| d.brass.can_take()));
        Some(TunnelAbove {
            brass: t.brass,
            axis: t.axis,
            junction: t.shape.is_junction(),
            flaps: t.flaps(pos, &level),
            distributes: t.brass && has_distribution(&level, pos),
            can_take,
        })
    }
    fn tunnel_take(&mut self, pos: Pos, stack: ItemStack, from: Dir) {
        let dim = self.game.dim.id().to_owned();
        with_state(|s| s.tunnels.entry((dim, pos)).or_default().brass.set_stack(Some(stack), Some(from)));
    }
    fn flap_tunnel(&mut self, pos: Pos, side: Dir, inward: bool) {
        if tunnel_at(self.game, pos).is_some() {
            flap(self.game, pos, side, inward);
        }
    }
    fn loaded(&self, pos: Pos) -> bool {
        self.game.world.loaded(pos[0], pos[2])
    }
    fn can_insert_from_side(&self, pos: Pos, side: Dir) -> bool {
        can_insert_from_side(self.game, pos, side)
    }
}

/// A `DirectBeltInputBehaviour` at `pos`: a belt, a depot, a chute open at
/// the top, a collecting funnel facing up.
fn has_direct_input(game: &Game<'_>, pos: Pos) -> bool {
    if belt_at(game, pos).is_some_and(BeltState::can_transport_objects) {
        return true;
    }
    match block_name(game, pos).as_deref() {
        Some(super::create_machines::DEPOT) => true,
        // canDirectlyInsert: an open chute not under one facing down.
        Some(name) if is_chute_block(name) => {
            chute_at(game, pos).is_some_and(|(f, s)| super::create_chute::is_open(f, s))
                && chute_at(game, above(pos)).is_none_or(|(f, _)| f != Dir::Down)
        }
        _ => funnel_at(game, pos).is_some_and(FunnelState::takes_from_belt_end),
    }
}

/// `handleInsertion` of what isn't a belt (a depot, chute or funnel):
/// what is left, none when it can't take anything.
fn direct_insert(game: &mut Game<'_>, pos: Pos, stack: ItemStack) -> Option<Option<ItemStack>> {
    match block_name(game, pos).as_deref() {
        Some(super::create_machines::DEPOT) => {
            // DepotBehaviour.tryInsertingFromSide: an empty depot only.
            let max = stack.max_stack();
            game.native_with_mod_block_entity(pos, |be| {
                if be.items.iter().any(Option::is_some) {
                    return None;
                }
                if be.items.is_empty() {
                    be.items.push(None);
                }
                let mut held = stack.clone();
                let left = (held.count > max).then(|| {
                    let mut l = held.clone();
                    l.count -= max;
                    l
                });
                held.count = held.count.min(max);
                be.items[0] = Some(held);
                Some(left)
            })
            .flatten()
        }
        // ChuteItemHandler.insertItem: what `canAcceptItem` takes.
        Some(name) if is_chute_block(name) && !chute_accepts(game, pos, &stack) => None,
        Some(name) if is_chute_block(name) => game
            .native_with_mod_block_entity(pos, |be| {
                if be.items.first().is_some_and(Option::is_some) {
                    return None;
                }
                set_chute_item(be, Some(stack.clone()), 1.0);
                Some(None)
            })
            .flatten(),
        _ => {
            let f = funnel_at(game, pos)?;
            if !f.takes_from_belt_end() || !funnel_accepts(game, pos, &stack) {
                return None;
            }
            let behind = f.behind(pos);
            if !has_inventory(game, behind) {
                return None;
            }
            let left = insert_into(game, behind, f.facing, stack.clone());
            (left.as_ref() != Some(&stack)).then_some(left)
        }
    }
}

/// The level's belts after the kinetics' tick: chains checked, items moved
/// and handed on, items dropped on belts taken, what clients see.
/// `belts`: each controller (a start segment), its state and speed.
pub(super) fn belts_tick(game: &mut Game<'_>, dimension: &str, belts: &[(Pos, BeltState, f32)], full: bool) {
    load(game, dimension);
    {
        let mut guard = MAX_STACK.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(game.items().items.iter().map(|(k, v)| (k.to_string(), v.max_stack)).collect());
        }
    }
    let mut taken: HashMap<Pos, Entry> = with_state(|s| {
        let keys: Vec<(String, Pos)> = s.belts.keys().filter(|(d, _)| d == dimension).cloned().collect();
        keys.into_iter()
            .filter_map(|k| s.belts.remove(&k).map(|e| (k.1, e)))
            .collect()
    });
    let mut live = HashSet::new();
    let mut destroy = Vec::new();
    for &(pos, state, speed) in belts {
        // The kinetics learn of a removed controller a tick late: what is
        // there now (a shaft from `belts_removed`) is no belt to break.
        if belt_at(game, pos).is_none_or(|b| b.part != super::create_belt::Part::Start) {
            continue;
        }
        let chain = belt_chain(&World { game }, pos);
        if chain.len() < 2 {
            destroy.push(pos);
            continue;
        }
        live.insert(pos);
        let belt = Belt {
            controller: pos,
            state,
            length: chain.len() as i32,
            speed,
        };
        let e = taken.entry(pos).or_insert_with(|| Entry {
            inv: BeltInventory::new(),
            belt,
        });
        e.belt = belt;
    }
    // A controller that is gone (its belt broke): its items drop
    // (`ejectAll`).
    let gone: Vec<Pos> = taken
        .keys()
        .filter(|p| game.world.loaded(p[0], p[2]) && !live.contains(*p))
        .copied()
        .collect();
    for p in gone {
        if let Some(mut e) = taken.remove(&p) {
            let belt = e.belt;
            let mut w = BeltsWorld {
                game,
                others: &mut HashMap::new(),
                belt,
            };
            e.inv.eject_all(&belt, &mut w);
        }
    }
    let controllers: Vec<Pos> = live.iter().copied().collect();
    for p in controllers {
        let Some(mut e) = taken.remove(&p) else {
            continue;
        };
        let belt = e.belt;
        let mut w = BeltsWorld {
            game,
            others: &mut taken,
            belt,
        };
        e.inv.tick(&belt, &mut w);
        taken.insert(p, e);
    }
    take_dropped(game, &mut taken);
    carry_mobs(game, dimension);
    let message = belt_updates(dimension, &mut taken, full);
    with_state(|s| {
        for (p, e) in taken {
            s.belts.insert((dimension.to_owned(), p), e);
        }
    });
    for [x, y, z] in destroy {
        game.native_run_command(&format!("execute in {dimension} run setblock {x} {y} {z} air destroy"));
    }
    if let Some(m) = message {
        for player in game.native_server_players() {
            if player.dimension == dimension
                && let Err(e) = game.native_net_send_to_player(player.uuid, BELTS, &m)
            {
                log::debug!("Create: belts to {}: {e}", player.name);
            }
        }
    }
}

/// The belt segment something with its feet at `at` stands on, and the
/// segment's place.
fn belt_standing(game: &Game<'_>, at: [f64; 3]) -> Option<(Pos, BeltState)> {
    let x = at[0].floor() as i32;
    let z = at[2].floor() as i32;
    let y = at[1].floor() as i32;
    [y, y - 1].into_iter().find_map(|y| belt_at(game, [x, y, z]).map(|b| ([x, y, z], b)))
}

/// Whether moving to `to` would walk into a block (one that isn't a belt).
fn belt_blocked(game: &Game<'_>, to: [f64; 3]) -> bool {
    let p = [to[0].floor() as i32, (to[1] + 0.05).floor() as i32, to[2].floor() as i32];
    if belt_at(game, p).is_some() {
        return false;
    }
    let state = game.world.block(p[0], p[1], p[2]);
    !game.world.is_air(state) && game.res.facts().get(state).sturdy_full != 0
}

/// `BeltMovementHandler.transportEntity` for the level's mobs.
fn carry_mobs(game: &mut Game<'_>, dimension: &str) {
    let moves: Vec<(u32, [f64; 3])> = game
        .native_mobs(dimension)
        .into_iter()
        .filter_map(|m| {
            let (pos, state) = belt_standing(game, m.pos)?;
            let speed = belt_speed(game, pos);
            let motion = super::create_belt::entity_motion(state, pos, speed, m.pos, false)?;
            let mine = game.mobs.iter().find(|x| x.id == m.id).map(|x| aabb(x.bounds()))?;
            if rider_blocked(game, mine, motion, speed, Some(m.id)) {
                return None;
            }
            let to = [0, 1, 2].map(|i| m.pos[i] + motion[i]);
            (!belt_blocked(game, to)).then_some((m.id, to))
        })
        .collect();
    for (id, to) in moves {
        game.native_teleport_mob(dimension, id, to);
    }
}

fn aabb(b: horizon_core::geometry::Aabb) -> [f64; 6] {
    let (lo, hi) = (b.min_position(), b.max_position());
    [lo.x, lo.y, lo.z, hi.x, hi.y, hi.z]
}

/// `BeltMovementHandler`'s entity collisions: another mob (or the window's
/// player, for a mob) in the way stops a rider on a slow belt (`mob` the
/// rider itself, none for the player).
fn rider_blocked(game: &Game<'_>, mine: [f64; 6], motion: [f64; 3], speed: f32, mob: Option<u32>) -> bool {
    let Some(ahead) = super::create_belt::blocking_box(mine, motion, speed) else {
        return false;
    };
    let meets = |b: horizon_core::geometry::Aabb| super::create_belt::boxes_meet(ahead, aabb(b));
    game.mobs.iter().any(|m| Some(m.id) != mob && meets(m.bounds()))
        || (mob.is_some() && game.player.as_ref().is_some_and(|p| meets(p.bounds())))
}

/// `BeltMovementHandler` for the window's own player (as Create moves it on
/// its client): not while sneaking, not centred while walking.
pub(super) fn belt_client_tick(game: &mut Game<'_>) {
    use winit::keyboard::KeyCode as K;
    if !game.native_window() || game.keys.contains(&K::ShiftLeft) {
        return;
    }
    let Some(p) = game.player.as_ref() else {
        return;
    };
    let at = [p.position.x, p.position.y, p.position.z];
    let Some((pos, state)) = belt_standing(game, at) else {
        return;
    };
    let Some(speed) = super::create_glue::client_speed(pos) else {
        return;
    };
    let walking = [K::KeyW, K::KeyA, K::KeyS, K::KeyD].iter().any(|k| game.keys.contains(k));
    let Some(motion) = super::create_belt::entity_motion(state, pos, speed, at, walking) else {
        return;
    };
    let mine = aabb(p.bounds());
    if rider_blocked(game, mine, motion, speed, None) {
        return;
    }
    let to = [0, 1, 2].map(|i| at[i] + motion[i]);
    if belt_blocked(game, to) {
        return;
    }
    let Some(p) = game.player.as_mut() else {
        return;
    };
    p.position.x = to[0];
    p.position.y = to[1];
    p.position.z = to[2];
    p.fall_distance = 0.0;
    p.ground.on_ground = true;
}

/// `BeltBlock.updateEntityAfterFallOn` and `entityInside` for item
/// entities: on a belt (or resting on top of one), not rising, they go onto
/// its segment.
fn take_dropped(game: &mut Game<'_>, belts: &mut HashMap<Pos, Entry>) {
    let mut changed = false;
    for i in 0..game.dropped.len() {
        let d = &game.dropped[i];
        if d.stack.count == 0 || d.velocity.y > 0.0 {
            continue;
        }
        let p = [d.position.x.floor() as i32, d.position.y.floor() as i32, d.position.z.floor() as i32];
        let below = [p[0], p[1] - 1, p[2]];
        let on_ground = d.on_ground;
        let segment = if belt_at(game, p).is_some() {
            p
        } else if on_ground && belt_at(game, below).is_some() {
            below
        } else {
            continue;
        };
        if !belt_at(game, segment).is_some_and(BeltState::can_transport_objects) {
            continue;
        }
        let Chain::Belt { controller, segments } = find_chain(&World { game }, segment) else {
            continue;
        };
        let Some(e) = belts.get_mut(&controller) else {
            continue;
        };
        let index = segments.iter().position(|q| *q == segment).unwrap_or(0) as i32;
        let angle = with_state(Logistics::angle);
        let stack = game.dropped[i].stack.clone();
        let belt = e.belt;
        let left = e.inv.insert_at(&belt, index, stack, angle, false);
        let n = left.map_or(0, |l| l.count);
        if n != game.dropped[i].stack.count {
            game.dropped[i].stack.count = n;
            changed = true;
        }
    }
    if changed {
        game.dropped.retain(|d| d.stack.count > 0);
    }
}

/// What a belt segment's controller is and where along it the segment is.
fn segment_of(game: &Game<'_>, pos: Pos) -> Option<(Pos, i32)> {
    let Chain::Belt { controller, segments } = find_chain(&World { game }, pos) else {
        return None;
    };
    Some((controller, segments.iter().position(|p| *p == pos)? as i32))
}

/// `BeltBlock.onRemove` for segments of ours that changed into something
/// else: the rest of the chain becomes shafts (at pulleys) and air. Create
/// walks the chain within the one removal (each `setBlock` calls `onRemove`
/// again); so does this, or `belts_tick` would find a controller whose
/// chain is too short and break it (`initBelt`) before its pulley turns
/// into a shaft.
pub(super) fn belts_removed(game: &mut Game<'_>, removed: &[(Pos, BeltState)]) {
    let dimension = game.dim.id().to_owned();
    let registry = game.pack.assets.registry;
    let air = registry.default_state("minecraft:air");
    let shaft = registry.default_state(SHAFT);
    let mut queue: Vec<(Pos, BeltState)> = removed.to_vec();
    let mut done: HashSet<Pos> = removed.iter().map(|(p, _)| *p).collect();
    let sliced: Vec<Pos> = with_state(|s| {
        removed.iter().filter(|(p, _)| s.sliced.remove(&(dimension.clone(), *p))).map(|(p, _)| *p).collect()
    });
    queue.retain(|(p, _)| !sliced.contains(p));
    while let Some((pos, state)) = queue.pop() {
        for (p, axis) in on_removed(&World { game }, state, pos) {
            if !done.insert(p) {
                continue;
            }
            let Some(old) = belt_at(game, p) else {
                continue;
            };
            let target = match axis {
                Some(a) => shaft.and_then(|s| game.with_props(s, &[("axis", a.name())])),
                None => air,
            };
            if let Some(t) = target {
                game.native_set_block(&dimension, p, t, super::native_hooks::flags::UPDATE_ALL);
                queue.push((p, old));
            }
        }
    }
}

// ---- Clients ------------------------------------------------------------------

/// `B1`, whether it is everything, then per belt its controller, its
/// length (0: gone) and its items: position, previous position, side
/// offset, angle, count and item id.
fn belt_updates(dimension: &str, belts: &mut HashMap<Pos, Entry>, full: bool) -> Option<Vec<u8>> {
    let mut out = b"B1".to_vec();
    out.push(u8::from(full));
    let mut any = false;
    let mut shown = with_state(|s| std::mem::take(&mut s.shown));
    let mut was: HashSet<Pos> = shown
        .iter()
        .filter(|(d, _)| d == dimension)
        .map(|(_, p)| *p)
        .collect();
    let mut entries: Vec<(&Pos, &mut Entry)> = belts.iter_mut().collect();
    entries.sort_by_key(|(p, _)| **p);
    for (pos, e) in entries {
        let moving = e.belt.speed != 0.0 && !e.inv.items.is_empty();
        let send = full || e.inv.dirty || moving || (was.contains(pos) && e.inv.items.is_empty());
        e.inv.dirty = false;
        was.remove(pos);
        if !send {
            continue;
        }
        if out.len() > 28_000 {
            break;
        }
        any = true;
        for v in pos {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let items = &e.inv.items;
        out.push(e.belt.length.clamp(1, 255) as u8);
        out.push(items.len().min(255) as u8);
        for t in items.iter().take(255) {
            for f in [t.belt_position, t.prev_belt_position, t.side_offset] {
                out.extend_from_slice(&f.to_le_bytes());
            }
            out.extend_from_slice(&(t.angle.rem_euclid(360) as u16).to_le_bytes());
            out.push(t.stack.count.min(255) as u8);
            let id = t.stack.item.as_bytes();
            out.push(id.len().min(255) as u8);
            out.extend_from_slice(&id[..id.len().min(255)]);
        }
        let key = (dimension.to_owned(), *pos);
        if items.is_empty() {
            shown.remove(&key);
        } else {
            shown.insert(key);
        }
    }
    // Belts that are gone.
    for p in was {
        any = true;
        for v in p {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.push(0);
        out.push(0);
        shown.remove(&(dimension.to_owned(), p));
    }
    with_state(|s| s.shown = shown);
    (any || full).then_some(out)
}

#[derive(Clone, Debug)]
struct ClientItem {
    position: f32,
    prev: f32,
    side: f32,
    angle: u16,
    count: u32,
    id: String,
}

static CLIENT: Mutex<Option<HashMap<Pos, (Vec<ClientItem>, i32, Instant)>>> = Mutex::new(None);

/// `native_net_from_server` on the belts' channel.
pub(super) fn from_server(data: &[u8]) {
    let Some(rest) = data.strip_prefix(b"B1") else {
        return;
    };
    let Some((&full, mut rest)) = rest.split_first() else {
        return;
    };
    let mut guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    let view = guard.get_or_insert_with(HashMap::new);
    if full != 0 {
        view.clear();
    }
    let now = Instant::now();
    let float = |b: &[u8]| f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    while rest.len() >= 14 {
        let int = |b: &[u8]| i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let pos = [int(&rest[0..4]), int(&rest[4..8]), int(&rest[8..12])];
        let length = i32::from(rest[12]);
        let n = rest[13] as usize;
        rest = &rest[14..];
        let mut items = Vec::new();
        for _ in 0..n {
            if rest.len() < 16 || rest.len() < 16 + rest[15] as usize {
                return;
            }
            let len = rest[15] as usize;
            items.push(ClientItem {
                position: float(&rest[0..4]),
                prev: float(&rest[4..8]),
                side: float(&rest[8..12]),
                angle: u16::from_le_bytes([rest[12], rest[13]]),
                count: u32::from(rest[14]),
                id: String::from_utf8_lossy(&rest[16..16 + len]).into_owned(),
            });
            rest = &rest[16 + len..];
        }
        if length == 0 {
            view.remove(&pos);
        } else {
            view.insert(pos, (items, length, now));
        }
    }
}

/// `BeltRenderer.renderItems`: each belt's items where they are along it,
/// moved on between the server's updates.
pub(super) fn draw_belts(game: &Game<'_>, camera: glam::DVec3, out: &mut Vec<horizon_client::mesher::Vertex>) {
    let guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    let Some(view) = guard.as_ref() else {
        return;
    };
    for (pos, (items, length, at)) in view {
        let Some(state) = belt_at(game, *pos) else {
            continue;
        };
        let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
        if corner.distance_squared(camera) > 80.0 * 80.0 {
            continue;
        }
        // Partial ticks since the update came.
        let partial = (at.elapsed().as_secs_f32() * 20.0).min(1.0);
        for (i, t) in items.iter().enumerate() {
            if t.count == 0 || t.id.is_empty() {
                continue;
            }
            let offset = t.prev + (t.position - t.prev) * partial;
            let (p, tilt) = item_place(*pos, state, *length, offset, t.side);
            let segment = offset.floor() as i32;
            let lit = super::create_belt::position_for_offset(*pos, state, *length, segment.clamp(0, length - 1));
            let light = game.world.light(lit[0], lit[1], lit[2]);
            let along_x = state.facing.axis() == Axis::X;
            let tilt_axis = if along_x { glam::Vec3::Z } else { glam::Vec3::X };
            let at = glam::DVec3::from_array(p);
            let stack = ItemStack::new(&t.id, t.count);
            let owner = super::item_draw::owner_at(p, 0.0, "create:belt");
            let resolved = game.resolve_item(
                &stack,
                horizon_assets::item_model::DisplayContext::Fixed,
                Some(&owner),
                pos[0] ^ pos[2] ^ i as i32,
            );
            // isGui3d: block items stand, others lie flat; upright ones
            // (`create:upright_on_belt`) stand facing the camera, untilted.
            let block = game.pack.assets.registry.default_state(&t.id).is_some();
            let upright = game.items().item_in_tag("create:upright_on_belt", &t.id);
            let copies = if at.distance(camera) < 16.0 { super::create_belt::stacked_copies(t.count) } else { 0 };
            let mut random = super::create_belt::JavaRandom::new(i64::from(t.angle));
            let mut base = glam::Mat4::from_translation((at - camera).as_vec3());
            if upright {
                let diff = at - camera;
                let y_rot = diff.x.atan2(diff.z) as f32 + std::f32::consts::PI;
                base = base
                    * glam::Mat4::from_rotation_y(y_rot)
                    * glam::Mat4::from_translation(glam::Vec3::new(0.0, 3.0 / 32.0, 1.0 / 16.0));
            } else {
                base *= glam::Mat4::from_axis_angle(tilt_axis, tilt.to_radians());
            }
            for copy in 0..=copies {
                let mut m = base * glam::Mat4::from_rotation_y((f32::from(t.angle)).to_radians());
                if !block && !upright {
                    m = m
                        * glam::Mat4::from_translation(glam::Vec3::new(0.0, -0.09375, 0.0))
                        * glam::Mat4::from_rotation_x(90f32.to_radians());
                }
                if block {
                    let x = random.next_float() * 0.0625 * copy as f32;
                    let z = random.next_float() * 0.0625 * copy as f32;
                    m *= glam::Mat4::from_translation(glam::Vec3::new(x, 0.0, z));
                }
                m *= glam::Mat4::from_scale(glam::Vec3::splat(0.5));
                game.submit_item_world(&resolved, m, horizon_client::block_entity_render::vertex_light(light), out);
                if upright {
                    base *= glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.0, -1.0 / 16.0));
                } else {
                    if !block {
                        base *= glam::Mat4::from_rotation_y(10f32.to_radians());
                    }
                    let up = if block { 1.0 / 64.0 } else { 1.0 / 16.0 };
                    base *= glam::Mat4::from_translation(glam::Vec3::new(0.0, up, 0.0));
                }
            }
        }
    }
}

/// `ChuteRenderer`: the chute's item at its place, lying flat at half size.
pub(super) fn draw_chute(
    game: &Game<'_>,
    camera: glam::DVec3,
    pos: Pos,
    slots: &[(String, u32)],
    out: &mut Vec<horizon_client::mesher::Vertex>,
) {
    let smart = is_smart_chute(game, pos);
    if smart {
        // SmartChuteBlock's blockstate: its model by `POWERED`.
        let powered = slots.get(3).is_some_and(|(_, f)| f & 32 != 0);
        let holder = if powered {
            "create:hz_part_smart_chute_powered"
        } else {
            "create:hz_part_smart_chute"
        };
        if let Some(state) = game.pack.assets.registry.default_state(holder) {
            let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
            let light = [[0, 1, 0], [0, -1, 0], [1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]]
                .iter()
                .map(|d| game.world.light(pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]))
                .fold(0u8, |a, l| (a & 0xF0).max(l & 0xF0) | (a & 0x0F).max(l & 0x0F));
            let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
            let seed = horizon_core::random::position_seed(horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]));
            let m = glam::Mat4::from_translation((corner - camera).as_vec3());
            game.block_model_vertices(state, seed, m, packed, false, out);
        }
    }
    if smart && let Some((filter, _)) = slots.get(2).filter(|(id, _)| !id.is_empty()) {
        draw_smart_chute_filter(game, camera, pos, filter, out);
    }
    let Some((id, count)) = slots.first() else {
        return;
    };
    if *count == 0 || id.is_empty() {
        return;
    }
    let place = slots.get(1).map_or(0.5, |(_, p)| *p as f32 / 200.0);
    // ChuteRenderer: straight chutes only, in a window or pulled in below;
    // SmartChuteRenderer: at its bottom only.
    match chute_at(game, pos) {
        _ if smart && place > 0.0 => return,
        Some((Dir::Down, ChuteShape::Window)) => {}
        Some((Dir::Down, _)) if place <= 0.5 => {}
        _ => return,
    }
    let centre = glam::DVec3::new(
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]) + f64::from(place),
        f64::from(pos[2]) + 0.5,
    );
    if centre.distance_squared(camera) > 64.0 * 64.0 {
        return;
    }
    let light = game.world.light(pos[0], pos[1], pos[2]);
    let m = glam::Mat4::from_translation((centre - camera).as_vec3()) * glam::Mat4::from_scale(glam::Vec3::splat(0.5));
    let stack = ItemStack::new(id, *count);
    let owner = super::item_draw::owner_at(centre.to_array(), 0.0, CHUTE);
    let state = game.resolve_item(&stack, horizon_assets::item_model::DisplayContext::Fixed, Some(&owner), pos[0] ^ pos[2]);
    game.submit_item_world(&state, m, horizon_client::block_entity_render::vertex_light(light), out);
}

/// FilteringRenderer on a smart chute: its filter in the slot on each side
/// (`SmartChuteFilterSlotPositioning`: 8, 11, 15.5 turned to the side).
fn draw_smart_chute_filter(
    game: &Game<'_>,
    camera: glam::DVec3,
    pos: Pos,
    id: &str,
    out: &mut Vec<horizon_client::mesher::Vertex>,
) {
    let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
    if corner.distance_squared(camera) > 32.0 * 32.0 {
        return;
    }
    let light = game.world.light(pos[0], pos[1], pos[2]);
    // isGui3d: block items stand at full size, others lie flat at half.
    let block = game.pack.assets.registry.default_state(id).is_some();
    let scale = if block { 1.0 } else { 0.5 } + 1.0 / 64.0;
    let z_offset = if block { 0.0 } else { -0.15 };
    let stack = ItemStack::new(id, 1);
    for side in [Dir::North, Dir::South, Dir::West, Dir::East] {
        let angle = side.horizontal_angle();
        let at = super::create_funnel::rotate_centered_y([0.5, 11.0 / 16.0, 15.5 / 16.0], angle);
        let centre = corner + glam::DVec3::new(f64::from(at[0]), f64::from(at[1]), f64::from(at[2]));
        let m = glam::Mat4::from_translation((centre - camera).as_vec3())
            * glam::Mat4::from_rotation_y((angle + 180.0).to_radians())
            * glam::Mat4::from_scale(glam::Vec3::splat(0.5 * scale))
            * glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.0, z_offset));
        let owner = super::item_draw::owner_at(centre.to_array(), 0.0, SMART_CHUTE);
        let resolved =
            game.resolve_item(&stack, horizon_assets::item_model::DisplayContext::Fixed, Some(&owner), pos[0] ^ pos[2]);
        game.submit_item_world(&resolved, m, horizon_client::block_entity_render::vertex_light(light), out);
    }
}

// ---- Players ------------------------------------------------------------------

/// `native_right_click_block` on a belt: an empty hand takes the items on
/// the segment (`BeltBlock.useItemOn`), a shaft on a middle segment makes a
/// pulley. True for every belt: the template's own menu never opens.
pub(super) fn right_click(game: &mut Game<'_>, pos: Pos, face: [i32; 3]) -> bool {
    // ChuteBlock.useItemOn: industrial iron encases it (and stays in hand).
    if right_click_smart_chute(game, pos, face) {
        return true;
    }
    if let Some((facing, shape)) = chute_at(game, pos).filter(|_| !is_smart_chute(game, pos))
        && game.held().is_some_and(|h| &*h.item == "create:industrial_iron_block")
    {
        if let Some(next) = super::create_chute::encased(shape) {
            set_chute(game, pos, facing, next);
            return true;
        }
        return false;
    }
    if right_click_filter(game, pos, face) || right_click_tunnel(game, pos) || right_click_funnel(game, pos, face) {
        return true;
    }
    let Some(state) = belt_at(game, pos) else {
        return false;
    };
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    let mode = game.native_acting_player().map_or(0, |p| p.game_mode);
    if sneaking || mode == 2 || mode == 3 {
        return true;
    }
    let held = game.held().cloned();
    // applyColor: a dye colours the whole belt, water washes it (the item
    // isn't used up).
    // GenericItemEmptying: a water bottle empties water too
    // (`PotionFluidHandler`: a plain potion of water).
    let water_bottle = held
        .as_ref()
        .is_some_and(|h| &*h.item == "minecraft:potion" && h.potion() == Some("minecraft:water"));
    let dye = held
        .as_ref()
        .and_then(|h| super::create_belt::belt_dye(&h.item))
        .or(water_bottle.then_some(0));
    if let Some(dye) = dye {
        if let Chain::Belt { segments, .. } = find_chain(&World { game }, pos) {
            super::create_glue::with_kinetics(game, |k, _| {
                for p in &segments {
                    let casing = super::create_belt::material_casing(k.get(*p).map_or(0, |e| e.material));
                    k.set_wheel_material(*p, super::create_belt::belt_material(dye, casing));
                }
            });
        }
        return true;
    }
    // BeltSlicer.useConnector: the connector on a belt's end extends it.
    if held.as_ref().is_some_and(|h| &*h.item == CONNECTOR) {
        extend_belt(game, pos, state, face);
        return true;
    }
    // setCasingType: a casing on this segment (the block isn't used up).
    if let Some(casing) = held.as_ref().and_then(|h| super::create_belt::belt_casing(&h.item)) {
        set_belt_casing(game, pos, casing);
        return true;
    }
    match held {
        None => {
            let Some((controller, index)) = segment_of(game, pos) else {
                return true;
            };
            let dim = game.dim.id().to_owned();
            let taken = with_state(|s| {
                s.belts
                    .get_mut(&(dim, controller))
                    .map(|e| e.inv.take_within(index as f32 + 0.5, 0.55))
                    .unwrap_or_default()
            });
            for s in taken {
                game.give_or_drop(s);
            }
        }
        Some(stack) if &*stack.item == SHAFT && state.part == super::create_belt::Part::Middle => {
            let pulley = BeltState {
                part: super::create_belt::Part::Pulley,
                ..state
            };
            if set_belt(game, pos, pulley) && mode != 1
                && let Some(h) = game.held_mut().as_mut()
            {
                h.count -= 1;
                if h.count == 0 {
                    *game.held_mut() = None;
                }
            }
        }
        // BeltBlock.useItemOn passes other items to their own use: a block
        // item is placed against the belt (a tunnel or funnel on it). The
        // grindstone template's menu would take the click instead, so the
        // block is placed here, as the item would place it.
        Some(stack) => place_against(game, pos, face, &stack, mode == 1),
    }
    true
}

/// `BlockItem.useOn` of `stack` on the face `face` of the block at `pos`
/// (the hook has no hit position: the face's centre stands for it).
fn place_against(game: &mut Game<'_>, pos: Pos, face: [i32; 3], stack: &ItemStack, creative: bool) {
    if game.items().get(&stack.item).and_then(|i| i.block).is_none() {
        return;
    }
    let dir = Dir::nearest(face);
    let n = dir.normal();
    let hit = [0, 1, 2].map(|i| f64::from(pos[i]) + 0.5 + f64::from(n[i]) * 0.5);
    let ctx = horizon_world::level::PlaceContext {
        clicked_pos: horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]),
        face: core_dir(dir),
        hit: horizon_core::geometry::Vec3::new(hit[0], hit[1], hit[2]),
        inside: false,
        yaw: game.yaw,
        pitch: game.pitch,
        sneaking: game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft),
        item: stack.item.to_string(),
        can_use_game_master_blocks: false,
    };
    let outcome = game.with_level(|b, level, ticks| b.place_block(level, ticks, &ctx));
    let horizon_world::level::PlaceOutcome::Placed { pos: at, state } = outcome else {
        let _ = game.process_level_events();
        return;
    };
    let placed = [at.x, at.y, at.z];
    let state = game.apply_item_block_state(placed, state, stack).unwrap_or(state);
    let name = game.res.facts().get(state).name.clone();
    if let Some(entity) = game.new_block_entity(&name) {
        game.world
            .block_entities
            .insert((placed[0], placed[1], placed[2]), entity);
        game.apply_placed_item(placed, stack, false);
    }
    let _ = game.process_level_events();
    game.swing_arm();
    if !creative && let Some(h) = game.held_mut().as_mut() {
        h.count -= 1;
        if h.count == 0 {
            *game.held_mut() = None;
        }
    }
}

/// `BeltBlockEntity.setCasingType` on the segment at `pos`.
fn set_belt_casing(game: &Game<'_>, pos: Pos, casing: u8) -> bool {
    super::create_glue::with_kinetics(game, |k, _| {
        let dye = super::create_belt::material_dye(k.get(pos).map_or(0, |e| e.material));
        k.set_wheel_material(pos, super::create_belt::belt_material(dye, casing))
    })
}

/// Where the click hit the block at `pos`, from its centre: the client's
/// hit, else where the acting player's look meets face `face`.
fn click_point(game: &Game<'_>, pos: Pos, face: Dir) -> [f64; 3] {
    let centre = pos.map(|v| f64::from(v) + 0.5);
    // The hit the client sent with the click, when there is one.
    if let Some(hit) = game.pick().filter(|h| h.pos == pos) {
        return [0, 1, 2].map(|i| hit.point[i] - centre[i]);
    }
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    let Some(p) = game.native_acting_player() else {
        return [0.0; 3];
    };
    let eye = [p.position[0], p.position[1] + if sneaking { 1.27 } else { 1.62 }, p.position[2]];
    let (y, pt) = (f64::from(game.yaw).to_radians(), f64::from(game.pitch).to_radians());
    let dir = [-y.sin() * pt.cos(), -pt.sin(), y.cos() * pt.cos()];
    let n = face.normal();
    let axis = n.iter().position(|v| *v != 0).unwrap_or(1);
    let plane = f64::from(pos[axis]) + if n[axis] > 0 { 1.0 } else { 0.0 };
    let t = if dir[axis].abs() < 1e-9 { 0.0 } else { (plane - eye[axis]) / dir[axis] };
    [0, 1, 2].map(|i| eye[i] + dir[i] * t - centre[i])
}

/// The wrench on a belt: `BeltSlicer.useWrench` (shorten an end, split in
/// two), else `BeltBlock.onWrenched` (a casing off, a pulley back to a
/// middle segment and its shaft back).
pub(super) fn wrench_belt(game: &mut Game<'_>, pos: Pos, face: [i32; 3], sneaking: bool) -> bool {
    use super::create_belt::{Part, Slice};
    let Some(state) = belt_at(game, pos) else {
        return false;
    };
    let mode = game.native_acting_player().map_or(0, |p| p.game_mode);
    if sneaking || mode == 2 || mode == 3 {
        return true;
    }
    let Chain::Belt { controller, segments } = find_chain(&World { game }, pos) else {
        return true;
    };
    let index = segments.iter().position(|p| *p == pos).unwrap_or(0) as i32;
    let length = segments.len() as i32;
    let material = super::create_glue::with_kinetics(game, |k, _| k.get(pos).map_or(0, |e| e.material));
    let cased = super::create_belt::material_casing(material) != super::create_belt::CASING_NONE;
    let face = Dir::nearest(face);
    let hit = click_point(game, pos, face);
    let creative = mode == 1;
    let dim = game.dim.id().to_owned();
    match super::create_belt::wrench_slice(state, pos, index, length, cased, face, hit) {
        Slice::Fail => {}
        Slice::Pass => {
            if cased {
                set_belt_casing(game, pos, super::create_belt::CASING_NONE);
            } else if state.part == Part::Pulley {
                set_belt(game, pos, BeltState { part: Part::Middle, ..state });
                if !creative {
                    game.give_or_drop(ItemStack::new(SHAFT, 1));
                }
            }
        }
        Slice::Shorten { next } => {
            let replaced = belt_at(game, next);
            let entry = with_state(|s| {
                s.sliced.insert((dim.clone(), pos));
                s.belts.remove(&(dim.clone(), controller))
            });
            if let Some(n) = replaced {
                set_belt(game, next, BeltState { part: state.part, ..n });
            }
            if let Some(air) = game.pack.assets.registry.default_state("minecraft:air") {
                game.native_set_block(&dim, pos, air, super::native_hooks::flags::UPDATE_ALL);
            }
            if !creative && replaced.is_some_and(|n| n.part == Part::Pulley) {
                game.give_or_drop(ItemStack::new(SHAFT, 1));
            }
            let Some(mut e) = entry else {
                return true;
            };
            if state.part == Part::End {
                // Items past the new end are thrown off it.
                let gone = e.inv.take_beyond(length);
                let belt = e.belt;
                let mut w = BeltsWorld {
                    game,
                    others: &mut HashMap::new(),
                    belt,
                };
                for t in gone {
                    e.inv.eject(&belt, &mut w, t);
                }
                with_state(|s| s.belts.insert((dim.clone(), controller), e));
            } else {
                // The next segment leads now; what was on the cut one falls.
                let c = pos.map(|v| f64::from(v) + 0.5);
                for stack in e.inv.shift_back() {
                    game.spawn_item(stack, [c[0], c[1] - 0.5 + 11.0 / 16.0, c[2]], [0.0; 3], PICKUP_DELAY);
                }
                with_state(|s| s.belts.insert((dim.clone(), next), e));
            }
        }
        Slice::Split { next, toward_positive } => {
            let next_state = belt_at(game, next);
            if !creative && !take_split_cost(game, state, next_state) {
                return true;
            }
            set_belt(game, pos, BeltState { part: if toward_positive { Part::End } else { Part::Start }, ..state });
            if let Some(n) = next_state {
                set_belt(game, next, BeltState { part: if toward_positive { Part::Start } else { Part::End }, ..n });
            }
            let new_controller = if toward_positive { next } else { pos };
            with_state(|s| {
                let Some(e) = s.belts.get_mut(&(dim.clone(), controller)) else {
                    return;
                };
                let inv = e.inv.split_off(index, toward_positive);
                let belt = e.belt;
                s.belts.insert((dim.clone(), new_controller), Entry { inv, belt });
            });
        }
    }
    true
}

/// `BeltSlicer.useConnector` on the end segment at `pos`: a new segment
/// beyond it, or the belt there joined to this one (flipped to run the same
/// way; its two pulleys' shafts and the connector come back).
fn extend_belt(game: &mut Game<'_>, pos: Pos, state: BeltState, face: [i32; 3]) {
    use super::create_belt::Part;
    let Chain::Belt { controller, segments } = find_chain(&World { game }, pos) else {
        return;
    };
    let length = segments.len() as i32;
    if length >= super::create_belt::MAX_BELT_LENGTH {
        return;
    }
    let hit = click_point(game, pos, Dir::nearest(face));
    if !super::create_belt::hovering_end(state, hit) {
        return;
    }
    let v = super::create_belt::belt_vector(state).map(|c| c as i32);
    let sign = if state.part == Part::Start { -1 } else { 1 };
    let next = [pos[0] + v[0] * sign, pos[1] + v[1] * sign, pos[2] + v[2] * sign];
    let dim = game.dim.id().to_owned();
    let creative = game.native_acting_player().is_some_and(|p| p.game_mode == 1);
    let dye = super::create_glue::with_kinetics(game, |k, _| {
        super::create_belt::material_dye(k.get(controller).map_or(0, |e| e.material))
    });
    let free = Connector { game }.replaceable(next);
    if free {
        // Attach at the end: the end becomes a middle, the new one ends it.
        set_belt(game, pos, BeltState { part: Part::Middle, ..state });
        set_belt(game, next, state);
        super::create_glue::with_kinetics(game, |k, _| {
            if k.get(next).is_none() {
                k.on_placed(next, Kind::Belt(state));
            }
            k.set_wheel_material(next, super::create_belt::belt_material(dye, super::create_belt::CASING_NONE));
        });
        if state.part == Part::Start {
            with_state(|s| {
                if let Some(e) = s.belts.remove(&(dim.clone(), controller)) {
                    let belt = e.belt;
                    let items = e.inv.moved(length, false, 1);
                    let mut inv = BeltInventory::new();
                    inv.add_all(items);
                    s.belts.insert((dim.clone(), next), Entry { inv, belt });
                }
            });
        }
        return;
    }
    let Some(other) = belt_at(game, next) else {
        return;
    };
    if !super::create_belt::states_compatible(state, other) {
        return;
    }
    let Chain::Belt { controller: merged, segments: merged_segments } = find_chain(&World { game }, next) else {
        return;
    };
    let merged_length = merged_segments.len() as i32;
    if merged_length + length > super::create_belt::MAX_BELT_LENGTH {
        return;
    }
    let flipped = state.facing != other.facing;
    // The other belt takes this one's colour and, flipped, its direction.
    for p in &merged_segments {
        if flipped && let Some(b) = belt_at(game, *p) {
            set_belt(game, *p, super::create_belt::flip(b));
        }
    }
    super::create_glue::with_kinetics(game, |k, _| {
        for p in &merged_segments {
            let casing = super::create_belt::material_casing(k.get(*p).map_or(0, |e| e.material));
            k.set_wheel_material(*p, super::create_belt::belt_material(dye, casing));
        }
    });
    set_belt(game, pos, BeltState { part: Part::Middle, ..state });
    if let Some(n) = belt_at(game, next) {
        set_belt(game, next, BeltState { part: Part::Middle, ..n });
    }
    if !creative {
        game.give_or_drop(ItemStack::new(SHAFT, 2));
        game.give_or_drop(ItemStack::new(CONNECTOR, 1));
    }
    // The joined belt's items go to its start, past the first part's.
    let Chain::Belt { controller: joined, .. } = find_chain(&World { game }, pos) else {
        return;
    };
    with_state(|s| {
        let ours = s.belts.remove(&(dim.clone(), controller));
        let theirs = s.belts.remove(&(dim.clone(), merged));
        let belt = ours.as_ref().or(theirs.as_ref()).map(|e| e.belt);
        let Some(belt) = belt else {
            return;
        };
        let ours_first = joined == controller;
        let mut inv = BeltInventory::new();
        if let Some(e) = ours {
            inv.add_all(e.inv.moved(length, false, if ours_first { 0 } else { merged_length }));
        }
        if let Some(e) = theirs {
            inv.add_all(e.inv.moved(merged_length, flipped, if ours_first { length } else { 0 }));
        }
        s.belts.insert((dim.clone(), joined), Entry { inv, belt });
    });
}

/// A survival player's split takes a belt connector and a shaft for each
/// new end that had none; false (nothing taken) without them.
fn take_split_cost(game: &mut Game<'_>, state: BeltState, next: Option<BeltState>) -> bool {
    use super::create_belt::Part;
    let shafts = u32::from(!state.has_pulley()) + u32::from(next.is_some_and(|n| n.part == Part::Middle));
    let count = |game: &Game<'_>, item: &str| -> u32 {
        game.inventory.iter().flatten().filter(|s| &*s.item == item).map(|s| s.count).sum()
    };
    if count(game, CONNECTOR) < 1 || count(game, SHAFT) < shafts {
        return false;
    }
    for (item, mut left) in [(CONNECTOR, 1), (SHAFT, shafts)] {
        for slot in game.inventory.iter_mut() {
            if left == 0 {
                break;
            }
            let Some(s) = slot.as_mut().filter(|s| &*s.item == item) else {
                continue;
            };
            let take = left.min(s.count);
            s.count -= take;
            left -= take;
            if s.count == 0 {
                *slot = None;
            }
        }
    }
    true
}

fn set_belt(game: &mut Game<'_>, pos: Pos, state: BeltState) -> bool {
    let Some((name, face)) = state.block() else {
        return false;
    };
    let Some(base) = game.pack.assets.registry.default_state(name) else {
        return false;
    };
    let Some(id) = game.with_props(base, &[("facing", state.facing.name()), ("face", face)]) else {
        return false;
    };
    let dimension = game.dim.id().to_owned();
    game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL)
}

struct Connector<'a, 'g> {
    game: &'a Game<'g>,
}

impl ConnectorLevel for Connector<'_, '_> {
    fn is_loaded(&self, pos: Pos) -> bool {
        self.game.world.loaded(pos[0], pos[2])
    }
    fn shaft_axis(&self, pos: Pos) -> Option<Axis> {
        (block_name(self.game, pos)? == SHAFT)
            .then(|| prop(self.game, pos, "axis").and_then(|a| Axis::parse(&a)))
            .flatten()
    }
    fn theoretical_speed(&self, pos: Pos) -> Option<f32> {
        super::create_glue::with_kinetics(self.game, |k, _| k.get(pos).map(|e| e.speed))
    }
    fn replaceable(&self, pos: Pos) -> bool {
        let state = self.game.world.block(pos[0], pos[1], pos[2]);
        self.game.world.is_air(state) || self.game.res.facts().get(state).replaceable
    }
}

/// `native_use_item_on` with the belt connector on a shaft
/// (`BeltConnectorItem.useOn`): the first click remembers the shaft, the
/// second makes the belt between them. True when it took the click.
pub(super) fn use_connector(game: &mut Game<'_>, stack: &ItemStack, pos: Pos) -> bool {
    if &*stack.item != CONNECTOR {
        return false;
    }
    let level = Connector { game };
    if level.shaft_axis(pos).is_none() {
        return false;
    }
    let Some(player) = game.native_acting_player() else {
        return false;
    };
    let dim = game.dim.id().to_owned();
    let first = with_state(|s| s.first_shaft.get(&player.uuid).cloned());
    let first = first.filter(|(d, p)| {
        *d == dim
            && level.shaft_axis(*p).is_some()
            && (0..3).map(|k| i64::from(p[k] - pos[k]).pow(2)).sum::<i64>()
                < i64::from(super::create_belt::MAX_BELT_LENGTH * 2).pow(2)
    });
    let Some((_, start)) = first else {
        with_state(|s| s.first_shaft.insert(player.uuid, (dim, pos)));
        game.native_action_bar(player.uuid, horizon_client::text::Text::literal("Belt: first shaft".to_owned()));
        return true;
    };
    if !can_connect(&level, start, pos) {
        return true;
    }
    if start != pos {
        let belts = belts_between(&level, start, pos);
        let unbreakable = belts.iter().any(|(p, _)| {
            let state = game.world.block(p[0], p[1], p[2]);
            game.res.facts().get(state).hardness < 0.0
        });
        if unbreakable {
            with_state(|s| s.first_shaft.remove(&player.uuid));
            return true;
        }
        for (p, s) in belts {
            set_belt(game, p, s);
        }
        if player.game_mode != 1
            && let Some(h) = game.held_mut().as_mut()
        {
            h.count -= 1;
            if h.count == 0 {
                *game.held_mut() = None;
            }
        }
    }
    with_state(|s| s.first_shaft.remove(&player.uuid));
    true
}

/// `native_place_block`: a funnel placed while sneaking collects
/// (`EXTRACTING` is `!sneaking`); funnels are remembered for their ticks.
pub(super) fn placed(game: &Game<'_>, pos: Pos) {
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    // ChuteBlock.getStateForPlacement: a side clicked makes it diagonal,
    // but not when sneaking.
    if is_chute(game, pos) {
        let dim = game.dim.id().to_owned();
        with_state(|s| {
            if sneaking {
                s.sneak_chutes.push((dim.clone(), pos));
            }
            s.chute_updates.insert((dim, pos));
        });
        return;
    }
    // BeltTunnelItem.updateCustomBlockEntityTag: the belt under a new
    // tunnel takes its casing when it has none.
    if let Some(t) = tunnel_at(game, pos) {
        let below = [pos[0], pos[1] - 1, pos[2]];
        let casing = super::create_glue::with_kinetics(game, |k, _| {
            super::create_belt::material_casing(k.get(below).map_or(0, |e| e.material))
        });
        if casing == super::create_belt::CASING_NONE {
            let kind = if t.brass { super::create_belt::CASING_BRASS } else { super::create_belt::CASING_ANDESITE };
            set_belt_casing(game, below, kind);
        }
        return;
    }
    let Some(f) = funnel_at(game, pos) else {
        return;
    };
    let dim = game.dim.id().to_owned();
    with_state(|s| {
        s.funnels.insert((dim.clone(), pos), FunnelData::default());
        if sneaking && f.extracting {
            s.sneak_placed.push((dim, pos));
        }
    });
}

fn set_funnel(game: &mut Game<'_>, pos: Pos, f: FunnelState) -> bool {
    let Some(base) = game.pack.assets.registry.default_state(f.block()) else {
        return false;
    };
    let Some(id) = game.with_props(base, &[("facing", f.facing.name())]) else {
        return false;
    };
    let dimension = game.dim.id().to_owned();
    game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL)
}

/// The wrench on a funnel (`onWrenched` cycles `EXTRACTING`): true when
/// it was one.
pub(super) fn wrench_funnel(game: &mut Game<'_>, pos: Pos) -> bool {
    let Some(f) = funnel_at(game, pos) else {
        return false;
    };
    set_funnel(game, pos, f.wrenched());
    true
}

/// The item a block of ours drops when a wrench picks it up.
pub(super) fn item_of(name: &str) -> &str {
    if let Some(f) = FunnelState::from_block(name, |_| Some("north")) {
        f.item()
    } else if let Some(t) = TunnelState::from_block(name, |_| None) {
        if t.brass {
            super::create_tunnel::BRASS_TUNNEL
        } else {
            super::create_tunnel::ANDESITE_TUNNEL
        }
    } else if name == super::create_kinetics::DEPLOYER_ALONG {
        super::create_kinetics::DEPLOYER
    } else {
        name
    }
}

// ---- Funnels ------------------------------------------------------------------

/// Block changes of ours outside the kinetics: funnels and tunnels come
/// and go (a tunnel that is gone drops its stack at its next tick).
pub(super) fn changed(game: &Game<'_>, dimension: &str, positions: &[Pos]) {
    let found: Vec<(Pos, bool, bool)> = positions
        .iter()
        .map(|&p| (p, funnel_at(game, p).is_some(), tunnel_at(game, p).is_some()))
        .collect();
    with_state(|s| {
        // The chutes whose shape may follow (`onPlace`, `onRemove`,
        // `updateShape`): its own, the layer below and those above beside.
        for &p in positions {
            for dx in -1..=1 {
                for dz in -1..=1 {
                    s.chute_updates.insert((dimension.to_owned(), [p[0] + dx, p[1] - 1, p[2] + dz]));
                }
            }
            s.chute_updates.insert((dimension.to_owned(), p));
            for d in [Dir::North, Dir::South, Dir::West, Dir::East] {
                s.chute_updates.insert((dimension.to_owned(), step(above(p), d)));
            }
        }
        for (p, funnel, tunnel) in found {
            let key = (dimension.to_owned(), p);
            if funnel {
                s.funnels.entry(key.clone()).or_default();
            } else {
                s.funnels.remove(&key);
            }
            if tunnel {
                s.tunnels.entry(key).or_insert_with(|| TunnelData {
                    fresh: true,
                    ..TunnelData::default()
                });
            }
        }
    });
}

/// Sets the chute at `pos` to `facing` and `shape`, keeping its item.
fn set_chute(game: &mut Game<'_>, pos: Pos, facing: Dir, shape: ChuteShape) {
    use horizon_client::containers::BlockEntity as B;
    let kept = match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(B::Mod(m)) => Some((m.items.clone(), m.nbt.clone())),
        _ => None,
    };
    let Some(base) = game.pack.assets.registry.default_state(chute_block(shape)) else {
        return;
    };
    let Some(id) = game.with_props(base, &[("facing", facing.name())]) else {
        return;
    };
    let dimension = game.dim.id().to_owned();
    game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
    if let Some((items, nbt)) = kept {
        game.native_with_mod_block_entity(pos, |be| {
            be.items = items;
            be.nbt = nbt;
        });
    }
}

/// `AbstractChuteBlock.tick` for the chutes changes were near:
/// `updateChuteState`, and placed chutes' facing.
pub(super) fn chutes_update(game: &mut Game<'_>, dimension: &str) {
    let (todo, sneaked) = with_state(|s| {
        let mut todo = Vec::new();
        s.chute_updates.retain(|(d, p)| {
            if d == dimension {
                todo.push(*p);
                false
            } else {
                true
            }
        });
        let mut sneaked = Vec::new();
        s.sneak_chutes.retain(|(d, p)| {
            if d == dimension {
                sneaked.push(*p);
                false
            } else {
                true
            }
        });
        (todo, sneaked)
    });
    for p in sneaked {
        if let Some((_, shape)) = chute_at(game, p).filter(|_| !is_smart_chute(game, p)) {
            set_chute(game, p, Dir::Down, shape);
        }
    }
    for p in todo {
        if !game.world.loaded(p[0], p[2]) {
            continue;
        }
        // SmartChuteBlock.updateChuteState keeps its state.
        if is_smart_chute(game, p) {
            continue;
        }
        let Some((facing, shape)) = chute_at(game, p) else {
            continue;
        };
        let up = above(p);
        // ChuteBlock.canSurvive: not under a chute leading off sideways
        // (the block breaks, as `updateShape` leaves it).
        if chute_at(game, up).is_some_and(|(f, _)| f != Dir::Down) {
            let [x, y, z] = p;
            game.native_run_command(&format!("execute in {dimension} run setblock {x} {y} {z} air destroy"));
            continue;
        }
        let template_up = prop(game, p, "facing").as_deref() == Some("up");
        let around = ChuteAround {
            target_is_chute: is_chute(game, chute_target(p, facing)),
            inputs: [Dir::North, Dir::South, Dir::West, Dir::East]
                .map(|d| chute_at(game, step(up, d)).is_some_and(|(f, _)| f == d)),
            above_facing_down: chute_at(game, up).is_some_and(|(f, _)| f == Dir::Down)
                || funnel_at(game, up).is_some_and(|f| f.facing == Dir::Down),
        };
        let next = chute_state(facing, shape, around);
        if next != (facing, shape) || template_up {
            set_chute(game, p, next.0, next.1);
        }
    }
}

/// `ChuteBlock.onWrenched`: true for every chute.
pub(super) fn wrench_chute(game: &mut Game<'_>, pos: Pos) -> bool {
    let Some((facing, shape)) = chute_at(game, pos).filter(|_| !is_smart_chute(game, pos)) else {
        return false;
    };
    if let Some(next) = super::create_chute::wrenched(facing, shape).filter(|n| *n != shape) {
        set_chute(game, pos, facing, next);
    }
    true
}

/// `FunnelBlockEntity.tick` and `FunnelBlock.entityInside` for every funnel
/// of the level.
pub(super) fn funnels_tick(game: &mut Game<'_>, dimension: &str) {
    let sneaked: Vec<Pos> = with_state(|s| {
        let mut here = Vec::new();
        s.sneak_placed.retain(|(d, p)| {
            if d == dimension {
                here.push(*p);
                false
            } else {
                true
            }
        });
        here
    });
    for p in sneaked {
        if let Some(f) = funnel_at(game, p).filter(|f| f.extracting) {
            set_funnel(game, p, f.wrenched());
        }
    }
    let funnels: Vec<(Pos, FunnelData)> = with_state(|s| {
        s.funnels
            .iter()
            .filter(|((d, _), _)| d == dimension)
            .map(|((_, p), c)| (*p, c.clone()))
            .collect()
    });
    for (pos, data) in funnels {
        if !game.world.loaded(pos[0], pos[2]) {
            continue;
        }
        let Some(f) = funnel_at(game, pos) else {
            with_state(|s| s.funnels.remove(&(dimension.to_owned(), pos)));
            continue;
        };
        let mut cooldown = data.cooldown;
        let filter = f.brass.then_some(&data.filter);
        if f.brass {
            // FilteringRenderer: clients draw the filter's item.
            let view = vec![(data.filter.item.clone().unwrap_or_default(), data.filter.count)];
            super::create_machines::note_view(game, pos, view);
        }
        let below = [pos[0], pos[1] - 1, pos[2]];
        // A powered funnel pauses (`POWERED`, kept here: the template's own
        // `powered` would give off a signal).
        let powered = super::create_glue::signal(game, pos);
        let m = super::create_funnel::mode_powered(f, belt_under(game, pos), powered);
        if m == Mode::Collect {
            collect(game, pos, f, filter);
        }
        if tick_cooldown(&mut cooldown, m) {
            let behind = f.behind(pos);
            let acted = match m {
                Mode::Extract => extract(game, pos, f, behind, filter),
                Mode::PushingToBelt => push_to_belt(game, below, f, behind, filter),
                _ => false,
            };
            if acted {
                cooldown = EXTRACTION_TIMER;
            }
        }
        with_state(|s| {
            if let Some(c) = s.funnels.get_mut(&(dimension.to_owned(), pos)) {
                c.cooldown = cooldown;
            }
        });
    }
}

/// Funnels' and tunnels' block entities (`FunnelBlockEntity`,
/// `BeltTunnelBlockEntity`): they hold only what their flaps did, for
/// clients to swing them.
pub(super) const FUNNEL_ENTITY: &str = "create:funnel";
pub(super) const TUNNEL_ENTITY: &str = "create:belt_tunnel";

/// `native_block_entity` for funnels and tunnels.
pub(super) fn block_entity(block: &str, entity: &mut Option<(String, Compound)>) {
    if entity.is_some() {
        return;
    }
    if super::create_funnel::is_funnel(block) {
        *entity = Some((FUNNEL_ENTITY.to_owned(), Compound::new()));
    } else if TunnelState::from_block(block, |_| None).is_some() {
        *entity = Some((TUNNEL_ENTITY.to_owned(), Compound::new()));
    }
}

/// `FunnelBlockEntity.flap` / `BeltTunnelBlockEntity.flap` on the server:
/// the flap on `side` swings (in or out), told to clients with the block
/// entity (`FunnelFlapPacket`, `TunnelFlapPacket`).
pub(super) fn flap(game: &mut Game<'_>, pos: Pos, side: Dir, inward: bool) {
    let key = format!("Flap{}", side.name());
    let done = game.native_with_mod_block_entity(pos, |be| {
        let n = match be.nbt.get(&JavaString::from(key.as_str())) {
            Some(Tag::Int(n)) => *n,
            _ => 0,
        };
        be.nbt.insert(JavaString::from(key.as_str()), Tag::Int(n.wrapping_add(1)));
        be.nbt.insert(JavaString::from(format!("{key}In").as_str()), Tag::Byte(i8::from(inward)));
    });
    if done.is_some() {
        game.native_block_entity_sync(pos);
    }
}

/// `native_block_entity_update_tag`: a funnel's or tunnel's flaps.
pub(super) fn update_tag(entity: &horizon_client::containers::ModBlockEntity, tag: &mut Option<Compound>) {
    if entity.id == FUNNEL_ENTITY || entity.id == TUNNEL_ENTITY {
        *tag = Some(entity.nbt.clone());
    }
}

/// Clients' flaps: by position and side, the swings counted and the value
/// at the last one (`LerpedFloat` chasing 0 at .05, `createChasingFlap`).
type ClientFlaps = HashMap<(Pos, Dir), (i32, f32, std::time::Instant)>;
static CLIENT_FLAPS: Mutex<Option<ClientFlaps>> = Mutex::new(None);

/// `FlapStuffs`: the flaps' four segments, their step and pivot.
const FLAP_X_OFFSET: f32 = 0.075 / 16.0;
const FLAP_SEGMENT_STEP: f32 = -3.05 / 16.0;

/// `FlapStuffs.flapAngle`.
fn flap_angle(flapness: f32, segment: i32) -> f32 {
    let intensity = if segment == 3 { 1.5 } else { (segment + 1) as f32 };
    let angle = ((1.0 - flapness.abs()) * std::f32::consts::PI * intensity).sin() * 30.0 * flapness;
    if flapness < 0.0 { angle * 0.5 } else { angle }
}

/// `native_block_entity_submit`: a horizontal funnel's flap
/// (`FunnelRenderer`) and a tunnel's flaps (`BeltTunnelRenderer`), swinging
/// after each `flap` the server told of (`FlapStuffs.renderFlaps`).
pub(super) fn submit(
    game: &Game<'_>,
    pos: Pos,
    entity: &horizon_client::containers::ModBlockEntity,
    draw: &mut super::native_block_entity_draw::BlockEntityDraw,
) {
    // (side, pivot, z offset, flap model)
    let flaps: Vec<(Dir, [f32; 3], f32, &str)> = if entity.id == FUNNEL_ENTITY {
        match funnel_at(game, pos) {
            // -getFlapOffset: 1/16 out for a funnel not on a belt.
            Some(f) if f.facing.axis() != Axis::Y => {
                vec![(f.facing, [0.0, 10.0 / 16.0, 9.5 / 16.0], 1.0 / 16.0, "create:hz_part_funnel_flap")]
            }
            _ => Vec::new(),
        }
    } else if entity.id == TUNNEL_ENTITY {
        match tunnel_at(game, pos) {
            Some(t) => t
                .flaps(pos, &World { game })
                .into_iter()
                .map(|d| (d, [0.0, 10.0 / 16.0, 1.0 / 16.0], 0.0, "create:hz_part_belt_tunnel_flap"))
                .collect(),
            None => Vec::new(),
        }
    } else {
        return;
    };
    let now = std::time::Instant::now();
    let mut still = true;
    let mut guard = CLIENT_FLAPS.lock().unwrap_or_else(|e| e.into_inner());
    let known = guard.get_or_insert_with(HashMap::new);
    for (side, pivot, z, model) in flaps {
        let key = format!("Flap{}", side.name());
        let count = match entity.nbt.get(&JavaString::from(key.as_str())) {
            Some(Tag::Int(n)) => *n,
            _ => 0,
        };
        let inward = matches!(entity.nbt.get(&JavaString::from(format!("{key}In").as_str())), Some(Tag::Byte(1)));
        let entry = known.entry((pos, side)).or_insert((count, 0.25, now));
        if entry.0 != count {
            *entry = (count, if inward { -1.0 } else { 1.0 }, now);
        }
        let ticks = now.duration_since(entry.2).as_secs_f32() * 20.0;
        let mut f = entry.1 * 0.95f32.powf(ticks);
        if f.abs() < 1.0 / 128.0 {
            f = 0.0;
        } else {
            still = false;
        }
        draw.push();
        draw.rotate_about([0.0, 1.0, 0.0], side.opposite().horizontal_angle(), [0.5; 3]);
        draw.translate([FLAP_X_OFFSET, 0.0, z]);
        for segment in 0..4 {
            draw.push();
            draw.rotate_about([1.0, 0.0, 0.0], flap_angle(f, segment), pivot);
            game.native_draw_block(draw, model, &[]);
            draw.pop();
            draw.translate([FLAP_SEGMENT_STEP, 0.0, 0.0]);
        }
        draw.pop();
    }
    if still {
        draw.keep();
    }
}

/// `FilteringBehaviour.test` as a predicate on stacks.
fn passes(filter: Option<&Filter>) -> impl Fn(&ItemStack) -> bool + '_ {
    move |s| filter.is_none_or(|f| f.test(&s.item))
}

/// `activateExtractor`: one item out of the inventory behind, unless an
/// item still lies in front.
fn extract(game: &mut Game<'_>, pos: Pos, f: FunnelState, behind: Pos, filter: Option<&Filter>) -> bool {
    let (lo, hi) = overflow_area(pos, f.facing);
    let blocked = game.dropped.iter().any(|d| {
        let p = d.position;
        d.stack.count > 0
            && p.x + 0.125 > lo[0]
            && p.x - 0.125 < hi[0]
            && p.y + 0.25 > lo[1]
            && p.y < hi[1]
            && p.z + 0.125 > lo[2]
            && p.z - 0.125 < hi[2]
    });
    if blocked {
        return false;
    }
    let Some(stack) = extract_from(game, behind, amount_to_extract(Mode::Extract, filter), &passes(filter)) else {
        return false;
    };
    let (at, motion) = extracted_item(pos, f.facing);
    game.spawn_item(stack, at, motion, PICKUP_DELAY);
    flap(game, pos, f.facing, false);
    true
}

/// `activateExtractingBeltFunnel`: one item onto the belt below, from the
/// funnel's side.
fn push_to_belt(game: &mut Game<'_>, below: Pos, f: FunnelState, behind: Pos, filter: Option<&Filter>) -> bool {
    let Some(segment_state) = belt_at(game, below) else {
        return false;
    };
    let Some((controller, index)) = segment_of(game, below) else {
        return false;
    };
    let dim = game.dim.id().to_owned();
    let free = with_state(|s| {
        s.belts
            .get(&(dim.clone(), controller))
            .is_some_and(|e| !e.inv.is_occupied(&e.belt, index, f.facing))
    });
    if !free {
        return false;
    }
    let amount = amount_to_extract(Mode::PushingToBelt, filter);
    let Some(stack) = extract_from(game, behind, amount, &passes(filter)) else {
        return false;
    };
    let left = with_state(|s| {
        let angle = s.angle();
        let Some(e) = s.belts.get_mut(&(dim, controller)) else {
            return Some(stack.clone());
        };
        let belt = e.belt;
        let t = Transported::new(stack.clone(), angle);
        e.inv.insert_from_side(&belt, segment_state, index, &t, f.facing, false)
    });
    if let Some(l) = left {
        // Put back what didn't go on.
        if insert_into(game, behind, f.facing, l.clone()).is_some() {
            let c = behind.map(|v| f64::from(v) + 0.5);
            game.spawn_item(l, c, [0.0; 3], PICKUP_DELAY);
        }
    }
    true
}

/// `FunnelBlock.entityInside`: items at its open side go into the
/// inventory behind.
fn collect(game: &mut Game<'_>, pos: Pos, f: FunnelState, filter: Option<&Filter>) {
    let behind = f.behind(pos);
    if !f.collects() || !has_inventory(game, behind) {
        return;
    }
    let lo = pos.map(f64::from);
    let mut changed = false;
    for i in 0..game.dropped.len() {
        let d = &game.dropped[i];
        let p = [d.position.x, d.position.y, d.position.z];
        let inside = d.stack.count > 0
            && (lo[0] - 0.125..lo[0] + 1.125).contains(&p[0])
            && (lo[1] - 0.25..lo[1] + 1.0).contains(&p[1])
            && (lo[2] - 0.125..lo[2] + 1.125).contains(&p[2]);
        // tryInsert: what the filter refuses stays.
        if !inside || !collects_entity_at(pos, f.facing, p) || !passes(filter)(&d.stack) {
            continue;
        }
        let stack = d.stack.clone();
        let left = insert_into(game, behind, f.facing, stack.clone());
        let n = left.map_or(0, |l| l.count);
        if n != stack.count {
            game.dropped[i].stack.count = n;
            changed = true;
            // AbstractFunnelBlock.entityInside: the flap swings in.
            flap(game, pos, f.facing, true);
        }
    }
    if changed {
        game.dropped.retain(|d| d.stack.count > 0);
    }
}

/// `FilteringBehaviour.onShortInteract` on a brass funnel's filter side
/// (Horizon's click has no hit position to test the value box with: the
/// whole face counts): the held item becomes the filter, an empty hand
/// clears it. Items aren't used up (only Create's filter items are).
/// Whether `pos` holds a brass funnel.
pub(super) fn is_brass_funnel(game: &Game<'_>, pos: Pos) -> bool {
    funnel_at(game, pos).is_some_and(|f| f.brass)
}

/// Whether `face` of the brass funnel at `pos` holds its filter slot.
pub(super) fn funnel_filter_side(game: &Game<'_>, pos: Pos, face: Dir) -> bool {
    funnel_at(game, pos).is_some_and(|f| f.filter_sides().contains(&face))
}

/// The brass funnel at `pos` extracts up to `value` (1 to 64) at a time.
pub(super) fn set_funnel_amount(game: &Game<'_>, pos: Pos, value: i32) {
    if !is_brass_funnel(game, pos) || !(1..=64).contains(&value) {
        return;
    }
    let dim = game.dim.id().to_owned();
    with_state(|s| s.funnels.entry((dim, pos)).or_default().filter.count = value as u32);
}

/// `FunnelBlock.useItemOn`: the held item clicked into a collecting funnel's
/// opening goes into the inventory behind (what doesn't fit stays in hand).
/// A brass funnel's opening is its filter slot here (no hit position).
fn right_click_funnel(game: &mut Game<'_>, pos: Pos, face: [i32; 3]) -> bool {
    let Some(f) = funnel_at(game, pos) else {
        return false;
    };
    if f.brass || !f.collects() || Dir::nearest(face) != f.facing {
        return false;
    }
    let Some(stack) = game.held().cloned() else {
        return false;
    };
    if &*stack.item == "create:wrench" || &*stack.item == super::create_arms::ARM {
        return false;
    }
    if super::create_glue::signal(game, pos) {
        return false;
    }
    let behind = f.behind(pos);
    if !has_inventory(game, behind) {
        return true;
    }
    let left = insert_into(game, behind, f.facing, stack);
    *game.held_mut() = left;
    true
}

/// `FilteringBehaviour.onShortInteract` on a smart chute's side (its four
/// filter slots hold one filter): the held item becomes it, an empty hand
/// clears it.
fn right_click_smart_chute(game: &mut Game<'_>, pos: Pos, face: [i32; 3]) -> bool {
    if !is_smart_chute(game, pos) || Dir::nearest(face).axis() == Axis::Y {
        return false;
    }
    let held = game.held().map(|s| s.item.to_string());
    if held.as_deref().is_some_and(|i| i == "create:wrench" || i == super::create_arms::ARM) {
        return false;
    }
    if game.native_acting_player().is_some_and(|p| p.game_mode == 3) {
        return false;
    }
    let mut filter = smart_filter(game, pos).map(|(f, _)| f).unwrap_or_default();
    filter.set(held.as_deref());
    let text = filter.item.clone().unwrap_or_default();
    game.native_with_mod_block_entity(pos, |be| {
        be.nbt.insert(JavaString::from("Filter"), Tag::String(text.as_str().into()));
    });
    true
}

/// The smart chute at `pos` extracts up to `value` (1 to 64) at a time.
pub(super) fn set_smart_chute_amount(game: &mut Game<'_>, pos: Pos, value: i32) {
    if !is_smart_chute(game, pos) || !(1..=64).contains(&value) {
        return;
    }
    game.native_with_mod_block_entity(pos, |be| {
        be.nbt.insert(JavaString::from("FilterAmount"), Tag::Int(value));
    });
}

/// Whether `face` of the block at `pos` is a smart chute's filter slot.
pub(super) fn smart_chute_filter_side(game: &Game<'_>, pos: Pos, face: Dir) -> bool {
    is_smart_chute(game, pos) && face.axis() != Axis::Y
}

fn right_click_filter(game: &mut Game<'_>, pos: Pos, face: [i32; 3]) -> bool {
    let Some(f) = funnel_at(game, pos).filter(|f| f.brass) else {
        return false;
    };
    if !f.filter_sides().contains(&Dir::nearest(face)) {
        return false;
    }
    let held = game.held().map(|s| s.item.to_string());
    // canShortInteract: not the wrench or the mechanical arm.
    if held.as_deref().is_some_and(|i| i == "create:wrench" || i == super::create_arms::ARM) {
        return false;
    }
    if game.native_acting_player().is_some_and(|p| p.game_mode == 3) {
        return false;
    }
    let dim = game.dim.id().to_owned();
    let filter = with_state(|s| {
        let data = s.funnels.entry((dim, pos)).or_default();
        data.filter.set(held.as_deref());
        data.filter.clone()
    });
    let view = filter.item.iter().map(|i| (i.clone(), 1)).collect();
    super::create_machines::note_view(game, pos, view);
    true
}

/// `BrassTunnelBlock.useWithoutItem`: an empty hand takes the stacks the
/// connected brass tunnels hold (`grabAllStacksOfGroup`).
fn right_click_tunnel(game: &mut Game<'_>, pos: Pos) -> bool {
    if !tunnel_at(game, pos).is_some_and(|t| t.brass) || game.held().is_some() {
        return false;
    }
    let Some(group) = super::create_tunnel::group(&World { game }, pos, |p| game.world.loaded(p[0], p[2])) else {
        return false;
    };
    let dim = game.dim.id().to_owned();
    let stacks: Vec<ItemStack> = with_state(|s| {
        group
            .iter()
            .filter_map(|p| {
                let t = s.tunnels.get_mut(&(dim.clone(), *p))?;
                let stack = t.brass.stack.take();
                t.brass.set_stack(None, None);
                stack
            })
            .collect()
    });
    if stacks.is_empty() {
        return false;
    }
    for stack in stacks {
        game.give_or_drop(stack);
    }
    true
}

// ---- Tunnels ------------------------------------------------------------------

fn set_tunnel(game: &mut Game<'_>, pos: Pos, t: TunnelState) -> bool {
    let Some(base) = game.pack.assets.registry.default_state(&t.block()) else {
        return false;
    };
    let axis = if t.axis == Axis::Z { "z" } else { "x" };
    let Some(id) = game.with_props(base, &[("axis", axis)]) else {
        return false;
    };
    let dimension = game.dim.id().to_owned();
    game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL)
}

/// The brass tunnels connected to the one at `pos` one way after another
/// (`isConnected`, as `selectionMode`'s callback walks them).
fn connected_group(game: &Game<'_>, pos: Pos) -> Vec<Pos> {
    let level = World { game };
    let mut out = vec![pos];
    for left in [true, false] {
        let mut at = pos;
        while is_connected(&level, at, left) {
            let Some(next) = adjacent(&level, at, left) else {
                break;
            };
            if out.contains(&next) {
                break;
            }
            out.push(next);
            at = next;
        }
    }
    out
}

/// Sets the mode of the brass tunnel at `pos` and the ones connected to it.
fn set_mode(game: &Game<'_>, pos: Pos, mode: SelectionMode) {
    let group = connected_group(game, pos);
    let dim = game.dim.id().to_owned();
    with_state(|s| {
        for p in group {
            s.tunnels.entry((dim.clone(), p)).or_default().brass.mode = mode;
        }
    });
}

/// The wrench on a tunnel: a brass tunnel steps to its next selection mode
/// (standing in for its value box, which Horizon can't open;
/// the window of a straight brass tunnel isn't toggled), an andesite one
/// opens or closes its window (`onWrenched`). True when it was a tunnel.
pub(super) fn wrench_tunnel(game: &mut Game<'_>, pos: Pos) -> bool {
    let Some(t) = tunnel_at(game, pos) else {
        return false;
    };
    if t.brass {
        let dim = game.dim.id().to_owned();
        let mode = with_state(|s| s.tunnels.get(&(dim, pos)).map(|d| d.brass.mode).unwrap_or_default()).next();
        set_mode(game, pos, mode);
        if let Some(player) = game.native_acting_player() {
            let line = format!("Brass Tunnel: {}", mode.label());
            game.native_action_bar(player.uuid, horizon_client::text::Text::literal(line));
        }
        return true;
    }
    if let Some(w) = t.wrenched() {
        set_tunnel(game, pos, w);
    }
    true
}

/// The world as a brass tunnel's distribution sees it.
struct TunnelsAt<'a, 'g> {
    game: &'a mut Game<'g>,
}

impl TunnelLevel for TunnelsAt<'_, '_> {
    fn tunnel(&self, pos: Pos) -> Option<TunnelState> {
        tunnel_at(self.game, pos)
    }
    fn flat_belt_axis(&self, pos: Pos) -> Option<Axis> {
        World { game: self.game }.flat_belt_axis(pos)
    }
    fn belt_axis(&self, pos: Pos) -> Option<Axis> {
        World { game: self.game }.belt_axis(pos)
    }
    fn has_direct_input(&self, pos: Pos) -> bool {
        has_direct_input(self.game, pos)
    }
    fn can_insert_from_side(&self, pos: Pos, side: Dir) -> bool {
        can_insert_from_side(self.game, pos, side)
    }
    fn extended_funnel(&self, _: Pos) -> Option<Dir> {
        None
    }
}

impl TunnelWorld<ItemStack> for TunnelsAt<'_, '_> {
    fn is_loaded(&self, pos: Pos) -> bool {
        self.game.world.loaded(pos[0], pos[2])
    }
    fn belt(&self, pos: Pos) -> Option<(f32, Dir)> {
        let b = belt_at(self.game, pos).filter(|b| b.can_transport_objects())?;
        let speed = belt_speed(self.game, pos);
        Some((speed, b.movement_facing(speed)))
    }
    fn pulling_funnel(&self, pos: Pos, facing: Dir) -> bool {
        let Some(f) = funnel_at(self.game, pos).filter(|f| f.facing == facing) else {
            return false;
        };
        as_belt_funnel(f, belt_under(self.game, pos)).is_some_and(|b| b.shape == FunnelShape::Pulling)
    }
    fn solid_side(&self, pos: Pos, face: Dir) -> bool {
        sturdy(self.game, pos, face)
    }
    fn filter(&self, pos: Pos, stack: &ItemStack) -> Option<(bool, bool)> {
        let f = funnel_filter(self.game, pos)?;
        Some((f.test(&stack.item), f.item.is_none()))
    }
    fn insert(&mut self, pos: Pos, side: Dir, stack: ItemStack, simulate: bool) -> Option<ItemStack> {
        side_insert(self.game, pos, side, stack, simulate)
    }
    fn flap(&mut self, tunnel: Pos, side: Dir, inward: bool) {
        flap(self.game, tunnel, side, inward);
    }
    fn eject(&mut self, tunnel: Pos, side: Dir, stack: ItemStack) {
        let segment = [tunnel[0], tunnel[1] - 1, tunnel[2]];
        let Some((controller, index)) = segment_of(self.game, segment) else {
            return;
        };
        let dim = self.game.dim.id().to_owned();
        let Some(belt) = with_state(|s| s.belts.get(&(dim, controller)).map(|e| e.belt)) else {
            return;
        };
        let speed = belt.movement_speed();
        let movement = speed.abs().max(1.0 / 8.0);
        let at = belt.vector_for_offset((index + i32::from(speed > 0.0)) as f32);
        let n = side.normal();
        let motion = [
            f64::from(n[0] as f32 * movement),
            f64::from(n[1] as f32 * movement + 1.0 / 8.0),
            f64::from(n[2] as f32 * movement),
        ];
        self.game.spawn_item(stack, [at[0], at[1] + 6.0 / 16.0, at[2]], motion, PICKUP_DELAY);
    }
    fn random(&mut self, n: usize) -> usize {
        use horizon_core::random::RandomSource;
        self.game.world.random.next_int_bound(n.max(1) as i32).unwrap_or(0) as usize
    }
    fn has_stack(&self, pos: Pos) -> bool {
        let dim = self.game.dim.id().to_owned();
        with_state(|s| s.tunnels.get(&(dim, pos)).is_some_and(|t| t.brass.stack.is_some()))
    }
    fn set_synced(&mut self, pos: Pos, active: bool) {
        let dim = self.game.dim.id().to_owned();
        with_state(|s| {
            if let Some(t) = s.tunnels.get_mut(&(dim, pos)) {
                t.brass.synced = active;
            }
        });
    }
}

/// `DirectBeltInputBehaviour.handleInsertion(stack, side, simulate)` at
/// `pos`: what is left. Containers behind a funnel can't be simulated here:
/// hopeful.
fn side_insert(game: &mut Game<'_>, pos: Pos, side: Dir, stack: ItemStack, simulate: bool) -> Option<ItemStack> {
    if let Some(segment_state) = belt_at(game, pos) {
        let Some((controller, index)) = segment_of(game, pos) else {
            return Some(stack);
        };
        let dim = game.dim.id().to_owned();
        return with_state(|s| {
            let angle = s.angle();
            let Some(e) = s.belts.get_mut(&(dim, controller)) else {
                return Some(stack.clone());
            };
            let belt = e.belt;
            if simulate {
                let busy = !segment_state.can_transport_objects() || e.inv.is_occupied(&belt, index, side);
                return busy.then(|| stack.clone());
            }
            e.inv.insert_from_side(&belt, segment_state, index, &Transported::new(stack.clone(), angle), side, false)
        });
    }
    if simulate {
        let free = match block_name(game, pos).as_deref() {
            Some(name) if is_chute_block(name) => chute_accepts(game, pos, &stack),
            Some(name) if name == super::create_machines::DEPOT => {
                use horizon_client::containers::BlockEntity as B;
                match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
                    Some(B::Mod(m)) => m.items.iter().all(Option::is_none),
                    _ => false,
                }
            }
            _ => funnel_at(game, pos)
                .is_some_and(|f| f.takes_from_belt_end() && funnel_accepts(game, pos, &stack)),
        };
        return (!free).then_some(stack);
    }
    match direct_insert(game, pos, stack.clone()) {
        Some(left) => left,
        None => Some(stack),
    }
}

/// Every tunnel of the level: its shape kept up with its neighbours
/// (`updateTunnel`), broken off a belt it no longer stands on
/// (`neighborChanged`), its stack dropped when it is gone (`destroy`), and
/// brass tunnels' distribution (`BrassTunnelBlockEntity.tick`).
pub(super) fn tunnels_tick(game: &mut Game<'_>, dimension: &str) {
    let tunnels: Vec<(Pos, bool)> = with_state(|s| {
        s.tunnels
            .iter()
            .filter(|((d, _), _)| d == dimension)
            .map(|((_, p), t)| (*p, t.fresh))
            .collect()
    });
    let mut brass = Vec::new();
    for (pos, fresh) in tunnels {
        if !game.world.loaded(pos[0], pos[2]) {
            continue;
        }
        let key = (dimension.to_owned(), pos);
        let Some(state) = tunnel_at(game, pos) else {
            let gone = with_state(|s| s.tunnels.remove(&key));
            if let Some(stack) = gone.and_then(|t| t.brass.stack) {
                game.spawn_item(stack, pos.map(|v| f64::from(v) + 0.5), [0.0; 3], PICKUP_DELAY);
            }
            continue;
        };
        if !can_survive(pos, &World { game }) {
            let [x, y, z] = pos;
            game.native_run_command(&format!("execute in {dimension} run setblock {x} {y} {z} air destroy"));
            continue;
        }
        let new = updated_state(&World { game }, pos, state);
        // A tunnel placed with the template's default axis (y) reads as X.
        let turned = prop(game, pos, "axis").as_deref() == Some(if new.axis == Axis::Z { "z" } else { "x" });
        if new != state || !turned {
            set_tunnel(game, pos, new);
        }
        if fresh {
            with_state(|s| {
                if let Some(t) = s.tunnels.get_mut(&key) {
                    t.fresh = false;
                }
            });
            if state.brass {
                // The tunnels it joins take its mode.
                let mode = with_state(|s| s.tunnels.get(&key).map(|t| t.brass.mode).unwrap_or_default());
                set_mode(game, pos, mode);
            }
        }
        if state.brass {
            brass.push(pos);
        }
    }
    for pos in brass {
        let key = (dimension.to_owned(), pos);
        let Some(mut t) = with_state(|s| s.tunnels.remove(&key)) else {
            continue;
        };
        if t.brass.stack.is_some() || t.brass.progress > 0 || t.brass.synced {
            t.brass.tick(pos, &mut TunnelsAt { game });
        }
        with_state(|s| s.tunnels.insert(key, t));
    }
}

/// `FilteringRenderer` on a brass funnel: its filter's item in each of its
/// filter slots (`ValueBoxRenderer.renderItemIntoValueBox`).
pub(super) fn draw_funnel_filter(
    game: &Game<'_>,
    camera: glam::DVec3,
    pos: Pos,
    slots: &[(String, u32)],
    out: &mut Vec<horizon_client::mesher::Vertex>,
) {
    let Some((id, _)) = slots.first().filter(|(id, c)| *c > 0 && !id.is_empty()) else {
        return;
    };
    let Some(f) = funnel_at(game, pos).filter(|f| f.brass) else {
        return;
    };
    let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
    if corner.distance_squared(camera) > 32.0 * 32.0 {
        return;
    }
    let light = game.world.light(pos[0], pos[1], pos[2]);
    // isGui3d: block items stand at full size, others lie flat at half.
    let block = game.pack.assets.registry.default_state(id).is_some();
    let scale = if block { 1.0 } else { 0.5 } + 1.0 / 64.0;
    let z_offset = if block { 0.0 } else { -0.15 };
    let stack = ItemStack::new(id, 1);
    for side in f.filter_sides() {
        let (at, y_rot, x_rot) = filter_slot(f, side);
        let centre = corner + glam::DVec3::new(f64::from(at[0]), f64::from(at[1]), f64::from(at[2]));
        let m = glam::Mat4::from_translation((centre - camera).as_vec3())
            * glam::Mat4::from_rotation_y(y_rot.to_radians())
            * glam::Mat4::from_rotation_x(x_rot.to_radians())
            * glam::Mat4::from_scale(glam::Vec3::splat(0.5 * scale))
            * glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.0, z_offset));
        let owner = super::item_draw::owner_at(centre.to_array(), 0.0, f.block());
        let resolved =
            game.resolve_item(&stack, horizon_assets::item_model::DisplayContext::Fixed, Some(&owner), pos[0] ^ pos[2]);
        game.submit_item_world(&resolved, m, horizon_client::block_entity_render::vertex_light(light), out);
    }
}

// ---- Chutes -------------------------------------------------------------------

fn chute_item(be: &ModBlockEntity) -> Chute<ItemStack> {
    let position = match be.nbt.get(&JavaString::from("ItemPosition")) {
        Some(Tag::Float(bits)) => f32::from_bits(*bits),
        _ => 1.0,
    };
    // Put in by a hopper or a funnel (`setItem`): from the top.
    let seen = matches!(be.nbt.get(&JavaString::from("HasItem")), Some(Tag::Byte(1)));
    let item = be.items.first().cloned().flatten();
    let position = if item.is_some() && !seen { 1.0 } else { position };
    Chute { item, position }
}

pub(super) fn set_chute_item(be: &mut ModBlockEntity, item: Option<ItemStack>, position: f32) {
    if be.items.is_empty() {
        be.items.push(None);
    }
    let has = item.is_some();
    be.items[0] = item;
    be.nbt.insert(JavaString::from("ItemPosition"), Tag::Float(position.to_bits()));
    be.nbt.insert(JavaString::from("HasItem"), Tag::Byte(i8::from(has)));
}

struct ChuteAt<'a, 'g> {
    game: &'a mut Game<'g>,
    pos: Pos,
    facing: Dir,
    shape: ChuteShape,
    /// The input chute `chute_above_accepts` chose.
    best_input: std::cell::Cell<Option<Pos>>,
    /// A smart chute's filter and power.
    smart: Option<(Filter, bool)>,
}

impl ChuteAt<'_, '_> {
    /// `canAcceptItem`'s filter, apart from the chute's world.
    fn passes(&self) -> impl Fn(&ItemStack) -> bool + use<> {
        let filter = self.smart.as_ref().map(|(f, _)| f.clone());
        move |s| filter.as_ref().is_none_or(|f| f.test(&s.item))
    }
}

impl ChuteWorld<ItemStack> for ChuteAt<'_, '_> {
    fn insert_below(&mut self, stack: &ItemStack, simulate: bool) -> Option<Option<ItemStack>> {
        let below = [self.pos[0], self.pos[1] - 1, self.pos[2]];
        if !has_inventory(self.game, below) {
            return None;
        }
        if belt_at(self.game, below).is_some() {
            if simulate {
                let (controller, index) = segment_of(self.game, below)?;
                let dim = self.game.dim.id().to_owned();
                let free = with_state(|s| {
                    s.belts
                        .get(&(dim, controller))
                        .is_some_and(|e| e.inv.can_insert_at_from_side(&e.belt, index, Dir::Up))
                });
                return Some(if free { None } else { Some(stack.clone()) });
            }
            return Some(insert_on_belt_segment(self.game, below, stack.clone()));
        }
        if simulate {
            // Horizon can't simulate a container's insertion: hopeful.
            return Some(None);
        }
        Some(insert_into(self.game, below, Dir::Up, stack.clone()))
    }
    fn active(&self) -> bool {
        self.smart.as_ref().is_none_or(|(_, powered)| !powered)
    }
    fn extraction_amount(&self) -> u32 {
        self.smart.as_ref().map_or(super::create_chute::CHUTE_EXTRACTION, |(f, _)| f.count)
    }
    fn facing_down(&self) -> bool {
        self.facing == Dir::Down
    }
    fn chute_below_accepts(&self, stack: &ItemStack) -> Option<bool> {
        let below = chute_target(self.pos, self.facing);
        if !is_chute(self.game, below) {
            return None;
        }
        Some(chute_accepts(self.game, below, stack))
    }
    fn give_chute_below(&mut self, stack: ItemStack) {
        let below = chute_target(self.pos, self.facing);
        let position = if self.facing == Dir::Down { 1.0 } else { 0.51 };
        self.game.native_with_mod_block_entity(below, |be| set_chute_item(be, Some(stack), position));
    }
    fn funnel_below_facing_down(&self) -> bool {
        funnel_at(self.game, [self.pos[0], self.pos[1] - 1, self.pos[2]]).is_some_and(|f| f.facing == Dir::Down)
    }
    fn rigid_below(&self) -> bool {
        sturdy(self.game, [self.pos[0], self.pos[1] - 1, self.pos[2]], Dir::Up)
    }
    fn drop_below(&mut self, stack: ItemStack) {
        let c = self.pos.map(|v| f64::from(v) + 0.5);
        self.game.spawn_item(stack, [c[0], c[1] - 12.0 / 16.0, c[2]], [0.0, -0.25, 0.0], PICKUP_DELAY);
    }
    fn extract_above(&mut self, max: u32) -> Option<ItemStack> {
        let above = [self.pos[0], self.pos[1] + 1, self.pos[2]];
        if is_chute(self.game, above) {
            return None;
        }
        let passes = self.passes();
        extract_from(self.game, above, max, &passes)
    }
    fn insert_above(&mut self, stack: &ItemStack, simulate: bool) -> Option<Option<ItemStack>> {
        let above = [self.pos[0], self.pos[1] + 1, self.pos[2]];
        // `isOpenChute`: only an open chute hands items up.
        if !super::create_chute::is_open(self.facing, self.shape)
            || !has_inventory(self.game, above)
            || belt_at(self.game, above).is_some()
        {
            return None;
        }
        if simulate {
            return Some(None);
        }
        Some(insert_into(self.game, above, Dir::Down, stack.clone()))
    }
    fn chute_above_accepts(&self, stack: &ItemStack) -> Option<bool> {
        let inputs = input_chutes(self.game, self.pos);
        if inputs.is_empty() {
            return None;
        }
        // The input chute taking it whose item moves up fastest (or stands).
        let mut best: Option<(Pos, f32)> = None;
        for p in inputs {
            let accepts = chute_accepts(self.game, p, stack);
            let motion = chute_motion(self.game, p);
            if accepts && motion >= 0.0 && best.is_none_or(|(_, m)| m < motion) {
                best = Some((p, motion));
            }
        }
        self.best_input.set(best.map(|(p, _)| p));
        Some(best.is_some())
    }
    fn give_chute_above(&mut self, stack: ItemStack) {
        let Some(above) = self.best_input.get() else {
            return;
        };
        self.game.native_with_mod_block_entity(above, |be| set_chute_item(be, Some(stack), 0.0));
    }
    fn blocked_above(&self) -> bool {
        let above = [self.pos[0], self.pos[1] + 1, self.pos[2]];
        // An encased fan is a full block in Create (its stand-in isn't).
        funnel_at(self.game, above).is_some_and(|f| f.facing == Dir::Up)
            || sturdy(self.game, above, Dir::Down)
            || matches!(super::create_glue::kind_at(self.game, above), Some(Kind::EncasedFan(_)))
    }
    fn drop_above(&mut self, stack: ItemStack, _: f32) {
        let c = self.pos.map(|v| f64::from(v) + 0.5);
        let motion = f64::from(chute_motion(self.game, self.pos)) * 2.0;
        self.game.spawn_item(stack, [c[0], c[1] + 8.0 / 16.0, c[2]], [0.0, motion, 0.0], PICKUP_DELAY);
    }
    fn extract_below(&mut self, max: u32) -> Option<ItemStack> {
        let below = [self.pos[0], self.pos[1] - 1, self.pos[2]];
        // A smart chute below gives to the chute above it (`grabCapability`).
        if is_chute(self.game, below) && !is_smart_chute(self.game, below) {
            return None;
        }
        let passes = self.passes();
        extract_from(self.game, below, max, &passes)
    }
}

/// `calculatePull` and `calculatePush`: a fan above a chute facing down
/// pulls with its speed, else the input chutes' pulls add up; a fan below
/// facing up pushes, else the target chute's push is shared among its
/// inputs.
fn chute_fans(game: &Game<'_>, pos: Pos) -> (f32, f32) {
    (chute_push(game, pos, 0), chute_pull(game, pos, 0))
}

/// Chutes followed at most for a fan (Create keeps each chute's own).
const CHUTE_REACH: u32 = 64;

fn fan_speed(game: &Game<'_>, p: Pos, facing: Dir) -> Option<f32> {
    match super::create_glue::kind_at(game, p) {
        Some(Kind::EncasedFan(f)) if f == facing => {
            Some(super::create_glue::with_kinetics(game, |k, level| k.speed_of(level, p)))
        }
        _ => None,
    }
}

fn chute_pull(game: &Game<'_>, pos: Pos, depth: u32) -> f32 {
    if let Some(speed) = fan_speed(game, above(pos), Dir::Down) {
        return speed;
    }
    if depth >= CHUTE_REACH {
        return 0.0;
    }
    input_chutes(game, pos).into_iter().map(|p| chute_pull(game, p, depth + 1)).sum()
}

fn chute_push(game: &Game<'_>, pos: Pos, depth: u32) -> f32 {
    if let Some(speed) = fan_speed(game, below(pos), Dir::Up) {
        return speed;
    }
    let Some((facing, _)) = chute_at(game, pos) else {
        return 0.0;
    };
    let target = chute_target(pos, facing);
    if depth >= CHUTE_REACH || !is_chute(game, target) {
        return 0.0;
    }
    let branches = input_chutes(game, target).len().max(1) as f32;
    chute_push(game, target, depth + 1) / branches
}

/// `updateAirFlow`, `extractFromBelt` and `findEntities` of an empty chute
/// blowing up: the first belt within reach below gives its middle item,
/// else an item entity in the air drawn in under it.
fn pull_from_below(
    game: &mut Game<'_>,
    pos: Pos,
    speed: f32,
    chute: &mut Chute<ItemStack>,
    accept: &dyn Fn(&ItemStack) -> bool,
) {
    let below = [pos[0], pos[1] - 1, pos[2]];
    let max = if is_chute(game, below) { 0.0 } else { max_pull_distance(speed) };
    let flow = if max > 0.0 {
        super::create_machines::flow_limit_down(game, pos, max)
    } else {
        max
    };
    let mut i = 1;
    while i as f32 <= flow + 1.0 {
        let at = [pos[0], pos[1] - i, pos[2]];
        if belt_at(game, at).is_some() {
            // handleCenteredProcessingOnAllItems(.5): the first one taken.
            let taken = with_belt_segment(game, at, |inv, _, index| {
                let mut taken = None;
                inv.apply_to_each_within(index as f32 + 0.5, 0.5, |t| {
                    if taken.is_some() || !accept(&t.stack) {
                        return None;
                    }
                    taken = Some(t.stack.clone());
                    Some(super::create_belt::Conversion {
                        outputs: Vec::new(),
                        held: None,
                        locked: false,
                    })
                });
                taken
            })
            .flatten();
            if let Some(stack) = taken {
                chute.set_item(Some(stack), -(i - 1) as f32);
                return;
            }
            break;
        }
        i += 1;
    }
    let distance = f64::from(flow.max(0.0));
    if distance <= 0.0 {
        return;
    }
    // The box from the middle, `bottomPullDistance` + .5 down to .5 down,
    // grown by .45; an item's box is .25 high.
    let [x, y, z] = pos.map(f64::from);
    let found = game.dropped.iter().position(|d| {
        let p = d.position;
        d.stack.count > 0
            && accept(&d.stack)
            && (x + 0.05 - 0.125..=x + 0.95 + 0.125).contains(&p.x)
            && (z + 0.05 - 0.125..=z + 0.95 + 0.125).contains(&p.z)
            && (y - distance - 0.45 - 0.25..=y + 0.45).contains(&p.y)
    });
    if let Some(i) = found {
        let d = &mut game.dropped[i];
        let height = (d.position.y + 0.125 - y) as f32;
        chute.set_item(Some(d.stack.clone()), height);
        d.stack.count = 0;
        game.dropped.retain(|d| d.stack.count > 0);
    }
}

/// `getItemMotion` of the chute at `pos`.
fn chute_motion(game: &Game<'_>, pos: Pos) -> f32 {
    let (push, pull) = chute_fans(game, pos);
    item_motion(push, pull)
}

/// `ChuteBlockEntity.tick` on the server for the chute at `pos`, and
/// `ChuteBlock.updateEntityAfterFallOn` (items falling on top go in).
pub(super) fn chute_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let (push, pull) = chute_fans(game, pos);
    let motion = item_motion(push, pull);
    // SmartChuteBlock.tick: powered from around, it stops (noted for the
    // chutes handing it items).
    let smart = if is_smart_chute(game, pos) {
        let powered = super::create_glue::signal(game, pos);
        entity.nbt.insert(JavaString::from("Powered"), Tag::Byte(i8::from(powered)));
        Some((read_smart_filter(entity).0, powered))
    } else {
        None
    };
    let accept = |s: &ItemStack| smart.as_ref().is_none_or(|(f, powered)| !powered && f.test(&s.item));
    let seen = matches!(entity.nbt.get(&JavaString::from("HasItem")), Some(Tag::Byte(1)));
    let mut chute = chute_item(entity);
    // `setItem(stack)`: put in from the top, or the bottom when blown up.
    if chute.item.is_some() && !seen && motion > 0.0 {
        chute.position = 0.0;
    }
    // updateEntityAfterFallOn, when the chute takes items from above.
    if chute.item.is_none() && has_direct_input(game, pos) {
        super::create_machines::take_landed(game, pos, 0.75, |_, stack| {
            if chute.item.is_some() || !accept(&stack) {
                return Some(stack);
            }
            chute.set_item(Some(stack), 1.0);
            None
        });
    }
    if chute.item.is_none() && motion > 0.0 {
        pull_from_below(game, pos, pull - push, &mut chute, &accept);
    }
    let (facing, shape) = chute_at(game, pos).unwrap_or((Dir::Down, ChuteShape::Normal));
    let mut world = ChuteAt {
        game,
        pos,
        facing,
        shape,
        best_input: std::cell::Cell::new(None),
        smart: smart.clone(),
    };
    chute.tick(motion, &mut world);
    set_chute_item(entity, chute.item.clone(), chute.position);
    let mut view = vec![super::create_machines::stack_view(chute.item.as_ref())];
    view.push((String::new(), (chute.position.clamp(0.0, 1.0) * 200.0) as u32));
    // FilteringRenderer: a smart chute's filter and amount.
    match &smart {
        Some((f, _)) => view.push((f.item.clone().unwrap_or_default(), f.count)),
        None => view.push((String::new(), 0)),
    }
    // For the goggles (`addToGoggleTooltip`): the fans' push and pull and
    // which way items move.
    let sign = |v: f32, up: u32, down: u32| if v > 0.0 { up } else if v < 0.0 { down } else { 0 };
    let powered = smart.as_ref().is_some_and(|(_, p)| *p);
    let flags = sign(push, 1, 2) | sign(pull, 4, 8) | if motion > 0.0 { 0 } else { 16 } | u32::from(powered) << 5;
    view.push((String::new(), flags));
    super::create_machines::note_view(game, pos, view);
}

/// `ChuteBlockEntity.addToGoggleTooltip`, from what this client was told
/// of the chute at `pos`, in Create's English (`create.tooltip.chute.*`).
pub(super) fn chute_goggle_lines(game: &Game<'_>, pos: Pos) -> Option<Vec<(String, u32)>> {
    if !block_name(game, pos).is_some_and(|n| is_chute_block(&n)) {
        return None;
    }
    let slots = super::create_machines::client_slots(pos)?;
    let flags = slots.get(3).map_or(16, |(_, f)| *f);
    let (gray, yellow, green, white) = (0xAAAAAA, 0xFFFF55, 0x55FF55, 0xFFFFFF);
    let mut lines = vec![("Chute Information".to_owned(), white)];
    if flags & 15 == 0 {
        lines.push((" No attached fans".to_owned(), gray));
    }
    // `pull` > 0 pulls from above; `push` > 0 pushes from below.
    match flags & 12 {
        4 => lines.push((" Fans pull from Above".to_owned(), gray)),
        8 => lines.push((" Fans push from Above".to_owned(), gray)),
        _ => {}
    }
    match flags & 3 {
        1 => lines.push((" Fans push from Below".to_owned(), gray)),
        2 => lines.push((" Fans pull from Below".to_owned(), gray)),
        _ => {}
    }
    let way = if flags & 16 != 0 { "Downward" } else { "Upward" };
    lines.push((format!(" -> Items move {way}"), yellow));
    if let Some((id, count)) = slots.first().filter(|(id, c)| !id.is_empty() && *c > 0) {
        let name = game.pack.names.display_item(id);
        lines.push((format!(" Contains: {name} x{count}"), green));
    }
    Some(lines)
}

// ---- Saving ---------------------------------------------------------------------

fn save_path(root: &Path, dimension: &str) -> PathBuf {
    root.join("data").join(format!("create_logistics_{}.txt", dimension.replace(':', "_")))
}

fn load(game: &Game<'_>, dimension: &str) {
    let root = game.save.root.clone();
    let fresh = with_state(|s| {
        s.root.get_or_insert_with(|| root.clone());
        s.loaded.insert(dimension.to_owned())
    });
    if !fresh {
        return;
    }
    let Ok(text) = std::fs::read_to_string(save_path(&root, dimension)) else {
        return;
    };
    let mut belts: Vec<(Pos, BeltInventory<ItemStack>)> = Vec::new();
    let mut funnels = Vec::new();
    let mut tunnels = Vec::new();
    let mut lines = text.lines();
    if lines.next() != Some("create-logistics 1") {
        return;
    }
    let pos = |f: &[&str]| -> Option<Pos> { Some([f[1].parse().ok()?, f[2].parse().ok()?, f[3].parse().ok()?]) };
    for line in lines {
        let f: Vec<&str> = line.split(' ').collect();
        match f.first().copied() {
            Some("belt") if f.len() == 5 => {
                if let Some(p) = pos(&f) {
                    let mut inv = BeltInventory::new();
                    inv.movement_positive = f[4] == "1";
                    belts.push((p, inv));
                }
            }
            Some("item") if f.len() == 8 => {
                let Some((_, inv)) = belts.last_mut() else {
                    continue;
                };
                let num = |i: usize| f[i].parse::<f32>().unwrap_or(0.0);
                let int = |i: usize| f[i].parse::<i32>().unwrap_or(0);
                let mut stack = ItemStack::new(f[7], int(5).max(1) as u32);
                stack.damage = int(6).max(0) as u32;
                let mut t = Transported::new(stack, int(3));
                t.belt_position = num(1);
                t.prev_belt_position = t.belt_position;
                t.side_offset = num(2);
                t.prev_side_offset = t.side_offset;
                t.inserted_at = int(4);
                inv.items.push(t);
            }
            // A brass funnel's filter: its amount and item.
            Some("funnel") if (4..=6).contains(&f.len()) => {
                if let Some(p) = pos(&f) {
                    let mut filter = Filter::default();
                    if f.len() == 5 {
                        filter.count = f[4].parse().unwrap_or(64);
                    }
                    if f.len() == 6 {
                        filter.count = f[4].parse().unwrap_or(64);
                        filter.set(Some(f[5]));
                    }
                    funnels.push((p, filter));
                }
            }
            // A tunnel: its mode, and a brass tunnel's stack (where it came
            // from, count, damage, item).
            Some("tunnel") if f.len() == 5 || f.len() == 9 => {
                let Some(p) = pos(&f) else {
                    continue;
                };
                let mut t = TunnelData::default();
                t.brass.mode = SelectionMode::parse(f[4]).unwrap_or_default();
                if f.len() == 9 {
                    let mut stack = ItemStack::new(f[8], f[6].parse::<u32>().unwrap_or(1).max(1));
                    stack.damage = f[7].parse().unwrap_or(0);
                    t.brass.stack = Some(stack);
                    t.brass.entered_from = Dir::parse(f[5]);
                    t.brass.progress = -1;
                }
                tunnels.push((p, t));
            }
            _ => {}
        }
    }
    with_state(|s| {
        for (p, inv) in belts {
            let belt = Belt {
                controller: p,
                state: BeltState {
                    facing: Dir::North,
                    slope: Slope::Horizontal,
                    part: super::create_belt::Part::Start,
                },
                length: 2,
                speed: 0.0,
            };
            s.belts.insert((dimension.to_owned(), p), Entry { inv, belt });
        }
        for (p, filter) in funnels {
            s.funnels.insert((dimension.to_owned(), p), FunnelData { cooldown: 0, filter });
        }
        for (p, t) in tunnels {
            s.tunnels.insert((dimension.to_owned(), p), t);
        }
    });
}

/// Saves every dimension's belts, funnels and tunnels beside the world
/// (items keep their id, count and damage; other components aren't saved
/// yet).
pub(super) fn save_all() {
    with_state(|s| {
        let Some(root) = s.root.clone() else {
            return;
        };
        let _ = std::fs::create_dir_all(root.join("data"));
        let mut dims: Vec<String> = s.loaded.iter().cloned().collect();
        dims.sort();
        for dimension in dims {
            let mut out = String::from("create-logistics 1\n");
            let mut belts: Vec<(&Pos, &Entry)> =
                s.belts.iter().filter(|((d, _), _)| *d == dimension).map(|((_, p), e)| (p, e)).collect();
            belts.sort_by_key(|(p, _)| **p);
            for (p, e) in belts {
                out.push_str(&format!("belt {} {} {} {}\n", p[0], p[1], p[2], u8::from(e.inv.movement_positive)));
                for t in e.inv.items.iter().chain(&e.inv.to_insert) {
                    out.push_str(&format!(
                        "item {} {} {} {} {} {} {}\n",
                        t.belt_position, t.side_offset, t.angle, t.inserted_at, t.stack.count, t.stack.damage, t.stack.item
                    ));
                }
            }
            let mut funnels: Vec<(&Pos, &FunnelData)> =
                s.funnels.iter().filter(|((d, _), _)| *d == dimension).map(|((_, p), f)| (p, f)).collect();
            funnels.sort_by_key(|(p, _)| **p);
            for (p, f) in funnels {
                match &f.filter.item {
                    Some(item) => {
                        out.push_str(&format!("funnel {} {} {} {} {item}\n", p[0], p[1], p[2], f.filter.count))
                    }
                    None => out.push_str(&format!("funnel {} {} {} {}\n", p[0], p[1], p[2], f.filter.count)),
                }
            }
            let mut tunnels: Vec<(&Pos, &TunnelData)> =
                s.tunnels.iter().filter(|((d, _), _)| *d == dimension).map(|((_, p), t)| (p, t)).collect();
            tunnels.sort_by_key(|(p, _)| **p);
            for (p, t) in tunnels {
                let b = &t.brass;
                out.push_str(&format!("tunnel {} {} {} {}", p[0], p[1], p[2], b.mode.key()));
                if let Some(stack) = &b.stack {
                    let from = b.entered_from.map_or("-", Dir::name);
                    out.push_str(&format!(" {from} {} {} {}", stack.count, stack.damage, stack.item));
                }
                out.push('\n');
            }
            if let Err(e) = std::fs::write(save_path(&root, &dimension), out) {
                log::warn!("Create: can't save the belts of {dimension}: {e}");
            }
        }
    });
}

/// A world opened or closed: saved when it closes, then everything starts
/// fresh.
pub(super) fn world_phase(closing: bool) {
    if closing {
        save_all();
    }
    *state() = None;
    *MAX_STACK.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *CLIENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Runs on a belt's items from outside its tick (a fan's current):
/// `f` gets the inventory, the belt and the segment's index.
pub(super) fn with_belt_segment<R>(
    game: &Game<'_>,
    pos: Pos,
    f: impl FnOnce(&mut BeltInventory<ItemStack>, &Belt, i32) -> R,
) -> Option<R> {
    let (controller, index) = segment_of(game, pos)?;
    let dim = game.dim.id().to_owned();
    with_state(|s| {
        let e = s.belts.get_mut(&(dim, controller))?;
        let belt = e.belt;
        Some(f(&mut e.inv, &belt, index))
    })
}
