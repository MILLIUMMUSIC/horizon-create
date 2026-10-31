// Horizon's side of Create's machines, injected as `crate::game::create_machines`:
// the depot (DepotBehaviour) and millstone (MillstoneBlockEntity) as mod
// block entities with Horizon's own item slots, so hoppers and breaking work
// as for any container; the press's work on a depot or on items on the
// ground (PressingBehaviour, BeltPressingCallbacks); the water wheel's flow
// (WaterWheelBlockEntity); the basin (BasinBlockEntity) and the mixer's work
// in it (MechanicalMixerBlockEntity, BasinRecipe); the encased fan's air
// current over items on the ground and on depots (AirCurrent, FanProcessing).
// The rules are in processing.rs, fan.rs and kinetics.rs.

use super::Game;
use super::create_fan::{
    AirCurrent, FAN_BLOCK_CHECK_RATE, FanLevel, FanType, Fluid, Obstacle, Processing,
    aligned_distance_to_face, entity_can_process, entity_decrement, fan_type_at, push,
    transported_step,
};
use super::create_kinetics::{Axis, Dir, Kind, Pos};
use super::create_processing::{
    BASIN_SLOTS, BasinRecipe, MixStep, PRESS_CYCLE, PressMode, PressStep, Recipe,
    apply_processing_on, basin_insert_slot, basin_take, ingredient_json, millstone_speed,
    multiplied_output, parse_basin_recipe, parse_recipe, roll_results, water_wheel_flow_score,
};
use horizon_client::containers::{ModBlockEntity, ModContainer};
use horizon_client::crafting::Ingredient;
use horizon_client::items::ItemStack;
use horizon_world::nbt::{Compound, JavaString, Tag};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

pub(super) const DEPOTS: &str = "create:depots";
pub(super) const DEPOT: &str = "create:depot";
const MILLSTONE: &str = "create:millstone";
pub(super) const BASIN: &str = "create:basin";
/// DepotBehaviour: the held item, then `processingOutputBuffer`.
const DEPOT_SLOTS: usize = 9;
/// MillstoneBlockEntity: `inputInv`, then `outputInv`.
const MILLSTONE_SLOTS: usize = 10;
/// BasinBlockEntity: `inputInventory`, then `outputInventory`.
const BASIN_ALL_SLOTS: usize = 2 * BASIN_SLOTS;
/// `BasinBlockEntity.recipeBackupCheck`: 20 lazy ticks of 10.
const BASIN_BACKUP_CHECK: i32 = 200;

/// An encased fan's air current on the server, with its rebuild timer
/// (`airCurrentUpdateCooldown`) and the speed it was built for.
#[derive(Default)]
struct Fan {
    current: AirCurrent,
    cooldown: i32,
    speed: f32,
}

type BasinRecipes = Vec<(BasinRecipe, Vec<Option<Ingredient>>)>;

#[derive(Default)]
struct Machines {
    /// Recipes by type, with their ingredient as the game reads it.
    recipes: HashMap<&'static str, Vec<(Recipe, Option<Ingredient>)>>,
    /// What clients were last told each depot holds, by dimension.
    depots: HashMap<(String, Pos), Vec<(String, u32)>>,
    /// Depot changes for the level's tick end.
    pending: Vec<(String, Pos, Option<Vec<(String, u32)>>)>,
    random: u64,
    /// `create:mixing` recipes, most ingredients first, with theirs read.
    basin_recipes: Option<std::sync::Arc<BasinRecipes>>,
    /// Each mixer's `currentRecipe`, by id.
    mixing: HashMap<(String, Pos), String>,
    /// What each basin held last tick (`contentsChanged`).
    basins: HashMap<(String, Pos), Vec<(String, u32)>>,
    fans: HashMap<(String, Pos), Fan>,
    /// Item entities' `CreateData.Processing`, by dimension and entity id.
    processing: HashMap<(String, u32), Processing>,
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
    let chute = block == super::create_logistics::CHUTE;
    if entity.is_none() && (block == DEPOT || block == MILLSTONE || block == BASIN || chute) {
        *entity = Some((block.to_owned(), Compound::new()));
    }
}

/// `native_block_entity_ticks`.
pub(super) fn block_entity_ticks(block: &str, ticks: &mut bool) {
    if block == DEPOT || block == MILLSTONE || block == BASIN || block == super::create_logistics::CHUTE {
        *ticks = true;
    }
}

