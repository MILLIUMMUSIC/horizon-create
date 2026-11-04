// Horizon's side of Create's redstone logic, injected as
// `crate::game::create_redstone_glue`: the analog lever, the latches and the
// brass diodes as mod block entities with their own state properties, their
// signals through `native_block_signal`, their input read each tick (a diode's
// delay counted there rather than with block ticks). The rules are in redstone.rs.

use super::Game;
use super::create_kinetics::{Dir, Pos};
use super::create_redstone::{Brass, brass_output, brass_tick};
use horizon_client::containers::ModBlockEntity;
use horizon_world::nbt::{JavaString, Tag};
use horizon_world::registry::BlockStateId;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

pub(super) const LEVER: &str = "create:analog_lever";
pub(super) const TOGGLE_LATCH: &str = "create:powered_toggle_latch";
pub(super) const POWERED_LATCH: &str = "create:powered_latch";
pub(super) const REPEATER: &str = "create:pulse_repeater";
pub(super) const EXTENDER: &str = "create:pulse_extender";
pub(super) const TIMER: &str = "create:pulse_timer";

fn brass_of(name: &str) -> Option<Brass> {
    match name {
        REPEATER => Some(Brass::Repeater),
        EXTENDER => Some(Brass::Extender),
        TIMER => Some(Brass::Timer),
        _ => None,
    }
}

fn is_latch(name: &str) -> bool {
    name == TOGGLE_LATCH || name == POWERED_LATCH
}

pub(super) fn is_diode(name: &str) -> bool {
    is_latch(name) || brass_of(name).is_some()
}

/// Block entities of this module.
pub(super) fn is_ours(name: &str) -> bool {
    name == LEVER || name == LAMP || name == BELL || is_diode(name)
}

pub(super) const LAMP: &str = "create:rose_quartz_lamp";
pub(super) const BELL: &str = "create:desk_bell";

/// `scheduleActivation`: lamps with a tick coming.
static LAMP_TICKS: Mutex<Option<HashSet<(String, Pos)>>> = Mutex::new(None);

fn lamp_on(game: &Game<'_>, pos: Pos, key: &str) -> bool {
    prop(game, pos, key).as_deref() == Some("true")
}

fn name_is(game: &Game<'_>, pos: Pos, name: &str) -> bool {
    game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2])).is_some_and(|s| s.name == name)
}

/// `forEachInCluster`: the lamps joined to `pos`, within 16 blocks.
fn lamp_cluster(game: &Game<'_>, pos: Pos) -> Vec<Pos> {
    let mut frontier = vec![pos];
    let mut visited = HashSet::from([pos]);
    let mut out = Vec::new();
    while let Some(p) = (!frontier.is_empty()).then(|| frontier.remove(0)) {
        for d in Dir::ALL {
            let n = d.normal();
            let c = [p[0] + n[0], p[1] + n[1], p[2] + n[2]];
            if (0..3).map(|k| (c[k] - pos[k]).abs()).sum::<i32>() > 16 || !visited.insert(c) || !name_is(game, c, LAMP) {
                continue;
            }
            out.push(c);
            frontier.push(c);
        }
    }
    out
}

/// `RoseQuartzLampBlock.tick` when one is due, then `neighborChanged`: a rising
/// edge takes the power from the others in its cluster to itself.
fn lamp_tick(game: &mut Game<'_>, pos: Pos) {
    let key = (game.dim.id().to_owned(), pos);
    let due = LAMP_TICKS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).remove(&key);
    if due {
        let activate = lamp_on(game, pos, "activate");
        if lamp_on(game, pos, "powering") || activate {
            game.native_set_block_property(pos, "activate", "false");
            game.native_set_block_property(pos, "powering", if activate { "true" } else { "false" });
        }
    }
    let signal = super::create_glue::signal(game, pos);
    let powered = lamp_on(game, pos, "powered");
    if powered == signal {
        return;
    }
    if powered {
        game.native_set_block_property(pos, "powered", "false");
        return;
    }
    let mut guard = LAMP_TICKS.lock().unwrap_or_else(|e| e.into_inner());
    let due = guard.get_or_insert_with(HashSet::new);
    let others = lamp_cluster(game, pos);
    for &c in &others {
        due.insert((key.0.clone(), c));
    }
    due.insert(key);
    drop(guard);
    for c in others {
        game.native_set_block_property(c, "powering", "false");
    }
    game.native_set_block_property(pos, "powered", "true");
    game.native_set_block_property(pos, "powering", "true");
    game.native_set_block_property(pos, "activate", "true");
}

