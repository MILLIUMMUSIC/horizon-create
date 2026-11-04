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
    apply_processing_on, basin_insert_slot, basin_take, basin_take_fluids, ingredient_json, millstone_speed,
    multiplied_output, parse_basin_recipe, parse_recipe, roll_results, water_wheel_flow_score,
};
use super::create_fluids::BasinTanks;
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
pub(super) const BLAZE: &str = "create:blaze_burner";
pub(super) const LINK: &str = "create:redstone_link";
/// DepotBehaviour: the held item, then `processingOutputBuffer`.
pub(super) const DEPOT_SLOTS: usize = 9;
/// MillstoneBlockEntity: `inputInv`, then `outputInv`.
const MILLSTONE_SLOTS: usize = 10;
const SAW_SLOTS: usize = 32;
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

/// A brass funnel's amount as this client set it, before the server's
/// update.
pub(super) fn client_set_count(pos: Pos, slot: usize, count: u32) {
    let mut guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    let slots = guard.get_or_insert_with(HashMap::new).entry(pos).or_insert_with(Vec::new);
    if slots.len() <= slot {
        slots.resize(slot + 1, (String::new(), 64));
    }
    slots[slot].1 = count;
}

/// A depot's (`getComparatorInputOverride`: its stacks against 64) or a
/// basin's comparator level (`calcRedstoneFromInventory` of its inputs).
pub(super) fn container_analog(level: &super::GameLevel<'_>, pos: Pos, name: &str, max_stack: &dyn Fn(&str) -> u32) -> i32 {
    let Some(horizon_client::containers::BlockEntity::Mod(m)) = level.world.block_entities.get(&(pos[0], pos[1], pos[2]))
    else {
        return 0;
    };
    if name == DEPOT {
        let present: u32 = m.items.iter().flatten().map(|s| s.count).sum();
        let f = present as f32 / 64.0;
        return ((f * 14.0).floor() as i32 + i32::from(f > 0.0)).clamp(0, 15);
    }
    let inputs = (0..BASIN_SLOTS).map(|i| m.items.get(i).and_then(Option::as_ref));
    super::create_glue::redstone_from_inventory(inputs, 64, max_stack)
}

/// What this client was last told the block at `pos` holds.
/// Every position clients were told of.
pub(super) fn client_positions() -> Vec<Pos> {
    CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or_else(Vec::new, |m| m.keys().copied().collect())
}

pub(super) fn client_slots(pos: Pos) -> Option<Vec<(String, u32)>> {
    CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref()?.get(&pos).cloned()
}

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
    super::create_spout_glue::reset();
    super::create_crafter_glue::reset();
    super::create_crusher_glue::reset();
    super::create_redstone_glue::reset();
    *CLIENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

// ---- Block entities -------------------------------------------------------

/// `native_block_entity`: the depot's and millstone's block entities.
pub(super) fn block_entity(block: &str, entity: &mut Option<(String, Compound)>) {
    if super::create_arms::block_entity(block, entity) {
        return;
    }
    super::create_logistics::block_entity(block, entity);
    if entity.is_none() && super::create_logistics::is_chute_block(block) {
        *entity = Some((super::create_logistics::CHUTE.to_owned(), Compound::new()));
    }
    if entity.is_none() && (block == DEPOT || block == MILLSTONE || block == BASIN || block == BLAZE || block == LINK || block == super::create_spout_glue::SPOUT || block == super::create_spout_glue::DRAIN || block == super::create_crafter_glue::CRAFTER || block == super::create_crusher_glue::CONTROLLER || super::create_redstone_glue::is_ours(block) || is_saw(block) || block == super::create_fluid_glue::SMART || block == super::create_engine_glue::ENGINE || block == super::create_ejector_glue::EJECTOR || block == super::create_whistle_glue::WHISTLE) {
        *entity = Some((block.to_owned(), Compound::new()));
    }
}

/// `native_block_entity_ticks`.
pub(super) fn block_entity_ticks(block: &str, ticks: &mut bool) {
    if super::create_arms::block_entity_ticks(block) {
        *ticks = true;
    }
    if block == DEPOT || block == MILLSTONE || block == BASIN || block == BLAZE || block == LINK || block == super::create_spout_glue::SPOUT || block == super::create_spout_glue::DRAIN || block == super::create_crafter_glue::CRAFTER || block == super::create_crusher_glue::CONTROLLER || super::create_redstone_glue::is_ours(block) || is_saw(block) || super::create_logistics::is_chute_block(block) || block == super::create_fluid_glue::SMART || block == super::create_engine_glue::ENGINE || block == super::create_ejector_glue::EJECTOR || block == super::create_whistle_glue::WHISTLE {
        *ticks = true;
    }
}

