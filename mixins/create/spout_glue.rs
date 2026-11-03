// Horizon's side of Create's spout, injected as `crate::game::create_spout_glue`:
// its block entity (a 1000 mB tank kept in its data, `ProcessingTicks`), its
// work on an item on a depot or belt two blocks below
// (`SpoutBlockEntity.onItemReceived` / `whenItemHeld`, `FillingBySpout`
// with `create:filling` recipes), its tick, and the nozzle and stream drawn
// by its block entity (`SpoutRenderer`); the item drain's held item, moving
// over it and emptied into its 1500 mB tank (`ItemDrainBlockEntity`,
// `GenericItemEmptying`). The fluids they hold are reached by pipes through
// `create_fluid_glue`'s `FluidAccess`. The rules are in spout.rs.

use super::Game;
use super::create_fluids::{FluidHandler, FluidStack, FluidTank};
use super::create_kinetics::Pos;
use super::create_processing::{BasinRecipe, parse_basin_recipe};
use super::create_spout::{FILLING_TIME, SPOUT_CAPACITY, Spouting, generic_can_fill, generic_fill_amount, generic_fill_result};
use horizon_client::containers::{BlockEntity, ModBlockEntity};
use horizon_client::crafting::Ingredient;
use horizon_client::items::ItemStack;
use horizon_world::nbt::{JavaString, Tag};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub(super) const SPOUT: &str = "create:spout";

/// `create:filling` recipes with an item ingredient and one fluid
/// ingredient Horizon has, the item ingredient read.
type FillingRecipes = Vec<(BasinRecipe, Ingredient)>;

static RECIPES: Mutex<Option<Arc<FillingRecipes>>> = Mutex::new(None);

/// A world opened or closed: recipes are read again.
pub(super) fn reset() {
    *RECIPES.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *CLIENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn filling_recipes(game: &Game<'_>) -> Arc<FillingRecipes> {
    let mut guard = RECIPES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = guard.as_ref() {
        return r.clone();
    }
    let list: FillingRecipes = game
        .recipes()
        .custom
        .get("create:filling")
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|r| parse_basin_recipe(&r.id, &r.json))
        .filter(|r| r.supported() && r.ingredients.len() == 1 && r.fluid_ingredients.len() == 1)
        .filter_map(|r| {
            let json = serde_json::Value::Array(r.ingredients[0].iter().cloned().map(serde_json::Value::String).collect());
            let ingredient = super::create_machines::read_ingredient(game, &json)?;
            Some((r, ingredient))
        })
        .collect();
    let list = Arc::new(list);
    *guard = Some(list.clone());
    list
}

/// `ComparatorUtil.levelOfSmartFluidTank` of a spout or drain
/// (`fractionToRedstoneLevel` of its one tank).
pub(super) fn tank_analog(level: &super::GameLevel<'_>, pos: Pos, name: &str) -> i32 {
    let Some(BlockEntity::Mod(be)) = level.world.block_entities.get(&(pos[0], pos[1], pos[2])) else {
        return 0;
    };
    let tank = if name == DRAIN { drain_tank(be) } else { spout_tank(be) };
    super::create_spout::redstone_level(tank.amount(), tank.capacity)
}

pub(super) fn is_spout(game: &Game<'_>, pos: Pos) -> bool {
    game.pack
        .assets
        .registry
        .state(game.world.block(pos[0], pos[1], pos[2]))
        .is_some_and(|s| s.name == SPOUT)
}

// ---- Its tank and processing ticks, in its block entity's data -------------------

fn get_int(be: &ModBlockEntity, key: &str) -> Option<i32> {
    match be.nbt.get(&JavaString::from(key)) {
        Some(Tag::Int(n)) => Some(*n),
        _ => None,
    }
}

/// The spout's tank (`SmartFluidTankBehaviour`'s primary tank).
pub(super) fn spout_tank(be: &ModBlockEntity) -> FluidTank {
    let mut tank = FluidTank::new(SPOUT_CAPACITY);
    let fluid = match be.nbt.get(&JavaString::from("Fluid")) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    };
    let amount = get_int(be, "Amount").unwrap_or(0);
    tank.fluid = FluidStack::new(&fluid, amount).with_amount(amount);
    tank
}

