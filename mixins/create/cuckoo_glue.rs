// Horizon's side of the cuckoo clock (rules in cuckoo.rs).

use super::Game;
use super::create_cuckoo::{self as rules, Animation, Clock};
use super::create_kinetics::{Axis, Dir, Pos};
use horizon_client::containers::ModBlockEntity;
use horizon_world::nbt::{JavaString, Tag};
use std::collections::HashMap;
use std::sync::Mutex;

pub(super) const CLOCK: &str = "create:cuckoo_clock";
pub(super) const MYSTERIOUS: &str = "create:mysterious_cuckoo_clock";

pub(super) fn is_clock(name: &str) -> bool {
    name == CLOCK || name == MYSTERIOUS
}

fn facing_at(game: &Game<'_>, pos: Pos) -> Option<Dir> {
    let s = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2]))?;
    if !is_clock(&s.name) {
        return None;
    }
    s.properties.get("facing").and_then(|f| Dir::parse(f))
}

fn natural(game: &Game<'_>) -> bool {
    game.dim.id() == "minecraft:overworld"
}

/// `CuckooClockBlockEntity.tick` on the server.
pub(super) fn tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let get = |k: &str| match entity.nbt.get(&JavaString::from(k)) {
        Some(Tag::Int(v)) => *v,
        _ => 0,
    };
    let mut clock = Clock { animation: Animation::of(get("Animation") as u32), progress: get("Progress") as f32 };
    let speed = super::create_glue::with_kinetics(game, |k, level| k.speed_of(level, pos));
    let is_natural = natural(game);
    if speed != 0.0 && is_natural {
        let (hours, minutes) = rules::time(game.world.day_time, true);
        let mysterious = entity.id == MYSTERIOUS;
        if clock.tick(hours, minutes, mysterious) {
            // The surprise: `destroyBlock` with no drops and a blast of 3.
            let dimension = game.dim.id().to_owned();
            if let Some(air) = game.pack.assets.registry.default_state("minecraft:air") {
                game.native_set_block(&dimension, pos, air, super::native_hooks::flags::UPDATE_ALL);
            }
            game.native_explode(&dimension, pos.map(|v| f64::from(v) + 0.5), 3.0, false, Default::default());
            return;
        }
    }
    entity.nbt.insert(JavaString::from("Animation"), Tag::Int(clock.animation.code() as i32));
    entity.nbt.insert(JavaString::from("Progress"), Tag::Int(clock.progress as i32));
    super::create_machines::note_view(game, pos, vec![(String::new(), clock.animation.code() + 1), (String::new(), (clock.progress as u32).min(255))]);
}

/// A client's clock: the hands now and before, and the last value it played.
#[derive(Clone, Copy, Default)]
struct Shown {
    hour: (f32, f32),
    minute: (f32, f32),
    played: i32,
}

static CLIENT: Mutex<Option<HashMap<Pos, Shown>>> = Mutex::new(None);

fn view(pos: Pos) -> (Animation, i32) {
    let slots = super::create_machines::client_slots(pos).unwrap_or_default();
    let code = slots.first().map_or(1, |s| s.1);
    (Animation::of(code.saturating_sub(1)), slots.get(1).map_or(0, |s| s.1 as i32))
}