/// `native_block_entity_container`: DepotItemHandler (insert into the held
/// slot, take from any) and the millstone's input and output inventories.
pub(super) fn block_entity_container(block: &str, container: &mut Option<ModContainer>) {
    if let Some(c) = super::create_arms::block_entity_container(block) {
        *container = Some(c);
        return;
    }
    // Every chute's block has the chute's block entity.
    let block = if super::create_logistics::is_chute_block(block) { super::create_logistics::CHUTE } else { block };
    let (slots, inputs, outputs): (usize, Vec<usize>, Vec<usize>) = match block {
        DEPOT | super::create_ejector_glue::EJECTOR => (DEPOT_SLOTS, vec![0], (0..DEPOT_SLOTS).collect()),
        MILLSTONE => (MILLSTONE_SLOTS, vec![0], (1..MILLSTONE_SLOTS).collect()),
        // ProcessingInventory: 32 slots, input in the first, nothing taken.
        b if is_saw(b) => (SAW_SLOTS, vec![0], Vec::new()),
        // The output inventory forbids insertion; both are taken from
        // (`CombinedInvWrapper`).
        BASIN => (BASIN_ALL_SLOTS, (0..BASIN_SLOTS).collect(), (0..BASIN_ALL_SLOTS).collect()),
        // ChuteItemHandler: its one item.
        super::create_logistics::CHUTE => (1, vec![0], vec![0]),
        // ItemDrainItemHandler: its held item, from the sides only.
        super::create_spout_glue::DRAIN => (1, vec![0], vec![0]),
        // The crafter's slot: no face (funnels and chutes go through
        // `create_crafter_glue::insert`, one item at a time); kept as a slot
        // so breaking drops it.
        super::create_crafter_glue::CRAFTER => (1, Vec::new(), Vec::new()),
        // ProcessingInventory: its own insertion (`create_crusher_glue::insert`)
        // for what is empty, nothing taken; a slot list so breaking drops it.
        super::create_crusher_glue::CONTROLLER => (super::create_crusher::SLOTS, Vec::new(), Vec::new()),
        _ => return,
    };
    let all: Vec<usize> = (0..slots).collect();
    // The saw's inventory answers every side but the bottom.
    let bottom = is_saw(block);
    let sides_only = block == super::create_spout_glue::DRAIN;
    let no_faces = block == super::create_crafter_glue::CRAFTER || block == super::create_crusher_glue::CONTROLLER;
    *container = Some(ModContainer {
        slots,
        faces: std::array::from_fn(|i| if (bottom && i == 0) || (sides_only && i < 2) || no_faces { Vec::new() } else { all.clone() }),
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

pub(super) fn max_stack(game: &Game<'_>, stack: &ItemStack) -> u32 {
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
pub(super) fn find_recipe(game: &Game<'_>, kind: &'static str, stack: &ItemStack) -> Option<Recipe> {
    find_recipes(game, kind, stack).into_iter().next()
}

/// Every recipe of `kind` that takes `stack`, in file order.
fn find_recipes(game: &Game<'_>, kind: &'static str, stack: &ItemStack) -> Vec<Recipe> {
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
        .filter(|(_, i)| i.as_ref().is_some_and(|i| i.test(stack)))
        .map(|(r, _)| r.clone())
        .collect()
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
        .order
        .into_iter()
        .filter(|item| items.get(item).is_some())
        .collect();
    (!known.is_empty()).then(|| Ingredient::of(known))
}

/// `RecipeApplier.applyRecipeOn(.., returnProcessingRemainder)`: one
/// processed `input`'s crafting remainder (a bucket, a bottle) joins the
/// results (`ItemHelper.addToList`).
pub(super) fn with_remainder(game: &Game<'_>, input: &str, mut results: Vec<ItemStack>) -> Vec<ItemStack> {
    let Some(rem) = game.items().get(input).and_then(|i| i.crafting_remainder.clone()) else {
        return results;
    };
    let max = game.items().get(&rem).map_or(64, |i| i.max_stack);
    if let Some(r) = results.iter_mut().find(|r| *r.item == *rem && r.count < max) {
        r.count += 1;
    } else {
        results.push(ItemStack::new(&rem, 1));
    }
    results
}

/// `ingredient` for other modules (the spout's filling recipes).
pub(super) fn read_ingredient(game: &Game<'_>, json: &serde_json::Value) -> Option<Ingredient> {
    ingredient(game, json)
}

/// `roll` for other modules.
pub(super) fn roll_recipe(r: &Recipe) -> Vec<ItemStack> {
    roll(r)
}

/// The machines' random float, 0 to 1 (`level.random`'s place).
pub(super) fn random_float() -> f32 {
    machines().get_or_insert_with(Machines::default).next_float()
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

/// `BlazeBurnerBlock.getHeatLevelOf`: the heat of the burner at `pos` (none
/// when there is none).
fn burner_heat(game: &Game<'_>, pos: Pos) -> super::create_blaze::HeatLevel {
    use super::create_blaze::HeatLevel;
    game.pack
        .assets
        .registry
        .state(game.world.block(pos[0], pos[1], pos[2]))
        .filter(|s| s.name == BLAZE)
        .and_then(|s| s.properties.get("heat_level").and_then(|h| HeatLevel::parse(h)))
        .unwrap_or(HeatLevel::None)
}

/// A burner's fuel as its block entity keeps it (`fuelLevel`, `burnTimeRemaining`
/// and `isCreative`).
fn read_burner(nbt: &Compound) -> super::create_blaze::Burner {
    use super::create_blaze::{Burner, Fuel};
    Burner {
        fuel: match get_int(nbt, "fuelLevel") {
            1 => Fuel::Normal,
            2 => Fuel::Special,
            _ => Fuel::None,
        },
        remaining: get_int(nbt, "burnTimeRemaining"),
        creative: get_bool(nbt, "isCreative"),
    }
}

fn write_burner(nbt: &mut Compound, b: &super::create_blaze::Burner) {
    use super::create_blaze::Fuel;
    put_int(nbt, "fuelLevel", match b.fuel {
        Fuel::None => 0,
        Fuel::Normal => 1,
        Fuel::Special => 2,
    });
    put_int(nbt, "burnTimeRemaining", b.remaining);
    put_bool(nbt, "isCreative", b.creative);
}

/// `setBlockHeat`: the block takes `heat` as its `heat_level`.
fn set_burner_heat(game: &mut Game<'_>, pos: Pos, heat: super::create_blaze::HeatLevel) {
    if burner_heat(game, pos) == heat {
        return;
    }
    let Some(base) = game.pack.assets.registry.default_state(BLAZE) else {
        return;
    };
    if let Some(id) = game.with_props(base, &[("heat_level", heat.name())]) {
        let dimension = game.dim.id().to_owned();
        game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
    }
}

/// `BlazeBurnerRenderer.renderShared` through `native_block_entity_submit`:
/// the cage, the blaze's head (turned to the nearest player, or the way it
/// faces over a basin or tank) bobbing as it does, and the rods. Not yet: the
/// flame's scrolling texture, goggles and hats.
pub(super) fn submit_blaze(
    game: &Game<'_>,
    pos: Pos,
    entity: &ModBlockEntity,
    draw: &mut super::native_block_entity_draw::BlockEntityDraw,
) {
    use super::create_blaze::HeatLevel;
    if entity.id != BLAZE {
        return;
    }
    game.native_draw_block(draw, "create:hz_part_blaze_cage", &[]);
    let heat = burner_heat(game, pos);
    if heat == HeatLevel::None {
        return;
    }
    let above = [pos[0], pos[1] + 1, pos[2]];
    let above_name = game
        .pack
        .assets
        .registry
        .state(game.world.block(above[0], above[1], above[2]))
        .map(|s| s.name.clone())
        .unwrap_or_default();
    let valid_above = above_name == BASIN || super::create_fluid_glue::is_tank_block(&above_name);
    let active = heat.is_at_least(HeatLevel::Fading) && valid_above;
    let animation = if active { 0.175f32 } else { 0.0 };
    // The head turns to the nearest player; over a basin it faces south.
    let angle = if active {
        180.0
    } else {
        game.player.as_ref().map_or(180.0, |p| {
            let (dx, dz) = (p.position.x - (f64::from(pos[0]) + 0.5), p.position.z - (f64::from(pos[2]) + 0.5));
            (-dz.atan2(dx)).to_degrees() as f32 - 90.0
        })
    };
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let time = START.get_or_init(std::time::Instant::now).elapsed().as_secs_f32() * 20.0;
    let tick = time + ((pos[0].wrapping_mul(31) ^ pos[2].wrapping_mul(17) ^ pos[1]).rem_euclid(13)) as f32 * 16.0;
    let mult = if heat.is_at_least(HeatLevel::Fading) { 64.0 } else { 16.0 };
    let pi = std::f32::consts::PI;
    let wave = |shift: f32| ((tick / 16.0 + shift) % (2.0 * pi)).sin() / mult;
    let (offset, offset1, offset2) = (wave(0.0), wave(pi), wave(pi / 2.0));
    let head_y = offset - animation * 0.75;
    let model = format!("create:hz_part_blaze_{}", super::create_blaze::head_model(heat, valid_above));
    draw.push();
    draw.translate([0.0, head_y, 0.0]);
    draw.rotate_about([0.0, 1.0, 0.0], angle, [0.5; 3]);
    game.native_draw_block(draw, &model, &[]);
    draw.pop();
    if heat.is_at_least(HeatLevel::Fading) {
        let (rods, rods2) = if heat == HeatLevel::Seething {
            ("create:hz_part_blaze_super_rods", "create:hz_part_blaze_super_rods_2")
        } else {
            ("create:hz_part_blaze_rods", "create:hz_part_blaze_rods_2")
        };
        draw.push();
        draw.translate([0.0, offset1 + animation + 0.125, 0.0]);
        game.native_draw_block(draw, rods, &[]);
        draw.pop();
        draw.push();
        draw.translate([0.0, offset2 + animation - 3.0 / 16.0, 0.0]);
        game.native_draw_block(draw, rods2, &[]);
        draw.pop();
    }
}

/// One redstone link as the network sees it (`LinkBehaviour`): its two
/// frequency items, whether it listens, and what it sends and received.
#[derive(Clone, Debug, Default)]
struct LinkInfo {
    freq: (String, String),
    receiver: bool,
    transmit: i32,
    received: i32,
}

static LINKS: Mutex<Option<HashMap<(String, Pos), LinkInfo>>> = Mutex::new(None);

/// What the powered receiver at `pos` gives (`getReceivedSignal`).
pub(super) fn link_signal(dimension: &str, pos: Pos) -> i32 {
    LINKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(&(dimension.to_owned(), pos)))
        .map_or(0, |l| l.received)
}

fn link_frequency(nbt: &Compound) -> (String, String) {
    let text = |k: &str| match nbt.get(&JavaString::from(k)) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    };
    (text("FrequencyFirst"), text("FrequencyLast"))
}

/// `RedstoneLinkBlockEntity.tick` and `RedstoneLinkNetworkHandler`: a
/// transmitter sends the strongest signal beside it; a receiver takes the
/// strongest of the transmitters of its frequency pair within `linkRange`
/// (256) and powers the block it is mounted on.
fn link_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let Some((receiver, powered)) = game
        .pack
        .assets
        .registry
        .state(game.world.block(pos[0], pos[1], pos[2]))
        .filter(|s| s.name == LINK)
        .map(|s| {
            (
                s.properties.get("receiver").map(String::as_str) == Some("true"),
                s.properties.get("powered").map(String::as_str) == Some("true"),
            )
        })
    else {
        return;
    };
    let dimension = game.dim.id().to_owned();
    let freq = link_frequency(&entity.nbt);
    // RedstoneLinkFrequencySlot: clients draw the two items.
    note_view(game, pos, vec![(freq.0.clone(), u32::from(!freq.0.is_empty())), (freq.1.clone(), u32::from(!freq.1.is_empty()))]);
    let mut power = 0;
    if !receiver {
        power = game.with_level(|b, level, ticks| {
            b.best_neighbor_signal(level, ticks, horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]))
        });
    }
    let (received, changed) = {
        let mut guard = LINKS.lock().unwrap_or_else(|e| e.into_inner());
        let map = guard.get_or_insert_with(HashMap::new);
        // Links that are gone leave the network.
        let gone: Vec<(String, Pos)> = map
            .keys()
            .filter(|(d, p)| *d == dimension && game.world.loaded(p[0], p[2]) && burner_name(game, *p) != LINK)
            .cloned()
            .collect();
        for k in gone {
            map.remove(&k);
        }
        let me = map.entry((dimension.clone(), pos)).or_default();
        me.freq = freq.clone();
        me.receiver = receiver;
        if !receiver {
            me.transmit = power;
        } else {
            me.transmit = 0;
        }
        let before = me.received;
        let mut best = 0;
        if receiver {
            let range2 = 256.0f64 * 256.0;
            for ((d, p), other) in map.iter() {
                if *d != dimension || other.receiver || other.freq != freq {
                    continue;
                }
                let d2: f64 = (0..3).map(|k| f64::from(p[k] - pos[k]).powi(2)).sum();
                if d2 < range2 {
                    best = best.max(other.transmit);
                }
            }
        }
        let me = map.get_mut(&(dimension.clone(), pos)).expect("link");
        if receiver {
            me.received = best;
        } else {
            me.received = 0;
        }
        (me.received, me.received != before)
    };
    let wanted = if receiver { received > 0 } else { power > 0 };
    if wanted != powered {
        let facing = game
            .pack
            .assets
            .registry
            .state(game.world.block(pos[0], pos[1], pos[2]))
            .and_then(|s| s.properties.get("facing").cloned())
            .unwrap_or_else(|| "north".to_owned());
        if let Some(base) = game.pack.assets.registry.default_state(LINK)
            && let Some(id) = game.with_props(
                base,
                &[
                    ("facing", facing.as_str()),
                    ("receiver", if receiver { "true" } else { "false" }),
                    ("powered", if wanted { "true" } else { "false" }),
                ],
            )
        {
            game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
        }
    }
    if receiver && (changed || wanted != powered) {
        game.native_update_signal_neighbors(&dimension, pos);
    }
}

