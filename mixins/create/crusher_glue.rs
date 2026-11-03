// Horizon's side of Create's crushing wheels, injected as
// `crate::game::create_crusher_glue`: the controller block between two wheels
// (`CrushingWheelBlock.updateControllers`, `onRemove`) and its block entity
// (`CrushingWheelControllerBlockEntity`): items dropped into it, fed by belts,
// funnels and arms, crushed with `create:crushing` then `create:milling`
// recipes and put out below or thrown the way it faces. The rules are in
// crusher.rs.

use super::Game;
use super::create_crusher::{SLOTS, Step, Wanted};
use super::create_kinetics::{Axis, Dir, Pos};
use horizon_client::containers::{BlockEntity, ModBlockEntity};
use horizon_client::items::ItemStack;
use horizon_world::nbt::{JavaString, Tag};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

pub(super) const WHEEL: &str = "create:crushing_wheel";
pub(super) const CONTROLLER: &str = "create:crushing_wheel_controller";

#[derive(Default)]
struct State {
    /// Each wheel's speed when its controllers were last looked at
    /// (`onSpeedChanged`), by dimension and position.
    speeds: HashMap<(String, Pos), f32>,
    /// Controllers seen ticking, checked for their wheels.
    controllers: HashSet<(String, Pos)>,
    /// `BypassCrushingWheel`: items a controller threw out, which it doesn't
    /// take back (entity id and the controller).
    bypass: HashMap<u32, Pos>,
    ticks: u64,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(State::default))
}

/// A world opened or closed.
pub(super) fn reset() {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn offset(p: Pos, d: Dir, n: i32) -> Pos {
    let v = d.normal();
    [p[0] + v[0] * n, p[1] + v[1] * n, p[2] + v[2] * n]
}

fn wheel_axis(game: &Game<'_>, pos: Pos) -> Option<Axis> {
    let s = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2]))?;
    (s.name == WHEEL).then(|| s.properties.get("axis").and_then(|a| Axis::parse(a))).flatten()
}

/// The controller at `pos`: its facing and whether it works.
pub(super) fn controller_at(game: &Game<'_>, pos: Pos) -> Option<(Dir, bool)> {
    let s = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2]))?;
    if s.name != CONTROLLER {
        return None;
    }
    let facing = s.properties.get("facing").and_then(|f| Dir::parse(f))?;
    Some((facing, s.properties.get("valid").map(String::as_str) == Some("true")))
}

fn speed_at(game: &Game<'_>, pos: Pos) -> f32 {
    super::create_glue::with_kinetics(game, |k, level| k.get(pos).map_or(0.0, |_| k.speed_of(level, pos)))
}

// ---- The wheels and their controllers ------------------------------------------------

/// `CrushingWheelBlockEntity.fixControllers`, when a wheel's speed changed
/// (`onSpeedChanged`) and every 20 ticks (`lazyTick`); controllers whose
/// wheels are gone go, dropping what they hold (`onRemove`).
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str, wheels: &[(Pos, Axis, f32)]) {
    let lazy = with_state(|s| {
        s.ticks += 1;
        s.ticks.is_multiple_of(20)
    });
    let alive: HashSet<u32> = game.dropped.iter().map(|d| d.id).collect();
    with_state(|s| s.bypass.retain(|id, _| alive.contains(id)));
    let speeds: HashMap<Pos, (Axis, f32)> = wheels.iter().map(|(p, a, v)| (*p, (*a, *v))).collect();
    for &(pos, axis, speed) in wheels {
        let changed = with_state(|s| s.speeds.insert((dimension.to_owned(), pos), speed) != Some(speed));
        if !changed && !lazy {
            continue;
        }
        for side in Dir::ALL {
            if side.axis() != axis {
                update_controller(game, dimension, pos, axis, side, speed, speeds.get(&offset(pos, side, 2)).copied());
            }
        }
    }
    let controllers: Vec<Pos> = with_state(|s| {
        s.controllers.iter().filter(|(d, _)| d == dimension).map(|(_, p)| *p).collect()
    });
    for pos in controllers {
        if !game.world.loaded(pos[0], pos[2]) {
            continue;
        }
        if controller_at(game, pos).is_none() {
            with_state(|s| s.controllers.remove(&(dimension.to_owned(), pos)));
            continue;
        }
        if !has_wheels(game, pos) {
            remove_controller(game, dimension, pos);
        }
    }
}

