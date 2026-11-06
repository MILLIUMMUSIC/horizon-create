// Horizon's side of metal girders and brackets (rules in girder.rs). A girder's
// blockstate adds `brace_*` (`ConnectedGirderModel`'s side brackets) and `pole_up`,
// `pole_down` (`GirderCTBehaviour`) as properties of its own, as no model data
// reaches a baked block. Brackets (`BracketedBlockEntityBehaviour`) live here, saved
// in `data/create_brackets_*.txt` and told to clients on `create:brackets`.

use super::Game;
use super::create_girder::{self as rules, Action, Bracket, BracketType, Girder, Side};
use super::create_kinetics::{Axis, Dir, Kind, Pos};
use horizon_client::items::ItemStack;
use horizon_world::registry::BlockStateId;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

pub(super) const GIRDER: &str = "create:metal_girder";
pub(super) const ENCASED: &str = "create:metal_girder_encased_shaft";
const METAL_BRACKET: &str = "create:metal_bracket";
const WOODEN_BRACKET: &str = "create:wooden_bracket";
pub(super) const CHANNEL: &str = "create:brackets";

pub(super) fn is_bracket_item(name: &str) -> bool {
    name == METAL_BRACKET || name == WOODEN_BRACKET
}

fn add(pos: Pos, d: Dir) -> Pos {
    let n = d.normal();
    [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]]
}

// ---- Brackets' store ------------------------------------------------------------------

#[derive(Default)]
struct Store {
    dims: HashMap<String, HashMap<Pos, Bracket>>,
    loaded: HashSet<String>,
    dirty: HashSet<String>,
    root: Option<PathBuf>,
    ticks: u64,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);
static CLIENT: Mutex<Option<HashMap<Pos, Bracket>>> = Mutex::new(None);

fn store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut guard = STORE.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Store::default))
}

fn path(root: &std::path::Path, dimension: &str) -> PathBuf {
    root.join("data").join(format!("create_brackets_{}.txt", dimension.replace(':', "_")))
}

fn save(s: &Store) {
    let Some(root) = &s.root else {
        return;
    };
    let _ = std::fs::create_dir_all(root.join("data"));
    for (dimension, map) in &s.dims {
        let mut text = String::new();
        for (p, b) in map {
            text.push_str(&format!("{} {} {} {}\n", p[0], p[1], p[2], b.to_line()));
        }
        if let Err(e) = std::fs::write(path(root, dimension), text) {
            log::warn!("Create: can't save the brackets of {dimension}: {e}");
        }
    }
}

/// Periodic save, with the glue's.
pub(super) fn save_all() {
    store(|s| save(s));
}

