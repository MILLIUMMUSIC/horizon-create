// Horizon's side of the peculiar and haunted bells (rules in bell.rs): their block
// entities, drawing the bell swinging, and the haunted bell's soul pulses, from
// the server with Create's own particle.

use super::Game;
use super::create_bell::{self as rules, Attachment, Pulse, Pulses, Ringing, Shape};
use super::create_kinetics::{Dir, Pos};
use horizon_client::containers::ModBlockEntity;
use horizon_world::nbt::{JavaString, Tag};
use horizon_world::registry::BlockStateId;
use std::collections::HashMap;
use std::sync::Mutex;

pub(super) const PECULIAR: &str = "create:peculiar_bell";
pub(super) const HAUNTED: &str = "create:haunted_bell";

pub(super) fn is_ours(name: &str) -> bool {
    name == PECULIAR || name == HAUNTED
}

fn int(be: &ModBlockEntity, key: &str) -> i32 {
    match be.nbt.get(&JavaString::from(key)) {
        Some(Tag::Int(v)) => *v,
        _ => 0,
    }
}

fn set_int(be: &mut ModBlockEntity, key: &str, v: i32) {
    be.nbt.insert(JavaString::from(key), Tag::Int(v));
}

fn sturdy_towards(level: &super::GameLevel<'_>, pos: Pos, d: Dir) -> bool {
    let n = d.normal();
    let id = level.world.block(pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]);
    level.facts.get(id).sturdy_full & (1 << d.opposite() as u8) != 0
}

fn aim(attachment: Option<&String>, facing: Option<&String>) -> Option<(Attachment, Dir)> {
    Some((Attachment::parse(attachment?)?, Dir::parse(facing?)?))
}

/// Places turned from peculiar to haunted, for the next tick: sound and flames.
static CONVERTED: Mutex<Vec<(String, Pos)>> = Mutex::new(Vec::new());

/// `PeculiarBellBlock.tryConvert`: what the state becomes over `under`.
fn try_convert(level: &super::GameLevel<'_>, pos: Pos, state: BlockStateId, under: BlockStateId) -> BlockStateId {
    let Some(s) = level.registry.state(state).filter(|s| s.name == PECULIAR) else {
        return state;
    };
    let below = level.registry.state(under).map(|b| b.name.as_str());
    if !matches!(below, Some("minecraft:soul_fire" | "minecraft:soul_campfire")) {
        return state;
    }
    let Some(mut next) = level.registry.default_state(HAUNTED) else {
        return state;
    };
    for key in ["facing", "attachment", "powered"] {
        if let Some(v) = s.properties.get(key) {
            next = level.native_state_with_property(next, key, v).unwrap_or(next);
        }
    }
    CONVERTED.lock().unwrap_or_else(|e| e.into_inner()).push((super::create_glue::level_dimension_id(level.dimension).to_owned(), pos));
    next
}

/// `getStateForPlacement`.
pub(super) fn placement(level: &super::GameLevel<'_>, state: BlockStateId, pos: Pos, clicked: Dir, yaw: f32) -> Option<BlockStateId> {
    let horizontal = super::create_placement::horizontal(yaw);
    let (attachment, facing) = rules::placement(clicked, horizontal, |d| sturdy_towards(level, pos, d))?;
    let s = level.native_state_with_property(state, "attachment", attachment.name())?;
    let s = level.native_state_with_property(s, "facing", facing.name())?;
    Some(try_convert(level, pos, s, level.world.block(pos[0], pos[1] - 1, pos[2])))
}

/// `canSurvive`.
pub(super) fn can_survive(level: &super::GameLevel<'_>, pos: Pos, state: BlockStateId) -> Option<bool> {
    let s = level.registry.state(state)?;
    let (attachment, facing) = aim(s.properties.get("attachment"), s.properties.get("facing"))?;
    Some(rules::can_survive(attachment, facing, |d| sturdy_towards(level, pos, d)))
}