/// `getRotatedBlockState`: the wrench switches a lamp.
pub(super) fn wrench_lamp(game: &mut Game<'_>, pos: Pos) -> Option<bool> {
    if !name_is(game, pos, LAMP) {
        return None;
    }
    let next = if lamp_on(game, pos, "powering") { "false" } else { "true" };
    game.native_set_block_property(pos, "powering", next);
    let pitch = horizon_core::random::RandomSource::next_float(&mut game.world.random) + 0.5;
    super::create_glue::play_create_sound(game, "wrench_rotate", pos.map(|v| f64::from(v) + 0.5), 1.0, pitch);
    Some(true)
}

/// `DeskBellBlock.useWithoutItem`.
fn bell_click(game: &mut Game<'_>, pos: Pos) -> bool {
    if game.held().is_some() {
        return false;
    }
    let at = pos.map(|v| f64::from(v) + 0.5);
    super::create_glue::play_create_sound(game, "desk_bell", at, 1.0, 1.0);
    game.native_set_block_property(pos, "powered", "true");
    game.native_with_mod_block_entity(pos, |be| {
        be.nbt.insert(JavaString::from("Timer"), Tag::Int(20));
        let dings = match be.nbt.get(&JavaString::from("Dings")) {
            Some(Tag::Int(v)) => *v,
            _ => 0,
        };
        be.nbt.insert(JavaString::from("Dings"), Tag::Int((dings + 1) % 250));
    });
    true
}

/// `DeskBellBlockEntity.tick`: let go twenty ticks on; each ding told to clients.
fn bell_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let int = |k: &str| match entity.nbt.get(&JavaString::from(k)) {
        Some(Tag::Int(v)) => *v,
        _ => 0,
    };
    let (timer, dings) = (int("Timer"), int("Dings"));
    if timer > 0 {
        entity.nbt.insert(JavaString::from("Timer"), Tag::Int(timer - 1));
        if timer == 1 {
            game.native_set_block_property(pos, "powered", "false");
        }
    }
    super::create_machines::note_view(game, pos, vec![(String::new(), dings as u32 + 1)]);
}

/// Clients' bells: the dings seen and where `animation` is, now and before.
static BELLS: Mutex<Option<HashMap<Pos, (u32, f32, f32)>>> = Mutex::new(None);

/// The bells' animation a tick on, on the window's client.
pub(super) fn bells_client_tick(game: &Game<'_>) {
    for pos in super::create_machines::client_positions() {
        if !name_is(game, pos, BELL) {
            continue;
        }
        let dings = super::create_machines::client_slots(pos).and_then(|s| s.first().map(|v| v.1)).unwrap_or(0);
        let mut guard = BELLS.lock().unwrap_or_else(|e| e.into_inner());
        let e = guard.get_or_insert_with(HashMap::new).entry(pos).or_insert((dings, 0.0, 0.0));
        e.2 = e.1;
        if e.0 != dings {
            e.0 = dings;
            e.1 = 1.0;
            e.2 = 1.0;
        } else {
            e.1 = (e.1 - 0.05).max(0.0);
        }
    }
}

/// `DeskBellRenderer`.
pub(super) fn draw_bells(game: &Game<'_>, camera: glam::DVec3, time: f32, out: &mut Vec<horizon_client::mesher::Vertex>) {
    for pos in super::create_machines::client_positions() {
        if !name_is(game, pos, BELL) {
            continue;
        }
        let Some(facing) = prop(game, pos, "facing").and_then(|f| Dir::parse(&f)) else {
            continue;
        };
        let (now, last) = BELLS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&pos).map(|e| (e.1, e.2))).unwrap_or((0.0, 0.0));
        let p = last + (now - last) * time.fract();
        let (push, swing) = super::create_redstone::bell_motion(p);
        let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
        let centre = glam::Vec3::splat(0.5);
        let base = glam::Mat4::from_translation((corner - camera).as_vec3())
            * glam::Mat4::from_translation(centre)
            * glam::Mat4::from_rotation_y(facing.horizontal_angle().to_radians())
            * glam::Mat4::from_rotation_x((facing.vertical_angle() + 90.0).to_radians());
        let plunger = base * glam::Mat4::from_translation(-centre) * glam::Mat4::from_translation(glam::Vec3::new(0.0, push, 0.0));
        let bell = base * glam::Mat4::from_rotation_x(swing.to_radians()) * glam::Mat4::from_translation(-centre);
        let light = super::create_logistics::neighbour_light(game, pos);
        let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
        let seed = horizon_core::random::position_seed(horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]));
        for (holder, m) in [("create:hz_part_desk_bell_plunger", plunger), ("create:hz_part_desk_bell_bell", bell)] {
            if let Some(state) = game.pack.assets.registry.default_state(holder) {
                game.block_model_vertices(state, seed, m, packed, false, out);
            }
        }
    }
}