/// A world opened or closed.
pub(super) fn world_phase(closing: bool) {
    let mut guard = STORE.lock().unwrap_or_else(|e| e.into_inner());
    if closing && let Some(s) = guard.as_ref() {
        save(s);
    }
    *guard = None;
    *CLIENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// A bracket at `pos` in `dimension`.
pub(super) fn bracket_at(dimension: &str, pos: Pos) -> Option<Bracket> {
    store(|s| s.dims.get(dimension).and_then(|m| m.get(&pos)).copied())
}

fn set_bracket(dimension: &str, pos: Pos, b: Option<Bracket>) {
    store(|s| {
        let map = s.dims.entry(dimension.to_owned()).or_default();
        match b {
            Some(b) => {
                map.insert(pos, b);
            }
            None => {
                map.remove(&pos);
            }
        }
        s.dirty.insert(dimension.to_owned());
    });
}

/// `native_block_changed`: places whose bracket has lost its block's state, for
/// the tick to drop (`onRemove`).
static CHANGED: Mutex<Vec<(String, Pos)>> = Mutex::new(Vec::new());

/// `native_block_changed`.
pub(super) fn block_changed(dimension: &str, pos: Pos) {
    if bracket_at(dimension, pos).is_some() {
        CHANGED.lock().unwrap_or_else(|e| e.into_inner()).push((dimension.to_owned(), pos));
    }
}

/// What can hold a bracket at `pos` (`BracketedBlockEntityBehaviour`, `canHaveBracket`):
/// the axis it runs along, none for a pipe that isn't straight.
fn holder(game: &Game<'_>, pos: Pos) -> Option<(Option<Axis>, BracketType)> {
    match super::create_glue::kind_of(game, pos) {
        Some(Kind::Shaft(a)) => Some((Some(a), BracketType::Shaft)),
        Some(Kind::Cogwheel(a) | Kind::LargeCogwheel(a)) => Some((Some(a), BracketType::Cog)),
        _ => super::create_fluid_glue::bracket_pipe_axis(game, pos).map(|a| (a, BracketType::Pipe)),
    }
}

/// `updateNeighbourShapes`, as a bracket changes what girders beside it see.
fn update_neighbours(game: &mut Game<'_>, pos: Pos) {
    let dimension = game.dim.id().to_owned();
    for d in Dir::ALL {
        let at = add(pos, d);
        let id = game.world.block(at[0], at[1], at[2]);
        let next = game.with_level(|_, level, _| updated_girder(level, id, at, d.opposite()));
        if let Some(next) = next {
            game.native_set_block(&dimension, at, next, super::native_hooks::flags::UPDATE_ALL);
        }
    }
}

/// `BracketBlockItem.useOn`. True when the item is a bracket and the block takes one.
pub(super) fn use_bracket(game: &mut Game<'_>, pos: Pos, face: [i32; 3]) -> bool {
    let Some(item) = game.held().map(|s| s.item.to_string()).filter(|i| is_bracket_item(i)) else {
        return false;
    };
    let Some((axis, ty)) = holder(game, pos) else {
        return false;
    };
    let metal = item == METAL_BRACKET;
    let player = game.native_acting_player();
    let look = player.as_ref().map(|p| super::create_placement::nearest_looking(p.yaw, p.pitch));
    let Some(next) = rules::suitable_bracket(metal, axis, Dir::nearest(face), ty)
        .or_else(|| rules::suitable_bracket(metal, axis, look?.opposite(), ty))
    else {
        return true;
    };
    let dimension = game.dim.id().to_owned();
    let old = bracket_at(&dimension, pos);
    if old == Some(next) {
        return true;
    }
    let at = pos.map(|v| f64::from(v) + 0.5);
    let sound = if metal { "minecraft:block.netherite_block.place" } else { "minecraft:block.wood.place" };
    game.with_level(|_, level, _| level.native_play_sound(sound, at, super::create_sounds::BLOCKS, 0.75, 1.0));
    set_bracket(&dimension, pos, Some(next));
    if player.is_none_or(|p| p.game_mode != 1) {
        take_one(game);
        if let Some(o) = old {
            game.give_or_drop(ItemStack::new(o.block(), 1));
        }
    }
    update_neighbours(game, pos);
    true
}

/// `IWrenchableWithBracket.tryRemoveBracket`.
pub(super) fn wrench_bracket(game: &mut Game<'_>, pos: Pos) -> bool {
    let dimension = game.dim.id().to_owned();
    let Some(b) = bracket_at(&dimension, pos) else {
        return false;
    };
    set_bracket(&dimension, pos, None);
    if game.native_acting_player().is_none_or(|p| p.game_mode != 1) {
        game.give_or_drop(ItemStack::new(b.block(), 1));
    }
    update_neighbours(game, pos);
    // A fluid pipe reconnects (`updateBlockState`).
    super::create_fluid_glue::pipe_after_bracket(game, pos);
    true
}

/// The server's tick: brackets whose block changed drop; clients are told.
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str) {
    let (load, root) = store(|s| {
        s.root.get_or_insert_with(|| game.save.root.clone());
        (s.loaded.insert(dimension.to_owned()), s.root.clone())
    });
    if load && let Some(root) = root {
        let text = std::fs::read_to_string(path(&root, dimension)).unwrap_or_default();
        let mut map = HashMap::new();
        for line in text.lines() {
            let f: Vec<&str> = line.split(' ').collect();
            let [x, y, z, rest @ ..] = f.as_slice() else {
                continue;
            };
            if let (Ok(x), Ok(y), Ok(z), Some(b)) = (x.parse(), y.parse(), z.parse(), Bracket::parse_line(rest)) {
                map.insert([x, y, z], b);
            }
        }
        store(|s| {
            s.dims.insert(dimension.to_owned(), map);
            s.dirty.insert(dimension.to_owned());
        });
    }
    let changed: Vec<Pos> = {
        let mut guard = CHANGED.lock().unwrap_or_else(|e| e.into_inner());
        let (here, rest): (Vec<_>, Vec<_>) = guard.drain(..).partition(|(d, _)| d == dimension);
        *guard = rest;
        here.into_iter().map(|(_, p)| p).collect()
    };
    for pos in changed {
        let Some(b) = bracket_at(dimension, pos) else {
            continue;
        };
        // Only a changed state counts, and a pipe with a bracket doesn't change by
        // itself. What is still the same kind along the same axis keeps it.
        let keeps = holder(game, pos).is_some_and(|(axis, ty)| {
            ty == b.ty && rules::suitable_bracket(b.metal, axis, b.facing, ty) == Some(b)
        });
        if keeps {
            continue;
        }
        set_bracket(dimension, pos, None);
        let at = pos.map(|v| f64::from(v) + 0.5);
        game.spawn_item(ItemStack::new(b.block(), 1), at, [0.0; 3], 10);
    }
    let (send, list) = store(|s| {
        s.ticks += 1;
        let send = s.dirty.remove(dimension) || s.ticks % 100 == 0;
        (send, s.dims.get(dimension).map(|m| m.iter().map(|(p, b)| (*p, *b)).collect::<Vec<_>>()).unwrap_or_default())
    });
    if send && (!list.is_empty() || store(|s| s.ticks % 100 != 0)) {
        let mut text = String::from("B1\n");
        for (p, b) in &list {
            text.push_str(&format!("{} {} {} {}\n", p[0], p[1], p[2], b.to_line()));
        }
        for player in game.native_server_players() {
            if player.dimension == dimension
                && let Err(e) = game.native_net_send_to_player(player.uuid, CHANNEL, text.as_bytes())
            {
                log::debug!("Create: brackets: {e}");
            }
        }
    }
}

