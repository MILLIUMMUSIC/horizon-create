// Horizon's side of the weighted ejector (rules in ejector.rs): its block entity's
// tick, what goes in, its target, its lid and flying items drawn.

use super::Game;
use super::create_ejector::{Launcher, SCAN_INTERVAL, State};
use super::create_kinetics::{Dir, Pos};
use horizon_client::containers::ModBlockEntity;
use horizon_client::items::ItemStack;
use horizon_world::nbt::{Compound, JavaString, Tag};
use std::collections::HashMap;
use std::sync::Mutex;

pub(super) const EJECTOR: &str = "create:weighted_ejector";

fn int(nbt: &Compound, key: &str, default: i32) -> i32 {
    match nbt.get(&JavaString::from(key)) {
        Some(Tag::Int(v)) => *v,
        _ => default,
    }
}

fn float(nbt: &Compound, key: &str, default: f32) -> f32 {
    match nbt.get(&JavaString::from(key)) {
        Some(Tag::Float(v)) => f32::from_bits(*v),
        _ => default,
    }
}

fn put_float(nbt: &mut Compound, key: &str, v: f32) {
    nbt.insert(JavaString::from(key), Tag::Float(v.to_bits()));
}

fn put_int(nbt: &mut Compound, key: &str, v: i32) {
    nbt.insert(JavaString::from(key), Tag::Int(v));
}

pub(super) fn is_ejector(game: &Game<'_>, pos: Pos) -> bool {
    state_of(game, pos).is_some_and(|s| s.name == EJECTOR)
}

fn state_of<'a>(game: &'a Game<'_>, pos: Pos) -> Option<&'a horizon_world::chunk::BlockState> {
    game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2]))
}

fn facing_at(game: &Game<'_>, pos: Pos) -> Option<Dir> {
    state_of(game, pos).filter(|s| s.name == EJECTOR).and_then(|s| s.properties.get("facing")).and_then(|f| Dir::parse(f))
}

fn launcher(nbt: &Compound) -> Launcher {
    Launcher::new(int(nbt, "HorizontalDistance", 1), int(nbt, "VerticalDistance", 0))
}

fn state(nbt: &Compound) -> State {
    State::of(int(nbt, "State", State::Retracting.code()))
}

/// An item in the air (`launchedItems`).
struct Flying {
    time: i32,
    tracked: bool,
    stack: ItemStack,
}

fn read_flying(nbt: &Compound) -> Vec<Flying> {
    let Some(Tag::List(list)) = nbt.get(&JavaString::from("LaunchedItems")) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|c| {
            let stack = c.get("item").and_then(horizon_items::item_nbt::item_from_nbt)?;
            let time = match c.get("t") {
                Some(Tag::Int(t)) => *t,
                _ => 0,
            };
            Some(Flying { time, tracked: matches!(c.get("tracked"), Some(Tag::Byte(1))), stack })
        })
        .collect()
}

fn write_flying(nbt: &mut Compound, flying: &[Flying]) {
    if flying.is_empty() {
        nbt.remove(&JavaString::from("LaunchedItems"));
        return;
    }
    let list = flying
        .iter()
        .map(|f| {
            Tag::compound(vec![
                ("t", Tag::Int(f.time)),
                ("tracked", Tag::Byte(i8::from(f.tracked))),
                ("item", horizon_items::item_nbt::item_to_nbt(&f.stack)),
            ])
        })
        .collect();
    nbt.insert(JavaString::from("LaunchedItems"), Tag::List(list));
}

/// `earlyTarget`, and `earlyTargetTime`.
fn early(nbt: &Compound) -> Option<([f64; 3], Pos, f32)> {
    let time = float(nbt, "EarlyTime", -1.0);
    (time >= 0.0).then(|| {
        (
            [f64::from(float(nbt, "EarlyX", 0.0)), f64::from(float(nbt, "EarlyY", 0.0)), f64::from(float(nbt, "EarlyZ", 0.0))],
            [int(nbt, "EarlyPosX", 0), int(nbt, "EarlyPosY", 0), int(nbt, "EarlyPosZ", 0)],
            time,
        )
    })
}

