// Horizon's side of Create's machines, injected as `crate::game::create_machines`:
// the depot (DepotBehaviour) and millstone (MillstoneBlockEntity) as mod
// block entities with Horizon's own item slots, so hoppers and breaking work
// as for any container; the press's work on a depot or on items on the
// ground (PressingBehaviour, BeltPressingCallbacks); the water wheel's flow
// (WaterWheelBlockEntity). The rules are in processing.rs and kinetics.rs.

use super::Game;
use super::create_kinetics::{Dir, Kind, Pos};
use super::create_processing::{
    PRESS_CYCLE, PressMode, PressStep, Recipe, ingredient_json, millstone_speed, parse_recipe,
    roll_results, water_wheel_flow_score,
};
use horizon_client::containers::{ModBlockEntity, ModContainer};
use horizon_client::crafting::Ingredient;
use horizon_client::items::ItemStack;
use horizon_world::nbt::{Compound, JavaString, Tag};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

pub(super) const DEPOTS: &str = "create:depots";
const DEPOT: &str = "create:depot";
const MILLSTONE: &str = "create:millstone";
/// DepotBehaviour: the held item, then `processingOutputBuffer`.
const DEPOT_SLOTS: usize = 9;
/// MillstoneBlockEntity: `inputInv`, then `outputInv`.
const MILLSTONE_SLOTS: usize = 10;

#[derive(Default)]
struct Machines {
    /// Recipes by type, with their ingredient as the game reads it.
    recipes: HashMap<&'static str, Vec<(Recipe, Option<Ingredient>)>>,
    /// What clients were last told each depot holds, by dimension.
    depots: HashMap<(String, Pos), Vec<(String, u32)>>,
    /// Depot changes for the level's tick end.
    pending: Vec<(String, Pos, Option<Vec<(String, u32)>>)>,
    random: u64,
}

static MACHINES: Mutex<Option<Machines>> = Mutex::new(None);
static CLIENT: Mutex<Option<HashMap<Pos, Vec<(String, u32)>>>> = Mutex::new(None);

fn machines() -> MutexGuard<'static, Option<Machines>> {
    MACHINES.lock().unwrap_or_else(|e| e.into_inner())
}