/// The levers' strengths, for `native_block_signal`, which can't read the
/// world's block entities while one ticks.
static LEVERS: Mutex<Option<HashMap<(String, Pos), i32>>> = Mutex::new(None);

fn levers<R>(f: impl FnOnce(&mut HashMap<(String, Pos), i32>) -> R) -> R {
    let mut guard = LEVERS.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

/// A world opened or closed.
pub(super) fn reset() {
    *LEVERS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn int(be: &ModBlockEntity, key: &str) -> Option<i32> {
    match be.nbt.get(&JavaString::from(key)) {
        Some(Tag::Int(n)) => Some(*n),
        _ => None,
    }
}

fn put(be: &mut ModBlockEntity, key: &str, v: i32) {
    be.nbt.insert(JavaString::from(key), Tag::Int(v));
}

fn prop(game: &Game<'_>, pos: Pos, key: &str) -> Option<String> {
    let s = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2]))?;
    s.properties.get(key).cloned()
}

fn flag(game: &Game<'_>, pos: Pos, key: &str) -> bool {
    prop(game, pos, key).as_deref() == Some("true")
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

fn offset(p: Pos, d: Dir) -> Pos {
    let n = d.normal();
    [p[0] + n[0], p[1] + n[1], p[2] + n[2]]
}

/// `DiodeBlock.getInputSignal` / `isPoweredOnSides`: what the block on `d` gives
/// this way (a wire its power).
fn input(game: &mut Game<'_>, pos: Pos, d: Dir) -> i32 {
    let at = offset(pos, d);
    let block = horizon_core::position::BlockPos::new(at[0], at[1], at[2]);
    let dir = core_dir(d);
    let signal = game.with_level(|b, level, ticks| b.create_signal(level, ticks, block, dir));
    let wire = prop(game, at, "power").and_then(|p| p.parse::<i32>().ok()).filter(|_| {
        game.pack.assets.registry.state(game.world.block(at[0], at[1], at[2])).is_some_and(|s| s.name == "minecraft:redstone_wire")
    });
    signal.max(wire.unwrap_or(0))
}

/// Changes one property of the block at `pos`, telling what reads it.
fn set_flag(game: &mut Game<'_>, pos: Pos, key: &str, v: bool) {
    game.native_set_block_property(pos, key, if v { "true" } else { "false" });
}

/// `DiodeBlock.updateNeighborsInFront`.
fn tell_front(game: &mut Game<'_>, pos: Pos, facing: Dir) {
    let dimension = game.dim.id().to_owned();
    game.native_update_signal_neighbors(&dimension, offset(pos, facing.opposite()));
}

/// `native_block_entity_tick`.
pub(super) fn tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    if entity.id == LAMP {
        lamp_tick(game, pos);
        return;
    }
    if entity.id == BELL {
        bell_tick(game, pos, entity);
        return;
    }
    if entity.id == LEVER {
        lever_tick(game, pos, entity);
        return;
    }
    let Some(facing) = prop(game, pos, "facing").and_then(|f| Dir::parse(&f)) else {
        return;
    };
    let name = entity.id.clone();
    // The input, after the block's delay (`getDelay`: 1 for a latch, 2 for brass).
    let should = input(game, pos, facing) > 0;
    let delay = if is_latch(&name) { 1 } else { 2 };
    let powered = flag(game, pos, "powered");
    let powering = flag(game, pos, "powering");
    let mut wait = int(entity, "Wait").unwrap_or(0);
    let mut now_powered = powered;
    if should != powered {
        wait += 1;
        if wait >= delay {
            now_powered = should;
            wait = 0;
        }
    } else {
        wait = 0;
    }
    put(entity, "Wait", wait);
    if let Some(kind) = brass_of(&name) {
        if now_powered != powered {
            set_flag(game, pos, "powered", now_powered);
        }
        let max = int(entity, "MaxState").unwrap_or(kind.default_max());
        let state = int(entity, "State").unwrap_or(0);
        let (next, change) = brass_tick(kind, state, max, now_powered, powering);
        put(entity, "State", next);
        put(entity, "MaxState", max);
        if let Some(p) = change {
            set_flag(game, pos, "powering", p);
            tell_front(game, pos, facing);
        }
        return;
    }
    let mut next = powering;
    if name == TOGGLE_LATCH {
        next = super::create_redstone::toggle_latch(powered, now_powered, powering);
        if now_powered != powered {
            set_flag(game, pos, "powered", now_powered);
        }
    } else {
        // isPoweredOnSides: either side, after the same delay.
        let cw = facing.clockwise(super::create_kinetics::Axis::Y);
        let side_on = input(game, pos, cw) > 0 || input(game, pos, cw.opposite()) > 0;
        let side = flag(game, pos, "powered_side");
        next = super::create_redstone::powered_latch(powered, now_powered, side, side_on, next);
        if now_powered != powered {
            set_flag(game, pos, "powered", now_powered);
        }
        if side_on != side {
            set_flag(game, pos, "powered_side", side_on);
        }
    }
    if next != powering {
        set_flag(game, pos, "powering", next);
        tell_front(game, pos, facing);
    }
}