pub(super) fn set_spout_tank(be: &mut ModBlockEntity, tank: &FluidTank) {
    let fluid = if tank.fluid.is_empty() { String::new() } else { tank.fluid.fluid.clone() };
    be.nbt.insert(JavaString::from("Fluid"), Tag::String(JavaString::from(fluid.as_str())));
    be.nbt.insert(JavaString::from("Amount"), Tag::Int(tank.amount()));
}

/// `processingTicks`: -1 when idle (a new spout's data has none).
fn processing_ticks(be: &ModBlockEntity) -> i32 {
    get_int(be, "ProcessingTicks").unwrap_or(-1)
}

fn set_processing_ticks(be: &mut ModBlockEntity, ticks: i32) {
    be.nbt.insert(JavaString::from("ProcessingTicks"), Tag::Int(ticks));
}

/// The spout at `pos`: its tank and processing ticks.
fn spout_at(game: &Game<'_>, pos: Pos) -> Option<(FluidTank, i32)> {
    if !is_spout(game, pos) {
        return None;
    }
    match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(BlockEntity::Mod(be)) => Some((spout_tank(be), processing_ticks(be))),
        _ => Some((FluidTank::new(SPOUT_CAPACITY), -1)),
    }
}

// ---- FillingBySpout ------------------------------------------------------------

/// `FillingBySpout.canItemBeFilled`: a filling recipe takes it (whatever
/// fluid), or `GenericItemFilling` can.
fn can_be_filled(game: &Game<'_>, stack: &ItemStack) -> bool {
    filling_recipes(game).iter().any(|(_, i)| i.test(stack)) || generic_can_fill(&stack.item)
}

/// `FillingBySpout.getRequiredAmountForItem`: the first recipe for the item
/// whose fluid ingredient takes the tank's fluid, else the generic amount.
fn required_amount(game: &Game<'_>, stack: &ItemStack, fluid: &FluidStack) -> Option<i32> {
    for (r, i) in filling_recipes(game).iter() {
        let f = &r.fluid_ingredients[0];
        if i.test(stack) && !fluid.is_empty() && f.fluids.contains(&fluid.fluid) {
            return Some(f.amount);
        }
    }
    generic_fill_amount(&stack.item, fluid)
}

/// `FillingBySpout.fillItem`: what one item of `stack` becomes with
/// `required` of `fluid` (a recipe's first result, or the generic fill).
fn fill_item(game: &Game<'_>, required: i32, stack: &ItemStack, fluid: &FluidStack) -> Option<ItemStack> {
    let to_fill = fluid.with_amount(required);
    for (r, i) in filling_recipes(game).iter() {
        if i.test(stack) && r.fluid_ingredients[0].test(&to_fill) {
            return super::create_machines::roll_recipe(&r.as_recipe()).into_iter().next();
        }
    }
    let (item, potion) = generic_fill_result(&stack.item, &fluid.fluid)?;
    let mut out = ItemStack::new(item, 1);
    if let Some(p) = potion {
        out = out.with_potion(p);
    }
    Some(out)
}

// ---- BeltProcessingBehaviour ------------------------------------------------------

/// `SpoutBlockEntity.onItemReceived` of the spout at `spout` for `held` (on
/// a depot or belt two blocks below): none when there is no spout, else
/// whether it holds the item.
pub(super) fn spout_received(game: &Game<'_>, spout: Pos, held: &ItemStack) -> Option<bool> {
    let (tank, _) = spout_at(game, spout)?;
    let fillable = can_be_filled(game, held);
    let required = if fillable { required_amount(game, held, &tank.fluid) } else { None };
    Some(super::create_spout::on_item_received(fillable, &tank.fluid, required))
}

