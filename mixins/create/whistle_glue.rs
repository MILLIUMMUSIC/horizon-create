// Horizon's side of the steam whistle (rules in whistle.rs).

use super::Game;
use super::create_kinetics::{Dir, Pos};
use super::create_whistle::{self as rules, Above, Grow, Shape, Size};
use horizon_client::containers::ModBlockEntity;
use horizon_world::nbt::{JavaString, Tag};
use std::collections::HashMap;
use std::sync::Mutex;

pub(super) const WHISTLE: &str = "create:steam_whistle";
pub(super) const EXTENSION: &str = "create:steam_whistle_extension";

fn state_at<'a>(game: &'a Game<'_>, pos: Pos) -> Option<&'a horizon_world::chunk::BlockState> {
    game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2]))
}

fn up(pos: Pos, n: i32) -> Pos {
    [pos[0], pos[1] + n, pos[2]]
}

/// A whistle's `SIZE`, `FACING`, `WALL`.
fn whistle_of(s: &horizon_world::chunk::BlockState) -> Option<(Size, Dir, bool)> {
    if s.name != WHISTLE {
        return None;
    }
    let size = s.properties.get("size").and_then(|v| Size::parse(v))?;
    let facing = s.properties.get("facing").and_then(|v| Dir::parse(v))?;
    Some((size, facing, s.properties.get("wall").is_some_and(|v| v == "true")))
}

fn shape_of(s: &horizon_world::chunk::BlockState) -> Option<Shape> {
    if s.name != EXTENSION {
        return None;
    }
    s.properties.get("shape").and_then(|v| Shape::parse(v))
}

/// `WhistleBlock.getAttachedDirection` of the whistle at `pos`.
pub(super) fn attached_at(game: &Game<'_>, pos: Pos) -> Option<Dir> {
    state_at(game, pos).and_then(whistle_of).map(|(_, facing, wall)| rules::attached(wall, facing))
}

/// `getStateForPlacement`: only on a tank.
pub(super) fn placement(
    level: &super::GameLevel<'_>,
    state: horizon_world::registry::BlockStateId,
    pos: Pos,
    clicked: Dir,
    yaw: f32,
) -> Option<horizon_world::registry::BlockStateId> {
    let (wall, facing) = rules::placement(clicked, super::create_placement::horizontal(yaw).opposite());
    let d = rules::attached(wall, facing).normal();
    let tank = level.world.block(pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]);
    if !level.registry.state(tank).is_some_and(|s| super::create_fluid_glue::is_tank_block(&s.name)) {
        return None;
    }
    let s = level.native_state_with_property(state, "facing", facing.name())?;
    level.native_state_with_property(s, "wall", if wall { "true" } else { "false" })
}

/// `updateShape` of a whistle and an extension. `Some(None)` when it goes.
pub(super) fn update_shape(
    level: &super::GameLevel<'_>,
    state: horizon_world::registry::BlockStateId,
    pos: Pos,
    direction: Dir,
) -> Option<Option<horizon_world::registry::BlockStateId>> {
    let s = level.registry.state(state)?;
    let block = |p: Pos| level.registry.state(level.world.block(p[0], p[1], p[2]));
    if let Some((_, facing, wall)) = whistle_of(s) {
        let attached = rules::attached(wall, facing);
        let d = attached.normal();
        let tank = block([pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]]);
        if attached == direction && !tank.is_some_and(|t| super::create_fluid_glue::is_tank_block(&t.name)) {
            return Some(None);
        }
        return None;
    }
    let shape = shape_of(s)?;
    match direction {
        Dir::Up => {
            let above = block(up(pos, 1)).is_some_and(|a| a.name == EXTENSION);
            let next = rules::shape_with_above(shape, above);
            (next != shape).then(|| level.native_state_with_property(state, "shape", next.name()))
        }
        Dir::Down => {
            let below = block(up(pos, -1));
            let survives = below.is_some_and(|b| b.name == WHISTLE || shape_of(b).is_some_and(|sh| sh != Shape::Single));
            if !survives {
                return Some(None);
            }
            let size = below.and_then(|b| b.properties.get("size")).map(String::as_str)?;
            Some(level.native_state_with_property(state, "size", size))
        }
        _ => None,
    }
}

/// `WhistleExtenderBlock.findRoot`.
fn root(game: &Game<'_>, pos: Pos) -> Pos {
    let mut p = up(pos, -1);
    while state_at(game, p).is_some_and(|s| s.name == EXTENSION) {
        p = up(p, -1);
    }
    p
}

