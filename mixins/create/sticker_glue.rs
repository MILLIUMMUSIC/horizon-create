// Horizon's side of the sticker (`StickerBlock`, `StickerBlockEntity`,
// `StickerRenderer`). Its grip on a structure is in contraption.rs.

use super::Game;
use super::create_kinetics::{Dir, Pos};
use horizon_client::containers::ModBlockEntity;
use std::collections::HashMap;
use std::sync::Mutex;

pub(super) const BLOCK: &str = "create:sticker";

/// `neighborChanged`, from a look at the signal each tick.
pub(super) fn tick(game: &mut Game<'_>, pos: Pos, _entity: &mut ModBlockEntity) {
    let signal = super::create_glue::signal(game, pos);
    let Some(s) = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2])) else {
        return;
    };
    let prop = |k: &str| s.properties.get(k).is_some_and(|v| v == "true");
    let (was, extended) = (prop("powered"), prop("extended"));
    let (powered, next) = super::create_contraption::sticker_state(was, extended, signal);
    if powered != was {
        game.native_set_block_property(pos, "powered", if powered { "true" } else { "false" });
    }
    if next != extended {
        game.native_set_block_property(pos, "extended", if next { "true" } else { "false" });
    }
    super::create_machines::note_view(game, pos, vec![(String::new(), 1)]);
}

/// Clients' heads: `piston` now and before.
static CLIENT: Mutex<Option<HashMap<Pos, (f32, f32)>>> = Mutex::new(None);

fn state_of(game: &Game<'_>, pos: Pos) -> Option<(Dir, bool)> {
    let s = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2]))?;
    if s.name != BLOCK {
        return None;
    }
    let facing = s.properties.get("facing").and_then(|f| Dir::parse(f))?;
    Some((facing, s.properties.get("extended").is_some_and(|v| v == "true")))
}

/// `StickerBlockEntity.tick` on a client: the head chases by 0.4 a tick, with
/// a sound when it takes hold of a block or lets go.
pub(super) fn client_tick(game: &mut Game<'_>) {
    for pos in super::create_machines::client_positions() {
        let Some((facing, extended)) = state_of(game, pos) else {
            continue;
        };
        let target = if extended { 1.0f32 } else { 0.0 };
        let (was, now) = {
            let mut guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
            let e = guard.get_or_insert_with(HashMap::new).entry(pos).or_insert((target, target));
            let was = e.1;
            e.0 = e.1;
            e.1 += (target - e.1).clamp(-0.4, 0.4);
            (was, e.1)
        };
        if was == now {
            continue;
        }
        let n = facing.normal();
        let beyond = [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]];
        let attached = !game.block_collision_shape(beyond).boxes().is_empty();
        if attached && ((now == 1.0) || (was == 1.0 && target == 0.0)) {
            let pitch = if now == 1.0 { 0.75 } else { 0.2 };
            game.native_play_local_sound("create:slime_added", pos.map(|v| f64::from(v) + 0.5), 0.35, pitch, 0);
        }
    }
}

/// `StickerRenderer`.
pub(super) fn draw(game: &Game<'_>, camera: glam::DVec3, time: f32, out: &mut Vec<horizon_client::mesher::Vertex>) {
    for pos in super::create_machines::client_positions() {
        let Some((facing, extended)) = state_of(game, pos) else {
            continue;
        };
        let (last, now) = CLIENT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|m| m.get(&pos).copied())
            .unwrap_or(if extended { (1.0, 1.0) } else { (0.0, 0.0) });
        let offset = last + (now - last) * time.fract();
        let Some(state) = game.pack.assets.registry.default_state("create:hz_part_sticker_head") else {
            continue;
        };
        let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
        let centre = glam::Vec3::splat(0.5);
        let m = glam::Mat4::from_translation((corner - camera).as_vec3())
            * glam::Mat4::from_translation(centre)
            * glam::Mat4::from_rotation_y(facing.horizontal_angle().to_radians())
            * glam::Mat4::from_rotation_x((facing.vertical_angle() + 90.0).to_radians())
            * glam::Mat4::from_translation(-centre)
            * glam::Mat4::from_translation(glam::Vec3::new(0.0, offset * offset * 4.0 / 16.0, 0.0));
        let light = super::create_logistics::neighbour_light(game, pos);
        let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
        let seed = horizon_core::random::position_seed(horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]));
        game.block_model_vertices(state, seed, m, packed, false, out);
    }
}

/// `WrenchableDirectionalBlock.getRotatedBlockState`: about the clicked face's
/// axis, staying if it points along it.
pub(super) fn wrench(game: &mut Game<'_>, pos: Pos, face: Dir) -> Option<bool> {
    let (facing, _) = state_of(game, pos)?;
    if facing.axis() != face.axis() {
        let mut f = facing;
        loop {
            f = f.clockwise(face.axis());
            if f.axis() != face.axis() {
                break;
            }
        }
        game.native_set_block_property(pos, "facing", f.name());
        let pitch = horizon_core::random::RandomSource::next_float(&mut game.world.random) + 0.5;
        super::create_glue::play_create_sound(game, "wrench_rotate", pos.map(|v| f64::from(v) + 0.5), 1.0, pitch);
    }
    Some(true)
}

/// `getStateForPlacement`: looking at it, its face to the placer, sneaking away.
pub(super) fn placement(
    level: &super::GameLevel<'_>,
    state: horizon_world::registry::BlockStateId,
    yaw: f32,
    pitch: f32,
    sneaking: bool,
) -> Option<horizon_world::registry::BlockStateId> {
    let look = super::create_placement::nearest_looking(yaw, pitch);
    let facing = if sneaking { look } else { look.opposite() };
    level.native_state_with_property(state, "facing", facing.name())
}