/// `SpoutBlockEntity.whenItemHeld`: none when there is no spout, else
/// whether it still holds the item and, when it filled one, the results
/// and what is left of the held stack (`convertToAndLeaveHeld`).
pub(super) fn spout_held(
    game: &mut Game<'_>,
    spout: Pos,
    held: &ItemStack,
) -> Option<(bool, Option<super::create_machines::Pressed>)> {
    let (tank, ticks) = spout_at(game, spout)?;
    let fillable = can_be_filled(game, held);
    let required = if fillable { required_amount(game, held, &tank.fluid) } else { None };
    match super::create_spout::when_item_held(ticks, fillable, &tank.fluid, required) {
        Spouting::Pass => Some((false, None)),
        Spouting::Hold => Some((true, None)),
        Spouting::Start => {
            game.native_with_mod_block_entity(spout, |be| set_processing_ticks(be, FILLING_TIME));
            game.native_block_entity_sync(spout);
            let at = spout.map(|v| f64::from(v) + 0.5);
            let pitch = 0.9 + 0.2 * super::create_machines::random_float();
            game.with_level(|_, level, _| {
                level.native_play_sound("create:spout", at, super::create_sounds::BLOCKS, 0.75, pitch);
            });
            Some((true, None))
        }
        Spouting::Fill(required) => {
            let out = fill_item(game, required, held, &tank.fluid);
            game.native_with_mod_block_entity(spout, |be| {
                let mut t = spout_tank(be);
                t.drain_any(required, false);
                set_spout_tank(be, &t);
            });
            game.native_block_entity_sync(spout);
            // spawnSplash: 20 of the fluid's particles bursting up from the
            // item, 2 - 5/16 below the spout's centre.
            let at = [f64::from(spout[0]) + 0.5, f64::from(spout[1]) + 0.5 - (2.0 - 5.0 / 16.0), f64::from(spout[2]) + 0.5];
            fluid_particles(game, &tank.fluid.fluid, at, [0.0; 3], 0.125, 20);
            let Some(out) = out else {
                return Some((true, None));
            };
            let left = (held.count > 1).then(|| {
                let mut l = held.clone();
                l.count -= 1;
                l
            });
            Some((true, Some((vec![out], left))))
        }
    }
}

/// `SpoutBlockEntity.tick` on the server: the countdown, and the tank told
/// to clients. Not yet: `BlockSpoutingBehaviour` (filling blocks below,
/// other mods' only).
pub(super) fn spout_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let ticks = processing_ticks(entity);
    let next = super::create_spout::tick(ticks);
    if next != ticks {
        set_processing_ticks(entity, next);
    }
    let tank = spout_tank(entity);
    super::create_fluid_glue::note_spout(game.dim.id(), pos, &tank.fluid, next);
    // spawnProcessingParticles: while it pours, one a tick falling from its
    // nozzle.
    if next >= 8 && !tank.fluid.is_empty() {
        let at = [f64::from(pos[0]) + 0.5, f64::from(pos[1]), f64::from(pos[2]) + 0.5];
        fluid_particles(game, &tank.fluid.fluid, at, [0.0, -1.0, 0.0], 0.1, 0);
    }
}

/// `FluidFX.getFluidParticle`: Create's fluid particle takes the fluid's
/// texture; Horizon has no particle drawn from a fluid, so the fluid
/// block's own particle (its still texture, tinted as the block) stands in.
/// `count` 0 sends one moving along `spread` at `speed`.
fn fluid_particles(game: &mut Game<'_>, fluid: &str, at: [f64; 3], spread: [f64; 3], speed: f64, count: i32) {
    let Some(state) = game.pack.assets.registry.default_state(fluid) else {
        return;
    };
    let options = horizon_client::mod_api::ParticleOptions::simple(horizon_client::mod_api::ParticleType::Block);
    game.with_level(|_, level, _| {
        level.native_send_particles(&options, Some(state), at, spread, speed, count);
    });
}

// ---- Clients ------------------------------------------------------------------

/// Clients' spouts: the processing ticks they were told and when.
static CLIENT: Mutex<Option<HashMap<Pos, (i32, std::time::Instant)>>> = Mutex::new(None);