/// Two wheels opposite each other across the controller, on one axis off
/// the line between them.
fn has_wheels(game: &Game<'_>, pos: Pos) -> bool {
    Dir::ALL.into_iter().filter(|d| d.positive()).any(|d| {
        let (a, b) = (wheel_axis(game, offset(pos, d, 1)), wheel_axis(game, offset(pos, d, -1)));
        a.is_some() && a == b && a != Some(d.axis())
    })
}

fn remove_controller(game: &mut Game<'_>, dimension: &str, pos: Pos) {
    let items = game
        .native_with_mod_block_entity(pos, |be| be.items.iter_mut().filter_map(Option::take).collect::<Vec<_>>())
        .unwrap_or_default();
    let at = pos.map(|v| f64::from(v) + 0.5);
    for s in items {
        game.spawn_item(s, at, [0.0; 3], 10);
    }
    if let Some(air) = game.pack.assets.registry.default_state("minecraft:air") {
        game.native_set_block(dimension, pos, air, super::native_hooks::flags::UPDATE_ALL);
    }
    with_state(|s| s.controllers.remove(&(dimension.to_owned(), pos)));
}

/// `CrushingWheelBlock.updateControllers` for one side.
fn update_controller(
    game: &mut Game<'_>,
    dimension: &str,
    pos: Pos,
    axis: Axis,
    side: Dir,
    speed: f32,
    other: Option<(Axis, f32)>,
) {
    let at = offset(pos, side, 1);
    let current = controller_at(game, at);
    let Some(Wanted { valid, facing }) = super::create_crusher::wanted_controller(axis, side, speed, other) else {
        if current.is_some() {
            remove_controller(game, dimension, at);
        }
        return;
    };
    if current == Some((facing, valid)) {
        return;
    }
    if current.is_none() {
        let state = game.world.block(at[0], at[1], at[2]);
        if !game.world.is_air(state) && !game.res.facts().get(state).replaceable {
            return;
        }
    }
    let Some(base) = game.pack.assets.registry.default_state(CONTROLLER) else {
        return;
    };
    let valid_text = if valid { "true" } else { "false" };
    if let Some(id) = game.with_props(base, &[("facing", facing.name()), ("valid", valid_text)]) {
        game.native_set_block(dimension, at, id, super::native_hooks::flags::UPDATE_ALL);
    }
}

/// `CrushingWheelBlock.canSurvive`: none of its neighbours forbids it.
pub(super) fn wheel_survives(level: &super::GameLevel<'_>, pos: Pos, axis: Axis) -> bool {
    Dir::ALL.into_iter().all(|d| {
        let n = offset(pos, d, 1);
        let Some(s) = level.registry.state(level.world.block(n[0], n[1], n[2])) else {
            return true;
        };
        let wheel = (s.name == WHEEL).then(|| s.properties.get("axis").and_then(|a| Axis::parse(a))).flatten();
        super::create_crusher::survives_beside(axis, d, s.name == CONTROLLER, wheel)
    })
}

// ---- Its block entity ------------------------------------------------------------------

fn get_float(be: &ModBlockEntity, key: &str) -> f32 {
    match be.nbt.get(&JavaString::from(key)) {
        Some(Tag::Float(v)) => f32::from_bits(*v),
        _ => 0.0,
    }
}

fn get_bool(be: &ModBlockEntity, key: &str) -> bool {
    matches!(be.nbt.get(&JavaString::from(key)), Some(Tag::Byte(1)))
}