impl Machines {
    /// The level's random, for rolling outputs (`level.random.nextFloat`).
    fn next_float(&mut self) -> f32 {
        // xorshift64*: any fair source does for chances.
        let mut x = self.random.max(1);
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.random = x;
        ((x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32) / (1u64 << 24) as f32
    }
}

/// A world opened or closed: recipes and depot views start fresh.
pub(super) fn reset() {
    *machines() = None;
    *CLIENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

// ---- Block entities -------------------------------------------------------

/// `native_block_entity`: the depot's and millstone's block entities.
pub(super) fn block_entity(block: &str, entity: &mut Option<(String, Compound)>) {
    if entity.is_none() && (block == DEPOT || block == MILLSTONE) {
        *entity = Some((block.to_owned(), Compound::new()));
    }
}

/// `native_block_entity_ticks`.
pub(super) fn block_entity_ticks(block: &str, ticks: &mut bool) {
    if block == DEPOT || block == MILLSTONE {
        *ticks = true;
    }
}

/// `native_block_entity_container`: DepotItemHandler (insert into the held
/// slot, take from any) and the millstone's input and output inventories.
pub(super) fn block_entity_container(block: &str, container: &mut Option<ModContainer>) {
    let (slots, inputs, outputs): (usize, Vec<usize>, Vec<usize>) = match block {
        DEPOT => (DEPOT_SLOTS, vec![0], (0..DEPOT_SLOTS).collect()),
        MILLSTONE => (MILLSTONE_SLOTS, vec![0], (1..MILLSTONE_SLOTS).collect()),
        _ => return,
    };
    let all: Vec<usize> = (0..slots).collect();
    *container = Some(ModContainer {
        slots,
        faces: std::array::from_fn(|_| all.clone()),
        inputs,
        outputs,
        menu: None,
    });
}

fn get_bool(nbt: &Compound, key: &str) -> bool {
    matches!(nbt.get(&JavaString::from(key)), Some(Tag::Byte(b)) if *b != 0)
}

fn put_bool(nbt: &mut Compound, key: &str, v: bool) {
    nbt.insert(JavaString::from(key), Tag::Byte(i8::from(v)));
}

fn get_int(nbt: &Compound, key: &str) -> i32 {
    match nbt.get(&JavaString::from(key)) {
        Some(Tag::Int(v)) => *v,
        _ => 0,
    }
}

fn put_int(nbt: &mut Compound, key: &str, v: i32) {
    nbt.insert(JavaString::from(key), Tag::Int(v));
}

fn max_stack(game: &Game<'_>, stack: &ItemStack) -> u32 {
    game.items().get(&stack.item).map_or(64, |i| i.max_stack)
}

/// `ItemHandlerHelper.insertItemStacked` into `slots`: onto matching stacks
/// first, then into empty ones; what didn't fit.
fn insert_stacked(
    game: &Game<'_>,
    items: &mut [Option<ItemStack>],
    slots: std::ops::Range<usize>,
    mut stack: ItemStack,
) -> Option<ItemStack> {
    let max = max_stack(game, &stack);
    for i in slots.clone() {
        if let Some(s) = items[i].as_mut()
            && s.same_item_same_components(&stack)
            && s.count < max
        {
            let moved = (max - s.count).min(stack.count);
            s.count += moved;
            stack.count -= moved;
            if stack.count == 0 {
                return None;
            }
        }
    }
    for i in slots {
        if items[i].is_none() {
            items[i] = Some(stack);
            return None;
        }
    }
    Some(stack)
}

/// The recipes of `kind` (`create:pressing`) that take `stack`, first.
fn find_recipe(game: &Game<'_>, kind: &'static str, stack: &ItemStack) -> Option<Recipe> {
    let mut guard = machines();
    let m = guard.get_or_insert_with(Machines::default);
    // The server's table straight: `native_recipes_of_type` and
    // `native_recipe_ingredient` answer only inside a player's hook, and a
    // block entity's tick has no player.
    let list = m.recipes.entry(kind).or_insert_with(|| {
        game.recipes()
            .custom
            .get(kind)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter_map(|r| parse_recipe(&r.id, &r.json))
            .map(|r| {
                let ingredient = ingredient(game, &ingredient_json(&r));
                (r, ingredient)
            })
            .collect()
    });
    list.iter()
        .find(|(_, i)| i.as_ref().is_some_and(|i| i.test(stack)))
        .map(|(r, _)| r.clone())
}

/// `native_recipe_ingredient` without its player: ids and `#tags`, bare
/// ones as `minecraft:`, items the game doesn't know left out.
fn ingredient(game: &Game<'_>, json: &serde_json::Value) -> Option<Ingredient> {
    use horizon_client::atlas_sources::qualified;
    let qualify = |s: &str| match s.strip_prefix('#') {
        Some(tag) => String::from("#") + &qualified(tag),
        None => qualified(s),
    };
    let list = json
        .as_array()?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(|s| serde_json::Value::String(qualify(s)))
        .collect();
    let items = game.items();
    let known: Vec<String> = Ingredient::parse(&serde_json::Value::Array(list), items)?
        .1
        .into_iter()
        .filter(|item| items.get(item).is_some())
        .collect();
    (!known.is_empty()).then(|| Ingredient::of(known))
}

fn roll(r: &Recipe) -> Vec<ItemStack> {
    let mut guard = machines();
    let m = guard.get_or_insert_with(Machines::default);
    roll_results(r, || m.next_float())
        .into_iter()
        .map(|(id, count)| ItemStack::new(&id, count))
        .collect()
}

/// A depot's slots as clients are told them: the held item first.
fn depot_view(items: &[Option<ItemStack>]) -> Vec<(String, u32)> {
    items
        .iter()
        .map(|s| s.as_ref().map_or((String::new(), 0), |s| (s.item.to_string(), s.count)))
        .collect()
}

fn note_depot(game: &Game<'_>, pos: Pos, items: &[Option<ItemStack>]) {
    let view = depot_view(items);
    let dim = game.dim.id().to_owned();
    let mut guard = machines();
    let m = guard.get_or_insert_with(Machines::default);
    if m.depots.get(&(dim.clone(), pos)) != Some(&view) {
        m.depots.insert((dim.clone(), pos), view.clone());
        m.pending.push((dim, pos, Some(view)));
    }
}

/// `native_block_entity_tick`.
pub(super) fn block_entity_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    match entity.id.as_str() {
        DEPOT => depot_tick(game, pos, entity),
        MILLSTONE => millstone_tick(game, pos, entity),
        _ => {}
    }
}

/// `ItemEntity`s resting on the block at `pos` (`updateEntityAfterFallOn`):
/// their stacks go in through `accept`, which gives back what it left.
fn take_landed(
    game: &mut Game<'_>,
    pos: Pos,
    top: f64,
    mut accept: impl FnMut(&Game<'_>, ItemStack) -> Option<ItemStack>,
) {
    let [x, y, z] = pos.map(f64::from);
    let landed: Vec<usize> = (0..game.dropped.len())
        .filter(|&i| {
            let d = &game.dropped[i];
            let p = d.position;
            d.on_ground
                && d.stack.count > 0
                && (x..x + 1.0).contains(&p.x)
                && (z..z + 1.0).contains(&p.z)
                && (y + top - 0.3..=y + 1.25).contains(&p.y)
        })
        .collect();
    let mut changed = false;
    for i in landed {
        let stack = game.dropped[i].stack.clone();
        let count = stack.count;
        let left = accept(game, stack);
        let left_count = left.as_ref().map_or(0, |s| s.count);
        if left_count != count {
            changed = true;
            game.dropped[i].stack.count = left_count;
        }
    }
    if changed {
        game.dropped.retain(|d| d.stack.count > 0);
    }
}

/// `DepotBehaviour.tick` with the press above as its `BeltProcessingBehaviour`.
fn depot_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    if entity.items.len() < DEPOT_SLOTS {
        entity.items.resize(DEPOT_SLOTS, None);
    }
    // SharedDepotBlockMethods.onLanded: items falling on it go in.
    take_landed(game, pos, 13.0 / 16.0, |game, stack| {
        let items = &mut entity.items;
        match &mut items[0] {
            None => {
                items[0] = Some(stack);
                None
            }
            Some(held) if held.same_item_same_components(&stack) => {
                let room = max_stack(game, held).saturating_sub(held.count).min(stack.count);
                held.count += room;
                let mut left = stack;
                left.count -= room;
                (left.count > 0).then_some(left)
            }
            Some(_) => Some(stack),
        }
    });
    process_on_depot(game, pos, entity);
    note_depot(game, pos, &entity.items);
}

fn process_on_depot(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let Some(held) = entity.items[0].clone() else {
        put_bool(&mut entity.nbt, "Locked", false);
        return;
    };
    let press_pos = [pos[0], pos[1] + 2, pos[2]];
    let Some((speed, pressing)) = super::create_glue::with_kinetics(game, |k, level| {
        let e = k.get(press_pos)?;
        matches!(e.kind, Kind::MechanicalPress(_)).then(|| (k.speed_of(level, press_pos), e.pressing))
    }) else {
        if std::env::var_os("CREATE_DEBUG").is_some() {
            eprintln!("create: depot {pos:?} has no press at {press_pos:?}");
        }
        put_bool(&mut entity.nbt, "Locked", false);
        return;
    };
    let locked = get_bool(&entity.nbt, "Locked");
    if std::env::var_os("CREATE_DEBUG").is_some() {
        let recipe = find_recipe(game, "create:pressing", &held).map(|r| r.id);
        let n = game.recipes().custom.get("create:pressing").map_or(0, Vec::len);
        eprintln!(
            "create: depot {pos:?} held {} x{} press speed {speed} running {} ticks {} locked {locked} recipe {recipe:?} of {n}",
            held.item, held.count, pressing.running, pressing.running_ticks
        );
    }
    // BeltProcessingBehaviour.isBlocked: the space above must be free.
    let above = game.world.block(pos[0], pos[1] + 1, pos[2]);
    if !locked && !game.world.is_air(above) {
        return;
    }
    // ProcessingResult: Some(true) HOLD, Some(false) PASS.
    let hold = if !locked {
        // BeltPressingCallbacks.onItemReceived
        if speed == 0.0 {
            false
        } else if pressing.running {
            true
        } else if find_recipe(game, "create:pressing", &held).is_some() {
            super::create_glue::with_kinetics(game, |k, _| {
                if let Some(p) = k.pressing_mut(press_pos) {
                    p.start(PressMode::Belt);
                }
            });
            true
        } else {
            false
        }
    } else {
        // BeltPressingCallbacks.whenItemHeld
        if speed == 0.0 || !pressing.running {
            false
        } else if pressing.running_ticks != PRESS_CYCLE / 2 {
            true
        } else if let Some(recipe) = find_recipe(game, "create:pressing", &held) {
            // bulkPressing is off: one item at a time.
            let results = roll(&recipe);
            let mut rest = results.into_iter();
            if held.count == 1 {
                // TransportedResult.convertTo: the first result is held.
                entity.items[0] = rest.next();
            } else {
                // convertToAndLeaveHeld: the rest stay held.
                let mut left = held.clone();
                left.count -= 1;
                entity.items[0] = Some(left);
            }
            for out in rest {
                if entity.items[0].is_none() {
                    entity.items[0] = Some(out);
                    continue;
                }
                if let Some(drop) = insert_stacked(game, &mut entity.items, 1..DEPOT_SLOTS, out) {
                    let c = pos.map(|v| f64::from(v) + 0.5);
                    game.spawn_item(drop, [c[0], c[1] + 0.5, c[2]], [0.0, 0.1, 0.0], 10);
                }
            }
            true
        } else {
            false
        }
    };
    put_bool(&mut entity.nbt, "Locked", hold);
}

/// `MillstoneBlockEntity.tick` (and `process`).
fn millstone_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    if entity.items.len() < MILLSTONE_SLOTS {
        entity.items.resize(MILLSTONE_SLOTS, None);
    }
    // MillstoneBlock.updateEntityAfterFallOn: what it can mill goes in.
    take_landed(game, pos, 0.75, |game, stack| {
        if find_recipe(game, "create:milling", &stack).is_none() {
            return Some(stack);
        }
        insert_stacked(game, &mut entity.items, 0..1, stack)
    });
    let speed = super::create_glue::with_kinetics(game, |k, level| k.speed_of(level, pos));
    if speed == 0.0 {
        return;
    }
    if entity.items[1..]
        .iter()
        .flatten()
        .any(|s| s.count >= max_stack(game, s).min(64))
    {
        return;
    }
    let mut timer = get_int(&entity.nbt, "Timer");
    if timer > 0 {
        timer -= millstone_speed(speed);
        if timer <= 0 {
            mill(game, entity);
        }
        put_int(&mut entity.nbt, "Timer", timer);
        return;
    }
    let Some(input) = entity.items[0].clone() else {
        return;
    };
    timer = match find_recipe(game, "create:milling", &input) {
        Some(r) => r.processing_time,
        None => 100,
    };
    put_int(&mut entity.nbt, "Timer", timer);
}