/// The spout's processing ticks on this client, counted on from the last
/// it was told (`processingTicks - partialTicks`); none when idle.
fn client_processing(pos: Pos, told: i32) -> Option<f32> {
    let mut guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    let now = std::time::Instant::now();
    let entry = map.entry(pos).or_insert((told, now));
    if entry.0 != told {
        *entry = (told, now);
    }
    if told < 0 {
        return None;
    }
    let pt = told as f32 - entry.1.elapsed().as_secs_f32() * 20.0;
    (pt >= -1.0).then_some(pt)
}

/// `SpoutRenderer` through `native_block_entity_submit`: the three nozzle
/// bits, squeezed down while it spouts. The tank's fluid and the stream are
/// drawn with the other fluids (`create_fluid_glue::draw`).
pub(super) fn submit(
    game: &Game<'_>,
    pos: Pos,
    entity: &ModBlockEntity,
    draw: &mut super::native_block_entity_draw::BlockEntityDraw,
) {
    if entity.id != SPOUT {
        return;
    }
    let (fluid, ticks) = super::create_fluid_glue::client_spout(game, pos).unwrap_or_default();
    let (_, squeeze) = super::create_spout::nozzle(client_processing(pos, ticks), !fluid.is_empty());
    draw.push();
    for bit in ["create:hz_part_spout_top", "create:hz_part_spout_middle", "create:hz_part_spout_bottom"] {
        game.native_draw_block(draw, bit, &[]);
        draw.translate([0.0, -3.0 * squeeze / 32.0, 0.0]);
    }
    draw.pop();
}

/// The stream below a spouting spout, for `create_fluid_glue::draw`: its
/// box from the block's corner, none when there is none.
pub(super) fn client_stream(pos: Pos, ticks: i32, has_fluid: bool) -> Option<[f32; 6]> {
    let (radius, _) = super::create_spout::nozzle(client_processing(pos, ticks), has_fluid);
    let r = radius?.abs() / 32.0;
    (r > 0.0).then(|| [0.5 - r, -1.2, 0.5 - r, 0.5 + r, 0.0, 0.5 + r])
}

// ---- The item drain ---------------------------------------------------------------

pub(super) const DRAIN: &str = "create:item_drain";

/// `create:emptying` recipes whose fluid Horizon has: their item
/// ingredient read.
type EmptyingRecipes = Vec<(BasinRecipe, Ingredient)>;

static EMPTYING: Mutex<Option<Arc<EmptyingRecipes>>> = Mutex::new(None);

fn emptying_recipes(game: &Game<'_>) -> Arc<EmptyingRecipes> {
    let mut guard = EMPTYING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = guard.as_ref() {
        return r.clone();
    }
    let list: EmptyingRecipes = game
        .recipes()
        .custom
        .get("create:emptying")
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|r| parse_basin_recipe(&r.id, &r.json))
        .filter(|r| r.supported() && r.ingredients.len() == 1 && r.fluid_results.len() == 1)
        .filter_map(|r| {
            let json = serde_json::Value::Array(r.ingredients[0].iter().cloned().map(serde_json::Value::String).collect());
            let ingredient = super::create_machines::read_ingredient(game, &json)?;
            Some((r, ingredient))
        })
        .collect();
    let list = Arc::new(list);
    *guard = Some(list.clone());
    list
}

pub(super) fn is_drain(game: &Game<'_>, pos: Pos) -> bool {
    game.pack
        .assets
        .registry
        .state(game.world.block(pos[0], pos[1], pos[2]))
        .is_some_and(|s| s.name == DRAIN)
}

/// `GenericItemEmptying.emptyItem` of one item: the fluid it gives and what
/// is left of it (an `emptying` recipe, or the generic buckets and water
/// bottle); none when it can't be emptied (`canItemBeEmptied`).
fn empty_item(game: &Game<'_>, stack: &ItemStack) -> Option<(FluidStack, Option<ItemStack>)> {
    for (r, i) in emptying_recipes(game).iter() {
        if i.test(stack) {
            let out = super::create_machines::roll_recipe(&r.as_recipe()).into_iter().next();
            return Some((r.fluid_results[0].clone(), out));
        }
    }
    let potion = stack.potion();
    let (fluid, item) = super::create_spout::generic_empty(&stack.item, potion)?;
    Some((fluid, Some(ItemStack::new(item, 1))))
}