/// `updateShape`: none when it isn't one of these or nothing changes.
pub(super) fn update_shape(level: &super::GameLevel<'_>, state: BlockStateId, pos: Pos, side: Dir) -> Option<BlockStateId> {
    let s = level.registry.state(state).filter(|s| is_ours(&s.name))?;
    let (attachment, facing) = aim(s.properties.get("attachment"), s.properties.get("facing"))?;
    let next = match rules::update_shape(attachment, facing, side, |d| sturdy_towards(level, pos, d)) {
        Shape::Gone => return level.registry.default_state("minecraft:air"),
        Shape::Keep => state,
        Shape::Now(a, f) => {
            let t = level.native_state_with_property(state, "attachment", a.name())?;
            level.native_state_with_property(t, "facing", f.name())?
        }
    };
    let next = if side == Dir::Down { try_convert(level, pos, next, level.world.block(pos[0], pos[1] - 1, pos[2])) } else { next };
    (next != state).then_some(next)
}

fn load(be: &ModBlockEntity) -> Ringing {
    Ringing { ringing: int(be, "Ringing") != 0, ticks: int(be, "RingTicks") }
}

fn save(be: &mut ModBlockEntity, r: Ringing) {
    set_int(be, "Ringing", i32::from(r.ringing));
    set_int(be, "RingTicks", r.ticks);
}

/// `ring` on the block entity and block: its sound, a haunted bell's pulse.
/// True when it rang.
fn ring(game: &mut Game<'_>, pos: Pos, name: &str, be: &mut ModBlockEntity, direction: Dir) -> bool {
    let mut r = load(be);
    if name == HAUNTED {
        if !r.haunted_can_ring() {
            return false;
        }
        send_pulse(game, pos, rules::BELL_DISTANCE);
        set_int(be, "EffectTicks", rules::EFFECT_TICKS);
    }
    r.ring();
    save(be, r);
    set_int(be, "RingDir", direction as i32);
    set_int(be, "Rings", (int(be, "Rings") + 1) % 250);
    let at = pos.map(|v| f64::from(v) + 0.5);
    if name == HAUNTED {
        super::create_glue::play_create_sound(game, "haunted_bell_use", at, 4.0, 1.0);
    } else {
        // playExisting(SoundEvents.BELL_BLOCK).
        game.with_level(|_, level, _| level.native_play_sound("minecraft:block.bell.use", at, super::create_sounds::BLOCKS, 2.0, 0.94));
    }
    true
}

/// `BellBlock.useWithoutItem`, through `onHit`. True when it rang.
pub(super) fn right_click(game: &mut Game<'_>, pos: Pos, face: [i32; 3]) -> bool {
    let Some(s) = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2])).filter(|s| is_ours(&s.name)) else {
        return false;
    };
    let name = s.name.clone();
    let Some((attachment, facing)) = aim(s.properties.get("attachment"), s.properties.get("facing")) else {
        return false;
    };
    let hit = Dir::nearest(face);
    let height = game.pick().filter(|h| h.pos == pos).map_or(0.5, |h| h.point[1] - f64::from(pos[1]));
    if !rules::can_ring_from(attachment, facing, hit, height) {
        return false;
    }
    // The block entity comes out of the world for the ring: `ring` wants both.
    let key = (pos[0], pos[1], pos[2]);
    let Some(horizon_client::containers::BlockEntity::Mod(mut be)) = game.world.block_entities.remove(&key) else {
        return false;
    };
    let rang = ring(game, pos, &name, &mut be, hit);
    game.world.block_entities.insert(key, horizon_client::containers::BlockEntity::Mod(be));
    rang
}

/// `tick`, with `neighborChanged` from a look at the signal.
pub(super) fn tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let Some(s) = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2])) else {
        return;
    };
    let Some((attachment, facing)) = aim(s.properties.get("attachment"), s.properties.get("facing")) else {
        return;
    };
    let was = s.properties.get("powered").is_some_and(|v| v == "true");
    let name = entity.id.clone();
    let signal = super::create_glue::signal(game, pos);
    if signal != was {
        game.native_set_block_property(pos, "powered", if signal { "true" } else { "false" });
        if signal {
            ring(game, pos, &name, entity, rules::powered_ring_direction(attachment, facing));
        }
    }
    let mut r = load(entity);
    r.tick();
    save(entity, r);
    // HauntedBellBlock.onPlace and the block entity's effect.
    if name == HAUNTED {
        if int(entity, "Placed") == 0 {
            set_int(entity, "Placed", 1);
            set_int(entity, "EffectTicks", rules::EFFECT_TICKS);
        }
        let effect = int(entity, "EffectTicks");
        if effect > 0 {
            set_int(entity, "EffectTicks", effect - 1);
            haunted_effect(game, pos);
        }
    }
    let view = vec![(String::new(), int(entity, "Rings") as u32 + 1), (String::new(), int(entity, "RingDir") as u32)];
    super::create_machines::note_view(game, pos, view);
}