fn lever_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let dimension = game.dim.id().to_owned();
    let state = int(entity, "State").unwrap_or(0);
    levers(|m| m.insert((dimension.clone(), pos), state));
    let change = int(entity, "ChangeTimer").unwrap_or(0);
    if change > 0 {
        put(entity, "ChangeTimer", change - 1);
        if change == 1 {
            // AnalogLeverBlock.updateNeighbors: around it and its support.
            game.native_update_signal_neighbors(&dimension, pos);
            if let Some(d) = connected(game, pos) {
                game.native_update_signal_neighbors(&dimension, offset(pos, d.opposite()));
            }
        }
    }
}

/// `FaceAttachedHorizontalDirectionalBlock.getConnectedDirection`: away from
/// what it is attached to.
fn connected(game: &Game<'_>, pos: Pos) -> Option<Dir> {
    match prop(game, pos, "face").as_deref() {
        Some("ceiling") => Some(Dir::Down),
        Some("floor") => Some(Dir::Up),
        _ => prop(game, pos, "facing").and_then(|f| Dir::parse(&f)),
    }
}

/// `AnalogLeverBlock.useWithoutItem` and the diodes' `useItemOn` (`activated`,
/// `toggle`). True when it took the click.
pub(super) fn right_click(game: &mut Game<'_>, pos: Pos) -> bool {
    if name_is(game, pos, BELL) {
        return bell_click(game, pos);
    }
    let name = {
        let Some(s) = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2])) else {
            return false;
        };
        s.name.clone()
    };
    if !is_ours(&name) {
        return false;
    }
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    let at = pos.map(|v| f64::from(v) + 0.5);
    if name == LEVER {
        let state = game.native_with_mod_block_entity(pos, |be| {
            let old = int(be, "State").unwrap_or(0);
            let new = super::create_redstone::lever_state(old, sneaking);
            put(be, "State", new);
            if new != old {
                put(be, "ChangeTimer", 15);
            }
            new
        });
        game.native_block_entity_sync(pos);
        let pitch = 0.25 + (state.unwrap_or(0) + 5) as f32 / 15.0 * 0.5;
        game.with_level(|_, level, _| {
            level.native_play_sound("minecraft:block.lever.click", at, super::create_sounds::BLOCKS, 0.2, pitch);
        });
        return true;
    }
    let wrench = game.held().is_some_and(|h| &*h.item == "create:wrench");
    if sneaking || wrench {
        return false;
    }
    let key = if brass_of(&name).is_some() { "inverted" } else { "powering" };
    // PoweredLatchBlock.activated: only while both inputs agree.
    if name == POWERED_LATCH && flag(game, pos, "powered") != flag(game, pos, "powered_side") {
        return false;
    }
    let was = flag(game, pos, key);
    set_flag(game, pos, key, !was);
    if let Some(facing) = prop(game, pos, "facing").and_then(|f| Dir::parse(&f)) {
        tell_front(game, pos, facing);
    }
    let pitch = if was { 0.5 } else { 0.6 };
    game.with_level(|_, level, _| {
        level.native_play_sound("minecraft:block.lever.click", at, super::create_sounds::BLOCKS, 0.3, pitch);
    });
    true
}

/// A wheel step of a brass diode's interval (in place of Create 6's value
/// board): its new label for the action bar.
pub(super) fn step_interval(game: &mut Game<'_>, pos: Pos, up: bool) -> Option<String> {
    let name = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2]))?.name.clone();
    let kind = brass_of(&name)?;
    game.native_with_mod_block_entity(pos, |be| {
        let max = super::create_redstone::interval_step(int(be, "MaxState").unwrap_or(kind.default_max()), up);
        put(be, "MaxState", max);
        // onMaxDelayChanged
        let state = int(be, "State").unwrap_or(0).clamp(0, max);
        put(be, "State", state);
        super::create_redstone::interval_label(max)
    })
}