/// The drain's tank (1500 mB), in its block entity's data as the spout's.
pub(super) fn drain_tank(be: &ModBlockEntity) -> FluidTank {
    let mut tank = spout_tank(be);
    tank.capacity = super::create_spout::DRAIN_CAPACITY;
    tank
}

fn get_float(be: &ModBlockEntity, key: &str) -> f32 {
    match be.nbt.get(&JavaString::from(key)) {
        Some(Tag::Float(v)) => f32::from_bits(*v),
        _ => 0.0,
    }
}

/// The held item's place (`HeldItem`'s `BeltPosition` and `InsertedFrom`),
/// none when it holds nothing. An item put in through the slot without a
/// side (a hopper) travels south.
fn drain_item(be: &ModBlockEntity) -> Option<super::create_spout::DrainItem> {
    be.items.first()?.as_ref()?;
    let from = match be.nbt.get(&JavaString::from("InsertedFrom")) {
        Some(Tag::String(s)) => super::create_kinetics::Dir::parse(&s.to_string_lossy()),
        _ => None,
    };
    Some(super::create_spout::DrainItem {
        position: get_float(be, "BeltPosition"),
        inserted_from: from.unwrap_or(super::create_kinetics::Dir::South),
    })
}

fn set_drain_item(be: &mut ModBlockEntity, item: Option<super::create_spout::DrainItem>) {
    match item {
        Some(i) => {
            be.nbt.insert(JavaString::from("BeltPosition"), Tag::Float(i.position.to_bits()));
            be.nbt
                .insert(JavaString::from("InsertedFrom"), Tag::String(JavaString::from(i.inserted_from.name())));
        }
        None => {
            be.nbt.remove(&JavaString::from("BeltPosition"));
            be.nbt.remove(&JavaString::from("InsertedFrom"));
        }
    }
}

/// `ItemDrainBlockEntity.tryInsertingFromSide` (`DirectBeltInputBehaviour`'s
/// handler) and `ItemDrainItemHandler.insertItem`: into an empty drain, one
/// item when it can be emptied, else the whole stack; `side` is the way it
/// travels. What is left, none when the drain holds something already.
pub(super) fn drain_insert(
    game: &mut Game<'_>,
    pos: Pos,
    side: super::create_kinetics::Dir,
    stack: ItemStack,
    simulate: bool,
) -> Option<Option<ItemStack>> {
    let busy = match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(BlockEntity::Mod(be)) => be.items.first().is_some_and(Option::is_some),
        _ => return None,
    };
    if busy {
        return None;
    }
    let (inserted, returned) = if stack.count > 1 && empty_item(game, &stack).is_some() {
        let mut one = stack.clone();
        one.count = 1;
        let mut rest = stack;
        rest.count -= 1;
        (one, Some(rest))
    } else {
        (stack, None)
    };
    if simulate {
        return Some(returned);
    }
    game.native_with_mod_block_entity(pos, |be| {
        if be.items.is_empty() {
            be.items.push(None);
        }
        be.items[0] = Some(inserted);
        set_drain_item(be, Some(super::create_spout::DrainItem::inserted(side)));
    });
    Some(returned)
}