/// `create:brackets` on a client.
pub(super) fn from_server(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Some(rest) = text.strip_prefix("B1\n") else {
        return;
    };
    let mut map = HashMap::new();
    for line in rest.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        let [x, y, z, b @ ..] = f.as_slice() else {
            continue;
        };
        if let (Ok(x), Ok(y), Ok(z), Some(b)) = (x.parse(), y.parse(), z.parse(), Bracket::parse_line(b)) {
            map.insert([x, y, z], b);
        }
    }
    *CLIENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(map);
}

/// `BracketedKineticBlockModel`, `PipeAttachmentModel`: the bracket's own model.
pub(super) fn draw(game: &Game<'_>, camera: glam::DVec3, out: &mut Vec<horizon_client::mesher::Vertex>) {
    let list: Vec<(Pos, Bracket)> =
        CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or_else(Vec::new, |m| m.iter().map(|(p, b)| (*p, *b)).collect());
    for (pos, b) in list {
        let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
        if corner.distance_squared(camera) > 128.0 * 128.0 {
            continue;
        }
        let Some(base) = game.pack.assets.registry.default_state(b.block()) else {
            continue;
        };
        let along = if b.axis_along_first { "true" } else { "false" };
        let Some(state) = game.with_props(base, &[("facing", b.facing.name()), ("type", b.ty.name()), ("axis_along_first", along)]) else {
            continue;
        };
        let light = super::create_logistics::neighbour_light(game, pos);
        let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
        let seed = horizon_core::random::position_seed(horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]));
        let m = glam::Mat4::from_translation((corner - camera).as_vec3());
        game.block_model_vertices(state, seed, m, packed, false, out);
    }
}

// ---- Girders ---------------------------------------------------------------------------

