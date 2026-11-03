// Horizon's side of Create's spout, injected as `crate::game::create_spout_glue`:
// its block entity (a 1000 mB tank kept in its data, `ProcessingTicks`), its
// work on an item on a depot or belt two blocks below
// (`SpoutBlockEntity.onItemReceived` / `whenItemHeld`, `FillingBySpout`
// with `create:filling` recipes), its tick, and the nozzle and stream drawn
// by its block entity (`SpoutRenderer`). The fluid it holds is reached by
// pipes through `create_fluid_glue`'s `FluidAccess`. The rules are in spout.rs.

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