/// `ItemDrainBlockEntity.tick` on the server, with `ItemDrainBlock`'s
/// `updateEntityAfterFallOn` (an item dropped on it goes in, travelling
/// north as Create's `Direction.getNearest` of no motion does).
pub(super) fn drain_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    use super::create_spout::DrainStep;
    if entity.items.is_empty() {
        entity.items.push(None);
    }
    if entity.items[0].is_none() {
        let mut taken = None;
        super::create_machines::take_landed(game, pos, 13.0 / 16.0, |game, stack| {
            if taken.is_some() {
                return Some(stack);
            }
            let (one, rest) = if stack.count > 1 && empty_item(game, &stack).is_some() {
                let mut one = stack.clone();
                one.count = 1;
                let mut rest = stack;
                rest.count -= 1;
                (one, Some(rest))
            } else {
                (stack, None)
            };
            taken = Some(one);
            rest
        });
        if let Some(stack) = taken {
            entity.items[0] = Some(stack);
            set_drain_item(entity, Some(super::create_spout::DrainItem::inserted(super::create_kinetics::Dir::North)));
        }
    }
    let mut item = drain_item(entity);
    let mut processing = get_int(entity, "ProcessingTicks").unwrap_or(0);
    let held = entity.items[0].clone();
    let emptied = held.as_ref().and_then(|s| empty_item(game, s));
    let mut tank = drain_tank(entity);
    let fits = emptied.as_ref().is_none_or(|(f, _)| tank.fill(f, true) == f.amount);
    let step = super::create_spout::drain_tick(&mut item, &mut processing, emptied.is_some(), fits);
    entity.nbt.insert(JavaString::from("ProcessingTicks"), Tag::Int(processing));
    set_drain_item(entity, item);
    match step {
        DrainStep::Empty => {
            if let (Some((fluid, out)), true) = (emptied, held.is_some()) {
                tank.fill(&fluid, false);
                set_spout_tank(entity, &tank);
                // The held stack becomes what is left of one item.
                entity.items[0] = out;
                if entity.items[0].is_none() {
                    set_drain_item(entity, None);
                }
            }
        }
        DrainStep::Output => drain_output(game, pos, entity),
        _ => {}
    }
    let tank = drain_tank(entity);
    super::create_fluid_glue::note_drain(game.dim.id(), pos, &tank.fluid);
    let view = match (&entity.items[0], drain_item(entity)) {
        (Some(s), Some(i)) => {
            // D1 counts are bytes: the way and the place in eighths.
            let code = (i.inserted_from as u32) * 9 + (i.position * 8.0).round() as u32;
            vec![(s.item.to_string(), s.count), (String::new(), code)]
        }
        _ => Vec::new(),
    };
    super::create_machines::note_view(game, pos, view);
}

/// The held item at the far side (`ItemDrainBlockEntity.tick`): into what
/// takes items from belts there, else ejected when nothing solid is in
/// the way; a pulling belt funnel on the drain takes it first
/// (`tryExportingToBeltFunnel`).
fn drain_output(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let (Some(stack), Some(item)) = (entity.items[0].clone(), drain_item(entity)) else {
        return;
    };
    let side = item.inserted_from;
    // A belt funnel on its edge takes it first.
    let above = [pos[0], pos[1] + 1, pos[2]];
    if let Some(left) = super::create_logistics::export_to_belt_funnel(game, above, side.opposite(), &stack) {
        match left {
            None => {
                entity.items[0] = None;
                set_drain_item(entity, None);
            }
            Some(l) if l.count != stack.count => entity.items[0] = Some(l),
            _ => {}
        }
        return;
    }
    let n = side.normal();
    let next = [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]];
    if !super::create_logistics::has_belt_input(game, next) {
        let state = game.world.block(next[0], next[1], next[2]);
        if game.res.facts().get(state).sturdy_full & (1 << side.opposite() as u8) != 0 {
            return;
        }
        let speed = f64::from(super::create_spout::DRAIN_ITEM_SPEED);
        let at = [
            f64::from(pos[0]) + 0.5 + f64::from(n[0]) * 0.75,
            f64::from(pos[1]) + 0.5 + 6.0 / 16.0,
            f64::from(pos[2]) + 0.5 + f64::from(n[2]) * 0.75,
        ];
        let motion = [f64::from(n[0]) * speed, 0.125, f64::from(n[2]) * speed];
        game.spawn_item(stack, at, motion, 10);
        entity.items[0] = None;
        set_drain_item(entity, None);
        return;
    }
    if !super::create_logistics::can_insert_side(game, next, side) {
        return;
    }
    let left = super::create_logistics::insert_side(game, next, side, stack.clone());
    match left {
        None => {
            entity.items[0] = None;
            set_drain_item(entity, None);
        }
        Some(l) if l.count != stack.count => entity.items[0] = Some(l),
        _ => {}
    }
}