fn mill(game: &mut Game<'_>, entity: &mut ModBlockEntity) {
    let Some(input) = entity.items[0].clone() else {
        return;
    };
    let Some(recipe) = find_recipe(game, "create:milling", &input) else {
        return;
    };
    let mut left = input;
    left.count -= 1;
    entity.items[0] = (left.count > 0).then_some(left);
    // Outputs that don't fit are lost, as insertItemStacked's remainder is.
    for out in roll(&recipe) {
        let _ = insert_stacked(game, &mut entity.items, 1..MILLSTONE_SLOTS, out);
    }
}

// ---- The press over the world ---------------------------------------------

/// What a press's tick asks of the world, done once the kinetics' lock is
/// released: `applyInWorld` and the entity scan.
pub(super) fn press_world_step(game: &mut Game<'_>, pos: Pos, step: PressStep, mode: PressMode) {
    let below2 = game
        .pack
        .assets
        .registry
        .state(game.world.block(pos[0], pos[1] - 2, pos[2]))
        .is_some_and(|s| s.name == DEPOT);
    match step {
        PressStep::ScanWorld => {
            // A depot two below takes the work (TransportedItemStackHandlerBehaviour).
            if below2 {
                return;
            }
            let found = items_below(game, pos)
                .into_iter()
                .any(|i| find_recipe(game, "create:pressing", &game.dropped[i].stack).is_some());
            if found {
                super::create_glue::with_kinetics(game, |k, _| {
                    if let Some(p) = k.pressing_mut(pos) {
                        p.start(PressMode::World);
                    }
                });
            }
        }
        PressStep::Press if mode == PressMode::World => {
            for i in items_below(game, pos) {
                super::create_glue::with_kinetics(game, |k, _| {
                    if let Some(p) = k.pressing_mut(pos) {
                        p.rescan();
                    }
                });
                let stack = game.dropped[i].stack.clone();
                let Some(recipe) = find_recipe(game, "create:pressing", &stack) else {
                    continue;
                };
                let at = game.dropped[i].position;
                let results = roll(&recipe);
                if stack.count == 1 {
                    // RecipeApplier.applyRecipeOn(itemEntity): it becomes the
                    // first result, the others drop beside it.
                    let mut rest = results.into_iter();
                    match rest.next() {
                        Some(first) => game.dropped[i].stack = first,
                        None => game.dropped[i].stack.count = 0,
                    }
                    for r in rest {
                        game.spawn_item(r, [at.x, at.y, at.z], [0.0, 0.05, 0.0], 10);
                    }
                } else {
                    game.dropped[i].stack.count -= 1;
                    for r in results {
                        game.spawn_item(r, [at.x, at.y, at.z], [0.0, 0.05, 0.0], 10);
                    }
                }
                // Not canProcessInBulk: one entity a press.
                break;
            }
            game.dropped.retain(|d| d.stack.count > 0);
        }
        _ => {}
    }
}