fn side_of(level: &super::GameLevel<'_>, pos: Pos, d: Dir) -> Side {
    let at = add(pos, d);
    let id = level.world.block(at[0], at[1], at[2]);
    let facts = level.facts.get(id);
    let Some(s) = level.registry.state(id) else {
        return Side { air: true, no_support: true, no_shape: true, ..Side::default() };
    };
    let flag = |k: &str| s.properties.get(k).is_some_and(|v| v == "true");
    let name = s.name.as_str();
    let dimension = super::create_glue::level_dimension_id(level.dimension);
    let face = s.properties.get("face").map(String::as_str);
    let air = facts.classes.iter().any(|c| &**c == "AirBlock");
    Side {
        girder: (name == GIRDER).then(|| (flag("x"), flag("z"))),
        encased: if name == ENCASED { s.properties.get("axis").and_then(|a| Axis::parse(a)) } else { None },
        attached_towards: false,
        bracket_towards: bracket_at(dimension, at).is_some_and(|b| b.facing == d),
        wall_up: flag("up"),
        flap_display: false,
        lantern_hanging: facts.classes.iter().any(|c| &**c == "LanternBlock").then(|| flag("hanging")),
        chain_y: name.starts_with("minecraft:") && name.ends_with("chain") && s.properties.get("axis").is_some_and(|a| a == "y"),
        has_face: face.is_some(),
        face_floor: match face {
            Some("floor") => Some(true),
            Some("ceiling") => Some(false),
            _ => None,
        },
        air,
        full_face: facts.sturdy_full & (1 << d.opposite() as u8) != 0 && facts.legacy_solid,
        chute_down: super::create_logistics::is_chute_block(name) && s.properties.get("facing").is_some_and(|f| f == "down"),
        no_support: air || (facts.outline.is_empty() || facts.replaceable),
        no_shape: facts.outline.is_empty(),
    }
}

fn girder_of(level: &super::GameLevel<'_>, state: BlockStateId) -> Option<Girder> {
    let s = level.registry.state(state).filter(|s| s.name == GIRDER)?;
    let flag = |k: &str| s.properties.get(k).is_some_and(|v| v == "true");
    Some(Girder { x: flag("x"), z: flag("z"), top: flag("top"), bottom: flag("bottom"), axis: Axis::parse(s.properties.get("axis")?)? })
}

fn on(v: bool) -> &'static str {
    if v { "true" } else { "false" }
}

/// The state for `g` at `pos`, with what depends on its neighbours only for drawing.
fn state_of(level: &super::GameLevel<'_>, base: BlockStateId, pos: Pos, g: Girder) -> Option<BlockStateId> {
    let mut s = base;
    for (k, v) in [("x", g.x), ("z", g.z), ("top", g.top), ("bottom", g.bottom)] {
        s = level.native_state_with_property(s, k, on(v))?;
    }
    s = level.native_state_with_property(s, "axis", g.axis.name())?;
    for d in [Dir::North, Dir::South, Dir::West, Dir::East] {
        let braced = g.connected(d, &side_of(level, pos, d));
        s = level.native_state_with_property(s, &format!("brace_{}", d.name()), on(braced))?;
    }
    let pole = |d: Dir| !g.x && !g.z && side_of(level, pos, d).girder == Some((false, false));
    s = level.native_state_with_property(s, "pole_up", on(pole(Dir::Up)))?;
    level.native_state_with_property(s, "pole_down", on(pole(Dir::Down)))
}

/// `updateShape` of a girder: the state it should be, when different.
fn updated_girder(level: &super::GameLevel<'_>, state: BlockStateId, pos: Pos, d: Dir) -> Option<BlockStateId> {
    let g = girder_of(level, state)?;
    let next = g.update_shape(d, &|o| side_of(level, pos, o));
    state_of(level, state, pos, next).filter(|n| *n != state)
}

/// `GirderBlock.updateShape`, `GirderEncasedShaftBlock.updateShape`.
pub(super) fn update_shape(level: &super::GameLevel<'_>, state: BlockStateId, pos: Pos, d: Dir) -> Option<BlockStateId> {
    let s = level.registry.state(state)?;
    if s.name == GIRDER {
        return updated_girder(level, state, pos, d);
    }
    if s.name == ENCASED && d.axis() == Axis::Y {
        let key = if d == Dir::Up { "top" } else { "bottom" };
        let side = side_of(level, pos, d);
        let was = s.properties.get(key).is_some_and(|v| v == "true");
        let mut now = was && !side.no_support;
        now |= rules::can_attach_vertically(false, &side, d);
        return if now != was { level.native_state_with_property(state, key, on(now)) } else { None };
    }
    None
}

/// `getStateForPlacement`.
pub(super) fn placement(level: &super::GameLevel<'_>, state: BlockStateId, pos: Pos, face: Dir) -> Option<BlockStateId> {
    let g = Girder::placed(face, &|d| side_of(level, pos, d));
    state_of(level, state, pos, g)
}

fn set_state(game: &mut Game<'_>, pos: Pos, id: BlockStateId) {
    let dimension = game.dim.id().to_owned();
    game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
}