fn set_early(nbt: &mut Compound, target: Option<([f64; 3], Pos, f32)>) {
    let Some((at, pos, time)) = target else {
        put_float(nbt, "EarlyTime", -1.0);
        return;
    };
    put_float(nbt, "EarlyX", at[0] as f32);
    put_float(nbt, "EarlyY", at[1] as f32);
    put_float(nbt, "EarlyZ", at[2] as f32);
    put_int(nbt, "EarlyPosX", pos[0]);
    put_int(nbt, "EarlyPosY", pos[1]);
    put_int(nbt, "EarlyPosZ", pos[2]);
    put_float(nbt, "EarlyTime", time);
}

/// The first block a line from `a` to `b` runs into (`Level.clip` with
/// colliders): where, which block and the face's way out.
fn clip(game: &Game<'_>, a: [f64; 3], b: [f64; 3]) -> Option<([f64; 3], Pos, Dir)> {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let steps = ((length * 16.0).ceil() as i32).max(1);
    let mut last = a;
    for i in 1..=steps {
        let t = f64::from(i) / f64::from(steps);
        let p = [a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t];
        let cell = [p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32];
        let inside = game.block_collision_shape(cell).boxes().iter().any(|b| {
            let (min, max) = (b.min_position(), b.max_position());
            let local = [p[0] - f64::from(cell[0]), p[1] - f64::from(cell[1]), p[2] - f64::from(cell[2])];
            local[0] >= min.x && local[0] <= max.x && local[1] >= min.y && local[1] <= max.y && local[2] >= min.z && local[2] <= max.z
        });
        if inside {
            let came = [last[0] - p[0], last[1] - p[1], last[2] - p[2]];
            let k = (0..3).max_by(|x, y| came[*x].abs().total_cmp(&came[*y].abs())).unwrap_or(1);
            let face = match (k, came[k] > 0.0) {
                (0, true) => Dir::East,
                (0, false) => Dir::West,
                (1, true) => Dir::Up,
                (1, false) => Dir::Down,
                (_, true) => Dir::South,
                _ => Dir::North,
            };
            return Some((p, cell, face));
        }
        last = p;
    }
    None
}

/// `scanTrajectoryForObstacles`: true when the tracked item hit something.
fn scan(game: &Game<'_>, nbt: &mut Compound, l: &Launcher, facing: Dir, pos: Pos, time: i32) -> bool {
    if time <= 2 {
        return false;
    }
    let source = l.position(f64::from(time), facing, pos);
    let target = l.position(f64::from(time + 1), facing, pos);
    let mut hit = clip(game, source, target);
    // An extracting funnel lets it through.
    if let Some((_, cell, _)) = hit
        && state_of(game, cell).is_some_and(|s| {
            super::create_funnel::is_funnel(&s.name) && s.properties.get("extracting").is_some_and(|v| v == "true")
        })
    {
        hit = None;
    }
    let Some((at, cell, face)) = hit else {
        if early(nbt).is_some_and(|(_, _, t)| t < (time + 1) as f32) {
            set_early(nbt, None);
        }
        return false;
    };
    let n = face.normal();
    let early_at = [at[0] + f64::from(n[0]) * 0.25, at[1] + f64::from(n[1]) * 0.25, at[2] + f64::from(n[2]) * 0.25];
    let dist = |p: [f64; 3], q: [f64; 3]| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt();
    let t = time as f32 + (dist(source, at) / dist(source, target).max(1e-9)) as f32;
    set_early(nbt, Some((early_at, cell, t)));
    true
}

/// `DepotBehaviour.getPresentStackSize`.
fn present(items: &[Option<ItemStack>]) -> u32 {
    items.iter().flatten().map(|s| s.count).sum()
}

