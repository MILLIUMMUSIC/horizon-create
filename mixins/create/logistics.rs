// Horizon's side of Create's logistics, injected as
// `crate::game::create_logistics`: the mechanical belt's items (one
// `BeltInventory` per controller, kept per dimension and saved beside the
// world: belts have no block entity here), what they meet (presses, depots,
// chutes, funnels, the next belt, walls), items dropped on belts, the belt
// connector, the chute (a mod block entity), andesite funnels (no block
// entity: their timers live here), and drawing what belts and chutes carry.
// The rules are in belt.rs, chute.rs and funnel.rs.

use super::Game;
use super::create_belt::{
    Belt, BeltFunnel, BeltInventory, BeltLevel, BeltState, BeltWorld, Chain, ConnectorLevel,
    Conversion, ProcessingResult, Slope, Stack, Transported, belt_chain, belts_between, can_connect,
    find_chain, item_place, on_removed,
};
use super::create_chute::{Chute, ChuteWorld, item_motion};
use super::create_funnel::{
    BeltBelow, EXTRACTION_TIMER, FunnelState, Mode, amount_to_extract, as_belt_funnel,
    collects_entity_at, extracted_item, mode, overflow_area, tick_cooldown,
};
use super::create_kinetics::{Axis, Dir, Pos};
use horizon_client::containers::ModBlockEntity;
use horizon_client::items::ItemStack;
use horizon_world::nbt::{JavaString, Tag};
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

#[derive(Default)]
struct Logistics {
    belts: HashMap<(String, Pos), Entry>,
    /// Funnels by dimension, with their extraction timers.
    funnels: HashMap<(String, Pos), i32>,
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
    super::create_belt::BELT_BLOCKS.iter().any(|(n, _)| *n == name)
}