/// Item entities on the ground in the block below the press, shrunk by an
/// eighth (`new AABB(worldPosition.below()).deflate(.125f)`).
fn items_below(game: &Game<'_>, pos: Pos) -> Vec<usize> {
    let [x, y, z] = pos.map(f64::from);
    let (lo, hi) = ([x + 0.125, y - 1.0 + 0.125, z + 0.125], [x + 0.875, y - 0.125, z + 0.875]);
    (0..game.dropped.len())
        .filter(|&i| {
            let d = &game.dropped[i];
            let p = d.position;
            d.on_ground
                && d.stack.count > 0
                && (lo[0]..=hi[0]).contains(&p.x)
                && (lo[1]..=hi[1]).contains(&p.y)
                && (lo[2]..=hi[2]).contains(&p.z)
        })
        .collect()
}

// ---- The water wheel's flow -------------------------------------------------

struct Flows<'a, 'g> {
    game: &'a Game<'g>,
}

impl Flows<'_, '_> {
    fn state(&self, p: horizon_core::position::BlockPos) -> horizon_world::registry::BlockStateId {
        self.game.world.block(p.x, p.y, p.z)
    }
}

impl horizon_world::fluid_flow::FluidFlowWorld for Flows<'_, '_> {
    type Error = std::io::Error;
    type Block = horizon_world::registry::BlockStateId;
    fn fluid(
        &mut self,
        p: horizon_core::position::BlockPos,
    ) -> std::io::Result<horizon_world::fluid::FluidState> {
        let state = self
            .game
            .pack
            .assets
            .registry
            .state(self.state(p))
            .ok_or_else(|| std::io::Error::other("missing block state"))?;
        horizon_world::fluid::FluidState::from_block(state)
    }
    fn block(&mut self, p: horizon_core::position::BlockPos) -> std::io::Result<Self::Block> {
        Ok(self.state(p))
    }
    fn blocks_fluid_flow(&self, s: Self::Block) -> bool {
        let assets = &self.game.pack.assets;
        assets
            .registry
            .state(s)
            .is_some_and(|b| assets.blocks_fluid_flow.contains(&b.name))
    }
    fn is_ice_class(&self, s: Self::Block) -> bool {
        self.game.res.facts().get(s).classes.iter().any(|c| c.as_ref() == "IceBlock")
    }
    fn is_face_sturdy(
        &mut self,
        s: Self::Block,
        _: horizon_core::position::BlockPos,
        d: horizon_core::geometry::Direction,
    ) -> std::io::Result<bool> {
        Ok(self.game.res.facts().get(s).sturdy_full & (1 << d as u8) != 0)
    }
}