/// `EjectorBlockEntity.tick`, with `DepotBehaviour`'s.
pub(super) fn tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let Some(facing) = facing_at(game, pos) else {
        return;
    };
    if entity.items.len() < super::create_machines::DEPOT_SLOTS {
        entity.items.resize(super::create_machines::DEPOT_SLOTS, None);
    }
    let l = launcher(&entity.nbt);
    let speed = super::create_glue::with_kinetics(game, |k, level| k.speed_of(level, pos));
    let mut s = state(&entity.nbt);
    let mut lid = float(&entity.nbt, "Lid", 1.0);
    let max_stack = int(&entity.nbt, "MaxStack", 0);
    let mut scan_cooldown = int(&entity.nbt, "ScanCooldown", 0);
    let mut flying = read_flying(&entity.nbt);
    let powered = super::create_glue::signal(game, pos);
    if scan_cooldown > 0 {
        scan_cooldown -= 1;
    }
    // `canAcceptItems`: items landing on it go in only while it is charged.
    if s == State::Charged {
        super::create_machines::take_landed_depot(game, pos, entity, max_stack);
    }
    super::create_machines::process_on_depot_of(game, pos, entity);
    // Mobs stepping on it (`updateEntityAfterFallOn`), or a player's client.
    let [px, py, pz] = pos.map(f64::from);
    let dimension = game.dim.id().to_owned();
    let on_it = |p: [f64; 3]| {
        p[0] > px + 1.0 / 16.0 && p[0] < px + 15.0 / 16.0 && p[2] > pz + 1.0 / 16.0 && p[2] < pz + 15.0 / 16.0 && p[1] >= py && p[1] <= py + 1.0
    };
    let mobs: Vec<u32> = game.native_mobs(&dimension).into_iter().filter(|m| on_it(m.pos)).map(|m| m.id).collect();
    let mut launch = int(&entity.nbt, "Launch", 0) != 0;
    if !mobs.is_empty() && s != State::Retracting && !powered && l.horizontal != 0 {
        launch = true;
    }
    // ejectIfTriggered.
    if s == State::Charged && !powered && !launch {
        let size = present(&entity.items);
        let waiting = size == 0 || (size as i32) < max_stack;
        let held = entity.items[0].clone();
        // The target must have room for what it holds.
        let target = early(&entity.nbt).map_or_else(|| l.target(facing, pos), |e| e.1);
        let open = super::create_logistics::can_insert_side(game, target, Dir::Up);
        let blocked = open
            && held.as_ref().is_some_and(|h| {
                super::create_logistics::side_insert_simulated(game, target, Dir::Up, h.clone()).is_some_and(|left| left.count == h.count)
            });
        launch = !waiting && !blocked;
    }
    if launch && s == State::Charged {
        // activateDeferred.
        launch_items(game, pos, facing, entity, &mut flying, &mut scan_cooldown);
        for id in mobs {
            game.native_push_mob(&dimension, id, l.entity_motion(facing));
        }
        s = State::Launching;
        let at = pos.map(|v| f64::from(v) + 0.5);
        game.with_level(|_, level, _| {
            level.native_play_sound("minecraft:block.wooden_trapdoor.close", at, super::create_sounds::BLOCKS, 0.35, 1.0);
            level.native_play_sound("minecraft:block.chest.open", at, super::create_sounds::BLOCKS, 0.1, 1.4);
        });
    }
    put_int(&mut entity.nbt, "Launch", 0);
    // The items in the air.
    let total = (l.total_ticks as f32).max(3.0);
    let mut landed = Vec::new();
    flying.retain_mut(|f| {
        let hit = f.tracked && scan(game, &mut entity.nbt, &l, facing, pos, f.time);
        let max_time = early(&entity.nbt).map_or(total, |e| e.2.min(total));
        if hit || f.time > max_time as i32 {
            landed.push((f.stack.clone(), f.tracked, max_time));
            return false;
        }
        f.time += 1;
        true
    });
    for (stack, _, max_time) in landed {
        place_at_target(game, pos, facing, &l, entity, stack, max_time);
    }
    if flying.is_empty() {
        set_early(&mut entity.nbt, None);
    }
    // The lid.
    let wind = super::create_ejector::wind_up_speed(speed, l.horizontal, l.vertical);
    let before = s;
    (s, lid) = super::create_ejector::lid_tick(s, lid, wind);
    if before == State::Retracting && s == State::Retracting && wind > 0.0 {
        let rate = (1.0 / (wind * 5.0)) as i64 + 1;
        if game.world.game_time as i64 % rate == 0 {
            let at = pos.map(|v| f64::from(v) + 0.5);
            let pitch = 1.5 - lid;
            game.with_level(|_, level, _| {
                level.native_play_sound("minecraft:block.wooden_button.click_off", at, super::create_sounds::BLOCKS, 0.125, pitch);
            });
        }
    }
    put_int(&mut entity.nbt, "State", s.code());
    put_float(&mut entity.nbt, "Lid", lid);
    put_int(&mut entity.nbt, "ScanCooldown", scan_cooldown);
    write_flying(&mut entity.nbt, &flying);
    // What clients draw: its stack, then state, lid, distances, powered, then
    // the items in the air with their times.
    let held = entity.items[0].as_ref().map_or_else(|| (String::new(), 0), |h| (h.item.to_string(), h.count));
    let mut view = vec![
        held,
        (String::new(), s.code() as u32),
        (String::new(), (lid * 255.0) as u32),
        (String::new(), l.horizontal as u32),
        (String::new(), (l.vertical + 128) as u32),
        (String::new(), u32::from(powered)),
    ];
    view.extend(flying.iter().map(|f| (f.stack.item.to_string(), (f.time as u32).min(255))));
    super::create_machines::note_view(game, pos, view);
}