/// `tick` on the window's client: the hands, the ticking, the animation's sounds.
pub(super) fn client_tick(game: &mut Game<'_>) {
    let is_natural = natural(game);
    let ticks = game.world.game_time as i64;
    for pos in super::create_machines::client_positions() {
        if facing_at(game, pos).is_none() {
            continue;
        }
        if super::create_glue::client_speed(pos).is_none_or(|s| s == 0.0) {
            continue;
        }
        let (hours, minutes) = rules::time(game.world.day_time, is_natural);
        let (hour_target, minute_target) = rules::hand_targets(hours, minutes);
        let (animation, value) = view(pos);
        let mut sounds = Vec::new();
        {
            let mut guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
            let s = guard.get_or_insert_with(HashMap::new).entry(pos).or_default();
            s.hour = (s.hour.1, rules::chase_angle(s.hour.1, hour_target));
            s.minute = (s.minute.1, rules::chase_angle(s.minute.1, minute_target));
            if animation == Animation::None {
                s.played = 0;
            } else {
                for v in (s.played + 1)..=value {
                    sounds.extend(rules::animation_sounds(animation, v));
                }
                s.played = s.played.max(value);
            }
        }
        if animation == Animation::None
            && let Some(pitch) = rules::tick_pitch(ticks, is_natural)
        {
            sounds.push(("minecraft:block.note_block.hat", 1.0 / 16.0, pitch));
        }
        let at = pos.map(|v| f64::from(v) + 0.5);
        for (sound, volume, pitch) in sounds {
            game.native_play_local_sound(sound, at, volume, pitch, 0);
        }
    }
}

/// `CuckooClockRenderer`.
pub(super) fn draw(game: &Game<'_>, camera: glam::DVec3, time: f32, out: &mut Vec<horizon_client::mesher::Vertex>) {
    for pos in super::create_machines::client_positions() {
        let Some(facing) = facing_at(game, pos) else {
            continue;
        };
        let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
        if corner.distance_squared(camera) > 64.0 * 64.0 {
            continue;
        }
        let partial = time.fract();
        let shown = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&pos).copied()).unwrap_or_default();
        let lerp = |p: (f32, f32)| p.0 + (p.1 - p.0) * partial;
        let (animation, value) = view(pos);
        let door = rules::door_angle(animation, value as f32 + partial);
        let centre = glam::Vec3::splat(0.5);
        // `rotateCentered` by the counter-clockwise side's angle.
        let side = facing.clockwise(Axis::Y).clockwise(Axis::Y).clockwise(Axis::Y);
        let turned = glam::Mat4::from_translation((corner - camera).as_vec3())
            * glam::Mat4::from_translation(centre)
            * glam::Mat4::from_rotation_y(side.horizontal_angle().to_radians())
            * glam::Mat4::from_translation(-centre);
        let about = |pivot: glam::Vec3, r: glam::Mat4| turned * glam::Mat4::from_translation(pivot) * r * glam::Mat4::from_translation(-pivot);
        let hand_pivot = glam::Vec3::new(2.0 / 16.0, 6.0 / 16.0, 0.5);
        let mut parts = vec![
            ("create:hz_part_cuckoo_hour_hand", about(hand_pivot, glam::Mat4::from_rotation_x(lerp(shown.hour).to_radians()))),
            ("create:hz_part_cuckoo_minute_hand", about(hand_pivot, glam::Mat4::from_rotation_x(lerp(shown.minute).to_radians()))),
            ("create:hz_part_cuckoo_left_door", about(glam::Vec3::new(2.0 / 16.0, 0.0, 6.0 / 16.0), glam::Mat4::from_rotation_y(-door.to_radians()))),
            ("create:hz_part_cuckoo_right_door", about(glam::Vec3::new(2.0 / 16.0, 0.0, 10.0 / 16.0), glam::Mat4::from_rotation_y(door.to_radians()))),
        ];
        if animation != Animation::None {
            let holder = if animation == Animation::Pig { "create:hz_part_cuckoo_pig" } else { "create:hz_part_cuckoo_creeper" };
            parts.push((holder, turned * glam::Mat4::from_translation(glam::Vec3::new(rules::figure_offset(door), 0.0, 0.0))));
        }
        let light = super::create_logistics::neighbour_light(game, pos);
        let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
        let seed = horizon_core::random::position_seed(horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]));
        for (holder, m) in parts {
            if let Some(state) = game.pack.assets.registry.default_state(holder) {
                game.block_model_vertices(state, seed, m, packed, false, out);
            }
        }
    }
}