fn burner_name(game: &Game<'_>, pos: Pos) -> String {
    game.pack
        .assets
        .registry
        .state(game.world.block(pos[0], pos[1], pos[2]))
        .map(|s| s.name.clone())
        .unwrap_or_default()
}

/// `RedstoneLinkBlock.useWithoutItem` / `onWrenched` and `LinkBehaviour`'s
/// frequency slots: sneaking with an empty hand or the wrench flips it between
/// transmitter and receiver; an item in the hand sets the slot it was clicked on
/// (the upper or the lower, as the face's frame shows them). True when it
/// took the click.
fn link_click(game: &mut Game<'_>, pos: Pos, face: [i32; 3]) -> bool {
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    let held = game.held().map(|s| s.item.to_string());
    let wrench = held.as_deref() == Some("create:wrench");
    let Some(state) = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2])).cloned() else {
        return false;
    };
    let receiver = state.properties.get("receiver").map(String::as_str) == Some("true");
    let facing = state.properties.get("facing").cloned().unwrap_or_else(|| "north".to_owned());
    if wrench || (sneaking && held.is_none()) {
        // toggleMode: the other mode, powered as its surroundings say.
        let powered = game.native_acting_player().is_some() && !receiver && {
            game.with_level(|b, level, ticks| {
                b.has_neighbor_signal(level, ticks, horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]))
            })
        };
        let base = game.pack.assets.registry.default_state(LINK);
        if let Some(base) = base
            && let Some(id) = game.with_props(
                base,
                &[
                    ("facing", facing.as_str()),
                    ("receiver", if receiver { "false" } else { "true" }),
                    ("powered", if powered { "true" } else { "false" }),
                ],
            )
        {
            let dimension = game.dim.id().to_owned();
            game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
        }
        return true;
    }
    let Some(item) = held else {
        return false;
    };
    if game.native_acting_player().is_some_and(|p| p.game_mode == 3) {
        return false;
    }
    // Which slot: the face's two are one above the other (up and down faces:
    // one behind the other); down is turned over.
    let click = super::create_logistics::click_point(game, pos, super::create_kinetics::Dir::nearest(face));
    let upper = match facing.as_str() {
        "up" => click[2] > 0.0,
        "down" => click[2] < 0.0,
        _ => click[1] > 0.0,
    };
    let key = if upper { "FrequencyFirst" } else { "FrequencyLast" };
    game.native_with_mod_block_entity(pos, |be| {
        be.nbt.insert(JavaString::from(key), Tag::String(item.as_str().into()));
    });
    true
}

/// `BlazeBurnerBlockEntity.tick` on the server.
fn blaze_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let mut burner = read_burner(&entity.nbt);
    if burner.tick() {
        write_burner(&mut entity.nbt, &burner);
        set_burner_heat(game, pos, burner.heat_level());
    } else if !burner.creative {
        write_burner(&mut entity.nbt, &burner);
    }
}

/// `BlazeBurnerBlock.useItemOn` / `tryInsert`: fuel in the hand feeds the
/// burner. True when it took the click.
fn blaze_click(game: &mut Game<'_>, pos: Pos) -> bool {
    let Some(held) = game.held().cloned() else {
        return false;
    };
    if game.native_acting_player().is_some_and(|p| p.game_mode == 3) {
        return false;
    }
    let Some(container) = burner_insert(game, pos, &held, false) else {
        return false;
    };
    // BlazeBurnerBlock.useItemOn: one used up and its container given back,
    // unless creative.
    let creative_player = game.native_acting_player().is_some_and(|p| p.game_mode == 1);
    if !creative_player {
        let hand = game.held_mut();
        if let Some(h) = hand.as_mut() {
            h.count -= 1;
            if h.count == 0 {
                *hand = None;
            }
        }
        if let Some(c) = container {
            if game.held().is_none() {
                *game.held_mut() = Some(c);
            } else {
                game.give_or_drop(c);
            }
        }
    }
    true
}

/// `BlazeBurnerBlock.tryInsert` of one item of `stack` (not taken from it):
/// none when the burner doesn't take it, else its crafting remainder (a
/// lava bucket's bucket). The creative blaze cake lights it for good.
pub(super) fn burner_insert(game: &mut Game<'_>, pos: Pos, stack: &ItemStack, simulate: bool) -> Option<Option<ItemStack>> {
    use super::create_blaze::Fuel;
    let item = stack.item.to_string();
    // Special fuel (the superheated data map: the blaze cake), the creative
    // blaze cake, else what burns in a furnace.
    let creative = item == "create:creative_blaze_cake";
    let (kind, burn_time) = if item == "create:blaze_cake" {
        (Fuel::Special, 3200)
    } else {
        let time = game.res.providers().burn_time(game.items(), stack, false);
        if time > 0 { (Fuel::Normal, time) } else { (Fuel::None, 0) }
    };
    if !creative && kind == Fuel::None {
        return None;
    }
    let mut burner = game.native_with_mod_block_entity(pos, |be| read_burner(&be.nbt))?;
    let before = burner_heat(game, pos);
    if creative {
        if simulate {
            return Some(None);
        }
        // applyCreativeFuel: the burner stays lit at the next level up.
        let mut next = before.next_active_level();
        if next == super::create_blaze::HeatLevel::Fading {
            next = next.next_active_level();
        }
        burner = super::create_blaze::Burner { fuel: Fuel::None, remaining: 0, creative: true };
        set_burner_heat(game, pos, next);
    } else {
        if !burner.try_update_fuel(kind, burn_time, false, simulate) {
            return None;
        }
        if simulate {
            return Some(game.items().get(&item).and_then(|i| i.crafting_remainder.clone()).map(|r| ItemStack::new(&r, 1)));
        }
        set_burner_heat(game, pos, burner.heat_level());
    }
    game.native_with_mod_block_entity(pos, |be| write_burner(&mut be.nbt, &burner));
    // playSound, and the blaze's ambient when the level changed.
    let at = pos.map(|v| f64::from(v) + 0.5);
    let changed = burner_heat(game, pos) != before;
    game.with_level(|_, level, _| {
        level.native_play_sound("minecraft:entity.blaze.shoot", at, super::create_sounds::BLOCKS, 0.2, 0.65);
        if changed {
            level.native_play_sound("minecraft:entity.blaze.ambient", at, super::create_sounds::BLOCKS, 0.2, 1.05);
        }
    });
    if creative {
        return Some(None);
    }
    Some(game.items().get(&item).and_then(|i| i.crafting_remainder.clone()).map(|r| ItemStack::new(&r, 1)))
}