/// `native_block_entity_container`: DepotItemHandler (insert into the held
/// slot, take from any) and the millstone's input and output inventories.
pub(super) fn block_entity_container(block: &str, container: &mut Option<ModContainer>) {
    let (slots, inputs, outputs): (usize, Vec<usize>, Vec<usize>) = match block {
        DEPOT => (DEPOT_SLOTS, vec![0], (0..DEPOT_SLOTS).collect()),
        MILLSTONE => (MILLSTONE_SLOTS, vec![0], (1..MILLSTONE_SLOTS).collect()),
        // The output inventory forbids insertion; both are taken from
        // (`CombinedInvWrapper`).
        BASIN => (BASIN_ALL_SLOTS, (0..BASIN_SLOTS).collect(), (0..BASIN_ALL_SLOTS).collect()),
        // ChuteItemHandler: its one item.
        super::create_logistics::CHUTE => (1, vec![0], vec![0]),
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

/// One slot as clients are told it.
pub(super) fn stack_view(stack: Option<&ItemStack>) -> (String, u32) {
    stack.map_or((String::new(), 0), |s| (s.item.to_string(), s.count))
}

fn note_depot(game: &Game<'_>, pos: Pos, items: &[Option<ItemStack>]) {
    note_view(game, pos, depot_view(items));
}

/// What clients see of the block entity at `pos` (a depot, basin or
/// chute), on the depots' channel when it changed.
pub(super) fn note_view(game: &Game<'_>, pos: Pos, view: Vec<(String, u32)>) {
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
        BASIN => basin_tick(game, pos, entity),
        super::create_logistics::CHUTE => super::create_logistics::chute_tick(game, pos, entity),
        _ => {}
    }
}

/// `ItemEntity`s resting on the block at `pos` (`updateEntityAfterFallOn`):
/// their stacks go in through `accept`, which gives back what it left.
pub(super) fn take_landed(
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

/// The press at `press_pos`: its speed and pressing state.
fn press_at(game: &Game<'_>, press_pos: Pos) -> Option<(f32, super::create_processing::Pressing)> {
    super::create_glue::with_kinetics(game, |k, level| {
        let e = k.get(press_pos)?;
        matches!(e.kind, Kind::MechanicalPress(_)).then(|| (k.speed_of(level, press_pos), e.pressing))
    })
}

/// `BeltPressingCallbacks.onItemReceived` of the press at `press_pos` for
/// `held` (on a depot or a belt below it): none when there is no press,
/// else whether it holds the item (`HOLD`, or `PASS`).
pub(super) fn press_received(game: &Game<'_>, press_pos: Pos, held: &ItemStack) -> Option<bool> {
    let (speed, pressing) = press_at(game, press_pos)?;
    Some(if speed == 0.0 {
        false
    } else if pressing.running {
        true
    } else if find_recipe(game, "create:pressing", held).is_some() {
        super::create_glue::with_kinetics(game, |k, _| {
            if let Some(p) = k.pressing_mut(press_pos) {
                p.start(PressMode::Belt);
            }
        });
        true
    } else {
        false
    })
}

/// What the press made of one held item: its results, and what is left
/// of the held stack.
pub(super) type Pressed = (Vec<ItemStack>, Option<ItemStack>);

/// `BeltPressingCallbacks.whenItemHeld`: none when there is no press, else
/// whether it still holds the item and, at the cycle's middle, what it
/// pressed (`bulkPressing` off: one item at a time).
pub(super) fn press_held(game: &Game<'_>, press_pos: Pos, held: &ItemStack) -> Option<(bool, Option<Pressed>)> {
    let (speed, pressing) = press_at(game, press_pos)?;
    if speed == 0.0 || !pressing.running {
        return Some((false, None));
    }
    if pressing.running_ticks != PRESS_CYCLE / 2 {
        return Some((true, None));
    }
    let Some(recipe) = find_recipe(game, "create:pressing", held) else {
        return Some((false, None));
    };
    let results = roll(&recipe);
    let left = (held.count > 1).then(|| {
        let mut l = held.clone();
        l.count -= 1;
        l
    });
    Some((true, Some((results, left))))
}

fn process_on_depot(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let Some(held) = entity.items[0].clone() else {
        put_bool(&mut entity.nbt, "Locked", false);
        return;
    };
    let press_pos = [pos[0], pos[1] + 2, pos[2]];
    let locked = get_bool(&entity.nbt, "Locked");
    // BeltProcessingBehaviour.isBlocked: the space above must be free.
    let above = game.world.block(pos[0], pos[1] + 1, pos[2]);
    if !locked && !game.world.is_air(above) {
        return;
    }
    // ProcessingResult: true HOLD, false PASS.
    let hold = if !locked {
        press_received(game, press_pos, &held)
    } else {
        press_held(game, press_pos, &held).map(|(hold, pressed)| {
            if let Some((results, left)) = pressed {
                let mut rest = results.into_iter();
                // TransportedResult.convertTo: the first result is held;
                // convertToAndLeaveHeld: the rest of the stack stays held.
                entity.items[0] = match left {
                    Some(l) => Some(l),
                    None => rest.next(),
                };
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
            }
            hold
        })
    };
    put_bool(&mut entity.nbt, "Locked", hold.unwrap_or(false));
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
        .is_some_and(|s| s.name == DEPOT || super::create_logistics::is_belt(&s.name));
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
        Some(BASIN) => {
            // BasinBlock.useItemOn: an empty hand takes every item, and the
            // basin counts as emptied (`onEmptied`). Fluid items and others
            // pass to the default (here: nothing, the template's menu stays shut).
            if game.held().is_none() {
                let taken = game.native_with_mod_block_entity(pos, |be| {
                    be.items.iter_mut().filter_map(Option::take).collect::<Vec<_>>()
                });
                for s in taken.unwrap_or_default() {
                    game.give_or_drop(s);
                }
                let above = [pos[0], pos[1] + 2, pos[2]];
                super::create_glue::with_kinetics(game, |k, _| {
                    if let Some(m) = k.mixing_mut(above) {
                        m.basin_removed();
                    }
                });
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
                    .is_none_or(|s| s.name != DEPOT && s.name != BASIN && s.name != super::create_logistics::CHUTE)
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
/// around it, an eighth of a turn apart. Basins, told on the same channel,
/// draw as `BasinRenderer`.
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
        let basin = game
            .pack
            .assets
            .registry
            .state(game.world.block(pos[0], pos[1], pos[2]))
            .is_some_and(|s| s.name == BASIN);
        if basin {
            draw_basin(game, camera, *pos, slots, out);
            continue;
        }
        let chute = game
            .pack
            .assets
            .registry
            .state(game.world.block(pos[0], pos[1], pos[2]))
            .is_some_and(|s| s.name == super::create_logistics::CHUTE);
        if chute {
            super::create_logistics::draw_chute(game, camera, *pos, slots, out);
            continue;
        }
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

// ---- The basin and the mixer ---------------------------------------------------

/// `BasinBlockEntity.tick` (and `lazyTick`'s recipe backup check): items
/// falling in go to the input inventory, and a change of contents (or,
/// while it holds anything, every 200 ticks) has the mixer above look again.
fn basin_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    if entity.items.len() < BASIN_ALL_SLOTS {
        entity.items.resize(BASIN_ALL_SLOTS, None);
    }
    // BasinBlock.updateEntityAfterFallOn into `inputInventory`.
    take_landed(game, pos, 0.75, |game, stack| {
        basin_insert(game, &mut entity.items[..BASIN_SLOTS], stack)
    });
    let view = depot_view(&entity.items);
    let empty = entity.items.iter().all(Option::is_none);
    let mut backup = get_int(&entity.nbt, "Backup") - 1;
    let changed = {
        let mut guard = machines();
        let m = guard.get_or_insert_with(Machines::default);
        let key = (game.dim.id().to_owned(), pos);
        m.basins.insert(key, view.clone()) != Some(view)
    };
    let check = changed || (backup <= 0 && !empty);
    if backup <= 0 {
        backup = BASIN_BACKUP_CHECK;
    }
    put_int(&mut entity.nbt, "Backup", backup);
    if check {
        // getOperator().basinChecker.scheduleUpdate()
        let above = [pos[0], pos[1] + 2, pos[2]];
        super::create_glue::with_kinetics(game, |k, _| {
            if let Some(m) = k.mixing_mut(above) {
                m.needs_update = true;
            }
        });
    }
    note_depot(game, pos, &entity.items);
}

/// `ItemHandlerHelper.insertItem` into a `BasinInventory`: an item already
/// in it only stacks onto its slot, a new one takes the first empty slot.
fn basin_insert(game: &Game<'_>, slots: &mut [Option<ItemStack>], mut stack: ItemStack) -> Option<ItemStack> {
    let target = basin_insert_slot(
        slots.len(),
        |i| slots[i].as_ref().is_some_and(|s| s.same_item_same_components(&stack)),
        |i| slots[i].is_none(),
    )?;
    // SmartInventory's stack size: 64, or the item's own.
    let max = max_stack(game, &stack).min(64);
    match &mut slots[target] {
        Some(s) => {
            let moved = max.saturating_sub(s.count).min(stack.count);
            s.count += moved;
            stack.count -= moved;
        }
        None => {
            let moved = stack.count.min(max);
            let mut put = stack.clone();
            put.count = moved;
            slots[target] = Some(put);
            stack.count -= moved;
        }
    }
    (stack.count > 0).then_some(stack)
}

/// The supported `create:mixing` recipes (items only, no heat), most
/// ingredients first (`getMatchingRecipes`' sort).
fn basin_recipes(game: &Game<'_>) -> std::sync::Arc<BasinRecipes> {
    if let Some(r) = machines().as_ref().and_then(|m| m.basin_recipes.clone()) {
        return r;
    }
    let mut list: BasinRecipes = game
        .recipes()
        .custom
        .get("create:mixing")
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|r| parse_basin_recipe(&r.id, &r.json))
        .filter(BasinRecipe::supported)
        .map(|r| {
            let ingredients = r
                .ingredients
                .iter()
                .map(|choices| {
                    let json = serde_json::Value::Array(
                        choices.iter().cloned().map(serde_json::Value::String).collect(),
                    );
                    ingredient(game, &json)
                })
                .collect();
            (r, ingredients)
        })
        .filter(|(_, i): &(BasinRecipe, Vec<Option<Ingredient>>)| i.iter().all(Option::is_some))
        .collect();
    list.sort_by_key(|(r, _)| std::cmp::Reverse(r.ingredients.len()));
    let list = std::sync::Arc::new(list);
    machines().get_or_insert_with(Machines::default).basin_recipes = Some(list.clone());
    list
}

/// `BasinRecipe.apply` on a basin's 18 slots: one item for each ingredient
/// (from either inventory, as the basin's item capability offers both), the
/// rolled results into the output inventory, all or nothing. With `test`,
/// only whether it would (`BasinRecipe.match`).
fn basin_apply(
    game: &Game<'_>,
    items: &mut [Option<ItemStack>],
    recipe: &(BasinRecipe, Vec<Option<Ingredient>>),
    test: bool,
) -> bool {
    let (r, ingredients) = recipe;
    let counts: Vec<u32> = items.iter().map(|s| s.as_ref().map_or(0, |s| s.count)).collect();
    let Some(taken) = basin_take(ingredients.len(), &counts, |i, slot| {
        items[slot]
            .as_ref()
            .is_some_and(|s| ingredients[i].as_ref().is_some_and(|ing| ing.test(s)))
    }) else {
        return false;
    };
    let outputs = roll(&r.as_recipe());
    // acceptOutputs, simulated: every result must fit.
    let mut simulated = items.to_vec();
    for o in &outputs {
        if insert_stacked(game, &mut simulated, BASIN_SLOTS..BASIN_ALL_SLOTS, o.clone()).is_some() {
            return false;
        }
    }
    if test {
        return true;
    }
    for (slot, n) in taken.into_iter().enumerate() {
        if n == 0 {
            continue;
        }
        if let Some(s) = items[slot].as_mut() {
            s.count -= n;
            if s.count == 0 {
                items[slot] = None;
            }
        }
    }
    for o in outputs {
        let _ = insert_stacked(game, items, BASIN_SLOTS..BASIN_ALL_SLOTS, o);
    }
    true
}

/// The basin's slots two below the mixer at `pos`, if there is one.
fn basin_below(game: &mut Game<'_>, pos: Pos) -> Option<(Pos, Vec<Option<ItemStack>>)> {
    let at = [pos[0], pos[1] - 2, pos[2]];
    let is_basin = game
        .pack
        .assets
        .registry
        .state(game.world.block(at[0], at[1], at[2]))
        .is_some_and(|s| s.name == BASIN);
    if !is_basin {
        return None;
    }
    let mut items = game.native_with_mod_block_entity(at, |be| be.items.clone())?;
    items.resize(BASIN_ALL_SLOTS, None);
    Some((at, items))
}

/// What a mixer's tick asks of the basin below, once the kinetics' lock is
/// released: `updateBasin` and `applyBasinRecipe`.
pub(super) fn mixer_world_step(game: &mut Game<'_>, pos: Pos, step: MixStep) {
    let key = (game.dim.id().to_owned(), pos);
    match step {
        MixStep::CheckBasin => {
            let Some((_, mut items)) = basin_below(game, pos) else {
                return;
            };
            if items.iter().all(Option::is_none) {
                return;
            }
            let recipes = basin_recipes(game);
            let Some((r, _)) = recipes.iter().find(|r| basin_apply(game, &mut items, r, true)) else {
                return;
            };
            let duration = r.processing_time;
            machines()
                .get_or_insert_with(Machines::default)
                .mixing
                .insert(key, r.id.clone());
            super::create_glue::with_kinetics(game, |k, _| {
                if let Some(m) = k.mixing_mut(pos) {
                    m.start(duration);
                }
            });
        }
        MixStep::Apply => {
            let Some(id) = machines().as_ref().and_then(|m| m.mixing.get(&key).cloned()) else {
                return;
            };
            let recipes = basin_recipes(game);
            let Some(recipe) = recipes.iter().find(|(r, _)| r.id == id) else {
                return;
            };
            let Some((at, mut items)) = basin_below(game, pos) else {
                return;
            };
            if !basin_apply(game, &mut items, recipe, false) {
                return;
            }
            // Continue mixing while the basin still holds another batch.
            let again = basin_apply(game, &mut items.clone(), recipe, true);
            game.native_with_mod_block_entity(at, |be| be.items = items);
            if again {
                super::create_glue::with_kinetics(game, |k, _| {
                    if let Some(m) = k.mixing_mut(pos) {
                        m.continue_mixing();
                    }
                });
            }
        }
        MixStep::Idle => {}
    }
}

/// `BasinRenderer`: the basin's items in the bowl. Create places every
/// stack at the same angle (`anglePartition * itemCount`, the count of all
/// of them), so they lie together a little off the middle; lone ones in it.
fn draw_basin(
    game: &Game<'_>,
    camera: glam::DVec3,
    pos: Pos,
    slots: &[(String, u32)],
    out: &mut Vec<horizon_client::mesher::Vertex>,
) {
    let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
    if (corner + glam::DVec3::splat(0.5)).distance_squared(camera) > 64.0 * 64.0 {
        return;
    }
    let stacks: Vec<&(String, u32)> = slots.iter().filter(|(id, n)| *n > 0 && !id.is_empty()).collect();
    if stacks.is_empty() {
        return;
    }
    // No fluids yet: the level is its lowest.
    let level = 0.125f32;
    let count = stacks.len() as f32;
    let base = if stacks.len() == 1 {
        glam::Vec3::new(0.0, level, 0.0)
    } else {
        glam::Vec3::new(0.125, level, 0.0)
    };
    let angle = 360.0 / count * count;
    let p = super::create_kinetics::rotate_vec(base.to_array(), angle, Axis::Y);
    let light = game.world.light(pos[0], pos[1], pos[2]);
    let origin = (corner - camera).as_vec3();
    for (i, (id, n)) in stacks.into_iter().enumerate() {
        let m = glam::Mat4::from_translation(origin + glam::Vec3::new(0.5, 0.2, 0.5))
            * glam::Mat4::from_translation(glam::Vec3::from_array(p))
            * glam::Mat4::from_rotation_y((angle + 35.0).to_radians())
            * glam::Mat4::from_rotation_x(65f32.to_radians());
        let stack = ItemStack::new(id, *n);
        let centre = (corner + glam::DVec3::splat(0.5)).to_array();
        let owner = super::item_draw::owner_at(centre, 0.0, BASIN);
        let state = game.resolve_item(
            &stack,
            horizon_assets::item_model::DisplayContext::Ground,
            Some(&owner),
            pos[0] ^ pos[2] ^ i as i32,
        );
        game.submit_item_world(&state, m, horizon_client::block_entity_render::vertex_light(light), out);
    }
}

// ---- The encased fan ------------------------------------------------------------

/// The level in place, as an air current sees it.
struct FanWorld<'a, 'g> {
    game: &'a Game<'g>,
}

fn core_axis(a: Axis) -> horizon_core::geometry::Axis {
    match a {
        Axis::X => horizon_core::geometry::Axis::X,
        Axis::Y => horizon_core::geometry::Axis::Y,
        Axis::Z => horizon_core::geometry::Axis::Z,
    }
}

impl FanLevel for FanWorld<'_, '_> {
    fn is_loaded(&self, pos: Pos) -> bool {
        let w = &self.game.world;
        w.loaded(pos[0], pos[2]) && pos[1] >= w.shape.min_y && pos[1] < w.shape.max_y()
    }
    fn obstacle(&self, pos: Pos) -> Obstacle {
        let state = self.game.world.block(pos[0], pos[1], pos[2]);
        let name = self.game.res.facts().get(state).name.clone();
        // AllBlockTags.FAN_TRANSPARENT, from Create's data.
        if self.game.block_in_tag("create:fan_transparent", &name) {
            return Obstacle::Pass;
        }
        let shape = self.game.block_collision_shape(pos);
        if shape.is_empty() {
            Obstacle::Pass
        } else if shape.is_block() {
            Obstacle::Full
        } else {
            Obstacle::Partial
        }
    }
    fn shape_min(&self, pos: Pos, axis: Axis, first: f64, second: f64) -> f64 {
        self.game.block_collision_shape(pos).min_at(core_axis(axis), first, second)
    }
    fn shape_max(&self, pos: Pos, axis: Axis, first: f64, second: f64) -> f64 {
        self.game.block_collision_shape(pos).max_at(core_axis(axis), first, second)
    }
    fn fan_type(&self, pos: Pos) -> Option<FanType> {
        let registry = self.game.pack.assets.registry;
        let state = registry.state(self.game.world.block(pos[0], pos[1], pos[2]))?;
        let fluid = horizon_world::fluid::FluidState::from_block(state)
            .ok()
            .and_then(|f| {
                use horizon_world::fluid::FluidKind as K;
                match f.kind() {
                    K::Water | K::FlowingWater => Some(Fluid::Water),
                    K::Lava | K::FlowingLava => Some(Fluid::Lava),
                    K::Empty => None,
                }
            });
        let prop = |k: &str| state.properties.get(k).map(String::as_str);
        fan_type_at(&state.name, &prop, fluid)
    }
}

/// The type's `canProcess` of `stack`.
fn fan_can_process(game: &Game<'_>, kind: FanType, stack: &ItemStack) -> bool {
    let recipes = game.recipes();
    match kind {
        FanType::Splashing | FanType::Haunting => find_recipe(game, kind.id(), stack).is_some(),
        FanType::Smoking => recipes.cook("minecraft:smoking", stack).is_some(),
        FanType::Blasting => {
            recipes.cook("minecraft:smelting", stack).is_some()
                || recipes.cook("minecraft:blasting", stack).is_some()
                || !game.items().get(&stack.item).is_some_and(|i| i.fire_resistant)
        }
    }
}

/// The type's `process` of `stack`: none when there is nothing to do, an
/// empty list when the item burns away.
fn fan_process(game: &Game<'_>, kind: FanType, stack: &ItemStack) -> Option<Vec<ItemStack>> {
    let max = |id: &str| game.items().get(id).map_or(64, |i| i.max_stack);
    let stacks = |list: Vec<(String, u32)>| list.into_iter().map(|(id, n)| ItemStack::new(&id, n)).collect();
    let cooked = |c: &horizon_client::crafting::Cooking| {
        stacks(multiplied_output(stack.count, &c.result.item, c.result.count, max(&c.result.item)))
    };
    let recipes = game.recipes();
    match kind {
        FanType::Splashing | FanType::Haunting => {
            let recipe = find_recipe(game, kind.id(), stack)?;
            let list = apply_processing_on(
                stack.count,
                || {
                    let mut guard = machines();
                    let m = guard.get_or_insert_with(Machines::default);
                    roll_results(&recipe, || m.next_float())
                },
                max,
            );
            Some(stacks(list))
        }
        FanType::Smoking => recipes.cook("minecraft:smoking", stack).map(cooked),
        FanType::Blasting => {
            let smoking = recipes.cook("minecraft:smoking", stack);
            let smelting = recipes
                .cook("minecraft:smelting", stack)
                .or_else(|| recipes.cook("minecraft:blasting", stack));
            if let Some(s) = smelting
                && smoking.is_none_or(|k| k.result.item != s.result.item)
            {
                return Some(cooked(s));
            }
            Some(Vec::new())
        }
    }
}

fn fan_code(kind: Option<FanType>) -> i32 {
    kind.map_or(0, |k| FanType::SORTED.iter().position(|t| *t == k).unwrap_or(0) as i32 + 1)
}

fn fan_from_code(code: i32) -> Option<FanType> {
    usize::try_from(code - 1).ok().and_then(|i| FanType::SORTED.get(i).copied())
}

/// A stack's identity, to start a depot's timer over for a new item.
fn stack_mark(stack: &ItemStack) -> i32 {
    let mut h: u32 = 0x811C_9DC5;
    for b in stack.item.bytes().chain(stack.count.to_le_bytes()) {
        h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
    }
    h as i32
}

/// `AirCurrent.tickAffectedHandlers` on the depot at `pos`:
/// `FanProcessing.applyProcessing(TransportedItemStack)` on its held item,
/// the results held and buffered as `DepotBehaviour.applyToAllItems` does.
fn fan_on_depot(game: &mut Game<'_>, pos: Pos, kind: FanType) {
    let Some(Some((held, nbt))) = game.native_with_mod_block_entity(pos, |be| {
        be.items.first().cloned().flatten().map(|h| (h, be.nbt.clone()))
    }) else {
        return;
    };
    let mark = stack_mark(&held);
    let (mut by, mut time) = if get_int(&nbt, "FanItem") == mark {
        (fan_from_code(get_int(&nbt, "FanType")), get_int(&nbt, "FanTime"))
    } else {
        (None, 0)
    };
    let process = transported_step(&mut by, &mut time, kind, held.count, || {
        fan_can_process(game, kind, &held)
    });
    let results = if process { fan_process(game, kind, &held) } else { None };
    let mut drops = Vec::new();
    let max = |s: &ItemStack| max_stack(game, s);
    let maxes: Vec<u32> = results.iter().flatten().map(max).collect();
    game.native_with_mod_block_entity(pos, |be| {
        if be.items.len() < DEPOT_SLOTS {
            be.items.resize(DEPOT_SLOTS, None);
        }
        match results {
            Some(list) => {
                // TransportedResult.convertTo: the held item goes, the first
                // result is held, the others go to the output buffer.
                be.items[0] = None;
                for (out, max) in list.into_iter().zip(maxes) {
                    if be.items[0].is_none() {
                        be.items[0] = Some(out);
                        continue;
                    }
                    let mut left = Some(out);
                    for slot in be.items[1..].iter_mut() {
                        let Some(o) = left.as_mut() else { break };
                        match slot {
                            Some(s) if s.same_item_same_components(o) && s.count < max => {
                                let moved = (max - s.count).min(o.count);
                                s.count += moved;
                                o.count -= moved;
                                if o.count == 0 {
                                    left = None;
                                }
                            }
                            None => *slot = left.take(),
                            _ => {}
                        }
                    }
                    drops.extend(left);
                }
                for k in ["FanType", "FanTime", "FanItem"] {
                    be.nbt.remove(&JavaString::from(k));
                }
            }
            None => {
                put_int(&mut be.nbt, "FanType", fan_code(by));
                put_int(&mut be.nbt, "FanTime", time);
                put_int(&mut be.nbt, "FanItem", mark);
            }
        }
    });
    let c = pos.map(|v| f64::from(v) + 0.5);
    for d in drops {
        game.spawn_item(d, [c[0], c[1] + 0.5, c[2]], [0.0, 0.1, 0.0], 10);
    }
}

/// `TransportedItemStackHandlerBehaviour.handleProcessingOnAllItems` on the
/// belt segment at `pos` (`FanProcessing.applyProcessing` on each item
/// within .51 of its middle): results come in its place.
fn fan_on_belt(game: &mut Game<'_>, pos: Pos, kind: FanType) {
    let game: &Game<'_> = game;
    super::create_logistics::with_belt_segment(game, pos, |inv, _, index| {
        inv.apply_to_each_within(index as f32 + 0.5, 0.51, |t| {
            let stack = t.stack.clone();
            let process = transported_step(&mut t.processed_by, &mut t.processing_time, kind, stack.count, || {
                fan_can_process(game, kind, &stack)
            });
            if !process {
                return None;
            }
            let outputs = fan_process(game, kind, &stack)?;
            Some(super::create_belt::Conversion { outputs, held: None })
        });
    });
}

/// `EncasedFanBlockEntity.tick` and `AirCurrent.tick` on the server, for
/// every fan of `dimension` (`fans`: position, facing, speed): the current
/// rebuilt every 30 ticks and when the speed changes, items in it pushed and
/// processed, depots in it processed.
pub(super) fn fan_tick(game: &mut Game<'_>, dimension: &str, fans: &[(Pos, Dir, f32)]) {
    let mut states = {
        let mut guard = machines();
        let m = guard.get_or_insert_with(Machines::default);
        m.fans.retain(|(d, p), _| d != dimension || fans.iter().any(|f| f.0 == *p));
        let keys: Vec<(String, Pos)> = m.fans.keys().filter(|(d, _)| d == dimension).cloned().collect();
        keys.into_iter()
            .filter_map(|k| m.fans.remove_entry(&k))
            .collect::<HashMap<_, _>>()
    };
    let mut processing = {
        let mut guard = machines();
        let m = guard.get_or_insert_with(Machines::default);
        let alive: std::collections::HashSet<u32> = game.dropped.iter().map(|d| d.id).collect();
        m.processing.retain(|(d, id), _| d != dimension || alive.contains(id));
        let keys: Vec<(String, u32)> = m.processing.keys().filter(|(d, _)| d == dimension).cloned().collect();
        keys.into_iter()
            .filter_map(|k| m.processing.remove_entry(&k))
            .collect::<HashMap<_, _>>()
    };
    for &(pos, facing, speed) in fans {
        let fan = states.entry((dimension.to_owned(), pos)).or_default();
        fan.cooldown -= 1;
        if fan.cooldown < 0 || fan.speed != speed || fan.current.direction != Some(facing) {
            fan.cooldown = FAN_BLOCK_CHECK_RATE;
            fan.speed = speed;
            fan.current.rebuild(&FanWorld { game }, pos, facing, speed);
        }
        if speed == 0.0 || fan.current.max_distance <= 0.0 {
            continue;
        }
        let current = fan.current.clone();
        fan_items(game, dimension, pos, facing, speed, &current, &mut processing);
        for (spot, segment) in current.handler_spots(pos) {
            let name = game
                .pack
                .assets
                .registry
                .state(game.world.block(spot[0], spot[1], spot[2]))
                .map(|s| s.name.clone());
            let is_depot = name.as_deref() == Some(DEPOT);
            let is_belt = name.as_deref().is_some_and(super::create_logistics::is_belt);
            if !is_depot && !is_belt {
                continue;
            }
            let here = FanWorld { game }.fan_type(spot);
            if let Some(kind) = here.or(segment) {
                if is_depot {
                    fan_on_depot(game, spot, kind);
                } else {
                    fan_on_belt(game, spot, kind);
                }
            }
        }
    }
    let mut guard = machines();
    let m = guard.get_or_insert_with(Machines::default);
    m.fans.extend(states);
    m.processing.extend(processing);
}

/// `AirCurrent.tickAffectedEntities` for the item entities in the current
/// (other entities aren't pushed or hurt yet).
fn fan_items(
    game: &mut Game<'_>,
    dimension: &str,
    pos: Pos,
    facing: Dir,
    speed: f32,
    current: &AirCurrent,
    processing: &mut HashMap<(String, u32), Processing>,
) {
    let flow = if current.pushing { facing } else { facing.opposite() };
    let centre = pos.map(|v| f64::from(v) + 0.5);
    let mut spawned = Vec::new();
    for i in 0..game.dropped.len() {
        let d = &game.dropped[i];
        if d.stack.count == 0 {
            continue;
        }
        let p = [d.position.x, d.position.y, d.position.z];
        // ItemEntity: 0.25 wide and high.
        let bounds = [p[0] - 0.125, p[1], p[2] - 0.125, p[0] + 0.125, p[1] + 0.25, p[2] + 0.125];
        if !current.touches(bounds) {
            continue;
        }
        let distance_old = ((p[0] - centre[0]).powi(2) + (p[1] - centre[1]).powi(2) + (p[2] - centre[2]).powi(2)).sqrt();
        let v = d.velocity;
        let v = push([v.x, v.y, v.z], flow, speed, distance_old, current.max_distance);
        game.dropped[i].velocity = horizon_core::geometry::Vec3::new(v[0], v[1], v[2]);
        let distance = aligned_distance_to_face(p, pos, facing);
        let Some(kind) = current.type_at(distance as f32) else {
            continue;
        };
        let stack = game.dropped[i].stack.clone();
        let key = (dimension.to_owned(), game.dropped[i].id);
        let mut state = processing.get(&key).copied();
        if !entity_can_process(state, kind, || fan_can_process(game, kind, &stack)) {
            continue;
        }
        let left = entity_decrement(&mut state, kind, stack.count);
        if let Some(s) = state {
            processing.insert(key.clone(), s);
        }
        if left != 0 {
            continue;
        }
        // FanProcessing.applyProcessing(ItemEntity)
        let Some(mut results) = fan_process(game, kind, &stack) else {
            continue;
        };
        processing.remove(&key);
        if results.is_empty() {
            game.dropped[i].stack.count = 0;
            continue;
        }
        game.dropped[i].stack = results.remove(0);
        for r in results {
            spawned.push((r, p, v));
        }
    }
    game.dropped.retain(|d| d.stack.count > 0);
    for (r, p, v) in spawned {
        game.spawn_item(r, p, v, 10);
    }
}