/// `WhistleBlock.useItemOn` and the extender's: a whistle in hand grows it.
pub(super) fn click(game: &mut Game<'_>, pos: Pos) -> bool {
    if game.held().is_none_or(|s| &*s.item != WHISTLE) {
        return false;
    }
    let base = if state_at(game, pos).is_some_and(|s| s.name == EXTENSION) { root(game, pos) } else { pos };
    let Some((size, ..)) = state_at(game, base).and_then(whistle_of) else {
        return false;
    };
    let grown = rules::grow(|i| {
        let p = up(base, i);
        match state_at(game, p).and_then(shape_of) {
            Some(shape) => Above::Extension(shape),
            None if game.res.facts().get(game.world.block(p[0], p[1], p[2])).replaceable => Above::Free,
            None => Above::Blocked,
        }
    });
    let (at, shape) = match grown {
        Grow::Double(i) => (up(base, i), Shape::Double),
        Grow::Place(i) => (up(base, i), Shape::Single),
        Grow::Nothing => return true,
    };
    let dimension = game.dim.id().to_owned();
    let Some(extension) = game.pack.assets.registry.default_state(EXTENSION) else {
        return true;
    };
    let id = game.with_props(extension, &[("shape", shape.name()), ("size", size.name())]);
    if let Some(id) = id {
        game.native_set_block(&dimension, at, id, super::native_hooks::flags::UPDATE_ALL);
    }
    let pitch = rules::grow_pitch(grown);
    let sound_at = at.map(|v| f64::from(v) + 0.5);
    game.with_level(|_, level, _| {
        level.native_play_sound("minecraft:block.note_block.xylophone", sound_at, super::create_sounds::BLOCKS, 0.25, pitch);
        level.native_play_sound("minecraft:block.copper.hit", sound_at, super::create_sounds::BLOCKS, 1.0, pitch);
    });
    true
}

/// `getRotatedBlockState`: the wrench cycles its size, from an extension too.
pub(super) fn wrench(game: &mut Game<'_>, pos: Pos) -> Option<bool> {
    let name = state_at(game, pos)?.name.clone();
    if name != WHISTLE && name != EXTENSION {
        return None;
    }
    let base = if name == EXTENSION { root(game, pos) } else { pos };
    let (size, ..) = state_at(game, base).and_then(whistle_of)?;
    game.native_set_block_property(base, "size", size.next().name());
    let pitch = horizon_core::random::RandomSource::next_float(&mut game.world.random) + 0.5;
    super::create_glue::play_create_sound(game, "wrench_rotate", pos.map(|v| f64::from(v) + 0.5), 1.0, pitch);
    Some(true)
}

/// `WhistleExtenderBlock.onSneakWrenched`: the top half of a double one comes
/// off; `true` when that is what happened.
pub(super) fn sneak_wrench(game: &mut Game<'_>, pos: Pos) -> bool {
    let Some(shape) = state_at(game, pos).and_then(shape_of) else {
        return false;
    };
    let click_y = game.pick().filter(|h| h.pos == pos).map_or(0.0, |h| h.point[1] - f64::from(pos[1]));
    if click_y < 0.5 || shape == Shape::Single {
        return false;
    }
    game.native_set_block_property(pos, "shape", Shape::Single.name());
    let pitch = horizon_core::random::RandomSource::next_float(&mut game.world.random) * 0.5 + 0.5;
    super::create_glue::play_create_sound(game, "wrench_remove", pos.map(|v| f64::from(v) + 0.5), 1.0, pitch);
    true
}

/// `WhistleBlockEntity.tick` on the server, with `neighborChanged`'s `POWERED`
/// and `updatePitch`.
pub(super) fn tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let powered = super::create_glue::signal(game, pos);
    if state_at(game, pos).and_then(|s| s.properties.get("powered")).is_some_and(|v| (v == "true") != powered) {
        game.native_set_block_property(pos, "powered", if powered { "true" } else { "false" });
    }
    let pitch = rules::pitch(|i| state_at(game, up(pos, i)).and_then(shape_of));
    entity.nbt.insert(JavaString::from("Pitch"), Tag::Int(pitch));
    super::create_machines::note_view(game, pos, vec![(String::new(), pitch as u32 + 1)]);
}

/// The client's whistles: the mouth now and before, the ticks to the sound's next
/// start (none when silent).
struct Blowing {
    last: f32,
    now: f32,
    sound: Option<i32>,
    size: Size,
}

static CLIENT: Mutex<Option<HashMap<Pos, Blowing>>> = Mutex::new(None);