fn take_one(game: &mut Game<'_>) {
    let slot = game.held_slot();
    let stack = game.stack_in_mut(slot);
    if let Some(h) = stack.as_mut() {
        h.count -= 1;
        if h.count == 0 {
            *stack = None;
        }
    }
}

fn take_held(game: &mut Game<'_>) {
    if game.native_acting_player().is_none_or(|p| p.game_mode != 1) {
        take_one(game);
    }
}

/// `GirderPlacementHelper.getOffset` and `placeInWorld`.
fn extend(game: &mut Game<'_>, pos: Pos, hit: [f64; 3]) -> bool {
    let reg = game.pack.assets.registry;
    let can = |id: BlockStateId, d: Dir| {
        let Some(o) = reg.state(id) else {
            return false;
        };
        if o.name == GIRDER {
            rules::can_extend(o.properties.get("x").is_some_and(|v| v == "true"), o.properties.get("z").is_some_and(|v| v == "true"), d)
        } else if o.name == ENCASED {
            o.properties.get("axis").and_then(|a| Axis::parse(a)).is_some_and(|a| rules::can_extend_encased(a, d))
        } else {
            false
        }
    };
    let own = game.world.block(pos[0], pos[1], pos[2]);
    let dirs: Vec<Dir> = Dir::ALL.into_iter().filter(|d| can(own, *d)).collect();
    for d in rules::ordered_by_distance(hit, &dirs) {
        let mut at = add(pos, d);
        let mut poles = 0;
        while can(game.world.block(at[0], at[1], at[2]), d) {
            poles += 1;
            at = add(at, d);
        }
        if poles >= rules::PLACEMENT_RANGE {
            continue;
        }
        if !game.res.facts().get(game.world.block(at[0], at[1], at[2])).replaceable {
            continue;
        }
        let Some(base) = reg.default_state(GIRDER) else {
            return false;
        };
        let next = game.with_level(|_, level, _| {
            let g = Girder::extended(d.axis(), &|o| side_of(level, at, o));
            state_of(level, base, at, g)
        });
        let Some(next) = next else {
            return false;
        };
        set_state(game, at, next);
        let c = at.map(|v| f64::from(v) + 0.5);
        game.with_level(|_, level, _| level.native_play_sound("minecraft:block.netherite_block.place", c, super::create_sounds::BLOCKS, 1.0, 1.0));
        take_held(game);
        return true;
    }
    false
}

/// `GirderBlock.useItemOn`, and the placement helper on an encased one. True when
/// it took the click.
pub(super) fn right_click(game: &mut Game<'_>, pos: Pos, face: [i32; 3]) -> bool {
    let id = game.world.block(pos[0], pos[1], pos[2]);
    let Some(name) = game.pack.assets.registry.state(id).map(|s| s.name.clone()).filter(|n| n == GIRDER || n == ENCASED) else {
        return false;
    };
    if game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft) {
        return false;
    }
    let held = game.held().map(|s| s.item.to_string());
    let local = game.pick().filter(|h| h.pos == pos).map_or([0.0; 3], |h| [0, 1, 2].map(|k| h.point[k] - f64::from(pos[k]) - 0.5));
    if name == GIRDER && held.as_deref() == Some("create:shaft") {
        let next = game.with_level(|_, level, _| {
            let g = girder_of(level, id)?;
            let base = level.registry.default_state(ENCASED)?;
            let s = level.native_state_with_property(base, "axis", g.encased_axis(Dir::nearest(face)).name())?;
            let s = level.native_state_with_property(s, "top", on(g.top))?;
            level.native_state_with_property(s, "bottom", on(g.bottom))
        });
        if let Some(next) = next {
            set_state(game, pos, next);
            let c = pos.map(|v| f64::from(v) + 0.5);
            game.with_level(|_, level, _| level.native_play_sound("minecraft:block.netherite_block.hit", c, super::create_sounds::BLOCKS, 0.5, 1.25));
            take_held(game);
        }
        return true;
    }
    if held.as_deref() == Some(GIRDER) {
        return extend(game, pos, local);
    }
    false
}