/// `native_block_entity_tick`.
pub(super) fn block_entity_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    match entity.id.as_str() {
        DEPOT => depot_tick(game, pos, entity),
        super::create_ejector_glue::EJECTOR => super::create_ejector_glue::tick(game, pos, entity),
        super::create_whistle_glue::WHISTLE => super::create_whistle_glue::tick(game, pos, entity),
        MILLSTONE => millstone_tick(game, pos, entity),
        b if is_saw(b) => saw_tick(game, pos, entity),
        BLAZE => blaze_tick(game, pos, entity),
        LINK => link_tick(game, pos, entity),
        super::create_spout_glue::SPOUT => super::create_spout_glue::spout_tick(game, pos, entity),
        super::create_spout_glue::DRAIN => super::create_spout_glue::drain_tick(game, pos, entity),
        super::create_crafter_glue::CRAFTER => super::create_crafter_glue::tick(game, pos, entity),
        super::create_crusher_glue::CONTROLLER => super::create_crusher_glue::tick(game, pos, entity),
        n if super::create_redstone_glue::is_ours(n) => super::create_redstone_glue::tick(game, pos, entity),
        BASIN => basin_tick(game, pos, entity),
        super::create_fluid_glue::SMART => super::create_fluid_glue::smart_tick(game, pos, entity),
        super::create_engine_glue::ENGINE => super::create_engine_glue::tick(game, pos, entity),
        super::create_logistics::CHUTE => super::create_logistics::chute_tick(game, pos, entity),
        _ => {
            super::create_arms::block_entity_tick(game, pos, entity);
        }
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
    let results = with_remainder(game, &held.item, roll(&recipe));
    let left = (held.count > 1).then(|| {
        let mut l = held.clone();
        l.count -= 1;
        l
    });
    Some((true, Some((results, left))))
}

/// `SharedDepotBlockMethods.onLanded` on an ejector: up to its stack size.
pub(super) fn take_landed_depot(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity, stack_size: i32) {
    let limit = if stack_size == 0 { 64 } else { stack_size as u32 };
    take_landed(game, pos, 13.0 / 16.0, |game, stack| {
        let items = &mut entity.items;
        let present: u32 = items.iter().flatten().map(|s| s.count).sum();
        match &mut items[0] {
            Some(held) if !held.same_item_same_components(&stack) => Some(stack),
            slot => {
                let room = limit.min(max_stack(game, &stack)).saturating_sub(present).min(stack.count);
                if room == 0 {
                    return Some(stack);
                }
                match slot {
                    Some(held) => held.count += room,
                    None => {
                        let mut s = stack.clone();
                        s.count = room;
                        *slot = Some(s);
                    }
                }
                let mut left = stack;
                left.count -= room;
                (left.count > 0).then_some(left)
            }
        }
    });
}

pub(super) fn process_on_depot_of(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    process_on_depot(game, pos, entity);
}