/// `determineAndApplyFlowScore` for the small wheel at `pos` facing `f`:
/// each neighbour off its axis, the fluid's flow there (`getFlow`), and a
/// bubble column's push.
pub(super) fn water_wheel_flow(game: &Game<'_>, pos: Pos, f: Dir) -> i32 {
    let axis = f.axis() as usize;
    let mut flows = Vec::new();
    let mut world = Flows { game };
    for d in Dir::ALL {
        if d.axis() == f.axis() {
            continue;
        }
        let n = d.normal();
        let p = horizon_core::position::BlockPos::new(pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]);
        if !game.world.loaded(p.x, p.z) {
            continue;
        }
        let state = game.pack.assets.registry.state(world.state(p));
        let mut flow = [0.0; 3];
        if let Some(s) = state {
            if s.name == "minecraft:bubble_column" {
                let down = s.properties.get("drag").is_some_and(|v| v == "true");
                flow = [0.0, if down { -1.0 } else { 1.0 }, 0.0];
            } else if let Ok(fluid) = horizon_world::fluid::FluidState::from_block(s)
                && let Ok(v) = horizon_world::fluid_flow::flow_at(&mut world, fluid, p)
            {
                flow = [v.x, v.y, v.z];
            }
        }
        flows.push((n, flow));
    }
    let score = water_wheel_flow_score(axis, &flows);
    if std::env::var_os("CREATE_DEBUG").is_some() {
        eprintln!("create: water wheel {pos:?} facing {f:?} flows {flows:?} score {score}");
    }
    score
}