fn next_f(game: &mut Game<'_>) -> f64 {
    f64::from(horizon_core::random::RandomSource::next_float(&mut game.world.random))
}

/// `HauntedBellBlockEntity.tick`'s souls and their sound, a quarter of the ticks.
fn haunted_effect(game: &mut Game<'_>, pos: Pos) {
    if next_f(game) > 0.25 {
        return;
    }
    let at = [f64::from(pos[0]) + next_f(game), f64::from(pos[1]) + 0.5, f64::from(pos[2]) + next_f(game)];
    let options = horizon_client::mod_api::ParticleOptions::simple(horizon_client::mod_api::ParticleType::Soul);
    let vol = if (next_f(game) * 0.4 + next_f(game)) as f32 > 0.9 { 0.6 } else { 0.0 };
    let pitch = 0.6 + next_f(game) as f32 * 0.4;
    let centre = pos.map(|v| f64::from(v) + 0.5);
    game.with_level(|_, level, _| {
        level.native_send_particles(&options, None, at, [0.02, 0.0, 0.02], 0.1, 1);
        level.native_play_sound("minecraft:particle.soul_escape", centre, super::create_sounds::BLOCKS, vol, pitch);
    });
}

// ---- Soul pulses --------------------------------------------------------------------

#[derive(Default)]
struct PulseState {
    dims: HashMap<String, Pulses>,
    /// `WARMUP`: left and the ticks since the bell was held.
    warmup: Option<(i32, u32)>,
}

static PULSES: Mutex<Option<PulseState>> = Mutex::new(None);

fn pulses<R>(f: impl FnOnce(&mut PulseState) -> R) -> R {
    let mut guard = PULSES.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(PulseState::default))
}

/// `sendPulse`, and on this side the handler's `addPulse`.
fn send_pulse(game: &Game<'_>, pos: Pos, distance: i32) {
    let dimension = game.dim.id().to_owned();
    pulses(|p| p.dims.entry(dimension).or_default().add(Pulse::new(pos, distance)));
}

/// `SoulPulseEffect.canSpawnSoulAt`, as `ON_GROUND.isSpawnPositionOk` for a
/// zombie: a sturdy top below, room for it here and above, ignoring light.
fn can_spawn_soul_at(game: &Game<'_>, at: Pos) -> bool {
    let facts = game.res.facts();
    let [x, y, z] = at;
    let below = facts.get(game.world.block(x, y - 1, z));
    if below.sturdy_full & (1 << Dir::Up as u8) == 0 || below.emission >= 14 {
        return false;
    }
    [y, y + 1].iter().all(|&y| {
        let f = facts.get(game.world.block(x, y, z));
        !f.full_collision && f.fluid_level == 0
    }) && game.block_collision_shape(at).boxes().is_empty()
}

/// A world opened or closed.
pub(super) fn reset() {
    *PULSES.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The server's tick: converted bells, a held bell's pulses, souls.
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str) {
    let converted: Vec<Pos> = {
        let mut guard = CONVERTED.lock().unwrap_or_else(|e| e.into_inner());
        let (here, rest): (Vec<_>, Vec<_>) = guard.drain(..).partition(|(d, _)| d == dimension);
        *guard = rest;
        here.into_iter().map(|(_, p)| p).collect()
    };
    for pos in converted {
        let at = pos.map(|v| f64::from(v) + 0.5);
        super::create_glue::play_create_sound(game, "haunted_bell_convert", at, 1.0, 1.0);
        // spawnConversionParticles.
        let count = (next_f(game) * 10.0) as i32 + 15;
        let options = horizon_client::mod_api::ParticleOptions::simple(horizon_client::mod_api::ParticleType::SoulFireFlame);
        game.with_level(|_, level, _| {
            level.native_send_particles(&options, None, at, [0.0; 3], 0.2, count);
        });
    }
    // hauntedBellCreatesPulse, for the window's player.
    let holding = game.held().is_some_and(|s| &*s.item == HAUNTED) || game.offhand.as_ref().is_some_and(|s| &*s.item == HAUNTED);
    let time = game.world.game_time as u64;
    let pulse_now = pulses(|p| {
        if holding {
            let w = p.warmup.get_or_insert((rules::WARMUP_TICKS, 0));
            w.1 = 0;
            rules::held_pulse(&mut w.0, time)
        } else {
            // The cache lets go 250 ms after its last use.
            if let Some(w) = p.warmup.as_mut() {
                w.1 += 1;
                if w.1 >= 5 {
                    p.warmup = None;
                }
            }
            false
        }
    });
    if pulse_now && let Some(player) = game.native_server_players().first() {
        send_pulse(game, player.position.map(|v| v.floor() as i32), rules::HELD_DISTANCE);
    }
    let mut taken = pulses(|p| p.dims.remove(dimension));
    if let Some(all) = taken.as_mut() {
        let souls = all.tick(&mut |p| can_spawn_soul_at(game, p));
        for at in souls {
            // spawnParticles: where it is dark, as there mobs could spawn.
            if game.world.light(at[0], at[1], at[2]) & 15 >= 1 {
                continue;
            }
            let c = [f64::from(at[0]) + 0.5, f64::from(at[1]) + 0.5, f64::from(at[2]) + 0.5];
            let options = horizon_client::mod_api::ParticleOptions::named("create:soul");
            game.with_level(|_, level, _| {
                level.native_send_particles(&options, None, c, [0.0; 3], 0.0, 1);
            });
        }
        let all = taken.take().unwrap_or_default();
        pulses(|p| {
            p.dims.insert(dimension.to_owned(), all);
        });
    }
}