/// `tick` and `tickAudio` on a client. Horizon has no sound instance to hold
/// looping and change: the sound (3 seconds, by its pitch) starts again as it ends.
pub(super) fn client_tick(game: &mut Game<'_>) {
    let dimension = super::create_glue::dim_code_of(game);
    for pos in super::create_machines::client_positions() {
        let Some((size, facing, wall)) = state_at(game, pos).and_then(whistle_of) else {
            CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_mut().map(|m| m.remove(&pos));
            continue;
        };
        let d = rules::attached(wall, facing).normal();
        let tank = [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
        let heated = super::create_fluid_glue::client_boiler(dimension, tank).is_some_and(|(b, _)| b.is_active() && (b.passive_heat || b.active_heat > 0));
        let powered = heated && state_at(game, pos).and_then(|s| s.properties.get("powered")).is_some_and(|v| v == "true");
        let pitch = super::create_machines::client_slots(pos).and_then(|s| s.first().map(|v| v.1 as i32 - 1)).unwrap_or(0);
        let f = rules::sound_pitch(pitch);
        let at = pos.map(|v| f64::from(v) + 0.5);
        let (start, chiff) = {
            let mut guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
            let map = guard.get_or_insert_with(HashMap::new);
            let w = map.entry(pos).or_insert(Blowing { last: 0.0, now: 0.0, sound: None, size });
            w.last = w.now;
            w.now = rules::animate(w.now, powered);
            if !powered {
                w.sound = None;
                (false, false)
            } else {
                let changed = w.size != size;
                w.size = size;
                match w.sound {
                    Some(left) if left > 0 && !changed => {
                        w.sound = Some(left - 1);
                        (false, false)
                    }
                    was => {
                        w.sound = Some((60.0 / f) as i32 - 1);
                        (true, was.is_none() || changed)
                    }
                }
            }
        };
        if start {
            game.native_play_local_sound(size.sound(), at, 1.0, f, 0);
        }
        if chiff {
            let camera = game.player.as_ref().map_or([at[0], at[1], at[2]], |p| [p.position.x, p.position.y, p.position.z]);
            let distance = ((camera[0] - at[0]).powi(2) + (camera[1] - at[1]).powi(2) + (camera[2] - at[2]).powi(2)).sqrt();
            let max_volume = (((64.0 - distance) / 64.0) as f32).clamp(0.0, 1.0);
            let pitch = if size == Size::Small { f + 0.75 } else { f };
            game.native_play_local_sound("create:chiff", at, max_volume * 0.175, pitch, 0);
        }
    }
}

/// `addToGoggleTooltip`: the note of the whistle at `pos`.
pub(super) fn goggle_note(game: &Game<'_>, pos: Pos) -> Option<&'static str> {
    state_at(game, pos).and_then(whistle_of)?;
    let pitch = super::create_machines::client_slots(pos)?.first()?.1 as i32 - 1;
    Some(rules::note(pitch))
}

/// `WhistleRenderer`.
pub(super) fn draw(game: &Game<'_>, camera: glam::DVec3, time: f32, out: &mut Vec<horizon_client::mesher::Vertex>) {
    let shown: Vec<(Pos, f32, bool)> = {
        let guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
        let Some(map) = guard.as_ref() else {
            return;
        };
        map.iter().map(|(p, w)| (*p, w.last + (w.now - w.last) * time.fract(), w.sound.is_some())).collect()
    };
    for (pos, value, blowing) in shown {
        let Some((size, facing, _)) = state_at(game, pos).and_then(whistle_of) else {
            continue;
        };
        let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
        if corner.distance_squared(camera) > 128.0 * 128.0 {
            continue;
        }
        let holder = match size {
            Size::Small => "create:hz_part_whistle_mouth_small",
            Size::Medium => "create:hz_part_whistle_mouth_medium",
            Size::Large => "create:hz_part_whistle_mouth_large",
        };
        let Some(state) = game.pack.assets.registry.default_state(holder) else {
            continue;
        };
        let centre = glam::Vec3::splat(0.5);
        let m = glam::Mat4::from_translation((corner - camera).as_vec3())
            * glam::Mat4::from_translation(centre)
            * glam::Mat4::from_rotation_y(facing.horizontal_angle().to_radians())
            * glam::Mat4::from_translation(-centre)
            * glam::Mat4::from_translation(glam::Vec3::new(0.0, rules::mouth_offset(value, blowing, time, size), 0.0));
        let light = super::create_logistics::neighbour_light(game, pos);
        let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
        let seed = horizon_core::random::position_seed(horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]));
        game.block_model_vertices(state, seed, m, packed, false, out);
    }
}