/// `launchItems`: into a belt funnel above facing back, else into the air.
fn launch_items(game: &mut Game<'_>, pos: Pos, facing: Dir, entity: &mut ModBlockEntity, flying: &mut Vec<Flying>, cooldown: &mut i32) {
    let above = [pos[0], pos[1] + 1, pos[2]];
    let side = facing.opposite();
    if let Some(held) = entity.items[0].clone()
        && let Some(left) = super::create_logistics::export_to_belt_funnel(game, above, side, &held)
    {
        entity.items[0] = left;
        return;
    }
    for slot in entity.items.iter_mut() {
        if let Some(stack) = slot.take() {
            let tracked = !flying.iter().any(|f| f.tracked) && *cooldown == 0;
            if tracked {
                *cooldown = SCAN_INTERVAL;
            }
            flying.push(Flying { time: 0, tracked, stack });
        }
    }
    super::create_machines::set_locked(entity, false);
}

/// `placeItemAtTarget`.
fn place_at_target(game: &mut Game<'_>, pos: Pos, facing: Dir, l: &Launcher, entity: &ModBlockEntity, stack: ItemStack, max_time: f32) {
    let early_target = early(&entity.nbt);
    let target = early_target.map_or_else(|| l.target(facing, pos), |e| e.1);
    let left = if super::create_logistics::can_insert_side(game, target, Dir::Up) {
        super::create_logistics::insert_side(game, target, Dir::Up, stack)
    } else {
        Some(stack)
    };
    let Some(left) = left else {
        return;
    };
    let at = early_target.map_or_else(|| l.position(f64::from(max_time), facing, pos), |e| e.0);
    game.spawn_item(left, at, l.item_motion(f64::from(max_time), facing), 40);
}