// ---- Players ------------------------------------------------------------------

/// `native_right_click_block` on a depot or millstone; true when it took
/// the click (their templates' own menus never open).
pub(super) fn right_click(game: &mut Game<'_>, pos: Pos, face: [i32; 3]) -> bool {
    let name = game
        .pack
        .assets
        .registry
        .state(game.world.block(pos[0], pos[1], pos[2]))
        .map(|s| s.name.clone());
    match name.as_deref() {
        Some(DEPOT) => {
            // SharedDepotBlockMethods.onUse: from the top only.
            if face == [0, 1, 0] {
                use_depot(game, pos);
            }
            true
        }
        Some(MILLSTONE) => {
            // MillstoneBlock.useItemOn: an empty hand takes the outputs.
            if game.held().is_none() {
                let taken = game.native_with_mod_block_entity(pos, |be| {
                    be.items.iter_mut().skip(1).filter_map(Option::take).collect::<Vec<_>>()
                });
                for s in taken.unwrap_or_default() {
                    game.give_or_drop(s);
                }
            }
            true
        }
        _ => false,
    }
}

fn use_depot(game: &mut Game<'_>, pos: Pos) {
    let hand = game.held_mut().take();
    let returned = game.native_with_mod_block_entity(pos, |be| {
        if be.items.len() < DEPOT_SLOTS {
            be.items.resize(DEPOT_SLOTS, None);
        }
        // The held item and every output back to the player...
        let back: Vec<ItemStack> = be.items.iter_mut().filter_map(Option::take).collect();
        // ...and the hand's stack onto the depot.
        be.items[0] = hand.clone();
        put_bool(&mut be.nbt, "Locked", false);
        back
    });
    match returned {
        Some(back) => {
            for s in back {
                game.give_or_drop(s);
            }
        }
        None => *game.held_mut() = hand,
    }
}

// ---- Clients ------------------------------------------------------------------

/// The depot changes of `dimension` for its players, and forgets depots
/// that are gone; `full` repeats every one there.
pub(super) fn take_depot_updates(game: &Game<'_>, dimension: &str, full: bool) -> Vec<u8> {
    let mut guard = machines();
    let m = guard.get_or_insert_with(Machines::default);
    let registry = game.pack.assets.registry;
    let gone: Vec<Pos> = m
        .depots
        .keys()
        .filter(|(d, p)| {
            d == dimension
                && game.world.loaded(p[0], p[2])
                && registry
                    .state(game.world.block(p[0], p[1], p[2]))
                    .is_none_or(|s| s.name != DEPOT)
        })
        .map(|(_, p)| *p)
        .collect();
    for p in gone {
        m.depots.remove(&(dimension.to_owned(), p));
        m.pending.push((dimension.to_owned(), p, None));
    }
    let mut entries: Vec<(Pos, Option<Vec<(String, u32)>>)> = Vec::new();
    m.pending.retain(|(d, p, v)| {
        if d == dimension {
            entries.push((*p, v.clone()));
            false
        } else {
            true
        }
    });
    if full {
        entries = m
            .depots
            .iter()
            .filter(|((d, _), _)| d == dimension)
            .map(|((_, p), v)| (*p, Some(v.clone())))
            .collect();
    }
    if entries.is_empty() && !full {
        return Vec::new();
    }
    encode_depots(full, &entries)
}