fn set(be: &mut ModBlockEntity, remaining: f32, applied: bool, started: bool) {
    be.nbt.insert(JavaString::from("ProcessingTime"), Tag::Float(remaining.to_bits()));
    be.nbt.insert(JavaString::from("AppliedRecipe"), Tag::Byte(i8::from(applied)));
    be.nbt.insert(JavaString::from("Started"), Tag::Byte(i8::from(started)));
}

/// `findRecipe`: crushing, else milling.
fn find(game: &Game<'_>, stack: &ItemStack) -> Option<super::create_processing::Recipe> {
    super::create_machines::find_recipe(game, "create:crushing", stack)
        .or_else(|| super::create_machines::find_recipe(game, "create:milling", stack))
}

/// `updateSpeed`: the speed of a wheel beside a working controller.
fn crushing_speed(game: &Game<'_>, pos: Pos) -> f32 {
    for d in Dir::ALL {
        let n = offset(pos, d, 1);
        if wheel_axis(game, n).is_some_and(|a| a != d.axis()) {
            return super::create_crusher::crushing_speed(speed_at(game, n));
        }
    }
    0.0
}

/// `native_block_entity_tick` for a controller: `checkEntityForProcessing`
/// for items and its `tick`. An item in its cell goes in at once: Horizon has
/// no collision shape for one entity, so it isn't drawn through and pulled
/// slowly as Create's is.
pub(super) fn tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    if entity.items.len() < SLOTS {
        entity.items.resize(SLOTS, None);
    }
    let dimension = game.dim.id().to_owned();
    with_state(|s| s.controllers.insert((dimension.clone(), pos)));
    let Some((facing, valid)) = controller_at(game, pos) else {
        return;
    };
    let crushing = if valid { crushing_speed(game, pos) } else { 0.0 };
    if crushing == 0.0 {
        return;
    }
    if entity.items.iter().all(Option::is_none) {
        intake(game, pos, entity);
    }
    if entity.items.iter().all(Option::is_none) {
        return;
    }
    // itemInserted, however it came in.
    if !get_bool(entity, "Started") {
        let time = entity.items[0].as_ref().and_then(|s| find(game, s)).map(|r| r.processing_time as f32);
        set(entity, time.unwrap_or(super::create_crusher::NO_RECIPE_TIME), false, true);
    }
    let applied = get_bool(entity, "AppliedRecipe");
    let mut remaining = get_float(entity, "ProcessingTime");
    let count = entity.items[0].as_ref().map_or(1, |s| s.count);
    if let Some(s) = entity.items[0].as_ref() {
        spawn_particles(game, pos, s);
    }
    match super::create_crusher::tick(&mut remaining, applied, crushing, count) {
        Step::Wait => set(entity, remaining, applied, true),
        Step::Apply => {
            apply_recipe(game, entity);
            // `inventory.clear()` has reset the timer.
            set(entity, 0.0, true, true);
        }
        Step::Output => {
            set(entity, 0.0, applied, true);
            output(game, pos, entity, facing);
        }
    }
}

/// An item entity whose place is in the controller's cell.
fn intake(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let [x, y, z] = pos.map(f64::from);
    let bypass = with_state(|s| s.bypass.clone());
    let Some(i) = game.dropped.iter().position(|d| {
        let p = d.position;
        d.stack.count > 0
            && bypass.get(&d.id) != Some(&pos)
            && (x..x + 1.0).contains(&p.x)
            && (y..y + 1.0).contains(&p.y)
            && (z..z + 1.0).contains(&p.z)
    }) else {
        return;
    };
    entity.items[0] = Some(game.dropped.remove(i).stack);
    entity.nbt.remove(&JavaString::from("Started"));
}