/// `DepotBehaviour.tryInsertingFromSide`: `canAcceptItems` while charged, up
/// to its stack size, merging the same item. What is left, none when it can't
/// take anything.
pub(super) fn insert(game: &mut Game<'_>, pos: Pos, stack: ItemStack) -> Option<Option<ItemStack>> {
    let max = super::create_machines::max_stack(game, &stack);
    game.native_with_mod_block_entity(pos, |be| {
        if state(&be.nbt) != State::Charged {
            return None;
        }
        if be.items.len() < super::create_machines::DEPOT_SLOTS {
            be.items.resize(super::create_machines::DEPOT_SLOTS, None);
        }
        let limit = match int(&be.nbt, "MaxStack", 0) {
            0 => 64,
            n => n,
        }
        .min(max as i32);
        let room = (limit - present(&be.items) as i32).max(0);
        if room == 0 {
            return None;
        }
        if be.items[0].as_ref().is_some_and(|h| !h.same_item_same_components(&stack)) {
            return None;
        }
        let taken = room.min(stack.count as i32) as u32;
        match &mut be.items[0] {
            Some(h) => h.count += taken,
            slot => {
                let mut s = stack.clone();
                s.count = taken;
                *slot = Some(s);
            }
        }
        let mut left = stack.clone();
        left.count -= taken;
        Some((left.count > 0).then_some(left))
    })?
}

/// Whether it has room for some of `stack` now.
pub(super) fn can_accept(game: &Game<'_>, pos: Pos, stack: &ItemStack) -> bool {
    let Some(horizon_client::containers::BlockEntity::Mod(be)) = game.world.block_entities.get(&(pos[0], pos[1], pos[2])) else {
        return false;
    };
    if state(&be.nbt) != State::Charged {
        return false;
    }
    let limit = match int(&be.nbt, "MaxStack", 0) {
        0 => 64,
        n => n,
    };
    be.items.first().and_then(Option::as_ref).is_none_or(|h| h.same_item_same_components(stack)) && (present(&be.items) as i32) < limit
}

/// Each player's chosen target (`EjectorTargetHandler.currentSelection`).
static TARGETS: Mutex<Option<HashMap<u128, Pos>>> = Mutex::new(None);

/// `rightClickingBlocksSelectsThem`: sneaking with an ejector in hand.
pub(super) fn select_target(game: &mut Game<'_>, pos: Pos) -> bool {
    if game.held().is_none_or(|s| &*s.item != EJECTOR) || !game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft) {
        return false;
    }
    let Some(player) = game.native_acting_player() else {
        return false;
    };
    if player.game_mode == 3 {
        return false;
    }
    TARGETS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(player.uuid, pos);
    super::create_contraptions::tell_later(player.uuid, horizon_client::text::Text::literal("Target Selected".to_owned()));
    true
}

/// `EjectorItem.updateCustomBlockEntityTag`, `flushSettings` and
/// `EjectorPlacementPacket`: the placed ejector takes the player's target.
pub(super) fn placed(game: &mut Game<'_>, pos: Pos) {
    if !is_ejector(game, pos) {
        return;
    }
    let Some(player) = game.native_acting_player() else {
        return;
    };
    let target = TARGETS.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|m| m.remove(&player.uuid));
    let direction = target.and_then(|t| super::create_ejector::target_direction(pos, t));
    let line = match (target, direction) {
        (None, _) => "Ejecting to Adjacent block (No Target was Selected)".to_owned(),
        (Some(_), None) => "Ejecting to Adjacent block (Target was not Valid)".to_owned(),
        (Some(t), Some(d)) => {
            let (h, v) = super::create_ejector::target_distances(pos, t);
            game.native_with_mod_block_entity(pos, |be| {
                put_int(&mut be.nbt, "HorizontalDistance", h.max(1));
                put_int(&mut be.nbt, "VerticalDistance", v);
            });
            game.native_set_block_property(pos, "facing", d.name());
            format!("Ejecting to [{},{},{}]", t[0], t[1], t[2])
        }
    };
    super::create_contraptions::tell_later(player.uuid, horizon_client::text::Text::literal(line));
}