/// `ItemDrainBlock.useItemOn`: a full bucket empties into the tank
/// (`FluidHelper.tryEmptyItemIntoBE`, the tank taking it from the player),
/// an item it could empty does nothing, anything else (an empty hand too)
/// takes the held item back. Block items without fluid pass (false).
pub(super) fn drain_click(game: &mut Game<'_>, pos: Pos) -> bool {
    let hand = game.held().cloned();
    if let Some(h) = &hand {
        let block_item = game.pack.assets.registry.default_state(&h.item).is_some();
        if block_item && super::create_fluids::bucket_fluid(&h.item).is_none() {
            return false;
        }
        if let Some(fluid) = super::create_fluids::bucket_fluid(&h.item) {
            let stack = FluidStack::new(fluid, 1000);
            let filled = game
                .native_with_mod_block_entity(pos, |be| {
                    let mut t = drain_tank(be);
                    if t.fill(&stack, true) != 1000 {
                        return false;
                    }
                    t.fill(&stack, false);
                    set_spout_tank(be, &t);
                    true
                })
                .unwrap_or(false);
            if filled {
                let creative = game.native_acting_player().map_or(game.mode_id(), |p| p.game_mode) == 1;
                if !creative {
                    *game.held_mut() = Some(ItemStack::new(super::create_spout::BUCKET, 1));
                }
                return true;
            }
        }
        if empty_item(game, h).is_some() {
            return true;
        }
    }
    let back = game.native_with_mod_block_entity(pos, |be| {
        let back = be.items.first_mut().and_then(Option::take);
        set_drain_item(be, None);
        back
    });
    if let Some(s) = back.flatten() {
        game.give_or_drop(s);
    }
    true
}

/// Clients' drains: the held item and its place, from the depot view.
fn client_drain_item(slots: &[(String, u32)]) -> Option<(String, super::create_spout::DrainItem)> {
    let (id, count) = slots.first()?.clone();
    let code = slots.get(1)?.1;
    if count == 0 || id.is_empty() {
        return None;
    }
    let dir = super::create_kinetics::Dir::ALL.get((code / 9) as usize).copied()?;
    Some((id, super::create_spout::DrainItem { position: (code % 9) as f32 / 8.0, inserted_from: dir }))
}

/// `ItemDrainRenderer.renderItem`, without its roll: the held item lying on
/// the drain at its place along the way it travels.
pub(super) fn draw_drain_item(
    game: &Game<'_>,
    camera: glam::DVec3,
    pos: Pos,
    slots: &[(String, u32)],
    out: &mut Vec<horizon_client::mesher::Vertex>,
) {
    let Some((id, item)) = client_drain_item(slots) else {
        return;
    };
    if item.inserted_from.axis() == super::create_kinetics::Axis::Y {
        return;
    }
    let n = item.inserted_from.opposite().normal();
    let along = f64::from(0.5 - item.position);
    let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
    let centre = corner + glam::DVec3::new(0.5 + f64::from(n[0]) * along, 15.0 / 16.0, 0.5 + f64::from(n[2]) * along);
    // translate(.5, 15/16, .5), the offset along the way, scale(.5), and a
    // flat item laid down (rotateXDegrees(90)); a block item stands.
    let block = game.pack.assets.registry.default_state(&id).is_some();
    let mut m = glam::Mat4::from_translation((centre - camera).as_vec3());
    if !block {
        m *= glam::Mat4::from_rotation_x(90f32.to_radians());
    }
    m *= glam::Mat4::from_scale(glam::Vec3::splat(0.5));
    let stack = ItemStack::new(&id, 1);
    let owner = super::item_draw::owner_at(centre.to_array(), 0.0, DRAIN);
    let resolved =
        game.resolve_item(&stack, horizon_assets::item_model::DisplayContext::Fixed, Some(&owner), pos[0] ^ pos[2]);
    let light = game.world.light(pos[0], pos[1] + 1, pos[2]);
    game.submit_item_world(&resolved, m, horizon_client::block_entity_render::vertex_light(light), out);
}