// ---- Drawing --------------------------------------------------------------------------

/// Clients' bells: the rings seen, ticks since the last and its direction.
static CLIENT: Mutex<Option<HashMap<Pos, (u32, Option<(f32, Dir)>)>>> = Mutex::new(None);

/// The window's client: bells swing for `RING_DURATION` from each ring seen.
pub(super) fn client_tick(game: &Game<'_>) {
    let mut guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    for pos in super::create_machines::client_positions() {
        if !game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2])).is_some_and(|s| is_ours(&s.name)) {
            continue;
        }
        let Some(slots) = super::create_machines::client_slots(pos) else {
            continue;
        };
        let (Some(rings), Some(dir)) = (slots.first().map(|s| s.1), slots.get(1).map(|s| s.1)) else {
            continue;
        };
        let e = map.entry(pos).or_insert((rings, None));
        if e.0 != rings {
            e.0 = rings;
            e.1 = Some((0.0, Dir::ALL[(dir as usize).min(5)]));
        } else if let Some((ticks, _)) = e.1.as_mut() {
            *ticks += 1.0;
            if *ticks >= rules::RING_DURATION as f32 {
                e.1 = None;
            }
        }
    }
}

/// `BellRenderer`.
pub(super) fn draw(game: &Game<'_>, camera: glam::DVec3, time: f32, out: &mut Vec<horizon_client::mesher::Vertex>) {
    for pos in super::create_machines::client_positions() {
        let Some(s) = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2])).filter(|s| is_ours(&s.name)) else {
            continue;
        };
        let Some((attachment, facing)) = aim(s.properties.get("attachment"), s.properties.get("facing")) else {
            continue;
        };
        let holder = if s.name == HAUNTED { "create:hz_part_haunted_bell" } else { "create:hz_part_peculiar_bell" };
        let Some(state) = game.pack.assets.registry.default_state(holder) else {
            continue;
        };
        let ring = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&pos).and_then(|e| e.1));
        let centre = glam::Vec3::splat(0.5);
        let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
        let swing = ring.map_or(glam::Mat4::IDENTITY, |(ticks, d)| {
            // rotateCentered about ringDirection.getCounterClockWise().
            let axis = d.clockwise(super::create_kinetics::Axis::Y).opposite().normal().map(|v| v as f32);
            glam::Mat4::from_axis_angle(glam::Vec3::from(axis), rules::swing_angle(ticks + time.fract()))
        });
        let wall = matches!(attachment, Attachment::SingleWall | Attachment::DoubleWall);
        let turn = facing.horizontal_angle() + if wall { 90.0 } else { 0.0 };
        let m = glam::Mat4::from_translation((corner - camera).as_vec3())
            * glam::Mat4::from_translation(centre)
            * swing
            * glam::Mat4::from_rotation_y(turn.to_radians())
            * glam::Mat4::from_translation(-centre);
        let light = super::create_logistics::neighbour_light(game, pos);
        let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
        let seed = horizon_core::random::position_seed(horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]));
        game.block_model_vertices(state, seed, m, packed, false, out);
    }
}