/// `EjectorSlot`: a hit on its value box.
pub(super) fn hits_value_box(game: &Game<'_>, pos: Pos, side: Dir, point: [f64; 3]) -> bool {
    let Some(facing) = facing_at(game, pos) else {
        return false;
    };
    let s = client_view(pos).map_or(State::Retracting, |v| v.state);
    let Some(at) = super::create_ejector::value_box(facing, s, side) else {
        return false;
    };
    let local = [0, 1, 2].map(|k| (point[k] - f64::from(pos[k])) as f32);
    (0..3).map(|k| (local[k] - at[k]).powi(2)).sum::<f32>().sqrt() < 0.2
}

/// A step of `maxStackSize` (0 to 64, 0 being any), and the line to tell.
pub(super) fn step_stack_size(game: &mut Game<'_>, pos: Pos, up: bool) -> Option<String> {
    let value = game.native_with_mod_block_entity(pos, |be| {
        let next = (int(&be.nbt, "MaxStack", 0) + if up { 1 } else { -1 }).clamp(0, 64);
        put_int(&mut be.nbt, "MaxStack", next);
        next
    })?;
    Some(if value == 0 { "Ejected Stack Size: *".to_owned() } else { format!("Ejected Stack Size: {value}") })
}

/// A player's client says it stepped on the ejector at `pos`.
pub(super) fn trigger(game: &mut Game<'_>, pos: Pos) {
    game.native_with_mod_block_entity(pos, |be| {
        if state(&be.nbt) != State::Retracting {
            put_int(&mut be.nbt, "Launch", 1);
        }
    });
}

/// What a client knows of an ejector.
struct View {
    held: (String, u32),
    state: State,
    lid: f32,
    launcher: Launcher,
    powered: bool,
    flying: Vec<(String, u32)>,
}

fn client_view(pos: Pos) -> Option<View> {
    let slots = super::create_machines::client_slots(pos)?;
    if slots.len() < 6 {
        return None;
    }
    Some(View {
        held: slots[0].clone(),
        state: State::of(slots[1].1 as i32),
        lid: slots[2].1 as f32 / 255.0,
        launcher: Launcher::new(slots[3].1 as i32, slots[4].1 as i32 - 128),
        powered: slots[5].1 != 0,
        flying: slots[6..].to_vec(),
    })
}

/// The last state of the ejector under the window's player, and whether it was
/// told of them stepping on it.
static UNDER_PLAYER: Mutex<Option<(Pos, State, bool)>> = Mutex::new(None);

/// `updateEntityAfterFallOn` and `activateDeferred` for the window's player, whom
/// the server's copy can't move: stepping on a charged ejector asks it to fire;
/// when it fires under them, off they go.
pub(super) fn client_tick(game: &mut Game<'_>) {
    let Some(p) = game.player.as_ref() else {
        return;
    };
    let (x, y, z) = (p.position.x, p.position.y, p.position.z);
    let on_ground = p.ground.on_ground;
    let pos = [x.floor() as i32, (y - 0.2).floor() as i32, z.floor() as i32];
    let (Some(facing), Some(view)) = (facing_at(game, pos), client_view(pos)) else {
        *UNDER_PLAYER.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return;
    };
    let (was, asked) = {
        let guard = UNDER_PLAYER.lock().unwrap_or_else(|e| e.into_inner());
        match *guard {
            Some((at, state, asked)) if at == pos => (state, asked),
            _ => (view.state, false),
        }
    };
    let mut asked = asked && view.state == State::Charged;
    if on_ground && view.state == State::Charged && !view.powered && view.launcher.horizontal != 0 && !asked {
        asked = true;
        super::create_glue::send_ejector_trigger(game, pos);
    }
    // It is `Launching` for so few ticks that a client may not see it.
    if was == State::Charged && view.state != State::Charged {
        let m = view.launcher.entity_motion(facing);
        if let Some(p) = game.player.as_mut() {
            p.position = horizon_core::geometry::Vec3::new(f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 1.0, f64::from(pos[2]) + 0.5);
            p.velocity = horizon_core::geometry::Vec3::new(m[0], m[1], m[2]);
            p.ground.on_ground = false;
        }
    }
    *UNDER_PLAYER.lock().unwrap_or_else(|e| e.into_inner()) = Some((pos, view.state, asked));
}