pub(super) fn set_locked(entity: &mut ModBlockEntity, v: bool) {
    put_bool(&mut entity.nbt, "Locked", v);
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
            .or_else(|| super::create_arms::deployer_received(game, press_pos, &held))
            .or_else(|| super::create_spout_glue::spout_received(game, press_pos, &held))
    } else {
        let worked = press_held(game, press_pos, &held)
            .or_else(|| super::create_arms::deployer_held(game, press_pos, &held))
            .or_else(|| super::create_spout_glue::spout_held(game, press_pos, &held));
        worked.map(|(hold, pressed)| {
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

pub(super) fn is_saw(block: &str) -> bool {
    block == super::create_actors::SAW || block == super::create_actors::SAW_ALONG
}

fn get_float(nbt: &Compound, key: &str) -> Option<f32> {
    match nbt.get(&JavaString::from(key)) {
        Some(Tag::Float(v)) => Some(f32::from_bits(*v)),
        _ => None,
    }
}

fn put_float(nbt: &mut Compound, key: &str, v: f32) {
    nbt.insert(JavaString::from(key), Tag::Float(v.to_bits()));
}

/// Wood or stone for `SawBlockEntity.applyRecipe`'s `playEvent` (Create asks
/// the block's sound type; here the item's name stands in for it).
fn is_wood(item: &str) -> bool {
    const WOODS: [&str; 12] = [
        "oak", "spruce", "birch", "jungle", "acacia", "mangrove", "cherry", "bamboo", "crimson", "warped",
        "planks", "log",
    ];
    let path = item.rsplit(':').next().unwrap_or(item);
    WOODS.iter().any(|w| path.contains(w)) && !path.contains("stone")
}

/// `SawBlockEntity.tick` for a saw facing up (`canProcess`): the item it
/// holds is cut (`start`, `applyRecipe`) and its pieces handed on along the
/// saw's way (the next belt or depot, else thrown).
fn saw_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    if entity.items.len() < SAW_SLOTS {
        entity.items.resize(SAW_SLOTS, None);
    }
    let name = game
        .pack
        .assets
        .registry
        .state(game.world.block(pos[0], pos[1], pos[2]))
        .map(|s| (s.name.clone(), s.properties.get("facing").cloned()));
    let Some((block, facing)) = name else {
        return;
    };
    if facing.as_deref() != Some("up") {
        return;
    }
    // The filter, told to clients (it is drawn on the top).
    note_view(game, pos, saw_filter(entity).into_iter().map(|i| (i, 1)).collect());
    // SawBlock.updateEntityAfterFallOn: an item landing on it goes in.
    take_landed(game, pos, 1.0, |_, mut stack| {
        if entity.items.iter().any(Option::is_some) {
            return Some(stack);
        }
        // The slot limit of 1 (`bulkCutting` is off).
        let mut one = stack.clone();
        one.count = 1;
        entity.items[0] = Some(one);
        stack.count -= 1;
        (stack.count > 0).then_some(stack)
    });
    let speed = super::create_glue::with_kinetics(game, |k, level| k.speed_of(level, pos));
    if speed == 0.0 {
        return;
    }
    let empty = entity.items.iter().all(Option::is_none);
    let applied = get_bool(&entity.nbt, "AppliedRecipe");
    let remaining = get_float(&entity.nbt, "ProcessingTime").unwrap_or(-1.0);
    if remaining == -1.0 {
        if !empty && !applied {
            saw_start(game, entity);
        }
        return;
    }
    let remaining = remaining - (speed.abs() / 24.0).clamp(1.0, 128.0);
    put_float(&mut entity.nbt, "ProcessingTime", remaining);
    if remaining < 5.0 && !applied {
        if let Some(input) = entity.items[0].clone() {
            let sound = if is_wood(&input.item) { "saw_activate_wood" } else { "saw_activate_stone" };
            super::create_glue::play_create_sound(game, sound, pos.map(|v| f64::from(v) + 0.5), 3.0, 1.0);
        }
        saw_apply(game, entity);
        put_bool(&mut entity.nbt, "AppliedRecipe", true);
        put_float(&mut entity.nbt, "RecipeTime", 20.0);
        put_float(&mut entity.nbt, "ProcessingTime", 20.0);
        return;
    }
    if remaining > 0.0 {
        return;
    }
    put_float(&mut entity.nbt, "ProcessingTime", 0.0);
    // getItemMovementVec.
    let along_x = block != super::create_actors::SAW_ALONG;
    let sign = if speed < 0.0 { -1 } else { 1 };
    let movement = if along_x { [sign, 0, 0] } else { [0, 0, -sign] };
    let toward = super::create_kinetics::Dir::nearest(movement);
    let next = [pos[0] + movement[0], pos[1], pos[2] + movement[2]];
    if super::create_logistics::has_belt_input(game, next) {
        if !super::create_logistics::can_insert_side(game, next, toward) {
            return;
        }
        for slot in entity.items.iter_mut() {
            let Some(stack) = slot.clone() else {
                continue;
            };
            let left = super::create_logistics::insert_side(game, next, toward, stack.clone());
            if left.as_ref().is_some_and(|l| l.count == stack.count) {
                continue;
            }
            *slot = left;
        }
        if entity.items.iter().all(Option::is_none) {
            saw_reset(entity);
        }
        return;
    }
    // Eject Items.
    let at = [
        f64::from(pos[0]) + 0.5 + f64::from(movement[0]) * 0.5,
        f64::from(pos[1]) + 1.0,
        f64::from(pos[2]) + 0.5 + f64::from(movement[2]) * 0.5,
    ];
    let motion = [f64::from(movement[0]) * 0.0625, 0.125, f64::from(movement[2]) * 0.0625];
    for slot in entity.items.iter_mut() {
        if let Some(stack) = slot.take() {
            game.spawn_item(stack, at, motion, 10);
        }
    }
    saw_reset(entity);
}

/// `FilteringBehaviour` on a saw facing up (`SawFilterSlot`, on its top): a
/// click there sets the held item as the filter (an empty hand clears it).
/// True when it took the click.
fn saw_filter_click(game: &mut Game<'_>, pos: Pos, face: [i32; 3]) -> bool {
    let up = game
        .pack
        .assets
        .registry
        .state(game.world.block(pos[0], pos[1], pos[2]))
        .is_some_and(|s| s.properties.get("facing").map(String::as_str) == Some("up"));
    if !up || face != [0, 1, 0] {
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
    game.native_with_mod_block_entity(pos, |be| match &held {
        Some(id) => {
            be.nbt.insert(JavaString::from("Filter"), Tag::String(id.as_str().into()));
        }
        None => {
            be.nbt.remove(&JavaString::from("Filter"));
        }
    });
    let view = held.iter().map(|i| (i.clone(), 1)).collect();
    note_view(game, pos, view);
    true
}

/// The saw's filter item.
fn saw_filter(entity: &ModBlockEntity) -> Option<String> {
    match entity.nbt.get(&JavaString::from("Filter")) {
        Some(Tag::String(s)) => Some(s.to_string_lossy()),
        _ => None,
    }
}

/// `RecipeConditions.outputMatchesFilter`: the recipes with an output the
/// filter takes, all of them with no filter.
fn filtered(recipes: Vec<Recipe>, filter: Option<&str>) -> Vec<Recipe> {
    match filter {
        Some(f) => recipes.into_iter().filter(|r| r.results.iter().any(|o| o.item == f)).collect(),
        None => recipes,
    }
}

fn saw_reset(entity: &mut ModBlockEntity) {
    put_float(&mut entity.nbt, "ProcessingTime", -1.0);
    put_float(&mut entity.nbt, "RecipeTime", 0.0);
    put_bool(&mut entity.nbt, "AppliedRecipe", false);
}

/// `SawBlockEntity.start`: the recipe taken next (the round robin over
/// those that fit) and how long the cut takes.
fn saw_start(game: &Game<'_>, entity: &mut ModBlockEntity) {
    let Some(input) = entity.items[0].clone() else {
        return;
    };
    let recipes = filtered(find_recipes(game, "create:cutting", &input), saw_filter(entity).as_deref());
    put_bool(&mut entity.nbt, "AppliedRecipe", false);
    if recipes.is_empty() {
        put_float(&mut entity.nbt, "ProcessingTime", 10.0);
        put_float(&mut entity.nbt, "RecipeTime", 10.0);
        return;
    }
    let mut index = get_int(&entity.nbt, "RecipeIndex") + 1;
    if index as usize >= recipes.len() {
        index = 0;
    }
    put_int(&mut entity.nbt, "RecipeIndex", index);
    let time = recipes[index as usize].processing_time as f32 * (input.count / 5).max(1) as f32;
    put_float(&mut entity.nbt, "ProcessingTime", time);
    put_float(&mut entity.nbt, "RecipeTime", time);
}

/// `SawBlockEntity.applyRecipe`: the input's cuts rolled once per item of
/// the stack, into the slots after the first; with no recipe the input stays.
fn saw_apply(game: &Game<'_>, entity: &mut ModBlockEntity) {
    let Some(input) = entity.items[0].clone() else {
        return;
    };
    let recipes = filtered(find_recipes(game, "create:cutting", &input), saw_filter(entity).as_deref());
    if recipes.is_empty() {
        return;
    }
    let mut index = get_int(&entity.nbt, "RecipeIndex") as usize;
    if index >= recipes.len() {
        index = 0;
        put_int(&mut entity.nbt, "RecipeIndex", 0);
    }
    let recipe = &recipes[index];
    for slot in entity.items.iter_mut() {
        *slot = None;
    }
    let mut list: Vec<ItemStack> = Vec::new();
    let add = |list: &mut Vec<ItemStack>, stack: ItemStack| {
        if let Some(have) = list.iter_mut().find(|s| s.same_item_same_components(&stack)) {
            have.count += stack.count;
        } else {
            list.push(stack);
        }
    };
    for _ in 0..input.count {
        for out in roll(recipe) {
            add(&mut list, out);
        }
    }
    for (slot, stack) in list.into_iter().enumerate().take(SAW_SLOTS - 1) {
        entity.items[slot + 1] = Some(stack);
    }
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
    // PressingBehaviour.tick at the cycle's middle: a bonk over wool (a
    // belt), else a clang pitched by the speed.
    if step == PressStep::Press {
        let at = pos.map(|v| f64::from(v) + 0.5);
        let belt = game
            .pack
            .assets
            .registry
            .state(game.world.block(pos[0], pos[1] - 2, pos[2]))
            .is_some_and(|s| super::create_logistics::is_belt(&s.name));
        if belt {
            super::create_glue::play_create_sound(game, "mechanical_press_activation_belt", at, 1.0, 1.0);
        } else {
            let speed = press_at(game, pos).map_or(0.0, |(s, _)| s);
            let pitch = 0.75 + speed.abs() / 1024.0;
            super::create_glue::play_create_sound(game, "mechanical_press_activation", at, 0.5, pitch);
        }
    }
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
                let results = with_remainder(game, &stack.item, roll(&recipe));
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
    // `getEntitiesOfClass(ItemEntity.class, box)`: the item's bounding box
    // (0.25 wide and high, from its feet) meets the box. An item lying on
    // the floor has its feet below the deflated box.
    (0..game.dropped.len())
        .filter(|&i| {
            let d = &game.dropped[i];
            let p = d.position;
            d.on_ground
                && d.stack.count > 0
                && p.x + 0.125 > lo[0]
                && p.x - 0.125 < hi[0]
                && p.y + 0.25 > lo[1]
                && p.y < hi[1]
                && p.z + 0.125 > lo[2]
                && p.z - 0.125 < hi[2]
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
pub(super) fn water_wheel_flow(game: &Game<'_>, pos: Pos, axis: Axis, large: bool) -> i32 {
    let a = match axis {
        Axis::X => 0,
        Axis::Y => 1,
        Axis::Z => 2,
    };
    let mut flows = Vec::new();
    let mut world = Flows { game };
    for n in super::create_processing::wheel_offsets(a, large) {
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
    water_wheel_flow_score(a, &flows)
}

const STRUCTURE: &str = "create:water_wheel_structure";

/// `WaterWheelStructuralBlock.getMaster`: the large wheel a structure block
/// at `pos` belongs to (following its facing through other structure).
pub(super) fn structure_master(game: &Game<'_>, pos: Pos) -> Option<Pos> {
    let mut at = pos;
    for _ in 0..3 {
        let state = game.pack.assets.registry.state(game.world.block(at[0], at[1], at[2]))?;
        if state.name != STRUCTURE {
            return (at != pos).then_some(at);
        }
        let f = Dir::parse(state.properties.get("facing")?)?;
        at = [0, 1, 2].map(|k| at[k] + f.normal()[k]);
    }
    None
}

/// Large wheels' structures: kept whole (`LargeWaterWheelBlock.tick`), a
/// wheel whose structure was broken or is blocked is broken (with its drop),
/// and a wheel gone takes its structure with it.
static LARGE_WHEELS: std::sync::Mutex<Vec<((String, Pos), Axis, bool)>> = std::sync::Mutex::new(Vec::new());

pub(super) fn large_wheels_tick(game: &mut Game<'_>, dimension: &str, wheels: &[(Pos, Axis)]) {
    let known: Vec<((String, Pos), Axis, bool)> = {
        let mut g = LARGE_WHEELS.lock().unwrap_or_else(|e| e.into_inner());
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *g).into_iter().partition(|((d, _), ..)| d == dimension);
        *g = rest;
        mine
    };
    let air = game.res.facts().air;
    // Wheels gone: their structure goes too.
    for ((_, p), axis, _) in &known {
        if wheels.iter().any(|(w, _)| w == p) || !game.world.loaded(p[0], p[2]) {
            continue;
        }
        for (at, facing) in super::create_kinetics::large_wheel_structure(*p, *axis) {
            let id = game.world.block(at[0], at[1], at[2]);
            let here = game.pack.assets.registry.state(id);
            if here.is_some_and(|s| s.name == STRUCTURE && s.properties.get("facing").map(String::as_str) == Some(facing.name())) {
                game.native_set_block(dimension, at, air, super::native_hooks::flags::UPDATE_ALL);
            }
        }
    }
    let mut now = Vec::new();
    for &(p, axis) in wheels {
        let whole = known.iter().any(|((_, k), _, w)| *k == p && *w);
        let mut broken = false;
        for (at, facing) in super::create_kinetics::large_wheel_structure(p, axis) {
            let id = game.world.block(at[0], at[1], at[2]);
            let here = game.pack.assets.registry.state(id);
            if here.is_some_and(|s| s.name == STRUCTURE && s.properties.get("facing").map(String::as_str) == Some(facing.name())) {
                continue;
            }
            let replaceable = game.res.facts().get(id).replaceable || game.world.is_air(id);
            // onRemove of a structure block, or no room for it.
            if whole || !replaceable {
                broken = true;
                break;
            }
            if let Some(base) = game.pack.assets.registry.default_state(STRUCTURE)
                && let Some(s) = game.with_props(base, &[("facing", facing.name())])
            {
                game.native_set_block(dimension, at, s, super::native_hooks::flags::UPDATE_ALL);
            }
        }
        if broken {
            let _ = game.destroy_block_dropping(p);
            now.push(((dimension.to_owned(), p), axis, false));
            continue;
        }
        now.push(((dimension.to_owned(), p), axis, true));
    }
    LARGE_WHEELS.lock().unwrap_or_else(|e| e.into_inner()).extend(now);
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
        Some(n) if is_saw(n) => saw_filter_click(game, pos, face),
        Some(BLAZE) => blaze_click(game, pos),
        Some(super::create_fluid_glue::SMART) => super::create_fluid_glue::smart_click(game, pos),
        Some(super::create_whistle_glue::WHISTLE | super::create_whistle_glue::EXTENSION) => {
            super::create_whistle_glue::click(game, pos)
        }
        // EjectorBlock.useItemOn: a depot's, the wrench passing.
        Some(super::create_ejector_glue::EJECTOR) => {
            if game.held().is_none_or(|s| &*s.item != super::create_glue::WRENCH) && face == [0, 1, 0] {
                use_depot(game, pos);
                true
            } else {
                false
            }
        }
        Some(super::create_engine_glue::ENGINE) => {
            let (yaw, pitch) = (game.yaw, game.pitch);
            super::create_engine_glue::click(game, pos, yaw, pitch)
        }
        // SpoutBlock has no use of its own (the template's menu stays shut).
        Some(super::create_spout_glue::SPOUT) => true,
        Some(super::create_spout_glue::DRAIN) => super::create_spout_glue::drain_click(game, pos),
        Some(LINK) => link_click(game, pos, face),
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
    let at = pos.map(|v| f64::from(v) + 0.5);
    match returned {
        Some(back) => {
            // SharedDepotBlockMethods.onUse: the pick-up sound for what came
            // back, the slide for what went on.
            if !back.is_empty() {
                let pitch = 1.0 + horizon_core::random::RandomSource::next_float(&mut game.world.random);
                game.with_level(|_, level, _| {
                    level.native_play_sound("minecraft:entity.item.pickup", at, super::create_sounds::PLAYERS, 0.2, pitch);
                });
            }
            for s in back {
                game.give_or_drop(s);
            }
            if hand.is_some() {
                super::create_glue::play_create_sound(game, "depot_slide", at, 1.0, 1.0);
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
                    .is_none_or(|s| {
                        s.name != DEPOT
                            && s.name != BASIN
                            && s.name != LINK
                            && s.name != super::create_spout_glue::DRAIN
                            && s.name != super::create_crafter_glue::CRAFTER
                            && s.name != super::create_fluid_glue::SMART
                            && s.name != super::create_engine_glue::ENGINE
                            && s.name != super::create_ejector_glue::EJECTOR
                            && s.name != super::create_whistle_glue::WHISTLE
                            && !super::create_logistics::is_chute_block(&s.name)
                            && !super::create_funnel::is_funnel(&s.name)
                    })
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
            .is_some_and(|s| super::create_logistics::is_chute_block(&s.name));
        if chute {
            super::create_logistics::draw_chute(game, camera, *pos, slots, out);
            continue;
        }
        // Crafters draw their items from their block entity.
        if super::create_crafter_glue::crafter_at(game, *pos).is_some() {
            continue;
        }
        // These draw themselves, once this lock is let go.
        if super::create_engine_glue::engine_at(game, *pos).is_some() || super::create_ejector_glue::is_ejector(game, *pos) || super::create_whistle_glue::attached_at(game, *pos).is_some() {
            continue;
        }
        if game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2])).is_some_and(|s| s.name == super::create_fluid_glue::SMART) {
            super::create_fluid_glue::draw_smart_filter(game, camera, *pos, slots, out);
            continue;
        }
        if super::create_spout_glue::is_drain(game, *pos) {
            super::create_spout_glue::draw_drain_item(game, camera, *pos, slots, out);
            continue;
        }
        let link = game
            .pack
            .assets
            .registry
            .state(game.world.block(pos[0], pos[1], pos[2]))
            .filter(|s| s.name == LINK)
            .and_then(|s| s.properties.get("facing").and_then(|f| super::create_kinetics::Dir::parse(f)));
        if let Some(facing) = link {
            // RedstoneLinkFrequencySlot: the two items on the plate, the first
            // above (or behind) the second.
            for (i, (id, count)) in slots.iter().enumerate().take(2) {
                if *count == 0 || id.is_empty() {
                    continue;
                }
                let first = if i == 0 { 5.0 / 16.0 } else { 0.0 };
                let (at, y_rot, x_rot) = match facing {
                    super::create_kinetics::Dir::Up => ([0.5, 3.01 / 16.0, 5.5 / 16.0 + first], 0.0, 90.0),
                    super::create_kinetics::Dir::Down => ([0.5, 1.0 - 3.01 / 16.0, 1.0 - (5.5 / 16.0 + first)], 0.0, 270.0),
                    f => {
                        let a = f.horizontal_angle().to_radians();
                        let (dx, dz) = (0.0f32, 3.01 / 16.0 - 0.5);
                        let (rx, rz) = (dx * a.cos() + dz * a.sin(), -dx * a.sin() + dz * a.cos());
                        ([0.5 + rx, 5.5 / 16.0 + first, 0.5 + rz], f.horizontal_angle() + 180.0, 0.0)
                    }
                };
                super::create_logistics::draw_filter_item(game, camera, *pos, id, at, y_rot, x_rot, LINK, out);
            }
            continue;
        }
        let saw = game
            .pack
            .assets
            .registry
            .state(game.world.block(pos[0], pos[1], pos[2]))
            .filter(|s| is_saw(&s.name) && s.properties.get("facing").map(String::as_str) == Some("up"))
            .map(|s| s.name.clone());
        if let Some(block) = saw {
            // SawFilterSlot: on the top, 3/16 along the saw's way.
            if let Some((id, _)) = slots.first().filter(|(id, c)| *c > 0 && !id.is_empty())
                && glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2])).distance_squared(camera)
                    <= 32.0 * 32.0
            {
                let along = block == super::create_actors::SAW_ALONG;
                let at = if along { [0.5 + 3.0 / 16.0, 12.5 / 16.0, 0.5] } else { [0.5, 12.5 / 16.0, 0.5 + 3.0 / 16.0] };
                let y_rot = if along { 90.0 } else { 0.0 } + 180.0;
                super::create_logistics::draw_filter_item(game, camera, *pos, id, at, y_rot, 90.0, &block, out);
            }
            continue;
        }
        let funnel = game
            .pack
            .assets
            .registry
            .state(game.world.block(pos[0], pos[1], pos[2]))
            .is_some_and(|s| super::create_funnel::is_funnel(&s.name));
        if funnel {
            super::create_logistics::draw_funnel_filter(game, camera, *pos, slots, out);
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
    // Its tanks count too (`BasinBlockEntity`'s fluid `contentsChanged`).
    let fluids_changed = super::create_fluid_glue::basin_fluids_changed(entity);
    let tanks = super::create_fluid_glue::basin_tanks(entity);
    super::create_fluid_glue::note_basin(game.dim.id(), pos, &tanks);
    let empty = entity.items.iter().all(Option::is_none) && tanks.is_empty();
    let mut backup = get_int(&entity.nbt, "Backup") - 1;
    let changed = fluids_changed | {
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

/// The supported `create:mixing` recipes (items, water and lava; no heat), most
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
    tanks: &mut BasinTanks,
    recipe: &(BasinRecipe, Vec<Option<Ingredient>>),
    test: bool,
) -> bool {
    let (r, ingredients) = recipe;
    // The fluid pass, simulated (`FluidIngredients`).
    if !basin_take_fluids(&mut tanks.stacks(), &r.fluid_ingredients, true) {
        return false;
    }
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
    // acceptFluidOutputsIntoBasin, simulated: each result fills whole.
    if r.fluid_results.iter().any(|f| tanks.force_fill_output(f, true) != f.amount) {
        return false;
    }
    if test {
        return true;
    }
    let mut stacks = tanks.stacks();
    basin_take_fluids(&mut stacks, &r.fluid_ingredients, false);
    tanks.set_stacks(&stacks);
    for f in &r.fluid_results {
        tanks.force_fill_output(f, false);
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
fn basin_below(game: &mut Game<'_>, pos: Pos) -> Option<(Pos, Vec<Option<ItemStack>>, BasinTanks)> {
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
    let (mut items, tanks) = game.native_with_mod_block_entity(at, |be| {
        (be.items.clone(), super::create_fluid_glue::basin_tanks(be))
    })?;
    items.resize(BASIN_ALL_SLOTS, None);
    Some((at, items, tanks))
}

/// What a mixer's tick asks of the basin below, once the kinetics' lock is
/// released: `updateBasin` and `applyBasinRecipe`.
pub(super) fn mixer_world_step(game: &mut Game<'_>, pos: Pos, step: MixStep) {
    let key = (game.dim.id().to_owned(), pos);
    match step {
        MixStep::CheckBasin => {
            let Some((_, mut items, mut tanks)) = basin_below(game, pos) else {
                return;
            };
            if items.iter().all(Option::is_none) && tanks.is_empty() {
                return;
            }
            let recipes = basin_recipes(game);
            // HeatCondition.testBlazeBurner: the blaze burner under the basin.
            let heat = burner_heat(game, [pos[0], pos[1] - 3, pos[2]]);
            let Some((r, _)) = recipes
                .iter()
                .find(|r| super::create_blaze::heat_met(r.0.heat.requirement(), heat) && basin_apply(game, &mut items, &mut tanks, r, true))
            else {
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
            // MechanicalMixerBlockEntity.tickAudio: the mix at its twentieth tick.
            let at = pos.map(|v| f64::from(v) + 0.5);
            super::create_glue::play_create_sound(game, "mixing", at, 0.75, 1.0);
            let Some(id) = machines().as_ref().and_then(|m| m.mixing.get(&key).cloned()) else {
                return;
            };
            let recipes = basin_recipes(game);
            let Some(recipe) = recipes.iter().find(|(r, _)| r.id == id) else {
                return;
            };
            if !super::create_blaze::heat_met(recipe.0.heat.requirement(), burner_heat(game, [pos[0], pos[1] - 3, pos[2]])) {
                return;
            }
            let Some((at, mut items, mut tanks)) = basin_below(game, pos) else {
                return;
            };
            if !basin_apply(game, &mut items, &mut tanks, recipe, false) {
                return;
            }
            // Continue mixing while the basin still holds another batch.
            let again = basin_apply(game, &mut items.clone(), &mut tanks.clone(), recipe, true);
            game.native_with_mod_block_entity(at, |be| {
                be.items = items;
                super::create_fluid_glue::set_basin_tanks(be, &tanks);
            });
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
/// `AirCurrent.getFlowLimit` from `pos` downwards (a chute's pull).
pub(super) fn flow_limit_down(game: &Game<'_>, pos: Pos, max: f32) -> f32 {
    super::create_fan::flow_limit(&FanWorld { game }, pos, max, Dir::Down)
}

struct FanWorld<'a, 'g> {
    game: &'a Game<'g>,
}

pub(super) fn core_axis(a: Axis) -> horizon_core::geometry::Axis {
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
            let mut out: Vec<ItemStack> = stacks(list);
            for _ in 0..stack.count {
                out = with_remainder(game, &stack.item, out);
            }
            Some(out)
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
            Some(super::create_belt::Conversion {
                outputs,
                held: None,
                locked: false,
            })
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
        if speed != 0.0 {
            let current = fan.current.clone();
            fan_nozzle(game, dimension, pos, facing, speed, &current);
        }
        if speed == 0.0 || fan.current.max_distance <= 0.0 {
            continue;
        }
        let current = fan.current.clone();
        fan_items(game, dimension, pos, facing, speed, &current, &mut processing);
        fan_entities(game, dimension, pos, facing, speed, &current);
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

/// Horses' `CreateHaunting` progress, by dimension and mob.
static HAUNTING: std::sync::Mutex<Vec<((String, u32), i32)>> = std::sync::Mutex::new(Vec::new());

/// `AirCurrent.tickAffectedEntities` for the mobs and players in the
/// current: mobs pushed along (from rest: Horizon gives no mob's motion),
/// then each processing type's `affectEntity`, through vanilla commands
/// (`damage`, `effect`, `data`) with vanilla damage types standing in for
/// Create's. Players are pushed on their own side (`fan_client_tick`).
fn fan_entities(game: &mut Game<'_>, dimension: &str, pos: Pos, facing: Dir, speed: f32, current: &AirCurrent) {
    let flow = if current.pushing { facing } else { facing.opposite() };
    let centre = pos.map(|v| f64::from(v) + 0.5);
    let in_current = |p: [f64; 3]| current.touches([p[0] - 0.3, p[1], p[2] - 0.3, p[0] + 0.3, p[1] + 1.8, p[2] + 0.3]);
    let mut commands = Vec::new();
    let mut sounds: Vec<(&str, [f64; 3], f32, f32)> = Vec::new();
    for m in game.native_mobs(dimension) {
        if m.kind == "minecraft:item" || !in_current(m.pos) {
            continue;
        }
        let d = ((0..3).map(|k| (m.pos[k] - centre[k]).powi(2)).sum::<f64>()).sqrt().max(0.01);
        let v = super::create_fan::push([0.0; 3], flow, speed, d, current.max_distance, false);
        game.native_push_mob(dimension, m.id, v);
        let Some(kind) = current.type_at(aligned_distance_to_face(m.pos, pos, facing) as f32) else {
            continue;
        };
        let e = super::create_fan::entity_effect(kind);
        let [x, y, z] = m.pos;
        let sel = format!("@e[type={},limit=1,sort=nearest,distance=..0.75]", m.kind);
        let run = |c: String| format!("execute in {dimension} positioned {x} {y} {z} run {c}");
        if e.fire_seconds > 0 {
            commands.push(run(format!("data merge entity {sel} {{Fire:{}s}}", e.fire_seconds * 20)));
        }
        if let Some((amount, ty)) = e.damage
            && (e.hurt_kinds.is_empty() || e.hurt_kinds.contains(&m.kind.as_str()))
        {
            commands.push(run(format!("damage {sel} {amount} {ty}")));
        }
        if e.extinguish {
            commands.push(run(format!("data merge entity {sel} {{Fire:0s}}")));
        }
        if e.blind_and_slow {
            commands.push(run(format!("effect give {sel} minecraft:blindness 2 0 true")));
            commands.push(run(format!("effect give {sel} minecraft:slowness 1 1 true")));
        }
        if e.haunts_horses && m.kind == "minecraft:horse" {
            let key = (dimension.to_owned(), m.id);
            let mut h = HAUNTING.lock().unwrap_or_else(|e| e.into_inner());
            let progress = h.iter().find(|(k, _)| *k == key).map_or(0, |(_, p)| *p);
            let (next, sound, turns) = super::create_fan::haunt_horse(progress);
            h.retain(|(k, _)| *k != key);
            if turns {
                drop(h);
                sounds.push(("minecraft:block.fire.extinguish", m.pos, 1.25, 0.65));
                let spawn = Default::default();
                if game.native_spawn_mob(dimension, "minecraft:skeleton_horse", m.pos, &spawn).is_some() {
                    game.native_remove_mob(dimension, m.id);
                }
                continue;
            }
            h.push((key, next));
            if sound {
                sounds.push(("minecraft:particle.soul_escape", m.pos, 1.0, 1.5 * progress as f32 / 100.0));
            }
        }
    }
    for p in game.native_server_players() {
        if p.dimension != dimension || p.game_mode == 3 || !in_current(p.position) {
            continue;
        }
        let Some(kind) = current.type_at(aligned_distance_to_face(p.position, pos, facing) as f32) else {
            continue;
        };
        let e = super::create_fan::entity_effect(kind);
        if let Some((amount, ty)) = e.damage
            && e.hurt_kinds.is_empty()
        {
            commands.push(format!("damage {} {amount} {ty}", p.name));
        }
        if e.blind_and_slow {
            commands.push(format!("effect give {} minecraft:blindness 2 0 true", p.name));
            commands.push(format!("effect give {} minecraft:slowness 1 1 true", p.name));
        }
    }
    for c in commands {
        game.native_run_command(&c);
    }
    if !sounds.is_empty() {
        game.with_level(|_, level, _| {
            for (sound, at, volume, pitch) in sounds {
                level.native_play_sound(sound, at, 6, volume, pitch);
            }
        });
    }
}

/// The window's fans' currents, as its client builds them: by position,
/// the current, the speed it was built for and the ticks until it is
/// rebuilt (`EncasedFanBlockEntity.tick` runs on clients too).
type ClientFans = HashMap<Pos, (AirCurrent, f32, i32)>;
static CLIENT_FANS: std::sync::Mutex<Option<(u8, ClientFans)>> = std::sync::Mutex::new(None);

/// `AirCurrent.tickAffectedEntities` and `NozzleBlockEntity.tick` for the
/// window's own player, where the player moves (`native_tick`): the fans
/// the kinetics told this client of push it along their current (an eighth
/// as hard when sneaking, not when flying in creative), and their nozzles
/// push it away or draw it in.
pub(super) fn fan_client_tick(game: &mut Game<'_>) {
    if !game.native_window() {
        return;
    }
    let (dim, fans) = super::create_glue::client_fans(game);
    let mut guard = CLIENT_FANS.lock().unwrap_or_else(|e| e.into_inner());
    let c = guard.get_or_insert_with(|| (dim, HashMap::new()));
    if c.0 != dim {
        *c = (dim, HashMap::new());
    }
    c.1.retain(|p, _| fans.iter().any(|f| f.0 == *p));
    let Some(p) = game.player.as_ref() else {
        return;
    };
    let creative_flying = p.flying && game.mode_id() == 1;
    let at = [p.position.x, p.position.y, p.position.z];
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    let mut motion = [p.velocity.x, p.velocity.y, p.velocity.z];
    let mut pushed = false;
    for (pos, facing, speed) in fans {
        let (current, built, cooldown) = c.1.entry(pos).or_insert_with(|| (AirCurrent::default(), f32::NAN, 0));
        *cooldown -= 1;
        if *cooldown < 0 || *built != speed || current.direction != Some(facing) {
            *cooldown = FAN_BLOCK_CHECK_RATE;
            *built = speed;
            current.rebuild(&FanWorld { game }, pos, facing, speed);
        }
        if speed == 0.0 {
            continue;
        }
        // NozzleBlockEntity: every entity in range, players too.
        let n = facing.normal();
        let nozzle = [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]];
        let is_nozzle = game
            .pack
            .assets
            .registry
            .state(game.world.block(nozzle[0], nozzle[1], nozzle[2]))
            .is_some_and(|s| s.name == "create:nozzle" && s.properties.get("facing").map(String::as_str) == Some(facing.name()));
        if is_nozzle {
            let centre = nozzle.map(|v| f64::from(v) + 0.5);
            let diff = [0, 1, 2].map(|k| at[k] - centre[k]);
            let range = super::create_fan::max_distance(speed);
            if let Some(v) = super::create_fan::nozzle_push(diff, range, current.pushing, false) {
                motion = [0, 1, 2].map(|k| motion[k] + v[k]);
                pushed = true;
            }
        }
        if current.max_distance <= 0.0 || creative_flying {
            continue;
        }
        let in_current = current.touches([at[0] - 0.3, at[1], at[2] - 0.3, at[0] + 0.3, at[1] + 1.8, at[2] + 0.3]);
        if !in_current {
            continue;
        }
        let flow = if current.pushing { facing } else { facing.opposite() };
        let centre = pos.map(|v| f64::from(v) + 0.5);
        let d = ((0..3).map(|k| (at[k] - centre[k]).powi(2)).sum::<f64>()).sqrt().max(0.01);
        motion = super::create_fan::push(motion, flow, speed, d, current.max_distance, sneaking);
        pushed = true;
    }
    drop(guard);
    if !pushed {
        return;
    }
    if let Some(p) = game.player.as_mut() {
        p.velocity = horizon_core::geometry::Vec3::new(motion[0], motion[1], motion[2]);
        p.fall_distance = 0.0;
    }
}

/// `NozzleBlockEntity`: a nozzle on the fan's face pushes mobs and items
/// around it away (or draws them in) as far as the current reaches (players
/// on their own side, `fan_client_tick`); sight isn't checked.
fn fan_nozzle(game: &mut Game<'_>, dimension: &str, pos: Pos, facing: Dir, speed: f32, current: &AirCurrent) {
    let n = facing.normal();
    let at = [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]];
    let id = game.world.block(at[0], at[1], at[2]);
    let Some(s) = game.pack.assets.registry.state(id) else {
        return;
    };
    if s.name != "create:nozzle" || s.properties.get("facing").map(String::as_str) != Some(facing.name()) {
        return;
    }
    // IAirCurrentSource.getMaxDistance: the reach for the speed, not what
    // the nozzle leaves of the current.
    let range = super::create_fan::max_distance(speed);
    let pushing = current.pushing;
    let centre = at.map(|v| f64::from(v) + 0.5);
    for m in game.native_mobs(dimension) {
        let diff = [0, 1, 2].map(|k| m.pos[k] - centre[k]);
        if let Some(v) = super::create_fan::nozzle_push(diff, range, pushing, false) {
            game.native_push_mob(dimension, m.id, v);
        }
    }
    for d in game.dropped.iter_mut() {
        let diff = [d.position.x - centre[0], d.position.y - centre[1], d.position.z - centre[2]];
        if let Some(v) = super::create_fan::nozzle_push(diff, range, pushing, true) {
            d.velocity = horizon_core::geometry::Vec3::new(d.velocity.x + v[0], d.velocity.y + v[1], d.velocity.z + v[2]);
        }
    }
}

/// `AirCurrent.tickAffectedEntities` for the item entities in the current.
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
        let v = push([v.x, v.y, v.z], flow, speed, distance_old, current.max_distance, false);
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