/// `GirderBlock.useItemOn` with the wrench (`GirderWrenchBehavior.handleClick`) and
/// `GirderEncasedShaftBlock.onWrenched`. None for other blocks.
pub(super) fn wrench(game: &mut Game<'_>, pos: Pos) -> Option<bool> {
    let id = game.world.block(pos[0], pos[1], pos[2]);
    let s = game.pack.assets.registry.state(id)?;
    if s.name == ENCASED {
        let a = s.properties.get("axis").and_then(|a| Axis::parse(a))?;
        let flag = |k: &str| s.properties.get(k).is_some_and(|v| v == "true");
        let g = Girder::from_encased(a, flag("top"), flag("bottom"));
        let base = game.pack.assets.registry.default_state(GIRDER)?;
        let next = game.with_level(|_, level, _| state_of(level, base, pos, g))?;
        set_state(game, pos, next);
        if game.native_acting_player().is_none_or(|p| p.game_mode != 1) {
            game.give_or_drop(ItemStack::new("create:shaft", 1));
        }
        return Some(true);
    }
    if s.name != GIRDER {
        return None;
    }
    let hit = game.pick().filter(|h| h.pos == pos).map_or([0.0; 3], |h| [0, 1, 2].map(|k| h.point[k] - f64::from(pos[k]) - 0.5));
    let chosen = wrench_choice(game, pos, id, hit);
    let Some((d, action)) = chosen else {
        return Some(false);
    };
    let other_pos = add(pos, d);
    let other_id = game.world.block(other_pos[0], other_pos[1], other_pos[2]);
    let (next, other_next) = game.with_level(|_, level, _| {
        let mut g = girder_of(level, id)?;
        if d == Dir::Up {
            g.top = !g.top;
        } else {
            g.bottom = !g.bottom;
        }
        let next = state_of(level, id, pos, g.post_process())?;
        let other = if action == Action::Pair && let Some(mut o) = girder_of(level, other_id) {
            if d == Dir::Up {
                o.bottom = !o.bottom;
            } else {
                o.top = !o.top;
            }
            state_of(level, other_id, other_pos, o.post_process())
        } else {
            None
        };
        Some((next, other))
    })?;
    // setBlock with 2 | 16: no updates that would undo it.
    let dimension = game.dim.id().to_owned();
    game.native_set_block(&dimension, pos, next, 2 | 16);
    if let Some(o) = other_next {
        game.native_set_block(&dimension, other_pos, o, 2 | 16);
    }
    Some(true)
}

/// `getDirectionAndAction`.
fn wrench_choice(game: &mut Game<'_>, pos: Pos, id: BlockStateId, hit: [f64; 3]) -> Option<(Dir, Action)> {
    game.with_level(|_, level, _| {
        let g = girder_of(level, id)?;
        let valid = rules::wrench_directions(&g, &|d| side_of(level, pos, d).girder);
        let dirs: Vec<Dir> = valid.iter().map(|p| p.0).collect();
        let first = *rules::ordered_by_distance(hit, &dirs).first()?;
        valid.into_iter().find(|p| p.0 == first)
    })
}

/// `GirderWrenchBehavior.tick`: the wrench looking at a girder shows the part it
/// would change.
pub(super) fn client_tick(game: &mut Game<'_>) {
    if game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft) || game.held().is_none_or(|s| &*s.item != super::create_glue::WRENCH) {
        return;
    }
    let Some(h) = game.pick() else {
        return;
    };
    let (pos, point) = (h.pos, h.point);
    let id = game.world.block(pos[0], pos[1], pos[2]);
    let hit = [0, 1, 2].map(|k| point[k] - f64::from(pos[k]) - 0.5);
    let Some((d, action)) = wrench_choice(game, pos, id, hit) else {
        return;
    };
    let n = d.normal().map(f64::from);
    let normal = if action == Action::Pair { 4.0 } else { 1.0 };
    let c = pos.map(|v| f64::from(v) + 0.5);
    // edge = centre + n * 0.4; the box 0.3 out sideways, 0.1 (a pair 0.4) back.
    let across = |k: usize| if n[k] == 0.0 { 0.3 } else { 0.0 };
    let a = [0, 1, 2].map(|k| c[k] + n[k] * 0.4 - across(k));
    let b = [0, 1, 2].map(|k| c[k] + n[k] * 0.4 + across(k) - n[k] * 0.1 * normal);
    let bounds = [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2]), a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])];
    super::create_glue::show_outline("girderWrench", bounds, super::create_outline::Params { rgb: 0x7F_7F_7F, width: 1.0 / 32.0 });
}