/// `D1`, whether it is everything, then per depot its position, its slot
/// count (0: gone), and each slot's count and item id.
fn encode_depots(full: bool, entries: &[(Pos, Option<Vec<(String, u32)>>)]) -> Vec<u8> {
    let mut out = b"D1".to_vec();
    out.push(u8::from(full));
    for (pos, view) in entries {
        for v in pos {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let slots = view.as_deref().unwrap_or(&[]);
        out.push(slots.len() as u8);
        for (id, count) in slots {
            out.push((*count).min(255) as u8);
            out.push(id.len().min(255) as u8);
            out.extend_from_slice(&id.as_bytes()[..id.len().min(255)]);
        }
    }
    out
}

/// `native_net_from_server` on the depots' channel.
pub(super) fn from_server(data: &[u8]) {
    let Some(rest) = data.strip_prefix(b"D1") else {
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
    while rest.len() >= 13 {
        let int = |b: &[u8]| i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let pos = [int(&rest[0..4]), int(&rest[4..8]), int(&rest[8..12])];
        let n = rest[12] as usize;
        rest = &rest[13..];
        let mut slots = Vec::new();
        for _ in 0..n {
            if rest.len() < 2 || rest.len() < 2 + rest[1] as usize {
                return;
            }
            let count = u32::from(rest[0]);
            let len = rest[1] as usize;
            let id = String::from_utf8_lossy(&rest[2..2 + len]).into_owned();
            rest = &rest[2 + len..];
            slots.push((id, count));
        }
        if n == 0 {
            view.remove(&pos);
        } else {
            view.insert(pos, slots);
        }
    }
}

/// `DepotRenderer.renderItemsOf`: the held item lying on top, the outputs
/// around it, an eighth of a turn apart.
pub(super) fn draw_depots(
    game: &Game<'_>,
    camera: glam::DVec3,
    out: &mut Vec<horizon_client::mesher::Vertex>,
) {
    let guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    let Some(view) = guard.as_ref() else {
        return;
    };
    for (pos, slots) in view {
        let centre = glam::DVec3::new(
            f64::from(pos[0]) + 0.5,
            f64::from(pos[1]) + 15.0 / 16.0,
            f64::from(pos[2]) + 0.5,
        );
        if centre.distance_squared(camera) > 64.0 * 64.0 {
            continue;
        }
        let light = game.world.light(pos[0], pos[1] + 1, pos[2]);
        let base = glam::Mat4::from_translation((centre - camera).as_vec3());
        for (i, (id, count)) in slots.iter().enumerate() {
            if *count == 0 || id.is_empty() {
                continue;
            }
            let place = if i == 0 {
                glam::Mat4::IDENTITY
            } else {
                // The buffer: turned 360/8 per slot, out by .35.
                glam::Mat4::from_rotation_y((360.0 / 8.0 * (i - 1) as f32).to_radians())
                    * glam::Mat4::from_translation(glam::Vec3::new(0.35, 0.0, 0.0))
            };
            // renderItem: lying flat, at half size (FIXED).
            let m = base
                * place
                * glam::Mat4::from_rotation_x(90f32.to_radians())
                * glam::Mat4::from_scale(glam::Vec3::splat(0.5));
            let stack = ItemStack::new(id, *count);
            let owner = super::item_draw::owner_at(centre.to_array(), 0.0, DEPOT);
            let state = game.resolve_item(
                &stack,
                horizon_assets::item_model::DisplayContext::Fixed,
                Some(&owner),
                pos[0] ^ pos[2] ^ i as i32,
            );
            game.submit_item_world(
                &state,
                m,
                horizon_client::block_entity_render::vertex_light(light),
                out,
            );
        }
    }
}