pub(super) fn is_brass(name: &str) -> bool {
    brass_of(name).is_some()
}

/// `native_block_signal`: `face` is the block's side touching the reader.
pub(super) fn block_signal(
    level: &super::GameLevel<'_>,
    dimension: &str,
    pos: Pos,
    state: BlockStateId,
    face: Dir,
    direct: bool,
    signal: &mut i32,
) {
    let Some(s) = level.registry.state(state) else {
        return;
    };
    let on = |k: &str| s.properties.get(k).map(String::as_str) == Some("true");
    // DeskBellBlock.getSignal and getDirectSignal.
    if s.name == BELL {
        let facing = s.properties.get("facing").and_then(|f| Dir::parse(f));
        *signal = if on("powered") && (!direct || facing == Some(face.opposite())) { 15 } else { 0 };
        return;
    }
    // RoseQuartzLampBlock.getSignal: none to another lamp, to a comparator how
    // far along this column the powering one is.
    if s.name == LAMP {
        if direct {
            *signal = 0;
            return;
        }
        let n = face.normal();
        let reader = level.registry.state(level.world.block(pos[0] + n[0], pos[1] + n[1], pos[2] + n[2])).map(|r| r.name.as_str());
        *signal = match reader {
            Some(LAMP) => 0,
            Some("minecraft:comparator") => {
                let powering: Vec<Option<bool>> = (0..15)
                    .map(|i| {
                        let c = [pos[0] - n[0] * i, pos[1] - n[1] * i, pos[2] - n[2] * i];
                        let b = level.registry.state(level.world.block(c[0], c[1], c[2])).filter(|b| b.name == LAMP)?;
                        Some(b.properties.get("powering").is_some_and(|v| v == "true"))
                    })
                    .collect();
                super::create_redstone::lamp_distance(&powering)
            }
            _ if on("powering") => 15,
            _ => 0,
        };
        return;
    }
    if s.name == LEVER {
        let connected = match s.properties.get("face").map(String::as_str) {
            Some("ceiling") => Some(Dir::Down),
            Some("floor") => Some(Dir::Up),
            _ => s.properties.get("facing").and_then(|f| Dir::parse(f)),
        };
        let strength = levers(|m| m.get(&(dimension.to_owned(), pos)).copied()).unwrap_or(0);
        *signal = if !direct || connected == Some(face.opposite()) { strength } else { 0 };
        return;
    }
    if !is_diode(&s.name) {
        return;
    }
    let facing = s.properties.get("facing").and_then(|f| Dir::parse(f));
    *signal = if facing == Some(face.opposite()) {
        if is_latch(&s.name) {
            if on("powering") { 15 } else { 0 }
        } else {
            brass_output(on("powering"), on("inverted"))
        }
    } else {
        0
    };
}

/// `AnalogLeverRenderer` through `native_block_entity_submit`: the handle at
/// its strength (not yet `clientState`'s easing) and the indicator (not yet its
/// colour).
pub(super) fn submit(
    game: &Game<'_>,
    pos: Pos,
    entity: &ModBlockEntity,
    draw: &mut super::native_block_entity_draw::BlockEntityDraw,
) {
    if entity.id != LEVER {
        return;
    }
    let state = int(entity, "State").unwrap_or(0);
    let r_x = match prop(game, pos, "face").as_deref() {
        Some("floor") => 0.0,
        Some("wall") => 90.0,
        _ => 180.0,
    };
    let r_y = match prop(game, pos, "facing").and_then(|f| Dir::parse(&f)) {
        Some(Dir::West) => -90.0,
        Some(Dir::North) => 180.0,
        Some(Dir::East) => -270.0,
        _ => 0.0,
    };
    draw.push();
    draw.rotate_about([0.0, 1.0, 0.0], r_y, [0.5; 3]);
    draw.rotate_about([1.0, 0.0, 0.0], r_x, [0.5; 3]);
    game.native_draw_block(draw, "create:hz_part_analog_lever_indicator", &[]);
    draw.rotate_about([1.0, 0.0, 0.0], state as f32 / 15.0 * 90.0, [0.5, 1.0 / 16.0, 0.5]);
    game.native_draw_block(draw, "create:hz_part_analog_lever_handle", &[]);
    draw.pop();
}