/// Blocks of ours outside the kinetics, whose changes the glue reports.
pub(super) fn is_ours(name: &str) -> bool {
    is_belt(name)
        || name == CHUTE
        || name == super::create_funnel::EXTRACTING_FUNNEL
        || name == super::create_funnel::COLLECTING_FUNNEL
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

fn is_chute(game: &Game<'_>, pos: Pos) -> bool {
    block_name(game, pos).as_deref() == Some(CHUTE)
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

fn core_dir(d: Dir) -> horizon_core::geometry::Direction {
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
    if belt_at(game, pos).is_some_and(BeltState::can_transport_objects) {
        return true;
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
    if belt_at(game, pos).is_some() {
        return insert_on_belt_segment(game, pos, stack);
    }
    game.native_insert_item(pos, core_dir(face), stack)
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

/// `ItemHelper.extract` of up to `max` of one item from the inventory at
/// `pos` (chests and mod block entities' output slots).
fn extract_from(game: &mut Game<'_>, pos: Pos, max: u32) -> Option<ItemStack> {
    use horizon_client::containers::BlockEntity as B;
    // ItemHandlerBeltSegment.extractItem: the item over the segment.
    if belt_at(game, pos).is_some() {
        return with_belt_segment(game, pos, |inv, _, index| inv.extract_at(index, max, false)).flatten();
    }
    let key = (pos[0], pos[1], pos[2]);
    if let Some(B::Mod(m)) = game.world.block_entities.get(&key) {
        if m.id == CHUTE {
            return None;
        }
        let outputs = mod_outputs(&m.id);
        return game
            .native_with_mod_block_entity(pos, |be| {
                for i in outputs {
                    let Some(Some(s)) = be.items.get_mut(i) else {
                        continue;
                    };
                    let n = s.count.min(max);
                    let mut out = s.clone();
                    out.count = n;
                    s.count -= n;
                    if s.count == 0 {
                        be.items[i] = None;
                    }
                    return Some(out);
                }
                None
            })
            .flatten();
    }
    let Some(B::Chest(items)) = game.world.block_entities.get_mut(&key) else {
        return None;
    };
    let slot = items.iter_mut().find(|s| s.is_some())?;
    let s = slot.as_mut()?;
    let n = s.count.min(max);
    let mut out = s.clone();
    out.count = n;
    s.count -= n;
    if s.count == 0 {
        *slot = None;
    }
    game.world.modified.insert((pos[0] >> 4, pos[2] >> 4));
    Some(out)
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
        let hold = super::create_machines::press_received(self.game, press, &item.stack)?;
        Some((if hold { ProcessingResult::Hold } else { ProcessingResult::Pass }, None))
    }
    fn held(
        &mut self,
        pos: Pos,
        item: &Transported<ItemStack>,
    ) -> Option<(ProcessingResult, Option<Conversion<ItemStack>>)> {
        let press = [pos[0], pos[1] + 2, pos[2]];
        let (hold, pressed) = super::create_machines::press_held(self.game, press, &item.stack)?;
        let result = if hold { ProcessingResult::Hold } else { ProcessingResult::Pass };
        Some((result, pressed.map(|(outputs, held)| Conversion {
            outputs,
            held,
            locked: true,
        })))
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
        let behind = f.behind(pos);
        if !has_inventory(self.game, behind) {
            return Some(stack.clone());
        }
        insert_into(self.game, behind, f.facing, stack.clone())
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
}

/// A `DirectBeltInputBehaviour` at `pos`: a belt, a depot, a chute open at
/// the top, a collecting funnel facing up.
fn has_direct_input(game: &Game<'_>, pos: Pos) -> bool {
    if belt_at(game, pos).is_some_and(BeltState::can_transport_objects) {
        return true;
    }
    match block_name(game, pos).as_deref() {
        Some(super::create_machines::DEPOT) => true,
        // canDirectlyInsert: not under another chute.
        Some(CHUTE) => !is_chute(game, [pos[0], pos[1] + 1, pos[2]]),
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
        Some(CHUTE) => game
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
            if !f.takes_from_belt_end() {
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
            let m = glam::Mat4::from_translation((at - camera).as_vec3())
                * glam::Mat4::from_axis_angle(tilt_axis, tilt.to_radians())
                * glam::Mat4::from_rotation_y((f32::from(t.angle)).to_radians())
                * glam::Mat4::from_translation(glam::Vec3::new(0.0, -0.09375, 0.0))
                * glam::Mat4::from_rotation_x(90f32.to_radians())
                * glam::Mat4::from_scale(glam::Vec3::splat(0.5));
            let stack = ItemStack::new(&t.id, t.count);
            let owner = super::item_draw::owner_at(p, 0.0, "create:belt");
            let resolved = game.resolve_item(
                &stack,
                horizon_assets::item_model::DisplayContext::Fixed,
                Some(&owner),
                pos[0] ^ pos[2] ^ i as i32,
            );
            game.submit_item_world(&resolved, m, horizon_client::block_entity_render::vertex_light(light), out);
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
    let Some((id, count)) = slots.first() else {
        return;
    };
    if *count == 0 || id.is_empty() {
        return;
    }
    let place = slots.get(1).map_or(0.5, |(_, p)| *p as f32 / 200.0);
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

// ---- Players ------------------------------------------------------------------

/// `native_right_click_block` on a belt: an empty hand takes the items on
/// the segment (`BeltBlock.useItemOn`), a shaft on a middle segment makes a
/// pulley. True for every belt: the template's own menu never opens.
pub(super) fn right_click(game: &mut Game<'_>, pos: Pos) -> bool {
    let Some(state) = belt_at(game, pos) else {
        return false;
    };
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    let mode = game.native_acting_player().map_or(0, |p| p.game_mode);
    if sneaking || mode == 2 || mode == 3 {
        return true;
    }
    let held = game.held().cloned();
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
        _ => {}
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
        let placeable = belts.iter().all(|(_, s)| s.block().is_some());
        if unbreakable || !placeable {
            if !placeable {
                game.native_action_bar(
                    player.uuid,
                    horizon_client::text::Text::literal("Vertical and sideways belts aren't ported yet".to_owned()),
                );
            }
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
    let Some(f) = funnel_at(game, pos) else {
        return;
    };
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    let dim = game.dim.id().to_owned();
    with_state(|s| {
        s.funnels.insert((dim.clone(), pos), 0);
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
    if name == super::create_funnel::COLLECTING_FUNNEL {
        super::create_funnel::EXTRACTING_FUNNEL
    } else {
        name
    }
}

// ---- Funnels ------------------------------------------------------------------

/// Block changes of ours outside the kinetics: funnels come and go.
pub(super) fn changed(game: &Game<'_>, dimension: &str, positions: &[Pos]) {
    let found: Vec<(Pos, bool)> = positions.iter().map(|&p| (p, funnel_at(game, p).is_some())).collect();
    with_state(|s| {
        for (p, funnel) in found {
            let key = (dimension.to_owned(), p);
            if funnel {
                s.funnels.entry(key).or_insert(0);
            } else {
                s.funnels.remove(&key);
            }
        }
    });
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
    let funnels: Vec<(Pos, i32)> = with_state(|s| {
        s.funnels
            .iter()
            .filter(|((d, _), _)| d == dimension)
            .map(|((_, p), c)| (*p, *c))
            .collect()
    });
    for (pos, mut cooldown) in funnels {
        if !game.world.loaded(pos[0], pos[2]) {
            continue;
        }
        let Some(f) = funnel_at(game, pos) else {
            with_state(|s| s.funnels.remove(&(dimension.to_owned(), pos)));
            continue;
        };
        let below = [pos[0], pos[1] - 1, pos[2]];
        let belt = belt_at(game, below).filter(|b| b.can_transport_objects()).map(|b| {
            let speed = super::create_glue::with_kinetics(game, |k, level| k.speed_of(level, below));
            BeltBelow {
                facing: b.facing,
                movement: b.movement_facing(speed),
            }
        });
        let m = mode(f, belt);
        if m == Mode::Collect {
            collect(game, pos, f);
        }
        if tick_cooldown(&mut cooldown, m) {
            let behind = f.behind(pos);
            let acted = match m {
                Mode::Extract => extract(game, pos, f, behind),
                Mode::PushingToBelt => push_to_belt(game, below, f, behind),
                _ => false,
            };
            if acted {
                cooldown = EXTRACTION_TIMER;
            }
        }
        with_state(|s| {
            if let Some(c) = s.funnels.get_mut(&(dimension.to_owned(), pos)) {
                *c = cooldown;
            }
        });
    }
}

/// `activateExtractor`: one item out of the inventory behind, unless an
/// item still lies in front.
fn extract(game: &mut Game<'_>, pos: Pos, f: FunnelState, behind: Pos) -> bool {
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
    let Some(stack) = extract_from(game, behind, amount_to_extract(Mode::Extract)) else {
        return false;
    };
    let (at, motion) = extracted_item(pos, f.facing);
    game.spawn_item(stack, at, motion, PICKUP_DELAY);
    true
}

/// `activateExtractingBeltFunnel`: one item onto the belt below, from the
/// funnel's side.
fn push_to_belt(game: &mut Game<'_>, below: Pos, f: FunnelState, behind: Pos) -> bool {
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
    let Some(stack) = extract_from(game, behind, amount_to_extract(Mode::PushingToBelt)) else {
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
fn collect(game: &mut Game<'_>, pos: Pos, f: FunnelState) {
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
        if !inside || !collects_entity_at(pos, f.facing, p) {
            continue;
        }
        let stack = d.stack.clone();
        let left = insert_into(game, behind, f.facing, stack.clone());
        let n = left.map_or(0, |l| l.count);
        if n != stack.count {
            game.dropped[i].stack.count = n;
            changed = true;
        }
    }
    if changed {
        game.dropped.retain(|d| d.stack.count > 0);
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

fn set_chute_item(be: &mut ModBlockEntity, item: Option<ItemStack>, position: f32) {
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
    fn chute_below_accepts(&self) -> Option<bool> {
        let below = [self.pos[0], self.pos[1] - 1, self.pos[2]];
        if !is_chute(self.game, below) {
            return None;
        }
        let empty = |game: &Game<'_>| {
            use horizon_client::containers::BlockEntity as B;
            match game.world.block_entities.get(&(below[0], below[1], below[2])) {
                Some(B::Mod(m)) => m.items.first().is_none_or(Option::is_none),
                _ => false,
            }
        };
        Some(empty(self.game))
    }
    fn give_chute_below(&mut self, stack: ItemStack) {
        let below = [self.pos[0], self.pos[1] - 1, self.pos[2]];
        self.game.native_with_mod_block_entity(below, |be| set_chute_item(be, Some(stack), 1.0));
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
        extract_from(self.game, above, max)
    }
}

/// `ChuteBlockEntity.tick` on the server for the chute at `pos`, and
/// `ChuteBlock.updateEntityAfterFallOn` (items falling on top go in).
pub(super) fn chute_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let mut chute = chute_item(entity);
    if chute.item.is_none() {
        super::create_machines::take_landed(game, pos, 0.75, |_, stack| {
            if chute.item.is_some() {
                return Some(stack);
            }
            chute.set_item(Some(stack), 1.0);
            None
        });
    }
    let motion = item_motion(0.0, 0.0);
    chute.tick(motion, &mut ChuteAt { game, pos });
    set_chute_item(entity, chute.item.clone(), chute.position);
    let mut view = vec![super::create_machines::stack_view(chute.item.as_ref())];
    view.push((String::new(), (chute.position.clamp(0.0, 1.0) * 200.0) as u32));
    super::create_machines::note_view(game, pos, view);
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
            Some("funnel") if f.len() == 4 => funnels.extend(pos(&f)),
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
        for p in funnels {
            s.funnels.insert((dimension.to_owned(), p), 0);
        }
    });
}

/// Saves every dimension's belts and funnels beside the world (items keep
/// their id, count and damage; other components aren't saved yet).
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
            let mut funnels: Vec<Pos> =
                s.funnels.keys().filter(|(d, _)| *d == dimension).map(|(_, p)| *p).collect();
            funnels.sort();
            for p in funnels {
                out.push_str(&format!("funnel {} {} {}\n", p[0], p[1], p[2]));
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