/// `EjectorRenderer`: the lid, what lies on it and the items in the air.
pub(super) fn draw(game: &Game<'_>, camera: glam::DVec3, time: f32, out: &mut Vec<horizon_client::mesher::Vertex>) {
    for pos in super::create_machines::client_positions() {
        let Some(facing) = facing_at(game, pos) else {
            continue;
        };
        let Some(view) = client_view(pos) else {
            continue;
        };
        let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
        if corner.distance_squared(camera) > 128.0 * 128.0 {
            continue;
        }
        let light = super::create_logistics::neighbour_light(game, pos);
        let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
        let origin = glam::Mat4::from_translation((corner - camera).as_vec3());
        let centre = glam::Vec3::splat(0.5);
        let pivot = glam::Vec3::new(0.0, 11.25 / 16.0, 0.75 / 16.0);
        let angle = (180.0 + facing.horizontal_angle()).to_radians();
        // applyLidAngle.
        let lid = origin
            * glam::Mat4::from_translation(centre)
            * glam::Mat4::from_rotation_y(angle)
            * glam::Mat4::from_translation(-centre)
            * glam::Mat4::from_translation(pivot)
            * glam::Mat4::from_rotation_x(-(view.lid * 70.0).to_radians())
            * glam::Mat4::from_translation(-pivot);
        if let Some(state) = game.pack.assets.registry.default_state("create:hz_part_ejector_top") {
            let seed = horizon_core::random::position_seed(horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]));
            game.block_model_vertices(state, seed, lid, packed, false, out);
        }
        let draw_item = |game: &Game<'_>, id: &str, count: u32, m: glam::Mat4, at: glam::DVec3, out: &mut Vec<horizon_client::mesher::Vertex>| {
            let stack = ItemStack::new(id, count.max(1));
            let owner = super::item_draw::owner_at(at.to_array(), 0.0, EJECTOR);
            let resolved = game.resolve_item(&stack, horizon_assets::item_model::DisplayContext::Fixed, Some(&owner), pos[0] ^ pos[2]);
            game.submit_item_world(&resolved, m, horizon_client::block_entity_render::vertex_light(light), out);
        };
        if !view.held.0.is_empty() {
            // DepotRenderer.renderItemsOf, on the lid.
            let m = lid
                * glam::Mat4::from_translation(centre)
                * glam::Mat4::from_rotation_y(-angle)
                * glam::Mat4::from_translation(glam::Vec3::new(0.0, 13.5 / 16.0 - 0.5, 0.0))
                * glam::Mat4::from_rotation_x(90f32.to_radians())
                * glam::Mat4::from_scale(glam::Vec3::splat(0.5));
            draw_item(game, &view.held.0, view.held.1, m, corner, out);
        }
        let partial = time.fract();
        for (id, t) in &view.flying {
            let t = *t as f32 + partial;
            let at = view.launcher.position(f64::from(t), facing, pos);
            let at = glam::DVec3::new(at[0], at[1], at[2]);
            let offset = glam::Vec3::new(0.0, 2.0 / 16.0, -1.0 / 16.0);
            let m = glam::Mat4::from_translation((at - camera).as_vec3())
                * glam::Mat4::from_translation(offset)
                * glam::Mat4::from_scale(glam::Vec3::splat(0.5))
                * glam::Mat4::from_rotation_y(facing.horizontal_angle().to_radians())
                * glam::Mat4::from_rotation_x((t * 40.0).to_radians())
                * glam::Mat4::from_translation(-offset);
            draw_item(game, id, 1, m, at, out);
        }
    }
}