/// `spawnParticles`: four of a block item's particles (item particles don't
/// go over the network).
fn spawn_particles(game: &mut Game<'_>, pos: Pos, stack: &ItemStack) {
    let Some(state) = game.pack.assets.registry.default_state(&stack.item) else {
        return;
    };
    let options = horizon_client::mod_api::ParticleOptions::simple(horizon_client::mod_api::ParticleType::Block);
    let at = pos.map(|v| f64::from(v) + 0.5);
    game.with_level(|_, level, _| {
        level.native_send_particles(&options, Some(state), at, [0.25; 3], 0.0, 4);
    });
}

/// `applyRecipe`: each item of the stack rolled, the results into the slots
/// after the first; with no recipe the items are gone.
fn apply_recipe(game: &Game<'_>, entity: &mut ModBlockEntity) {
    let input = entity.items[0].clone();
    for slot in entity.items.iter_mut() {
        *slot = None;
    }
    let Some(input) = input else {
        return;
    };
    let Some(recipe) = find(game, &input) else {
        return;
    };
    let mut list: Vec<(String, u32)> = Vec::new();
    let max = |id: &str| game.items().get(id).map_or(64, |i| i.max_stack);
    for _ in 0..input.count {
        for out in super::create_machines::roll_recipe(&recipe) {
            super::create_crusher::add_to_list(&mut list, &out.item, out.count, max(&out.item));
        }
    }
    if let Some(rem) = game.items().get(&input.item).and_then(|i| i.crafting_remainder.clone()) {
        super::create_crusher::add_to_list(&mut list, &rem, 1, max(&rem));
    }
    for (slot, (id, n)) in list.into_iter().enumerate().take(SLOTS - 1) {
        entity.items[slot + 1] = Some(ItemStack::new(&id, n));
    }
}

/// `tick`'s output: into what takes items from belts below (or along a
/// sideways facing), else thrown out the way it faces.
fn output(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity, facing: Dir) {
    if let Some(next) = super::create_crusher::output_target(pos, facing)
        && super::create_logistics::has_belt_input(game, next)
    {
        if !super::create_logistics::can_insert_side(game, next, facing) {
            return;
        }
        for slot in entity.items.iter_mut() {
            let Some(stack) = slot.clone() else {
                continue;
            };
            *slot = super::create_logistics::insert_side(game, next, facing, stack);
        }
        return;
    }
    let (out, motion) = super::create_crusher::ejection(facing);
    let at = [0, 1, 2].map(|i| f64::from(pos[i]) + 0.5 + out[i]);
    for slot in entity.items.iter_mut() {
        if let Some(stack) = slot.take() {
            game.spawn_item(stack, at, motion, 10);
            if let Some(id) = game.dropped.last().map(|d| d.id) {
                with_state(|s| s.bypass.insert(id, pos));
            }
        }
    }
    entity.nbt.remove(&JavaString::from("Started"));
    entity.nbt.remove(&JavaString::from("AppliedRecipe"));
}

/// `ProcessingInventory.insertItem` (belts, funnels, chutes, arms): into its
/// first slot while it is empty, up to a stack. What is left.
pub(super) fn insert(game: &mut Game<'_>, pos: Pos, stack: ItemStack, simulate: bool) -> Option<ItemStack> {
    let empty = matches!(game.world.block_entities.get(&(pos[0], pos[1], pos[2])),
        Some(BlockEntity::Mod(m)) if m.items.iter().all(Option::is_none));
    if !empty {
        return Some(stack);
    }
    let n = stack.count.min(game.items().get(&stack.item).map_or(64, |i| i.max_stack));
    if !simulate {
        let mut held = stack.clone();
        held.count = n;
        game.native_with_mod_block_entity(pos, |be| {
            if be.items.len() < SLOTS {
                be.items.resize(SLOTS, None);
            }
            be.items[0] = Some(held);
            be.nbt.remove(&JavaString::from("Started"));
            be.nbt.remove(&JavaString::from("AppliedRecipe"));
        });
    }
    let mut left = stack;
    left.count -= n;
    (left.count > 0).then_some(left)
}
