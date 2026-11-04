// Horizon's side of Create, injected as `crate::game::create_glue`. The
// server keeps each dimension's kinetics (`create_kinetics`) and tells its
// players what turns on the `create:kinetics` channel; clients draw the
// rotating parts each frame, as Create's KineticBlockEntityRenderer does.
// Everything Horizon-specific lives here; the rules are in kinetics.rs.

use super::Game;
use super::create_kinetics::{
    Axis, ClientView, Dir, ENTRIES_PER_MESSAGE, Kind, Kinetics, Level, Pos, View,
    encode_update, gearbox_shaft_angle, hits_motor_value_box, motor_scroll_step, render_angle,
};
use super::create_belt::{BeltState, belt_partials, belt_scroll};
use super::create_processing::{
    MixStep, PressMode, PressStep, mixer_head_offset, mixer_head_speed, press_head_offset,
};
use horizon_world::registry::BlockStateId;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

const CHANNEL: &str = "create:kinetics";
/// A player's new scroll value for a block (`ValueSettingsPacket`).
const VALUES: &str = "create:values";
/// `BlockEntityConfigurationPacket`'s reach.
const VALUE_RANGE: f64 = 20.0;
pub(super) const WRENCH: &str = "create:wrench";
/// Ticks between full updates, so a player who just came sees everything.
const SNAPSHOT_EVERY: u64 = 100;
/// Ticks between saves of the kinetics beside the world.
const SAVE_EVERY: u64 = 1200;
/// `BlockEntityRenderer.getViewDistance` default.
const VIEW_DISTANCE: f64 = 64.0;
/// The creative motor's speeds, in the order a right click steps through
/// them (Create sets it with a scroll value; a menu for it comes later).
const MOTOR_STEPS: [i32; 10] = [16, 32, 64, 128, 256, -256, -128, -64, -32, -16];

#[derive(Default)]
struct Server {
    dims: HashMap<String, Kinetics>,
    loaded: HashSet<String>,
    /// Block changes heard since their level's last tick end.
    pending: Vec<(String, Pos)>,
    /// Blocks the kinetics broke, for the next tick end (with drops).
    destroy: Vec<(String, Pos)>,
    /// Every block state of ours, to skip everyone else's changes fast.
    states: HashSet<u32>,
    ticks: u64,
    root: Option<PathBuf>,
    /// Scroll values players set, for their level's tick: dimension,
    /// position, value.
    values: Vec<(String, Pos, i32)>,
}

#[derive(Default)]
struct Client {
    view: ClientView,
    /// Model holder block states by block and property value.
    parts: HashMap<(&'static str, &'static str), Option<BlockStateId>>,
    /// The small water wheel's OBJ model, read once (none: it isn't there).
    wheel: Option<Option<Vec<super::create_obj::Quad>>>,
    valve_handles: HashMap<u8, Option<Vec<super::create_obj::Quad>>>,
    /// The large water wheel's.
    large_wheel: Option<Option<Vec<super::create_obj::Quad>>>,
    large_wheel_extension: Option<Option<Vec<super::create_obj::Quad>>>,
}

static SERVER: Mutex<Option<Server>> = Mutex::new(None);
static CLIENT: Mutex<Option<Client>> = Mutex::new(None);

fn server() -> MutexGuard<'static, Option<Server>> {
    SERVER.lock().unwrap_or_else(|e| e.into_inner())
}

fn client() -> MutexGuard<'static, Option<Client>> {
    CLIENT.lock().unwrap_or_else(|e| e.into_inner())
}

/// The speed this client was last told the kinetic block at `pos` turns at.
/// The fans this client was told of in its dimension: position, facing,
/// speed; and the dimension's code.
pub(super) fn client_fans(game: &Game<'_>) -> (u8, Vec<(Pos, Dir, f32)>) {
    let dim = dim_code(game.dim.id());
    let guard = client();
    let fans = guard
        .as_ref()
        .filter(|c| c.view.dim == dim)
        .map(|c| {
            c.view
                .blocks
                .iter()
                .filter_map(|(p, v)| match v.kind {
                    Kind::EncasedFan(f) => Some((*p, f, v.speed)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    (dim, fans)
}

pub(super) fn client_speed(pos: Pos) -> Option<f32> {
    client().as_ref()?.view.blocks.get(&pos).map(|v| v.speed)
}

fn dim_code(id: &str) -> u8 {
    match id {
        "minecraft:the_nether" => 1,
        "minecraft:the_end" => 2,
        _ => 0,
    }
}

fn level_dimension_id(d: horizon_world::level::Dimension) -> &'static str {
    match d {
        horizon_world::level::Dimension::Overworld => "minecraft:overworld",
        horizon_world::level::Dimension::Nether => "minecraft:the_nether",
        horizon_world::level::Dimension::End => "minecraft:the_end",
    }
}

/// Our rotating block at `pos` in the level in place, if loaded.
/// `Level.hasNeighborSignal`: whether a neighbour gives `pos` a signal.
pub(super) fn signal(game: &mut Game<'_>, pos: Pos) -> bool {
    let at = horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]);
    game.with_level(|b, level, ticks| b.has_neighbor_signal(level, ticks, at))
}

pub(super) fn kind_at(game: &Game<'_>, pos: Pos) -> Option<Kind> {
    let [x, y, z] = pos;
    let world = &game.world;
    if !world.loaded(x, z) || y < world.shape.min_y || y >= world.shape.max_y() {
        return None;
    }
    let state = game.pack.assets.registry.state(world.block(x, y, z))?;
    if !state.name.starts_with("create:") {
        return None;
    }
    Kind::from_block(&state.name, |k| state.properties.get(k).map(String::as_str))
}

fn our_states(game: &Game<'_>) -> HashSet<u32> {
    let registry = game.pack.assets.registry;
    (0..registry.len() as u32)
        .filter(|&i| {
            registry.state(BlockStateId(i)).is_some_and(|s| {
                s.name.starts_with("create:")
                    && (Kind::from_block(&s.name, |k| s.properties.get(k).map(String::as_str))
                        .is_some()
                        || super::create_logistics::is_ours(&s.name)
                        || super::create_fluid_glue::is_ours(&s.name)
                        || s.name == "create:redstone_contact")
            })
        })
        .collect()
}

/// The level in place, as the kinetics see it.
struct InPlace<'a, 'g> {
    game: &'a Game<'g>,
    destroyed: Vec<Pos>,
}

impl Level for InPlace<'_, '_> {
    fn kind(&self, pos: Pos) -> Option<Kind> {
        kind_at(self.game, pos)
    }
    fn is_loaded(&self, pos: Pos) -> bool {
        self.game.world.loaded(pos[0], pos[2])
    }
    fn destroy_block(&mut self, pos: Pos) {
        self.destroyed.push(pos);
    }
}

fn save_path(root: &std::path::Path, dimension: &str) -> PathBuf {
    root.join("data")
        .join(format!("create_kinetics_{}.txt", dimension.replace(':', "_")))
}

fn save_all(s: &Server) {
    let Some(root) = &s.root else {
        return;
    };
    let _ = std::fs::create_dir_all(root.join("data"));
    for (dimension, kinetics) in &s.dims {
        if let Err(e) = std::fs::write(save_path(root, dimension), kinetics.save()) {
            log::warn!("Create: can't save the kinetics of {dimension}: {e}");
        }
    }
}

// ---- Server -------------------------------------------------------------

/// `GameLevel::native_block_changed`: a change of one of our blocks is
/// handled at its level's tick end.
pub(super) fn block_changed(
    dimension: horizon_world::level::Dimension,
    pos: horizon_core::position::BlockPos,
    old: BlockStateId,
    new: BlockStateId,
) {
    let mut guard = server();
    let Some(s) = guard.as_mut() else {
        return;
    };
    let ours = s.states.contains(&old.0) || s.states.contains(&new.0);
    super::create_fluid_glue::block_changed(level_dimension_id(dimension), [pos.x, pos.y, pos.z], ours);
    if ours {
        s.pending
            .push((level_dimension_id(dimension).to_owned(), [pos.x, pos.y, pos.z]));
    }
}

/// `native_place_block`: a player placed a block in their dimension.
pub(super) fn placed(game: &mut Game<'_>, pos: [i32; 3]) {
    super::create_contraptions::glue_in_off_hand(game, pos);
    super::create_logistics::placed(game, pos);
    super::create_arms::placed(game, pos);
    super::create_ejector_glue::placed(game, pos);
    if kind_at(game, pos).is_none() {
        return;
    }
    let mut guard = server();
    let s = guard.get_or_insert_with(Server::default);
    s.pending.push((game.dim.id().to_owned(), pos));
}

/// `native_level_tick_post`: the kinetics' tick for that level, then what
/// changed goes to its players.
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str) {
    let mut messages: Vec<Vec<u8>> = Vec::new();
    let mut destroy: Vec<Pos> = Vec::new();
    let mut press_steps = Vec::new();
    let mut mix_steps = Vec::new();
    let fans;
    let full_update;
    let mut belts: Vec<(Pos, BeltState, f32)> = Vec::new();
    let mut removed_belts: Vec<(Pos, BeltState)> = Vec::new();
    let mut changed_here: Vec<Pos> = Vec::new();
    let mut pumps: Vec<(Pos, Dir, f32)> = Vec::new();
    let mut drills: Vec<(Pos, Dir, f32, bool)> = Vec::new();
    let mut large_wheels: Vec<(Pos, Axis)> = Vec::new();
    let mut split_shafts: Vec<(Pos, Kind)> = Vec::new();
    let mut crushing_wheels: Vec<(Pos, Axis, f32)> = Vec::new();
    let mut chain_gearshifts: Vec<(Pos, Kind)> = Vec::new();
    let mut valves: Vec<(Pos, f32)> = Vec::new();
    let mut powered_shafts: Vec<(Pos, Axis, Option<Pos>)> = Vec::new();
    let mut turntables: Vec<(Pos, f32)> = Vec::new();
    let mut amounts: Vec<(Pos, i32)> = Vec::new();
    let save_now;
    {
        let mut guard = server();
        let s = guard.get_or_insert_with(Server::default);
        if s.states.is_empty() {
            s.states = our_states(game);
        }
        if s.root.is_none() {
            s.root = Some(game.save.root.clone());
        }
        if s.loaded.insert(dimension.to_owned()) {
            let saved = s
                .root
                .as_ref()
                .and_then(|r| std::fs::read_to_string(save_path(r, dimension)).ok());
            if let Some(text) = saved {
                s.dims.insert(dimension.to_owned(), Kinetics::load(&text));
            }
        }
        s.ticks += 1;
        let ticks = s.ticks;
        let mut changed = Vec::new();
        s.pending.retain(|(d, p)| {
            if d == dimension {
                changed.push(*p);
                false
            } else {
                true
            }
        });
        s.destroy.retain(|(d, p)| {
            if d == dimension {
                destroy.push(*p);
                false
            } else {
                true
            }
        });
        let mut values = Vec::new();
        s.values.retain(|(d, p, v)| {
            if d == dimension {
                values.push((*p, *v));
                false
            } else {
                true
            }
        });
        let kinetics = s.dims.entry(dimension.to_owned()).or_default();
        let mut level = InPlace {
            game,
            destroyed: Vec::new(),
        };
        // The creative motors' scroll values (`ScrollValueBehaviour.setValue`),
        // and brass funnels' amounts (their filter's count).
        for (pos, value) in values {
            if super::create_logistics::is_brass_funnel(game, pos)
                || super::create_logistics::smart_chute_filter_side(game, pos, Dir::North)
            {
                amounts.push((pos, value));
                continue;
            }
            if matches!(kind_at(game, pos), Some(Kind::SpeedController(_))) {
                kinetics.set_controller_target(&mut level, pos, value);
                continue;
            }
            if matches!(kind_at(game, pos), Some(Kind::CreativeMotor(_))) {
                if kinetics.get(pos).is_none() {
                    kinetics.on_placed(pos, kind_at(game, pos).expect("a motor"));
                }
                kinetics.set_motor_speed(&mut level, pos, value);
            }
        }
        // BeltBlock.onRemove: a segment that is no belt any more.
        let mut note_removed = |old: Option<Kind>, new: Option<Kind>, pos: Pos| {
            if let Some(Kind::Belt(b)) = old
                && !matches!(new, Some(Kind::Belt(_)))
            {
                removed_belts.push((pos, b));
            }
        };
        changed_here.clone_from(&changed);
        for pos in changed {
            let kind = kind_at(game, pos);
            note_removed(kinetics.get(pos).map(|e| e.kind), kind, pos);
            kinetics.on_changed(&mut level, pos, kind);
        }
        // Changes no hook told us of: a loaded block that isn't ours any more.
        for pos in kinetics.positions() {
            if !level.is_loaded(pos) {
                continue;
            }
            let kind = kind_at(game, pos);
            if kinetics.get(pos).map(|e| e.kind) != kind {
                note_removed(kinetics.get(pos).map(|e| e.kind), kind, pos);
                kinetics.on_changed(&mut level, pos, kind);
            }
        }
        kinetics.tick(&mut level);
        // WaterWheelBlockEntity.lazyTick: the flow, every 60 ticks.
        if ticks % 60 == 0 {
            for (p, axis, large) in kinetics.water_wheels() {
                if level.is_loaded(p) {
                    let score = super::create_machines::water_wheel_flow(game, p, axis, large);
                    kinetics.set_flow_score(&mut level, p, score);
                }
            }
        }
        // PressingBehaviour.tick; what it asks of the world waits for the lock.
        for p in kinetics.presses() {
            if let Some((step, mode)) = kinetics.tick_press(&level, p)
                && matches!(step, PressStep::ScanWorld | PressStep::Press)
            {
                press_steps.push((p, step, mode));
            }
        }
        // MechanicalMixerBlockEntity.tick; its basin waits for the lock too.
        for p in kinetics.mixers() {
            if let Some(step) = kinetics.tick_mixer(&level, p)
                && step != MixStep::Idle
            {
                mix_steps.push((p, step));
            }
        }
        fans = kinetics.fans(&level);
        // Belt controllers: their start segments.
        for p in kinetics.positions() {
            if let Some(Kind::Belt(b)) = kinetics.get(p).map(|e| e.kind)
                && b.part == super::create_belt::Part::Start
                && level.is_loaded(p)
            {
                belts.push((p, b, kinetics.speed_of(&level, p)));
            }
            // Mechanical pumps, for the fluids' tick.
            if let Some(Kind::MechanicalPump(f)) = kinetics.get(p).map(|e| e.kind)
                && level.is_loaded(p)
            {
                pumps.push((p, f, kinetics.speed_of(&level, p)));
            }
            // Adjustable chain gearshifts, for their signal; turntables, for
            // what stands on them.
            if let Some(k @ Kind::ChainGearshift(..)) = kinetics.get(p).map(|e| e.kind)
                && level.is_loaded(p)
            {
                chain_gearshifts.push((p, k));
            }
            if let Some(e) = kinetics.get(p)
                && let Kind::PoweredShaft(a) = e.kind
                && level.is_loaded(p)
            {
                powered_shafts.push((p, a, e.engine));
            }
            if matches!(kinetics.get(p).map(|e| e.kind), Some(Kind::FluidValve(..))) && level.is_loaded(p) {
                valves.push((p, kinetics.speed_of(&level, p)));
            }
            if matches!(kinetics.get(p).map(|e| e.kind), Some(Kind::Turntable)) && level.is_loaded(p) {
                turntables.push((p, kinetics.speed_of(&level, p)));
            }
            // Crushing wheels, for their controllers.
            if let Some(Kind::CrushingWheel(a)) = kinetics.get(p).map(|e| e.kind)
                && level.is_loaded(p)
            {
                crushing_wheels.push((p, a, kinetics.speed_of(&level, p)));
            }
            // Gearshifts and clutches, for their redstone signal.
            if let Some(k @ Kind::SplitShaft(..)) = kinetics.get(p).map(|e| e.kind)
                && level.is_loaded(p)
            {
                split_shafts.push((p, k));
            }
            // Large water wheels, for their structure.
            if let Some(Kind::LargeWaterWheel(a)) = kinetics.get(p).map(|e| e.kind)
                && level.is_loaded(p)
            {
                large_wheels.push((p, a));
            }
            // Standing drills and saws, for their breaking.
            let breaker = match kinetics.get(p).map(|e| e.kind) {
                Some(Kind::Drill(f)) => Some((f, false)),
                Some(Kind::Saw(f, _)) => Some((f, true)),
                _ => None,
            };
            if let Some((f, saw)) = breaker
                && level.is_loaded(p)
            {
                drills.push((p, f, kinetics.speed_of(&level, p), saw));
            }
        }
        let code = dim_code(dimension);
        let updates = kinetics.take_dirty(&level);
        let full = ticks % SNAPSHOT_EVERY == 0;
        full_update = full;
        let entries = if full {
            kinetics.snapshot(&level)
        } else {
            updates
        };
        for (i, chunk) in entries.chunks(ENTRIES_PER_MESSAGE).enumerate() {
            messages.push(encode_update(code, full && i == 0, chunk));
        }
        if full && entries.is_empty() {
            messages.push(encode_update(code, true, &[]));
        }
        destroy.extend(level.destroyed);
        save_now = ticks % SAVE_EVERY == 0;
        if save_now {
            save_all(s);
        }
    }
    if save_now {
        super::create_logistics::save_all();
        super::create_fluid_glue::save_all();
    }
    super::create_logistics::belts_removed(game, &removed_belts);
    for (pos, value) in amounts {
        super::create_logistics::set_funnel_amount(game, pos, value);
        super::create_logistics::set_smart_chute_amount(game, pos, value);
    }
    super::create_logistics::changed(game, dimension, &changed_here);
    super::create_contraptions::contacts_changed(game, &changed_here);
    // Outside the lock: these change blocks and items, which come back to us.
    for [x, y, z] in destroy {
        game.native_run_command(&format!(
            "execute in {dimension} run setblock {x} {y} {z} air destroy"
        ));
    }
    for (p, step, mode) in press_steps {
        super::create_machines::press_world_step(game, p, step, mode);
    }
    for (p, step) in mix_steps {
        super::create_machines::mixer_world_step(game, p, step);
    }
    super::create_machines::fan_tick(game, dimension, &fans);
    super::create_logistics::belts_tick(game, dimension, &belts, full_update);
    super::create_logistics::funnels_tick(game, dimension);
    super::create_logistics::chutes_update(game, dimension);
    super::create_logistics::tunnels_tick(game, dimension);
    super::create_crafter_glue::level_tick(game, dimension);
    super::create_crusher_glue::level_tick(game, dimension, &crushing_wheels);
    chain_gearshifts_tick(game, dimension, &chain_gearshifts);
    turntables_tick(game, dimension, &turntables);
    for (p, speed) in valves {
        super::create_fluid_glue::valve_tick(game, p, speed);
    }
    powered_shafts_tick(game, dimension, &powered_shafts);
    super::create_fluid_glue::level_tick(game, dimension, &pumps);
    super::create_arms::level_tick(game, dimension);
    super::create_actor_glue::breakers_tick(game, dimension, &drills);
    super::create_machines::large_wheels_tick(game, dimension, &large_wheels);
    // GearshiftBlock / ClutchBlock.neighborChanged: the block follows the
    // signal beside it (the kinetics detach and attach it again).
    for (p, k) in split_shafts {
        let Kind::SplitShaft(_, axis, powered) = k else {
            continue;
        };
        let signal = signal(game, p);
        if signal == powered {
            continue;
        }
        let Some(base) = game.pack.assets.registry.default_state(k.block()) else {
            continue;
        };
        if let Some(id) = game.with_props(base, &[("axis", axis.name()), ("powered", if signal { "true" } else { "false" })]) {
            game.native_set_block(dimension, p, id, super::native_hooks::flags::UPDATE_ALL);
        }
    }
    super::create_contraptions::level_tick(game, dimension);
    super::create_actor_glue::psis_tick(game, dimension);
    let depots = super::create_machines::take_depot_updates(game, dimension, full_update);
    if messages.is_empty() && depots.is_empty() {
        return;
    }
    for player in game.native_server_players() {
        if player.dimension != dimension {
            continue;
        }
        for m in &messages {
            if let Err(e) = game.native_net_send_to_player(player.uuid, CHANNEL, m) {
                log::debug!("Create: kinetics to {}: {e}", player.name);
                break;
            }
        }
        if !depots.is_empty()
            && let Err(e) =
                game.native_net_send_to_player(player.uuid, super::create_machines::DEPOTS, &depots)
        {
            log::debug!("Create: depots to {}: {e}", player.name);
        }
    }
}

/// Runs `f` on the kinetics of the level in place, from a block entity's
/// tick or a player's click (not from inside `level_tick`).
pub(super) fn with_kinetics<R>(
    game: &Game<'_>,
    f: impl FnOnce(&mut Kinetics, &mut dyn Level) -> R,
) -> R {
    let dimension = game.dim.id().to_owned();
    let mut guard = server();
    let s = guard.get_or_insert_with(Server::default);
    let kinetics = s.dims.entry(dimension.clone()).or_default();
    let mut level = InPlace {
        game,
        destroyed: Vec::new(),
    };
    let r = f(kinetics, &mut level);
    let destroyed = level.destroyed;
    s.destroy.extend(destroyed.into_iter().map(|p| (dimension.clone(), p)));
    r
}

/// The tick and block of the last main-hand click a Create block took.
static TAKEN_CLICK: Mutex<Option<(u64, [i32; 3])>> = Mutex::new(None);

/// `native_right_click_block`: hand cranks turn, creative motors step
/// their speed (as well as the scroll value box), the wrench acts on blocks
/// whose template has a use of its own.
pub(super) fn right_click(
    game: &mut Game<'_>,
    pos: [i32; 3],
    face: [i32; 3],
    canceled: &mut bool,
) {
    // Create's blocks answer SUCCESS on the client, which then sends no
    // off-hand use; Horizon's client doesn't run the mod and sends both. The
    // off hand's use of a block the main hand's just took is dropped.
    let mut taken = TAKEN_CLICK.lock().unwrap_or_else(|e| e.into_inner());
    if game.hand == horizon_client::first_person::Hand::Off && *taken == Some((game.ticks, pos)) {
        *canceled = true;
        return;
    }
    right_click_hand(game, pos, face, canceled);
    if *canceled && game.hand == horizon_client::first_person::Hand::Main {
        *taken = Some((game.ticks, pos));
    }
}

/// `WrenchItem.wrenchInstaKillsMinecarts` for the window's player
/// (`Game::attack_cart`): a wrench in the main hand of a player not in
/// creative breaks the minecart it hits at once (`hurt` for 100).
pub(super) fn wrench_attacks_cart(game: &mut Game<'_>) -> std::ops::ControlFlow<bool> {
    use std::ops::ControlFlow::{Break, Continue};
    if game.creative || !game.held().is_some_and(|s| &*s.item == WRENCH) {
        return Continue(());
    }
    let Some((i, d)) = game.cart_hit() else {
        return Continue(());
    };
    if !game.before_block(d) || game.mob_hit().is_some_and(|m| m.1 < d) {
        return Continue(());
    }
    let area = game.carts[i].bounds();
    game.hurt_cart(i, 100.0, false, false);
    game.vehicle_attack_sounds(area, None);
    game.swing_arm();
    Break(true)
}

/// `native_attack_entity` (a guest's blow): the wrench breaks a minecart at
/// once, as `wrench_attacks_cart` does for the window's player.
pub(super) fn attack_entity(game: &mut Game<'_>, entity: i32, canceled: &mut bool) {
    let Some(player) = game.native_acting_player() else {
        return;
    };
    if player.game_mode == 1 || !game.held().is_some_and(|s| &*s.item == WRENCH) {
        return;
    }
    let Some(i) = game.carts.iter().position(|c| c.id as i32 == entity) else {
        return;
    };
    game.hurt_cart(i, 100.0, false, false);
    *canceled = true;
}

/// `AllSoundEvents.<name>.playOnServer`: each part of Create's sound
/// event for the players near `at` (their resources define `create:*`).
pub(super) fn play_create_sound(game: &mut Game<'_>, name: &str, at: [f64; 3], volume: f32, pitch: f32) {
    let parts = super::create_sounds::parts(name, volume, pitch);
    if parts.is_empty() {
        return;
    }
    game.with_level(|_, level, _| {
        for (id, category, v, p) in &parts {
            level.native_play_sound(id, at, *category, *v, *p);
        }
    });
}

/// `Block.getStateForPlacement` of Create's kinetic blocks
/// (`placement.rs`): their axis or facing from the shafts around and the
/// placer's look, as Create chooses them.
pub(super) fn state_for_placement(
    level: &super::GameLevel<'_>,
    state: BlockStateId,
    context: &horizon_world::level::PlacementContext,
    answer: &mut Option<Option<BlockStateId>>,
) {
    let registry = level.registry;
    let Some(s) = registry.state(state) else {
        return;
    };
    let pos = [context.pos.x, context.pos.y, context.pos.z];
    let f = context.face.normal();
    let face = Dir::nearest(f);
    let looking = super::create_placement::nearest_looking(context.yaw, context.pitch);
    let set = |props: &[(&str, &str)]| {
        let mut st = state;
        for (k, v) in props {
            st = level.native_state_with_property(st, k, v)?;
        }
        Some(st)
    };
    let against = {
        let n = face.opposite().normal();
        let p = [context.pos.x + n[0], context.pos.y + n[1], context.pos.z + n[2]];
        level.registry.state(level.world.block(p[0], p[1], p[2]))
    };
    // BeltTunnelItem.canPlace: only on a flat belt.
    if super::create_tunnel::TunnelState::from_block(&s.name, |_| None).is_some() {
        let below = level.registry.state(level.world.block(context.pos.x, context.pos.y - 1, context.pos.z));
        let flat_belt = below.is_some_and(|b| {
            super::create_belt::BeltState::from_block(&b.name, |k| b.properties.get(k).map(String::as_str))
                .is_some_and(|belt| belt.slope == super::create_belt::Slope::Horizontal)
        });
        if !flat_belt {
            *answer = Some(None);
        }
        return;
    }
    let chosen = match s.name.as_str() {
        // RedstoneLinkBlock: mounted on the face clicked.
        super::create_machines::LINK => set(&[("facing", face.name())]),
        // DiodeBlock.getStateForPlacement: facing the placer.
        n if super::create_redstone_glue::is_diode(n) => {
            set(&[("facing", super::create_placement::horizontal(context.yaw).opposite().name())])
        }
        // MechanicalCrafterBlock.getStateForPlacement.
        super::create_crafter_glue::CRAFTER => {
            let other = against
                .filter(|a| a.name == super::create_crafter_glue::CRAFTER)
                .and_then(|a| a.properties.get("facing").and_then(|f| Dir::parse(f)));
            let horizontal = super::create_placement::horizontal(context.yaw);
            let (facing, pointing) = super::create_crafter::placement(face, other, context.sneaking, horizontal);
            set(&[("facing", facing.name()), ("pointing", pointing.name())])
        }
        // RedstoneContactBlock: facing away from the placer, or onto the
        // clicked block when sneaking.
        "create:redstone_contact" => {
            let f = if context.sneaking { face.opposite() } else { looking.opposite() };
            set(&[("facing", f.name())])
        }
        // PortableStorageInterfaceBlock: DirectionalBlock's.
        super::create_actors::PSI => {
            let f = if context.sneaking { looking } else { looking.opposite() };
            set(&[("facing", f.name())])
        }
        // ItemVaultBlock: the axis of a vault it is placed against unless
        // sneaking, else the placer's facing's.
        super::create_arms::VAULT => {
            let placed_on = against.filter(|a| a.name == super::create_arms::VAULT).map(|a| {
                match a.properties.get("axis").map(String::as_str) {
                    Some("z") => super::create_kinetics::Axis::Z,
                    _ => super::create_kinetics::Axis::X,
                }
            });
            let horizontal = super::create_placement::horizontal(context.yaw).axis();
            let axis = super::create_vault::placement_axis(placed_on, context.sneaking, horizontal);
            set(&[("axis", if axis == super::create_kinetics::Axis::X { "x" } else { "z" })])
        }
        // AttachedActorBlock (harvester, plough): on a grindstone's
        // template, kept on its wall.
        super::create_actors::HARVESTER | super::create_actors::PLOUGH => {
            let other = against
                .filter(|a| a.name == s.name)
                .and_then(|a| a.properties.get("facing").and_then(|f| Dir::parse(f)));
            let ctx = super::create_placement::Ctx {
                face,
                yaw: context.yaw,
                pitch: context.pitch,
                sneaking: context.sneaking,
            };
            let f = super::create_placement::attached_actor(&ctx, other);
            set(&[("face", "wall"), ("facing", f.name())])
        }
        _ => None,
    };
    if let Some(next) = chosen {
        *answer = Some(Some(next));
        return;
    }
    // ArmBlock.getStateForPlacement: hung when placed against a ceiling.
    if s.name == super::create_arms::ARM {
        let ceiling = if f == [0, -1, 0] { "true" } else { "false" };
        if let Some(next) = level.native_state_with_property(state, "ceiling", ceiling) {
            *answer = Some(Some(next));
        }
        return;
    }
    if s.name == super::create_whistle_glue::WHISTLE {
        let clicked = Dir::nearest(context.face.normal());
        *answer = Some(super::create_whistle_glue::placement(level, state, pos, clicked, context.yaw));
        return;
    }
    if s.name == super::create_engine_glue::ENGINE {
        let clicked = Dir::nearest(context.face.normal());
        *answer = Some(super::create_engine_glue::placement(level, state, pos, clicked, context.yaw, context.pitch));
        return;
    }
    if s.name == super::create_fluid_glue::SMART {
        let clicked = Dir::nearest(context.face.normal());
        if let Some(next) = super::create_fluid_glue::smart_placement(level, state, pos, clicked, context.yaw, context.pitch) {
            *answer = Some(Some(next));
        }
        return;
    }
    if s.name == super::create_fluid_glue::PIPE {
        let looking = super::create_placement::nearest_looking(context.yaw, context.pitch);
        if let Some(next) = super::create_fluid_glue::pipe_placement(level, state, pos, looking) {
            *answer = Some(Some(next));
        }
        return;
    }
    let Some(kind) = Kind::from_block(&s.name, |k| s.properties.get(k).map(String::as_str)) else {
        return;
    };
    struct Near<'a, 'w> {
        level: &'a super::GameLevel<'w>,
        pos: Pos,
    }
    impl Near<'_, '_> {
        fn kind_above(&self) -> Option<Kind> {
            let s = self.level.registry.state(self.level.world.block(self.pos[0], self.pos[1] + 1, self.pos[2]))?;
            Kind::from_block(&s.name, |k| s.properties.get(k).map(String::as_str))
        }
    }
    impl super::create_placement::Around for Near<'_, '_> {
        fn kind(&self, side: Dir) -> Option<Kind> {
            let n = side.normal();
            let p = [self.pos[0] + n[0], self.pos[1] + n[1], self.pos[2] + n[2]];
            let s = self.level.registry.state(self.level.world.block(p[0], p[1], p[2]))?;
            Kind::from_block(&s.name, |k| s.properties.get(k).map(String::as_str))
        }
        fn chute(&self, side: Dir) -> bool {
            let n = side.normal();
            let p = [self.pos[0] + n[0], self.pos[1] + n[1], self.pos[2] + n[2]];
            self.level
                .registry
                .state(self.level.world.block(p[0], p[1], p[2]))
                .is_some_and(|s| super::create_logistics::is_chute_block(&s.name))
        }
        fn pipe_connects(&self, side: Dir) -> bool {
            let n = super::create_fluid_glue::pipe_neighbour(self.level, self.pos, side);
            super::create_fluids::pipe_connects(n, side)
        }
    }
    let f = context.face.normal();
    let ctx = super::create_placement::Ctx {
        face: Dir::nearest(f),
        yaw: context.yaw,
        pitch: context.pitch,
        sneaking: context.sneaking,
    };
    let Some(mut placed) = super::create_placement::place(kind, &ctx, &Near { level, pos }) else {
        return;
    };
    if let Kind::SpeedController(_) = placed
        && let Some(above) = (Near { level, pos }).kind_above()
        && above.is_large_cog()
        && above.rotation_axis() != Axis::Y
    {
        placed = Kind::SpeedController(if above.rotation_axis() == Axis::X { Axis::Z } else { Axis::X });
    }
    // ChainDriveBlock.getStateForPlacement: its part from its neighbours, and
    // a gearshift's `powered` follows at its first tick.
    if let Kind::SequencedGearshift(a, v) = placed {
        let next = level
            .native_state_with_property(state, "axis", a.name())
            .and_then(|s| level.native_state_with_property(s, "vertical", if v { "true" } else { "false" }));
        *answer = next.map(Some);
        return;
    }
    if let Kind::FluidValve(f, along) = placed {
        let next = level
            .native_state_with_property(state, "facing", f.name())
            .and_then(|s| level.native_state_with_property(s, "axis_along_first", if along { "true" } else { "false" }))
            .and_then(|s| super::create_fluid_glue::with_attachments(level, s, pos));
        *answer = next.map(Some);
        return;
    }
    if let Kind::ChainDrive(c) | Kind::ChainGearshift(c, _) = placed {
        *answer = chain_state(level, state, chain_with_neighbours(level, pos, c.axis)).map(Some);
        return;
    }
    // The kinds are placed as their blocks are; the redstone contact, the
    // portable storage interface, the vault and the attached actors below.
    let Some((key, value)) = placed.state_property() else {
        return;
    };
    // A kind with a block for each of its orientations (the gauges' along
    // blocks) is placed as that block.
    let target = if placed.block() == s.name {
        Some(state)
    } else {
        level.registry.default_state(placed.block())
    };
    if let Some(next) = target.and_then(|t| level.native_state_with_property(t, key, value)) {
        *answer = Some(Some(next));
    }
}

fn chain_at(level: &super::GameLevel<'_>, p: Pos) -> Option<super::create_chain::Chain> {
    let s = level.registry.state(level.world.block(p[0], p[1], p[2]))?;
    match Kind::from_block(&s.name, |k| s.properties.get(k).map(String::as_str))? {
        Kind::ChainDrive(c) | Kind::ChainGearshift(c, _) => Some(c),
        _ => None,
    }
}

/// `updateAfterWrenched` and placement: a chain block with `axis`, its part
/// from each neighbour.
fn chain_with_neighbours(level: &super::GameLevel<'_>, pos: Pos, axis: Axis) -> super::create_chain::Chain {
    let mut c = super::create_chain::Chain::new(axis);
    for d in Dir::ALL {
        if d.axis() != axis {
            let n = d.normal();
            c = c.update_shape(d, chain_at(level, [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]]));
        }
    }
    c
}

fn chain_state(level: &super::GameLevel<'_>, state: BlockStateId, c: super::create_chain::Chain) -> Option<BlockStateId> {
    let st = level.native_state_with_property(state, "axis", c.axis.name())?;
    let st = level.native_state_with_property(st, "part", c.part.name())?;
    level.native_state_with_property(st, "axis_along_first", if c.along_first { "true" } else { "false" })
}

/// `BlockBehaviour.updateShape` of Create's blocks: a neighbour on
/// `direction` changed.
pub(super) fn update_shape(
    level: &super::GameLevel<'_>,
    pos: Pos,
    state: BlockStateId,
    direction: horizon_core::geometry::Direction,
    new: &mut Option<BlockStateId>,
) {
    let d = Dir::nearest(direction.normal());
    if let Some(next) = super::create_fluid_glue::pipe_update_shape(level, state, pos, d) {
        *new = Some(next);
    }
    if let Some(next) = super::create_whistle_glue::update_shape(level, state, pos, d) {
        *new = next.or_else(|| level.registry.default_state("minecraft:air"));
    }
    // ChainDriveBlock.updateShape.
    if let Some(s) = level.registry.state(state)
        && let Some(Kind::ChainDrive(c) | Kind::ChainGearshift(c, _)) =
            Kind::from_block(&s.name, |k| s.properties.get(k).map(String::as_str))
    {
        let n = d.normal();
        let next = c.update_shape(d, chain_at(level, [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]]));
        if next != c {
            *new = chain_state(level, state, next);
        }
    }
    // DiodeBlock.canSurvive: a diode without the block under it breaks.
    if d == Dir::Down
        && level.registry.state(state).is_some_and(|s| super::create_redstone_glue::is_diode(&s.name))
        && level.facts.get(level.world.block(pos[0], pos[1] - 1, pos[2])).sturdy_full & (1 << Dir::Up as u8) == 0
    {
        *new = level.registry.default_state("minecraft:air");
        return;
    }
    // RedstoneLinkBlock.neighborChanged: its support gone, it breaks (with
    // its drops, as the level removes a block whose shape update is air).
    if let Some(facing) = link_facing(level, state)
        && d == facing.opposite()
        && !link_supported(level, pos, facing)
    {
        *new = level.registry.default_state("minecraft:air");
    }
}

/// A redstone link's facing (away from the block it is mounted on).
fn link_facing(level: &super::GameLevel<'_>, state: BlockStateId) -> Option<Dir> {
    let s = level.registry.state(state).filter(|s| s.name == super::create_machines::LINK)?;
    s.properties.get("facing").and_then(|f| Dir::parse(f))
}

/// `RedstoneLinkBlock.canSurvive`: the block behind it isn't replaceable.
fn link_supported(level: &super::GameLevel<'_>, pos: Pos, facing: Dir) -> bool {
    let n = facing.opposite().normal();
    let behind = level.world.block(pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]);
    !level.facts.get(behind).replaceable
}

/// `native_can_survive` for Create's blocks: the redstone link on a block
/// that isn't replaceable.
pub(super) fn can_survive(level: &super::GameLevel<'_>, pos: Pos, state: BlockStateId, survives: &mut Option<bool>) {
    if let Some(s) = level.registry.state(state).filter(|s| s.name == super::create_crusher_glue::WHEEL)
        && let Some(axis) = s.properties.get("axis").and_then(|a| Axis::parse(a))
    {
        *survives = Some(super::create_crusher_glue::wheel_survives(level, pos, axis));
        return;
    }
    if let Some(facing) = link_facing(level, state) {
        *survives = Some(link_supported(level, pos, facing));
    }
    if level.registry.state(state).is_some_and(|s| super::create_redstone_glue::is_diode(&s.name)) {
        *survives = Some(level.facts.get(level.world.block(pos[0], pos[1] - 1, pos[2])).sturdy_full & (1 << Dir::Up as u8) != 0);
    }
}

/// `RedstoneLinkBlock.getSignal` and `getDirectSignal`: a powered receiver
/// gives its received strength to the block it is mounted on (the side the
/// link faces away from).
pub(super) fn block_signal(
    level: &super::GameLevel<'_>,
    dimension: horizon_world::level::Dimension,
    pos: Pos,
    state: BlockStateId,
    face: horizon_core::geometry::Direction,
    _direct: bool,
    signal: &mut i32,
) {
    let Some(s) = level.registry.state(state) else {
        return;
    };
    if super::create_redstone_glue::is_ours(&s.name) {
        super::create_redstone_glue::block_signal(
            level,
            level_dimension_id(dimension),
            pos,
            state,
            Dir::nearest(face.normal()),
            _direct,
            signal,
        );
        return;
    }
    if s.name != super::create_machines::LINK {
        return;
    }
    *signal = 0;
    let receiver = s.properties.get("receiver").map(String::as_str) == Some("true");
    let facing = s.properties.get("facing").and_then(|f| Dir::parse(f));
    if receiver && facing.is_some_and(|f| Dir::nearest(face.normal()) == f.opposite()) {
        *signal = super::create_machines::link_signal(level_dimension_id(dimension), pos);
    }
}

/// `RedstoneLinkBlock.isSignalSource`: a powered receiver.
pub(super) fn is_signal_source(level: &super::GameLevel<'_>, state: BlockStateId, source: &mut bool) {
    let Some(s) = level.registry.state(state) else {
        return;
    };
    if super::create_redstone_glue::is_ours(&s.name) {
        *source = true;
    }
    if s.name == super::create_machines::LINK {
        *source = s.properties.get("receiver").map(String::as_str) == Some("true")
            && s.properties.get("powered").map(String::as_str) == Some("true");
    }
}

/// `BlockState.hasAnalogOutputSignal` for Create's blocks comparators read.
pub(super) fn has_analog_output(level: &super::GameLevel<'_>, state: BlockStateId, output: &mut bool) {
    let Some(s) = level.registry.state(state) else {
        return;
    };
    let name = s.name.as_str();
    if super::create_fluid_glue::is_tank_block(name)
        || name == super::create_arms::VAULT
        || name == super::create_machines::DEPOT
        || name == super::create_ejector_glue::EJECTOR
        || name == super::create_sequencer_glue::BLOCK
        || name == super::create_machines::BASIN
        || name == super::create_spout_glue::SPOUT
        || name == super::create_spout_glue::DRAIN
    {
        *output = true;
    }
}

/// `getAnalogOutputSignal` of Create's blocks: a tank's fill
/// (`ComparatorUtil.fractionToRedstoneLevel` of its controller), the whole
/// vault's and a basin's inputs (`ItemHelper.calcRedstoneFromInventory`), a
/// depot's stacks (`SharedDepotBlockMethods.getComparatorInputOverride`).
pub(super) fn analog_output(
    level: &super::GameLevel<'_>,
    dimension: horizon_world::level::Dimension,
    pos: Pos,
    output: &mut i32,
) {
    let Some(s) = level.registry.state(level.world.block(pos[0], pos[1], pos[2])) else {
        return;
    };
    let name = s.name.as_str();
    let max_stack = |id: &str| level.items.get(id).map_or(64, |i| i.max_stack.max(1));
    if super::create_fluid_glue::is_tank_block(name) {
        *output = super::create_fluid_glue::tank_analog(level_dimension_id(dimension), pos);
    } else if name == super::create_sequencer_glue::BLOCK {
        // SequencedGearshiftBlock.getAnalogOutputSignal: its STATE.
        *output = s.properties.get("state").and_then(|v| v.parse().ok()).unwrap_or(0);
    } else if name == super::create_arms::VAULT {
        *output = super::create_arms::vault_analog(level, pos, &max_stack);
    } else if name == super::create_machines::DEPOT || name == super::create_machines::BASIN || name == super::create_ejector_glue::EJECTOR {
        *output = super::create_machines::container_analog(level, pos, name, &max_stack);
    } else if name == super::create_spout_glue::SPOUT || name == super::create_spout_glue::DRAIN {
        *output = super::create_spout_glue::tank_analog(level, pos, name);
    }
}

/// `ItemHelper.calcRedstoneFromInventory` over `slots` (each holding up to
/// `limit`, an item at most its own stack size).
pub(super) fn redstone_from_inventory<'a>(
    slots: impl Iterator<Item = Option<&'a horizon_client::items::ItemStack>>,
    limit: u32,
    max_stack: &dyn Fn(&str) -> u32,
) -> i32 {
    let mut total = 0;
    let mut filled = 0;
    let mut f = 0.0f32;
    for s in slots {
        total += 1;
        if let Some(s) = s.filter(|s| s.count > 0) {
            f += s.count as f32 / limit.min(max_stack(&s.item)) as f32;
            filled += 1;
        }
    }
    if total == 0 {
        return 0;
    }
    f /= total as f32;
    (f * 14.0).floor() as i32 + i32::from(filled > 0)
}

/// `native_use_item`: Create's client answers SUCCESS to a click its block
/// took and sends no `UseItem` after it; Horizon's client sends one (a
/// bucket would then pour beside a tank). The use of the hand whose click
/// a Create block took this tick or the last is dropped.
pub(super) fn use_item(game: &mut Game<'_>, _stack: &horizon_client::items::ItemStack, claimed: &mut bool) {
    if game.hand != horizon_client::first_person::Hand::Main {
        return;
    }
    let taken = *TAKEN_CLICK.lock().unwrap_or_else(|e| e.into_inner());
    if taken.is_some_and(|(t, _)| game.ticks.saturating_sub(t) <= 1) {
        *claimed = true;
    }
}

/// `ValveHandleBlock.onBlockActivated`: a dye of another colour paints it, else
/// it turns (not for a sneaking wrench, which goes on to take it).
fn valve_handle_click(game: &mut Game<'_>, pos: Pos) -> bool {
    let Some(Kind::ValveHandle(f, colour)) = kind_at(game, pos) else {
        return false;
    };
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    let held = game.held().map(|s| s.item.to_string());
    if held.as_deref() == Some(WRENCH) && sneaking {
        return false;
    }
    if game.native_acting_player().is_some_and(|p| p.game_mode == 2 || p.game_mode == 3) {
        return false;
    }
    if let Some(dye) = held.as_deref().and_then(super::create_belt::belt_dye).filter(|d| *d != 0) {
        if dye != colour {
            set_kind(game, pos, Kind::ValveHandle(f, dye));
        }
        return true;
    }
    with_kinetics(game, |k, level| {
        if k.get(pos).is_none() {
            k.on_placed(pos, Kind::ValveHandle(f, colour));
        }
        k.activate_valve_handle(level, pos, sneaking)
    });
    true
}

/// A valve handle under the crosshair at its value box.
fn valve_handle_box(game: &Game<'_>, pos: Pos, side: Dir, point: [f64; 3]) -> bool {
    let Some(Kind::ValveHandle(f, _)) = kind_at(game, pos) else {
        return false;
    };
    if side != f {
        return false;
    }
    let at = super::create_funnel::rotate_centered_y([0.5, 0.5, 4.5 / 16.0], f.horizontal_angle());
    let at = match f {
        Dir::Up => [0.5, 1.0 - 4.5 / 16.0, 0.5],
        Dir::Down => [0.5, 4.5 / 16.0, 0.5],
        _ => at,
    };
    let local = [0, 1, 2].map(|k| (point[k] - f64::from(pos[k])) as f32);
    (0..3).map(|k| (local[k] - at[k]).powi(2)).sum::<f32>().sqrt() < 0.4 / 1.5
}

fn right_click_hand(
    game: &mut Game<'_>,
    pos: [i32; 3],
    face: [i32; 3],
    canceled: &mut bool,
) {
    if valve_handle_click(game, pos) {
        *canceled = true;
        return;
    }
    if super::create_contraptions::right_click(game, pos, face)
        || super::create_arms::right_click(game, pos, face)
        || super::create_crafter_glue::right_click(game, pos, face)
    {
        *canceled = true;
        return;
    }
    if super::create_ejector_glue::select_target(game, pos)
        || try_encase(game, pos)
        || super::create_fluid_glue::try_encase(game, pos)
        || super::create_redstone_glue::right_click(game, pos)
    {
        *canceled = true;
        return;
    }
    let holding_wrench = game.held().is_some_and(|s| &*s.item == WRENCH);
    if holding_wrench {
        let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
        let name = block_name(game, pos);
        let kind = kind_at(game, pos);
        // Templates with a use of their own (a grindstone's or an enchanting
        // table's menu) would take the click before `native_use_item_on`:
        // the wrench acts here for them.
        // Belts (a grindstone's template): BeltSlicer, then onWrenched.
        if matches!(kind, Some(Kind::Belt(_))) {
            *canceled = true;
            super::create_logistics::wrench_belt(game, pos, face, sneaking);
            return;
        }
        let own_use = matches!(
            kind,
            Some(Kind::MechanicalPress(_) | Kind::MechanicalMixer | Kind::Millstone)
        ) || name == super::create_machines::DEPOT
            || name == super::create_machines::BASIN
            || super::create_logistics::is_chute_block(&name)
            || name == super::create_arms::VAULT
            || name == super::create_arms::ARM
            || name == super::create_spout_glue::SPOUT
            || name == super::create_spout_glue::DRAIN;
        if own_use {
            *canceled = true;
            wrench(game, pos, face, sneaking);
            return;
        }
        // HandCrankBlock.useItemOn turns the crank whatever is held; a
        // sneaking player's click goes to the wrench.
        if !(matches!(kind, Some(Kind::HandCrank(_))) && !sneaking) {
            return;
        }
    }
    // WaterWheelBlock.useItemOn: planks make the wheel of their wood (a
    // large wheel's structure passes the click on to it).
    let wheel_at = super::create_machines::structure_master(game, pos).unwrap_or(pos);
    if matches!(kind_at(game, wheel_at), Some(Kind::WaterWheel(_) | Kind::LargeWaterWheel(_)))
        && let Some(wood) = game.held().and_then(|h| super::create_kinetics::wheel_wood_of(&h.item))
    {
        let pos = wheel_at;
        *canceled = true;
        let dimension = game.dim.id().to_owned();
        let mut guard = server();
        let s = guard.get_or_insert_with(Server::default);
        if let Some(k) = s.dims.get_mut(&dimension) {
            k.set_wheel_material(pos, wood);
        }
        return;
    }
    // Buckets on tanks and basins first: the basin's own click takes any.
    if super::create_fluid_glue::right_click(game, pos)
        || super::create_machines::right_click(game, pos, face)
        || super::create_logistics::right_click(game, pos, face)
    {
        *canceled = true;
        return;
    }
    let Some(kind) = kind_at(game, pos) else {
        return;
    };
    // The press's and the mixer's template is a grindstone: its menu
    // doesn't open.
    if matches!(kind, Kind::MechanicalPress(_) | Kind::MechanicalMixer) {
        *canceled = true;
        return;
    }
    if !matches!(kind, Kind::HandCrank(_) | Kind::CreativeMotor(_)) {
        return;
    }
    *canceled = true;
    let dimension = game.dim.id().to_owned();
    let mut told = None;
    let mut crank_sound = None;
    {
        let mut guard = server();
        let s = guard.get_or_insert_with(Server::default);
        let kinetics = s.dims.entry(dimension.clone()).or_default();
        if kinetics.get(pos).is_none() {
            kinetics.on_placed(pos, kind);
        }
        let mut level = InPlace {
            game,
            destroyed: Vec::new(),
        };
        match kind {
            Kind::HandCrank(_) => {
                kinetics.turn_crank(&mut level, pos, false);
                crank_sound = Some(pos);
            }
            _ => {
                let now = kinetics.get(pos).map_or(16, |e| e.motor_speed);
                let i = MOTOR_STEPS.iter().position(|&v| v == now).unwrap_or(0);
                let next = MOTOR_STEPS[(i + 1) % MOTOR_STEPS.len()];
                kinetics.set_motor_speed(&mut level, pos, next);
                told = Some(format!("Creative Motor: {next} RPM"));
            }
        }
        let destroyed = std::mem::take(&mut level.destroyed);
        s.destroy
            .extend(destroyed.into_iter().map(|p| (dimension.clone(), p)));
    }
    if let (Some(line), Some(player)) = (told, game.native_acting_player()) {
        game.native_action_bar(player.uuid, horizon_client::text::Text::literal(line));
    }
    // HandCrankBlockEntity.tickAudio: while it is in use, a sound every ten
    // ticks (loud and low at first), as the crank is turned.
    if let Some(pos) = crank_sound {
        static LAST: Mutex<Option<(Pos, u64)>> = Mutex::new(None);
        let now = game.ticks as u64;
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_none_or(|(p, t)| p != pos || now.saturating_sub(t) >= 10) {
            *last = Some((pos, now));
            drop(last);
            play_create_sound(game, "cranking", pos.map(|v| f64::from(v) + 0.5), 10.0 / 2.5, 0.65);
        }
    }
}

fn block_name(game: &Game<'_>, pos: Pos) -> String {
    game.pack
        .assets
        .registry
        .state(game.world.block(pos[0], pos[1], pos[2]))
        .map(|s| s.name.clone())
        .unwrap_or_default()
}

/// `native_use_item_on`: the wrench on our blocks whose template lets the
/// click through (`WrenchItem.useOn`).
pub(super) fn use_item_on(
    game: &mut Game<'_>,
    stack: &horizon_client::items::ItemStack,
    pos: [i32; 3],
    face: [i32; 3],
    claimed: &mut bool,
) {
    if super::create_logistics::use_connector(game, stack, pos)
        || super::create_arms::use_item_on(game, stack, pos)
        || large_wheel_blocked(game, stack, pos, face)
    {
        *claimed = true;
        return;
    }
    if &*stack.item != WRENCH {
        return;
    }
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    if wrench(game, pos, face, sneaking) {
        *claimed = true;
    }
}

/// `LargeWaterWheelBlock.getStateForPlacement` and
/// `LargeWaterWheelBlockItem.place`: a large water wheel is placed only
/// where the eight blocks around it across its axis can be replaced; else
/// nothing is placed and the player is told ("Clear Blocks for Placement").
/// The axis is the clicked face's, as the chain's template places it, so
/// Create's second try one block further (for a face across the axis) never
/// applies. True when the placement was stopped.
fn large_wheel_blocked(game: &mut Game<'_>, stack: &horizon_client::items::ItemStack, pos: Pos, face: [i32; 3]) -> bool {
    if &*stack.item != "create:large_water_wheel" {
        return false;
    }
    let replaceable = |game: &Game<'_>, p: Pos| {
        let w = &game.world;
        if !w.loaded(p[0], p[2]) || p[1] < w.shape.min_y || p[1] >= w.shape.max_y() {
            return false;
        }
        let id = w.block(p[0], p[1], p[2]);
        w.is_air(id) || game.res.facts().get(id).replaceable
    };
    // BlockItem.place: into the clicked block if it can be replaced, else
    // beside it.
    let at = if replaceable(game, pos) {
        pos
    } else {
        [pos[0] + face[0], pos[1] + face[1], pos[2] + face[2]]
    };
    let axis = Dir::nearest(face).axis();
    let mut room = true;
    for x in -1..=1 {
        for y in -1..=1 {
            for z in -1..=1 {
                let o = [x, y, z];
                if o[axis as usize] != 0 || o == [0, 0, 0] {
                    continue;
                }
                room &= replaceable(game, [at[0] + x, at[1] + y, at[2] + z]);
            }
        }
    }
    if room {
        return false;
    }
    if let Some(player) = game.native_acting_player() {
        let line = horizon_client::text::Text::translate("create.large_water_wheel.not_enough_space", &[])
            .with_color(0xFF5D6C);
        game.native_action_bar(player.uuid, line);
        // showBounds: the block grown by one, but one thick along the axis.
        let mut b = [0.0; 6];
        for k in 0..3 {
            let grow = if k == axis as usize { 0.0 } else { 1.0 };
            b[k] = f64::from(at[k]) - grow;
            b[k + 3] = f64::from(at[k]) + 1.0 + grow;
        }
        let params = super::create_outline::Params {
            rgb: 0xFF5D6C,
            ..Default::default()
        };
        send_outline(game, player.uuid, &format!("waterwheel {at:?}"), b, params, 1);
    }
    true
}

/// `WrenchItem.useOn` on an `IWrenchable` of ours: `onWrenched` turns it
/// (`getRotatedBlockState`), `onSneakWrenched` takes it into the player's
/// inventory. False when the block isn't one (or the player may not build).
/// `ChainGearshiftBlock.neighborChanged` and `ChainGearshiftBlockEntity.tick`:
/// `powered` and the signal follow what is beside it.
fn chain_gearshifts_tick(game: &mut Game<'_>, dimension: &str, gearshifts: &[(Pos, Kind)]) {
    for &(p, k) in gearshifts {
        let at = horizon_core::position::BlockPos::new(p[0], p[1], p[2]);
        let power = game.with_level(|b, level, ticks| b.best_neighbor_signal(level, ticks, at));
        let powered = signal(game, p);
        if matches!(k, Kind::ChainGearshift(_, was) if was != powered) {
            game.native_set_block_property(p, "powered", if powered { "true" } else { "false" });
        }
        let mut guard = server();
        let s = guard.get_or_insert_with(Server::default);
        let Some(kinetics) = s.dims.get_mut(dimension) else {
            continue;
        };
        if kinetics.get(p).is_some_and(|e| e.chain_signal != power) {
            let mut level = InPlace { game, destroyed: Vec::new() };
            kinetics.set_chain_signal(&mut level, p, power);
        }
    }
}

/// `TurntableBlock.entityInside` for mobs: what stands on a turning table turns
/// with it, at `speed * 3 / 10` degrees a tick. Through `/tp`, as nothing else
/// sets a mob's facing.
fn turntables_tick(game: &mut Game<'_>, dimension: &str, tables: &[(Pos, f32)]) {
    if tables.iter().all(|t| t.1 == 0.0) {
        return;
    }
    let mobs = game.native_mobs(dimension);
    for &(p, speed) in tables {
        if speed == 0.0 {
            continue;
        }
        let [x, y, z] = p.map(f64::from);
        for m in &mobs {
            let [mx, my, mz] = m.pos;
            if (x..x + 1.0).contains(&mx) && (z..z + 1.0).contains(&mz) && (y + 0.5..y + 1.0).contains(&my) {
                let yaw = m.yaw - speed * 3.0 / 10.0;
                game.native_run_command(&format!(
                    "execute in {dimension} positioned {mx} {my} {mz} run tp @e[type={},limit=1,sort=nearest,distance=..0.75] {mx} {my} {mz} {yaw} 0",
                    m.kind
                ));
            }
        }
    }
}

/// `TurntableHandler.gameRenderFrame` and `TurntableBlock.entityInside` for the
/// window's player: on a table its view turns (less toward the edge); only
/// its box over a table's edge carries it round.
pub(super) fn turntable_client_tick(game: &mut Game<'_>) {
    if game.native_window() {
        super::create_fluid_glue::valves_client_tick();
        super::create_fluid_glue::gauges_client_tick();
        super::create_ejector_glue::client_tick(game);
        super::create_whistle_glue::client_tick(game);
        super::create_sequencer_glue::client_tick(game);
        super::create_cuckoo_glue::client_tick(game);
    }
    let Some(p) = game.player.as_ref() else {
        return;
    };
    if !p.ground.on_ground {
        return;
    }
    let [x, y, z] = [p.position.x, p.position.y, p.position.z];
    let is_table = |g: &Game<'_>, b: Pos| {
        g.pack.assets.registry.state(g.world.block(b[0], b[1], b[2])).is_some_and(|s| s.name == "create:turntable")
    };
    let on = [x.floor() as i32, (y - 0.2).floor() as i32, z.floor() as i32];
    if is_table(game, on)
        && let Some(speed) = client_speed(on).filter(|s| *s != 0.0)
    {
        let offset = ((x - f64::from(on[0]) - 0.5).powi(2) + (z - f64::from(on[2]) - 0.5).powi(2)).sqrt() as f32;
        let mut turn = speed * 2.0 / 3.0 * 0.5;
        if offset > 0.25 {
            turn *= ((0.5 - offset) * 2.0).clamp(0.0, 1.0);
        }
        game.yaw -= turn;
    }
    let cell = [x.floor() as i32, y.floor() as i32, z.floor() as i32];
    for (dx, dz) in [(-0.3, -0.3), (-0.3, 0.3), (0.3, -0.3), (0.3, 0.3)] {
        let t = [(x + dx).floor() as i32, y.floor() as i32, (z + dz).floor() as i32];
        if t == cell || y < f64::from(t[1]) + 0.5 || !is_table(game, t) {
            continue;
        }
        let Some(speed) = client_speed(t).filter(|s| *s != 0.0) else {
            continue;
        };
        let a = f64::from((speed * 3.0 / 10.0).clamp(-16.0, 16.0)).to_radians();
        let (ox, oz) = (x - f64::from(t[0]) - 0.5, z - f64::from(t[2]) - 0.5);
        let (nx, nz) = (ox * a.cos() + oz * a.sin(), oz * a.cos() - ox * a.sin());
        if let Some(p) = game.player.as_mut() {
            p.velocity.x += nx - ox;
            p.velocity.z += nz - oz;
        }
        break;
    }
}

/// `EncasableBlock.tryEncase` (`ShaftBlock`, `CogWheelBlock`): a casing used on
/// a bare shaft or cogwheel, not sneaking, encases it. The casing is kept, as
/// in Create. A cog takes a shaft on each side where a neighbour reaches it
/// (`EncasedCogwheelBlock.handleEncasing`).
fn try_encase(game: &mut Game<'_>, pos: Pos) -> bool {
    let brass = match game.held().map(|s| s.item.to_string()).as_deref() {
        Some("create:andesite_casing") => false,
        Some("create:brass_casing") => true,
        _ => return false,
    };
    if game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft) {
        return false;
    }
    let next = match kind_at(game, pos) {
        Some(Kind::Shaft(a)) => Kind::EncasedShaft(brass, a),
        Some(k @ (Kind::Cogwheel(a) | Kind::LargeCogwheel(a))) => {
            let reaches = |game: &Game<'_>, d: Dir| {
                let n = d.normal();
                kind_at(game, [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]])
                    .is_some_and(|o| o.has_shaft_towards(d.opposite()))
            };
            let top = reaches(game, Dir::positive_of(a));
            let bottom = reaches(game, Dir::positive_of(a).opposite());
            Kind::EncasedCog(brass, k.is_large_cog(), a, top, bottom)
        }
        _ => return false,
    };
    set_kind(game, pos, next);
    let at = pos.map(|v| f64::from(v) + 0.5);
    game.with_level(|_, level, _| {
        level.native_play_sound("minecraft:block.wood.place", at, super::create_sounds::BLOCKS, 1.0, 0.8);
    });
    true
}

/// `KineticBlockEntity.switchToBlockState` to `kind`'s block.
pub(super) fn set_kind(game: &mut Game<'_>, pos: Pos, kind: Kind) {
    let Some(base) = game.pack.assets.registry.default_state(kind.block()) else {
        return;
    };
    let on = |v: bool| if v { "true" } else { "false" };
    let id = match kind {
        Kind::EncasedCog(_, _, a, top, bottom) => {
            game.with_props(base, &[("axis", a.name()), ("top_shaft", on(top)), ("bottom_shaft", on(bottom))])
        }
        Kind::ValveHandle(f, _) => game.with_props(base, &[("facing", f.name())]),
        k => game.with_props(base, &[("axis", k.rotation_axis().name())]),
    };
    if let Some(id) = id {
        let dimension = game.dim.id().to_owned();
        game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
    }
}

fn wrench(game: &mut Game<'_>, pos: Pos, face: [i32; 3], sneaking: bool) -> bool {
    // Player.mayBuild: not in adventure or spectator mode.
    let mode = game.native_acting_player().map_or(0, |p| p.game_mode);
    if mode == 2 || mode == 3 {
        return false;
    }
    let name = block_name(game, pos);
    let kind = kind_at(game, pos);
    // EncasedShaftBlock / EncasedCogwheelBlock.onSneakWrenched: back to the
    // bare block, the casing gone with no drop.
    if sneaking && super::create_whistle_glue::sneak_wrench(game, pos) {
        return true;
    }
    if sneaking {
        let bare = match kind {
            Some(Kind::EncasedShaft(_, a)) => Some(Kind::Shaft(a)),
            Some(Kind::EncasedCog(_, false, a, ..)) => Some(Kind::Cogwheel(a)),
            Some(Kind::EncasedCog(_, true, a, ..)) => Some(Kind::LargeCogwheel(a)),
            _ => None,
        };
        if let Some(bare) = bare {
            set_kind(game, pos, bare);
            return true;
        }
    }
    // ChainDriveBlock.getRotatedBlockState, then updateAfterWrenched.
    if !sneaking && let Some(k @ (Kind::ChainDrive(_) | Kind::ChainGearshift(..))) = kind {
        let axis = k.wrenched(Dir::nearest(face)).rotation_axis();
        let (state, c) = {
            let state = game.world.block(pos[0], pos[1], pos[2]);
            game.with_level(|_, level, _| (state, chain_with_neighbours(level, pos, axis)))
        };
        let next = game.with_level(|_, level, _| chain_state(level, state, c));
        if let Some(next) = next {
            let dimension = game.dim.id().to_owned();
            game.native_set_block(&dimension, pos, next, super::native_hooks::flags::UPDATE_ALL);
            let pitch = horizon_core::random::RandomSource::next_float(&mut game.world.random) + 0.5;
            play_create_sound(game, "wrench_rotate", pos.map(|v| f64::from(v) + 0.5), 1.0, pitch);
        }
        return true;
    }
    // EncasedCogwheelBlock.onWrenched: on its axis, a shaft on or off.
    if let Some(Kind::EncasedCog(b, l, a, top, bottom)) = kind {
        let d = Dir::nearest(face);
        if d.axis() == a {
            let next = if d.positive() { (!top, bottom) } else { (top, !bottom) };
            set_kind(game, pos, Kind::EncasedCog(b, l, a, next.0, next.1));
            let pitch = horizon_core::random::RandomSource::next_float(&mut game.world.random) + 0.5;
            play_create_sound(game, "wrench_rotate", pos.map(|v| f64::from(v) + 0.5), 1.0, pitch);
            return true;
        }
    }
    if matches!(kind, Some(Kind::Belt(_))) {
        return true;
    }
    if !sneaking
        && (super::create_logistics::wrench_funnel(game, pos)
            || super::create_logistics::wrench_tunnel(game, pos)
            || super::create_logistics::wrench_chute(game, pos)
            || super::create_fluid_glue::wrench(game, pos)
            || super::create_arms::wrench(game, pos, face))
    {
        return true;
    }
    if !sneaking && let Some(done) = super::create_fluid_glue::wrench_pipe(game, pos, Dir::nearest(face)) {
        return done;
    }
    let machine = name == super::create_machines::DEPOT
        || super::create_fluid_glue::is_ours(&name)
        || name == super::create_machines::BASIN
        || super::create_logistics::is_ours(&name)
        || super::create_fluid_glue::is_tank_block(&name)
        || super::create_arms::is_ours(&name);
    if kind.is_none() && !machine {
        return false;
    }
    if sneaking {
        // IBE.onRemove drops a block entity's items; then the block goes
        // without drops (`destroyBlock(pos, false)`), and a player not in
        // creative gets it (`Block.getDrops`: the block's own item).
        let items = game
            .native_with_mod_block_entity(pos, |be| {
                be.items.iter_mut().filter_map(Option::take).collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let c = pos.map(|v| f64::from(v) + 0.5);
        for s in items {
            game.spawn_item(s, c, [0.0, 0.1, 0.0], 10);
        }
        let Some(air) = game.pack.assets.registry.default_state("minecraft:air") else {
            return true;
        };
        let dimension = game.dim.id().to_owned();
        let removed = game.native_set_block(&dimension, pos, air, super::native_hooks::flags::UPDATE_ALL);
        if removed && mode != 1 && game.items().get(super::create_logistics::item_of(&name)).is_some() {
            game.give_or_drop(horizon_client::items::ItemStack::new(super::create_logistics::item_of(&name), 1));
        }
        if removed {
            // IWrenchable.playRemoveSound.
            let pitch = horizon_core::random::RandomSource::next_float(&mut game.world.random) * 0.5 + 0.5;
            play_create_sound(game, "wrench_remove", pos.map(|v| f64::from(v) + 0.5), 1.0, pitch);
        }
        return true;
    }
    if let Some(k) = kind {
        let turned = k.wrenched(Dir::nearest(face));
        if turned != k
            && let Some((key, value)) = turned.state_property()
        {
            // KineticBlockEntity.switchToBlockState: the kinetics see the
            // change at the tick's end.
            if matches!(turned, Kind::EncasedCog(..)) {
                set_kind(game, pos, turned);
            } else if let Kind::FluidValve(f, along) = turned {
                super::create_fluid_glue::set_valve(game, pos, f, along);
            } else if let Kind::SequencedGearshift(a, v) = turned {
                game.native_set_block_property(pos, "axis", a.name());
                game.native_set_block_property(pos, "vertical", if v { "true" } else { "false" });
            } else {
                game.native_set_block_property(pos, key, value);
            }
            // IWrenchable.playRotateSound.
            let pitch = horizon_core::random::RandomSource::next_float(&mut game.world.random) + 0.5;
            play_create_sound(game, "wrench_rotate", pos.map(|v| f64::from(v) + 0.5), 1.0, pitch);
        }
    }
    true
}

/// `native_net_from_client`: a player's scroll value
/// (`ValueSettingsPacket`), applied at their level's tick.
pub(super) fn from_client(game: &mut Game<'_>, player: u128, channel: &str, data: &[u8]) {
    if channel == super::create_contraptions::CHASSIS {
        super::create_contraptions::from_client(game, player, data);
        return;
    }
    if channel != VALUES {
        return;
    }
    let Some((pos, value)) = decode_value(data) else {
        return;
    };
    let Some(p) = game.native_server_players().into_iter().find(|p| p.uuid == player) else {
        return;
    };
    // ValueSettingsInputHandler.canInteract, and the packet's reach.
    if p.game_mode == 2 || p.game_mode == 3 {
        return;
    }
    let c = pos.map(|v| f64::from(v) + 0.5);
    let d2 = (0..3).map(|k| (p.position[k] - c[k]).powi(2)).sum::<f64>();
    if d2 > VALUE_RANGE * VALUE_RANGE {
        return;
    }
    if block_name(game, pos) == super::create_sequencer_glue::BLOCK && value & super::create_sequencer::CONFIG_FLAG != 0 {
        super::create_sequencer_glue::configure(game, pos, value);
        return;
    }
    let max = super::create_kinetics::MAX_ROTATION_SPEED as i32;
    if value == 0 || value.abs() > max {
        return;
    }
    if matches!(kind_at(game, pos), Some(Kind::ValveHandle(..))) && (value == TUNNEL_MODE_UP || value == TUNNEL_MODE_DOWN) {
        let angle = with_kinetics(game, |k, _| k.step_valve_angle(pos, value == TUNNEL_MODE_UP));
        if let Some(angle) = angle {
            let turn = if angle < 0 { "\u{27f3}" } else { "\u{27f2}" };
            let line = format!("Angle of Rotation: {}\u{b0} {turn}", angle.abs());
            super::create_contraptions::tell_later(player, horizon_client::text::Text::literal(line));
        }
        return;
    }
    if block_name(game, pos) == super::create_ejector_glue::EJECTOR
        && (value == PISTON_MODE_UP || value == TUNNEL_MODE_UP || value == TUNNEL_MODE_DOWN)
    {
        if value == PISTON_MODE_UP {
            super::create_ejector_glue::trigger(game, pos);
        } else if let Some(line) = super::create_ejector_glue::step_stack_size(game, pos, value == TUNNEL_MODE_UP) {
            super::create_contraptions::tell_later(player, horizon_client::text::Text::literal(line));
        }
        return;
    }
    if value == PISTON_MODE_UP || value == PISTON_MODE_DOWN {
        if let Some(label) = super::create_contraptions::step_piston_mode(game, pos, value == PISTON_MODE_UP) {
            let line = format!("Movement Mode: {label}");
            super::create_contraptions::tell_later(player, horizon_client::text::Text::literal(line));
        }
        return;
    }
    if value == TUNNEL_MODE_UP && block_name(game, pos) == super::create_engine_glue::ENGINE {
        if let Some(line) = super::create_engine_glue::step_direction(game, pos) {
            super::create_contraptions::tell_later(player, horizon_client::text::Text::literal(line.to_owned()));
        }
        return;
    }
    if (value == TUNNEL_MODE_UP || value == TUNNEL_MODE_DOWN)
        && let Some(label) = super::create_redstone_glue::step_interval(game, pos, value == TUNNEL_MODE_UP)
    {
        let line = format!("Interval: {label}");
        super::create_contraptions::tell_later(player, horizon_client::text::Text::literal(line));
        return;
    }
    if value == TUNNEL_MODE_UP || value == TUNNEL_MODE_DOWN {
        if let Some(label) = super::create_logistics::step_tunnel_mode(game, pos, value == TUNNEL_MODE_UP) {
            let line = format!("Brass Tunnel: {label}");
            super::create_contraptions::tell_later(player, horizon_client::text::Text::literal(line));
        }
        return;
    }
    if value == ARM_MODE_UP || value == ARM_MODE_DOWN {
        if block_name(game, pos) == super::create_arms::ARM
            && let Some(label) = super::create_arms::step_arm_mode(game, pos, value == ARM_MODE_UP)
        {
            // Shown at the next level tick (a client's message can't).
            super::create_contraptions::tell_later(player, horizon_client::text::Text::literal(label.to_owned()));
        }
        return;
    }
    let mut guard = server();
    let s = guard.get_or_insert_with(Server::default);
    s.values.push((p.dimension, pos, value));
}

/// A step of an arm's selection mode, sent as a value outside any speed.
const ARM_MODE_UP: i32 = 255;
const ARM_MODE_DOWN: i32 = -255;
/// The same for a brass tunnel's selection mode.
const TUNNEL_MODE_UP: i32 = 254;
const TUNNEL_MODE_DOWN: i32 = -254;
/// And a mechanical piston's movement mode.
const PISTON_MODE_UP: i32 = 253;
const PISTON_MODE_DOWN: i32 = -253;

/// `V1`, the block's position and its new value.
fn encode_value(pos: Pos, value: i32) -> Vec<u8> {
    let mut out = b"V1".to_vec();
    for v in pos.into_iter().chain([value]) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn decode_value(data: &[u8]) -> Option<(Pos, i32)> {
    let rest = data.strip_prefix(b"V1")?;
    if rest.len() != 16 {
        return None;
    }
    let int = |i: usize| i32::from_le_bytes([rest[i], rest[i + 1], rest[i + 2], rest[i + 3]]);
    Some(([int(0), int(4), int(8)], int(12)))
}

/// The creative motor and the place on it the player looks at, when it is
/// its value box: (position, current setting).
fn looked_at_value_box(game: &Game<'_>) -> Option<(Pos, i32)> {
    let hit = game.pick()?;
    let guard = client();
    let c = guard.as_ref()?;
    if c.view.dim != dim_code(game.dim.id()) {
        return None;
    }
    let view = c.view.blocks.get(&hit.pos)?;
    let local = [0, 1, 2].map(|k| (hit.point[k] - f64::from(hit.pos[k])) as f32);
    if let Kind::SpeedController(axis) = view.kind {
        return super::create_kinetics::hits_controller_value_box(axis, Dir::nearest(hit.face), local)
            .then_some((hit.pos, i32::from(view.aux)));
    }
    let Kind::CreativeMotor(facing) = view.kind else {
        return None;
    };
    hits_motor_value_box(facing, Dir::nearest(hit.face), local)
        .then_some((hit.pos, i32::from(view.aux)))
}

/// A brass funnel's filter slot under the crosshair and its amount
/// (`FilteringBehaviour.count`): the wheel steps it.
/// A smart chute's filter (on its sides) too: its amount is its view's
/// third slot. The slot comes back with the amount.
fn looked_at_funnel_amount(game: &Game<'_>) -> Option<(Pos, i32, usize)> {
    let hit = game.pick()?;
    let face = Dir::nearest(hit.face);
    let slot = if super::create_logistics::is_brass_funnel(game, hit.pos)
        && super::create_logistics::funnel_filter_side(game, hit.pos, face)
    {
        0
    } else if super::create_logistics::smart_chute_filter_side(game, hit.pos, face) {
        2
    } else {
        return None;
    };
    let count = super::create_machines::client_slots(hit.pos)
        .and_then(|s| s.get(slot).map(|(_, c)| *c))
        .unwrap_or(64);
    Some((hit.pos, count as i32, slot))
}

/// `ValueSettingsInputHandler.canInteract` on this client.
fn can_interact(game: &Game<'_>) -> bool {
    let mode = game.mode_id();
    !game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft) && mode != 2 && mode != 3
}

/// `native_wheel`: the wheel over a creative motor's value box steps its
/// speed (Create 0.5's scrolling; Create 6 opens a value board here).
pub(super) fn wheel(game: &mut Game<'_>, steps: f32, claimed: &mut bool) {
    if super::create_contraptions::wheel(game, steps) {
        *claimed = true;
        return;
    }
    if steps == 0.0 || !can_interact(game) {
        return;
    }
    // An arm's selection mode: a step either way, which the server applies.
    if let Some(hit) = game.pick()
        && block_name(game, hit.pos) == super::create_arms::ARM
        && Dir::nearest(hit.face).axis() != Axis::Y
    {
        *claimed = true;
        let step = if steps > 0.0 { ARM_MODE_UP } else { ARM_MODE_DOWN };
        if let Err(e) = game.native_net_send_to_server(VALUES, &encode_value(hit.pos, step)) {
            log::debug!("Create: arm mode: {e}");
        }
        return;
    }
    // A mechanical piston's movement mode, likewise.
    if let Some(hit) = game.pick()
        && super::create_contraptions::is_piston(game, hit.pos)
    {
        *claimed = true;
        let step = if steps > 0.0 { PISTON_MODE_UP } else { PISTON_MODE_DOWN };
        if let Err(e) = game.native_net_send_to_server(VALUES, &encode_value(hit.pos, step)) {
            log::debug!("Create: piston mode: {e}");
        }
        return;
    }
    // A valve handle's angle, likewise.
    if let Some(hit) = game.pick()
        && valve_handle_box(game, hit.pos, Dir::nearest(hit.face), hit.point)
    {
        *claimed = true;
        let step = if steps > 0.0 { TUNNEL_MODE_UP } else { TUNNEL_MODE_DOWN };
        if let Err(e) = game.native_net_send_to_server(VALUES, &encode_value(hit.pos, step)) {
            log::debug!("Create: valve handle angle: {e}");
        }
        return;
    }
    // A weighted ejector's stack size, likewise.
    if let Some(hit) = game.pick()
        && super::create_ejector_glue::hits_value_box(game, hit.pos, Dir::nearest(hit.face), hit.point)
    {
        *claimed = true;
        let step = if steps > 0.0 { TUNNEL_MODE_UP } else { TUNNEL_MODE_DOWN };
        if let Err(e) = game.native_net_send_to_server(VALUES, &encode_value(hit.pos, step)) {
            log::debug!("Create: ejector stack size: {e}");
        }
        return;
    }
    // A steam engine's rotation direction, likewise.
    if let Some(hit) = game.pick()
        && super::create_engine_glue::hits_value_box(game, hit.pos, Dir::nearest(hit.face), hit.point)
    {
        *claimed = true;
        if let Err(e) = game.native_net_send_to_server(VALUES, &encode_value(hit.pos, TUNNEL_MODE_UP)) {
            log::debug!("Create: engine direction: {e}");
        }
        return;
    }
    // A brass diode's interval, likewise.
    if let Some(hit) = game.pick()
        && super::create_redstone_glue::is_brass(&block_name(game, hit.pos))
    {
        *claimed = true;
        let step = if steps > 0.0 { TUNNEL_MODE_UP } else { TUNNEL_MODE_DOWN };
        if let Err(e) = game.native_net_send_to_server(VALUES, &encode_value(hit.pos, step)) {
            log::debug!("Create: diode interval: {e}");
        }
        return;
    }
    // A brass tunnel's selection mode, likewise.
    if let Some(hit) = game.pick()
        && super::create_logistics::is_brass_tunnel(game, hit.pos)
    {
        *claimed = true;
        let step = if steps > 0.0 { TUNNEL_MODE_UP } else { TUNNEL_MODE_DOWN };
        if let Err(e) = game.native_net_send_to_server(VALUES, &encode_value(hit.pos, step)) {
            log::debug!("Create: tunnel mode: {e}");
        }
        return;
    }
    let Some((pos, current, slot)) = looked_at_value_box(game)
        .map(|(p, c)| (p, c, None))
        .or_else(|| looked_at_funnel_amount(game).map(|(p, c, s)| (p, c, Some(s))))
    else {
        return;
    };
    *claimed = true;
    let mut value = current;
    for _ in 0..(steps.abs().round() as usize).max(1) {
        value = if slot.is_some() {
            // FilteringBehaviour's count: 1 to 64.
            (value + if steps > 0.0 { 1 } else { -1 }).clamp(1, 64)
        } else {
            motor_scroll_step(value, steps > 0.0)
        };
    }
    if value == current {
        return;
    }
    // Sent once the wheel rests (`send_scrolled_values`), as Create's value
    // board sends when let go: each change of a motor's speed counts towards
    // its network's flicker score, and a quick scroll would break it.
    PENDING_VALUES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(pos, (value, std::time::Instant::now()));
    // Shown at once; the server's update follows.
    if let Some(slot) = slot {
        super::create_machines::client_set_count(pos, slot, value as u32);
        return;
    }
    let mut guard = client();
    if let Some(v) = guard.as_mut().and_then(|c| c.view.blocks.get_mut(&pos)) {
        v.aux = value as i16;
    }
}

/// Values scrolled on this client, waiting for the wheel to rest.
static PENDING_VALUES: Mutex<Option<HashMap<Pos, (i32, std::time::Instant)>>> = Mutex::new(None);

/// The window's tick: scrolled values half a second (10 ticks) after the
/// last step go to the server.
pub(super) fn send_scrolled_values(game: &mut Game<'_>) {
    if !game.native_window() {
        return;
    }
    let ready: Vec<(Pos, i32)> = {
        let mut guard = PENDING_VALUES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(pending) = guard.as_mut() else {
            return;
        };
        let ready: Vec<(Pos, i32)> = pending
            .iter()
            .filter(|(_, (_, at))| at.elapsed().as_millis() >= 500)
            .map(|(p, (v, _))| (*p, *v))
            .collect();
        for (p, _) in &ready {
            pending.remove(p);
        }
        ready
    };
    for (pos, value) in ready {
        if let Err(e) = game.native_net_send_to_server(VALUES, &encode_value(pos, value)) {
            log::debug!("Create: scroll value: {e}");
        }
    }
}

/// `native_world_lifecycle`: a world's kinetics start empty and are saved
/// when it closes.
pub(super) fn world_phase(phase: super::native_lifecycle::WorldPhase) {
    use super::native_lifecycle::WorldPhase;
    super::create_machines::reset();
    let closing = matches!(phase, WorldPhase::WorldCloseFinal | WorldPhase::WorldCloseFailed);
    super::create_logistics::world_phase(closing);
    super::create_fluid_glue::world_phase(closing);
    super::create_arms::world_phase(closing);
    super::create_contraptions::world_phase(closing);
    let mut guard = server();
    match phase {
        WorldPhase::WorldOpenReady | WorldPhase::WorldOpenFailed => *guard = None,
        WorldPhase::WorldCloseFinal | WorldPhase::WorldCloseFailed => {
            if let Some(s) = guard.as_ref() {
                save_all(s);
            }
            *guard = None;
        }
    }
}

// ---- Client -------------------------------------------------------------

/// `native_net_from_server`.
pub(super) fn from_server(channel: &str, data: &[u8]) {
    if channel == super::create_sequencer_glue::CHANNEL {
        super::create_sequencer_glue::from_server(data);
        return;
    }
    if channel == super::create_machines::DEPOTS {
        super::create_machines::from_server(data);
        return;
    }
    if channel == super::create_fluid_glue::FLUIDS {
        super::create_fluid_glue::from_server(data);
        return;
    }
    if channel == super::create_logistics::BELTS {
        super::create_logistics::from_server(data);
        return;
    }
    if channel == super::create_contraptions::CONTRAPTIONS {
        super::create_contraptions::from_server(data);
        return;
    }
    if channel == super::create_arms::ARMS {
        super::create_arms::from_server(data);
        return;
    }
    if channel == OUTLINES {
        outlines_from_server(data);
        return;
    }
    if channel != CHANNEL {
        return;
    }
    let mut guard = client();
    let c = guard.get_or_insert_with(Client::default);
    if !c.view.apply(data) {
        log::debug!("Create: a bad kinetics message");
    }
}

/// `getSpeed` and `getTheoreticalSpeed` of the kinetic block at `pos`.
pub(super) fn speeds(game: &Game<'_>, dimension: &str, pos: Pos) -> (f32, f32) {
    let theoretical = server().as_ref().and_then(|s| s.dims.get(dimension)).and_then(|k| k.get(pos)).map_or(0.0, |e| e.speed);
    (with_kinetics(game, |k, level| k.speed_of(level, pos)), theoretical)
}

/// `sequenceContext.getEffectiveValue` of the block at `pos`, for a turn of an
/// angle (or else a distance).
pub(super) fn sequence_limit(dimension: &str, pos: Pos, angle: bool) -> Option<f64> {
    let guard = server();
    let e = guard.as_ref()?.dims.get(dimension)?.get(pos)?;
    let (turn, relative) = e.sequence?;
    (turn == angle).then(|| super::create_sequencer::effective(relative, e.speed))
}

/// `SequencedGearshiftBlockEntity.run`'s part in the kinetics.
pub(super) fn set_sequencer(game: &mut Game<'_>, pos: Pos, modifier: i32, context: Option<(bool, f64)>, detach: bool) {
    with_kinetics(game, |k, level| k.set_sequencer(level, pos, modifier, context, detach));
}

/// A client's value for the block at `pos`.
pub(super) fn send_value(game: &mut Game<'_>, pos: Pos, value: i32) {
    if let Err(e) = game.native_net_send_to_server(VALUES, &encode_value(pos, value)) {
        log::debug!("Create: value: {e}");
    }
}

/// A player's client stepped on the weighted ejector at `pos`.
pub(super) fn send_ejector_trigger(game: &mut Game<'_>, pos: Pos) {
    if let Err(e) = game.native_net_send_to_server(VALUES, &encode_value(pos, PISTON_MODE_UP)) {
        log::debug!("Create: ejector trigger: {e}");
    }
}

/// This game's dimension, as clients' messages code it.
pub(super) fn dim_code_of(game: &Game<'_>) -> u8 {
    dim_code(game.dim.id())
}

/// The kinetic block at `pos` as a client sees it.
pub(super) fn client_view(pos: Pos) -> Option<View> {
    client().as_ref()?.view.blocks.get(&pos).copied()
}

/// The kind of the block at `pos`, by its state.
pub(super) fn kind_of(game: &Game<'_>, pos: Pos) -> Option<Kind> {
    kind_at(game, pos)
}

/// A powered shaft as the engine's tick asks after it.
pub(super) struct PoweredShaft {
    pub axis: Axis,
    pub engine: Option<Pos>,
    pub efficiency: f32,
    pub has_source: bool,
    /// `getTheoreticalSpeed`.
    pub speed: f32,
}

pub(super) fn powered_shaft(dimension: &str, pos: Pos) -> Option<PoweredShaft> {
    let guard = server();
    let e = guard.as_ref()?.dims.get(dimension)?.get(pos)?;
    let Kind::PoweredShaft(axis) = e.kind else {
        return None;
    };
    Some(PoweredShaft {
        axis,
        engine: e.engine,
        efficiency: e.engine_efficiency,
        has_source: e.source.is_some(),
        speed: e.speed,
    })
}

/// `PoweredShaftBlockEntity.update`.
pub(super) fn update_engine(game: &mut Game<'_>, dimension: &str, shaft: Pos, engine: Pos, direction: i32, efficiency: f32) {
    let mut guard = server();
    let Some(kinetics) = guard.as_mut().and_then(|s| s.dims.get_mut(dimension)) else {
        return;
    };
    let mut level = InPlace { game, destroyed: Vec::new() };
    kinetics.update_engine(&mut level, shaft, engine, direction, efficiency);
}

/// `PoweredShaftBlock.tick` and `SteamEngineBlockEntity.remove`: a shaft whose
/// engine went loses it and, with no other engine, goes back to a shaft.
fn powered_shafts_tick(game: &mut Game<'_>, dimension: &str, shafts: &[(Pos, Axis, Option<Pos>)]) {
    for &(p, axis, engine) in shafts {
        if let Some(engine) = engine
            && game.world.loaded(engine[0], engine[2])
            && super::create_engine_glue::engine_at(game, engine).is_none()
        {
            let mut guard = server();
            let Some(kinetics) = guard.as_mut().and_then(|s| s.dims.get_mut(dimension)) else {
                continue;
            };
            let mut level = InPlace { game, destroyed: Vec::new() };
            kinetics.remove_engine(&mut level, p);
        }
        let around_loaded = [[2, 0], [-2, 0], [0, 2], [0, -2]].iter().all(|o| game.world.loaded(p[0] + o[0], p[2] + o[1]));
        if around_loaded && !super::create_engine_glue::shaft_still_valid(game, p, axis) {
            set_kind(game, p, Kind::Shaft(axis));
        }
    }
}

/// `AnimationTickHolder.getRenderTime`, from the wall clock.
pub(super) fn render_time() -> f32 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32() * 20.0
}

/// A model holder's state: block `block` (one of the hidden `create:hz_*`)
/// with property `prop` set, or its only state.
fn holder(
    game: &Game<'_>,
    c: &mut Client,
    block: &'static str,
    prop: Option<(&'static str, &'static str)>,
) -> Option<BlockStateId> {
    let key = (block, prop.map_or("", |p| p.1));
    *c.parts.entry(key).or_insert_with(|| {
        let state = game.pack.assets.registry.default_state(block)?;
        match prop {
            Some((k, v)) => game.with_props(state, &[(k, v)]),
            None => Some(state),
        }
    })
}

fn unit(axis: Axis) -> glam::Vec3 {
    match axis {
        Axis::X => glam::Vec3::X,
        Axis::Y => glam::Vec3::Y,
        Axis::Z => glam::Vec3::Z,
    }
}

/// `CachedBuffers.rotateToFace`: a partial model, made facing south, turned
/// to face `facing` (`AngleHelper.horizontalAngle`, then `verticalAngle`).
pub(super) fn rotate_to_face(facing: Dir) -> glam::Mat4 {
    glam::Mat4::from_rotation_y(facing.horizontal_angle().to_radians())
        * glam::Mat4::from_rotation_x(facing.vertical_angle().to_radians())
}

/// `CachedBuffers.rotateToFaceVertical`: a model made upright (the water
/// wheel's, its axle along Y) turned to face `facing`.
fn rotate_to_face_vertical(facing: Dir) -> glam::Mat4 {
    glam::Mat4::from_rotation_y(facing.horizontal_angle().to_radians())
        * glam::Mat4::from_rotation_x((facing.vertical_angle() + 90.0).to_radians())
}

/// `EncasedFanRenderer`: the propeller turns five times the fan's speed,
/// between 80 and 1280 either way, with no offset.
fn fan_angle(speed: f32, time: f32) -> f32 {
    let mut s = speed * 5.0;
    if s > 0.0 {
        s = s.clamp(80.0, 64.0 * 20.0);
    }
    if s < 0.0 {
        s = s.clamp(-64.0 * 20.0, -80.0);
    }
    (time * s * 3.0 / 10.0) % 360.0
}

/// One part to draw: a model holder state, how it is turned to face its
/// way, and its spin in degrees about `axis` through the block's centre
/// (`kineticRotationTransform`).
struct Part {
    state: BlockStateId,
    orient: glam::Mat4,
    axis: Axis,
    angle: f32,
    /// Moved down this far, after turning (the press's and mixer's heads).
    drop: f32,
    /// `SpriteShiftEntry` and scroll (`shiftUVScrolling`): the belt's
    /// texture swapped for its scrolling one, moved this far along it.
    scroll: Option<(&'static str, String, f32)>,
}

type Want = (&'static str, Option<(&'static str, &'static str)>, glam::Mat4, Axis, f32, f32);

/// A gauge's dial as drawn: chasing its target an eighth a tick
/// (`GaugeBlockEntity.tick`), by position.
static GAUGE_DIALS: Mutex<Option<HashMap<Pos, (f32, std::time::Instant)>>> = Mutex::new(None);

fn gauge_dial(pos: Pos, target: f32) -> f32 {
    let now = std::time::Instant::now();
    let mut guard = GAUGE_DIALS.lock().unwrap_or_else(|e| e.into_inner());
    let entry = guard.get_or_insert_with(HashMap::new).entry(pos).or_insert((0.0, now));
    let ticks = now.duration_since(entry.1).as_secs_f32() * 20.0;
    entry.0 = super::create_gauge::chase(entry.0, target, ticks.min(200.0));
    entry.1 = now;
    entry.0
}

/// What Create's renderer for `view` draws at `pos`.
fn parts(game: &Game<'_>, c: &mut Client, pos: Pos, view: View, time: f32) -> Vec<Part> {
    let standard = render_angle(view.kind, pos, view.speed, time);
    let id = glam::Mat4::IDENTITY;
    // (holder, property, orientation, spin axis, angle, drop)
    let mut want: Vec<Want> = Vec::new();
    let mut scrolls: Vec<Option<(&'static str, String, f32)>> = Vec::new();
    match view.kind {
        Kind::Shaft(a) => want.push(("create:hz_shaft", Some(("axis", a.name())), id, a, standard, 0.0)),
        // CuckooClockRenderer: a shaft half at its back (the rest: create_cuckoo_glue).
        Kind::CuckooClock(f, _) => {
            want.push(("create:hz_part_shaft_half", None, rotate_to_face(f.opposite()), f.axis(), standard, 0.0));
        }
        // EjectorRenderer's ShaftRenderer (its lid: create_ejector_glue).
        Kind::WeightedEjector(_) => {
            let a = view.kind.rotation_axis();
            want.push(("create:hz_shaft", Some(("axis", a.name())), id, a, standard, 0.0));
        }
        Kind::PoweredShaft(a) => {
            want.push(("create:hz_powered_shaft", Some(("axis", a.name())), id, a, standard, 0.0))
        }
        Kind::CrushingWheel(a) => {
            want.push(("create:hz_crushing_wheel", Some(("axis", a.name())), id, a, standard, 0.0))
        }
        // FlywheelRenderer: the wheel and its shaft. Not yet: `visualSpeed`'s
        // easing.
        Kind::Flywheel(a) => {
            want.push(("create:hz_flywheel", Some(("axis", a.name())), id, a, standard, 0.0));
            want.push(("create:hz_shaft", Some(("axis", a.name())), id, a, standard, 0.0));
        }
        Kind::Turntable => want.push(("create:hz_part_turntable", None, id, Axis::Y, standard, 0.0)),
        // SpeedControllerRenderer: its shaft, and the bracket holding a large
        // cog above (`hasBracket`).
        Kind::SpeedController(a) => {
            want.push(("create:hz_shaft", Some(("axis", a.name())), id, a, standard, 0.0));
            let above = kind_at(game, [pos[0], pos[1] + 1, pos[2]]);
            if above.is_some_and(|k| k.is_large_cog() && k.rotation_axis() != Axis::Y && k.rotation_axis() != a) {
                let turn = if a == Axis::X { std::f32::consts::PI } else { std::f32::consts::FRAC_PI_2 };
                want.push(("create:hz_part_speed_controller_bracket", None, glam::Mat4::from_rotation_y(turn), Axis::Y, 0.0, -1.0));
            }
        }
        Kind::ChainDrive(c) | Kind::ChainGearshift(c, _) => {
            want.push(("create:hz_shaft", Some(("axis", c.axis.name())), id, c.axis, standard, 0.0))
        }
        // The casing stays in the chunk mesh; KineticBlockEntityRenderer's
        // shaft turns inside it.
        Kind::EncasedShaft(_, a) => want.push(("create:hz_shaft", Some(("axis", a.name())), id, a, standard, 0.0)),
        // EncasedCogRenderer: the shaftless cog, and a shaft half each way it
        // has a shaft (a large cog's at the shaft's own offset).
        Kind::EncasedCog(_, large, a, ..) => {
            let cog = if large { "create:hz_part_large_cogwheel_shaftless" } else { "create:hz_part_cogwheel_shaftless" };
            want.push((cog, None, rotate_to_face_vertical(Dir::positive_of(a)), a, standard, 0.0));
            let shaft = if large { render_angle(Kind::Shaft(a), pos, view.speed, time) } else { standard };
            for d in [Dir::positive_of(a), Dir::positive_of(a).opposite()] {
                if view.kind.has_shaft_towards(d) {
                    want.push(("create:hz_part_shaft_half", None, rotate_to_face(d), a, shaft, 0.0));
                }
            }
        }
        Kind::Cogwheel(a) => {
            want.push(("create:hz_cogwheel", Some(("axis", a.name())), id, a, standard, 0.0))
        }
        Kind::LargeCogwheel(a) => {
            want.push(("create:hz_large_cogwheel", Some(("axis", a.name())), id, a, standard, 0.0))
        }
        Kind::Gearbox(a) => {
            // GearboxRenderer: a shaft half on each side off its axis.
            for side in Dir::ALL.into_iter().filter(|s| s.axis() != a) {
                let angle = gearbox_shaft_angle(pos, &view, side, time);
                want.push(("create:hz_part_shaft_half", None, rotate_to_face(side), side.axis(), angle, 0.0));
            }
        }
        Kind::CreativeMotor(f) => {
            // CreativeMotorRenderer: the shaft half out of its face.
            want.push(("create:hz_part_shaft_half", None, rotate_to_face(f), f.axis(), standard, 0.0));
        }
        Kind::HandCrank(f) => {
            // HandCrankRenderer: its own model turning with the network, and
            // the handle at the crank's own chasing angle.
            want.push(("create:hz_hand_crank", Some(("facing", f.name())), id, f.axis(), standard, 0.0));
            let angle = c.view.crank_angle(pos, view.speed, time);
            want.push((
                "create:hz_part_hand_crank_handle",
                None,
                rotate_to_face(f.opposite()),
                f.axis(),
                angle, 0.0));
        }
        Kind::MechanicalPress(f) => {
            // MechanicalPressRenderer: a shaft through it, and the head, let
            // down by the pressing cycle.
            let a = f.axis();
            want.push(("create:hz_shaft", Some(("axis", a.name())), id, a, standard, 0.0));
            let mode = if view.press_belt {
                PressMode::Belt
            } else {
                PressMode::World
            };
            let drop = press_head_offset(view.aux >= 0, mode, f32::from(view.aux));
            want.push(("create:hz_part_press_head", None, rotate_to_face(f), a, 0.0, drop));
        }
        Kind::Millstone => {
            // MillstoneRenderer: the inner cog, turning about Y.
            want.push(("create:hz_part_millstone_inner", None, id, Axis::Y, standard, 0.0));
        }
        // WaterWheelRenderer: its OBJ wheel, drawn by `draw_wheel`.
        Kind::WaterWheel(_) | Kind::LargeWaterWheel(_) | Kind::ValveHandle(..) => {}
        Kind::EncasedFan(f) => {
            // EncasedFanRenderer: the shaft half behind, the propeller
            // spinning faster in front.
            let back = f.opposite();
            want.push(("create:hz_part_shaft_half", None, rotate_to_face(back), f.axis(), standard, 0.0));
            let spin = fan_angle(view.speed, time);
            want.push(("create:hz_part_encased_fan_inner", None, rotate_to_face(back), f.axis(), spin, 0.0));
        }
        Kind::Saw(f, along) => {
            // SawRenderer: a shaft along its axis, and the blade (its texture
            // runs while it turns), turned to face its way.
            let a = view.kind.rotation_axis();
            want.push(("create:hz_shaft", Some(("axis", a.name())), id, a, standard, 0.0));
            let vertical = f.axis() == Axis::Y;
            let blade = match (vertical, view.speed.partial_cmp(&0.0)) {
                (true, Some(std::cmp::Ordering::Greater)) => "create:hz_part_saw_blade_vertical_active",
                (true, Some(std::cmp::Ordering::Less)) => "create:hz_part_saw_blade_vertical_reversed",
                (true, _) => "create:hz_part_saw_blade_vertical_inactive",
                (false, Some(std::cmp::Ordering::Greater)) => "create:hz_part_saw_blade_horizontal_active",
                (false, Some(std::cmp::Ordering::Less)) => "create:hz_part_saw_blade_horizontal_reversed",
                (false, _) => "create:hz_part_saw_blade_horizontal_inactive",
            };
            let mut orient = rotate_to_face(f);
            if vertical && along {
                orient = glam::Mat4::from_rotation_y(90f32.to_radians()) * orient;
            }
            want.push((blade, None, orient, a, 0.0, 0.0));
        }
        Kind::SplitShaft(gearshift, a, powered) => {
            // SplitShaftRenderer: a shaft half on each end, the one away from
            // the source turning as `getRotationSpeedModifier` says.
            for side in Dir::ALL.into_iter().filter(|d| d.axis() == a) {
                let away = view.source.is_some_and(|src| src != side);
                let modifier = if away && powered { if gearshift { -1.0 } else { 0.0 } } else { 1.0 };
                let angle = render_angle(view.kind, pos, view.speed * modifier, time);
                want.push(("create:hz_part_shaft_half", None, rotate_to_face(side), a, angle, 0.0));
            }
        }
        Kind::SequencedGearshift(..) => {
            // SplitShaftRenderer, with `getModifier` on the side away from its source.
            let a = view.kind.rotation_axis();
            for side in Dir::ALL.into_iter().filter(|d| d.axis() == a) {
                let away = view.source.is_some_and(|src| src != side);
                let modifier = if away { f32::from(view.aux) } else { 1.0 };
                let angle = render_angle(view.kind, pos, view.speed * modifier, time);
                want.push(("create:hz_part_shaft_half", None, rotate_to_face(side), a, angle, 0.0));
            }
        }
        Kind::Gauge(stress, f, _) => {
            // GaugeRenderer: a shaft across it and, on each side it shows
            // on, the head and the dial turned by its value.
            let a = view.kind.rotation_axis();
            want.push(("create:hz_shaft", Some(("axis", a.name())), id, a, standard, 0.0));
            let target = if stress {
                super::create_gauge::stress_dial_target(view.speed, view.stress, view.capacity)
            } else {
                super::create_gauge::speed_dial_target(view.speed)
            };
            let progress = gauge_dial(pos, target);
            let head = if stress { "create:hz_part_gauge_head_stress" } else { "create:hz_part_gauge_head_speed" };
            for face in [Dir::North, Dir::South, Dir::West, Dir::East] {
                let n = face.normal();
                let beside = [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]];
                let open = game.world.is_air(game.world.block(beside[0], beside[1], beside[2]));
                if !super::create_gauge::shows_head(f, a, face, open) {
                    continue;
                }
                // rotateBufferTowards: about Y, centred; the dial first turns about
                // East around its pivot (5.75/16 up and in).
                let turn = glam::Mat4::from_rotation_y((-super::create_gauge::y_rot(face) - 90.0).to_radians());
                let pivot = glam::Vec3::new(0.0, 5.75 / 16.0 - 0.5, 5.75 / 16.0 - 0.5);
                let dial = turn
                    * glam::Mat4::from_translation(pivot)
                    * glam::Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2 * progress)
                    * glam::Mat4::from_translation(-pivot);
                want.push(("create:hz_part_gauge_dial", None, dial, Axis::Y, 0.0, 0.0));
                want.push((head, None, turn, Axis::Y, 0.0, 0.0));
            }
        }
        // ElevatorPulleyRenderer: a shaft across it (its coil and belt aren't drawn).
        Kind::ElevatorPulley(_) => {
            let a = view.kind.rotation_axis();
            want.push(("create:hz_shaft", Some(("axis", a.name())), id, a, standard, 0.0));
        }
        Kind::Drill(f) => {
            // DrillRenderer: its head, turned to face its way.
            want.push(("create:hz_part_drill_head", None, rotate_to_face(f), f.axis(), standard, 0.0));
        }
        Kind::MechanicalCrafter(f) => {
            // MechanicalCrafterRenderer.renderFast: the shaftless cog about
            // the facing's axis.
            want.push(("create:hz_part_cogwheel_shaftless", None, rotate_to_face_vertical(f), f.axis(), standard, 0.0));
        }
        // FluidValveRenderer: a shaft, and the pointer turned as far as it is
        // open.
        Kind::FluidValve(f, along) => {
            let a = view.kind.rotation_axis();
            want.push(("create:hz_shaft", Some(("axis", a.name())), id, a, standard, 0.0));
            let enabled = game
                .pack
                .assets
                .registry
                .state(game.world.block(pos[0], pos[1], pos[2]))
                .is_some_and(|s| s.properties.get("enabled").is_some_and(|v| v == "true"));
            let pointer = super::create_fluid_glue::client_valve_pointer(pos, enabled, time.fract());
            let pipe = super::create_fluids::valve_pipe_axis(f, along);
            let offset = if (pipe != Axis::Y && a == Axis::X) || pipe == Axis::Y { 90.0f32 } else { 0.0 };
            let tilt = match f {
                Dir::Up => 0.0f32,
                Dir::Down => 180.0,
                _ => 90.0,
            };
            let orient = glam::Mat4::from_rotation_y(f.horizontal_angle().to_radians())
                * glam::Mat4::from_rotation_x(tilt.to_radians())
                * glam::Mat4::from_rotation_y((offset - 90.0 * pointer).to_radians());
            want.push(("create:hz_part_fluid_valve_pointer", None, orient, Axis::Y, 0.0, 0.0));
        }
        Kind::MechanicalPump(f) => {
            // PumpRenderer: its cog, turned to face its way.
            want.push(("create:hz_part_mechanical_pump_cog", None, rotate_to_face(f), f.axis(), standard, 0.0));
        }
        Kind::MechanicalMixer => {
            // MechanicalMixerRenderer: its cog, then the pole and the head
            // let down by the cycle, the head whisking at its own speed.
            want.push(("create:hz_part_cogwheel_shaftless", None, id, Axis::Y, standard, 0.0));
            let running = view.aux >= 0;
            let ticks = i32::from(view.aux.max(0));
            let partial = if running && ticks != 20 { time.fract() } else { 0.0 };
            let drop = mixer_head_offset(running, ticks, partial);
            let head = mixer_head_speed(view.speed, running, ticks);
            let whisk = (time * head * 6.0 / 10.0) % 360.0;
            want.push(("create:hz_part_mechanical_mixer_pole", None, id, Axis::Y, 0.0, drop));
            want.push(("create:hz_part_mechanical_mixer_head", None, id, Axis::Y, whisk, drop));
        }
        Kind::Belt(b) => {
            // BeltRenderer: the belt's partials turned its way, their texture
            // scrolling with its speed; the pulley turning at pulleys.
            let (partials, angle) = belt_partials(b);
            // localTransforms: about Y, then Z for a sideways belt, then X
            // for a vertical or sideways one.
            let sideways = b.slope == super::create_belt::Slope::Sideways;
            let upright = sideways || b.slope == super::create_belt::Slope::Vertical;
            let turn = glam::Mat4::from_rotation_y(angle.to_radians())
                * glam::Mat4::from_rotation_z(if sideways { 90f32 } else { 0.0 }.to_radians())
                * glam::Mat4::from_rotation_x(if upright { 90f32 } else { 0.0 }.to_radians());
            for (name, bottom, diagonal) in partials {
                let holder = belt_holder(name);
                let original = if diagonal {
                    "create:block/belt_diagonal"
                } else if bottom {
                    "create:block/belt_offset"
                } else {
                    "create:block/belt"
                };
                // A dyed belt's own scrolling sprite.
                let dye = super::create_belt::material_dye(view.aux.max(0) as u8);
                let target = super::create_belt::belt_scroll_sprite(dye, diagonal);
                let scroll = belt_scroll(b, view.speed, time, bottom);
                scrolls.resize(want.len(), None);
                scrolls.push(Some((original, target, scroll)));
                want.push((holder, None, turn, Axis::Y, 0.0, 0.0));
            }
            // BeltModel: a cased segment's blockstate model over it, its
            // brass sprite swapped for andesite's on an andesite casing.
            let casing = super::create_belt::material_casing(view.aux.max(0) as u8);
            if casing != super::create_belt::CASING_NONE
                && let Some((model, y)) = super::create_belt::casing_model(b)
            {
                let orient = glam::Mat4::from_rotation_y((-y).to_radians());
                scrolls.resize(want.len(), None);
                if casing == super::create_belt::CASING_ANDESITE {
                    scrolls.push(Some((
                        "create:block/belt/brass_belt_casing",
                        "create:block/belt/andesite_belt_casing".to_owned(),
                        0.0,
                    )));
                } else {
                    scrolls.push(None);
                }
                want.push((casing_holder(model), None, orient, Axis::Y, 0.0, 0.0));
                // BeltModel's cover, under a block covering the belt
                // (`BeltBlock.isBlockCoveringBelt`).
                let above = [pos[0], pos[1] + 1, pos[2]];
                let name = game
                    .pack
                    .assets
                    .registry
                    .state(game.world.block(above[0], above[1], above[2]))
                    .map(|s| (s.name.clone(), s.properties.get("facing").cloned()));
                let covered = name.is_some_and(|(n, facing)| {
                    let bounds = game.block_collision_shape(above).bounds().map(|b| {
                        let (lo, hi) = (b.min_position(), b.max_position());
                        [lo.x, lo.y, lo.z, hi.x, hi.y, hi.z]
                    });
                    let funnel_not_up = super::create_funnel::is_funnel(&n) && facing.as_deref() != Some("up");
                    let tunnel = n.starts_with("create:") && n.contains("_tunnel");
                    super::create_belt::covers_belt(bounds, funnel_not_up, tunnel)
                });
                if covered {
                    let along_x = b.facing.axis() == Axis::X;
                    let cover = match super::create_belt::cover_model(casing, along_x) {
                        "brass_x" => "create:hz_part_belt_cover_brass_x",
                        "brass_z" => "create:hz_part_belt_cover_brass_z",
                        "andesite_x" => "create:hz_part_belt_cover_andesite_x",
                        _ => "create:hz_part_belt_cover_andesite_z",
                    };
                    scrolls.resize(want.len(), None);
                    scrolls.push(None);
                    want.push((cover, None, glam::Mat4::IDENTITY, Axis::Y, 0.0, 0.0));
                }
            }
            if b.has_pulley() {
                let dir = if sideways { Dir::Up } else { b.facing.clockwise(Axis::Y) };
                let orient = match dir.axis() {
                    Axis::X => glam::Mat4::from_rotation_y(90f32.to_radians()),
                    Axis::Y => glam::Mat4::from_rotation_x(90f32.to_radians()),
                    Axis::Z => glam::Mat4::IDENTITY,
                } * glam::Mat4::from_rotation_x(90f32.to_radians());
                want.push(("create:hz_part_belt_pulley", None, orient, b.rotation_axis(), standard, 0.0));
            }
        }
        // ArmRenderer: its cog (the body is create_arms').
        Kind::MechanicalArm => want.push(("create:hz_part_arm_cog", None, id, Axis::Y, standard, 0.0)),
        // DeployerRenderer: a shaft along its axis (pole and hand: create_arms).
        // DeployerRenderer and MechanicalPistonRenderer: a shaft along its
        // axis (the rest: create_arms, create_contraptions).
        Kind::Deployer(..) | Kind::MechanicalPiston(..) => {
            let a = view.kind.rotation_axis();
            want.push(("create:hz_shaft", Some(("axis", a.name())), id, a, standard, 0.0));
        }
        // KineticBlockEntityRenderer draws a gantry shaft's own model
        // turning (its part from the block); GantryCarriageRenderer the
        // carriage's wheels.
        Kind::GantryShaft(f) => {
            let block = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2]));
            let part = block
                .and_then(|s| super::create_contraption::gantry_shaft_of(&s.name))
                .unwrap_or(super::create_contraption::GantryPart::Single);
            let holder = match part {
                super::create_contraption::GantryPart::Start => "create:hz_gantry_shaft_holder_start",
                super::create_contraption::GantryPart::Middle => "create:hz_gantry_shaft_holder_middle",
                super::create_contraption::GantryPart::End => "create:hz_gantry_shaft_holder_end",
                super::create_contraption::GantryPart::Single => "create:hz_gantry_shaft_holder_single",
            };
            want.push((holder, Some(("facing", f.name())), id, f.axis(), standard, 0.0));
        }
        Kind::GantryCarriage(f, _) => {
            let a = view.kind.rotation_axis();
            want.push(("create:hz_part_gantry_carriage_wheels", None, rotate_to_face_vertical(f), a, standard, 0.0));
        }
        // PulleyRenderer (AbstractPulleyRenderer): the shaft, the coil, and
        // below it the rope and magnet as far as the rope runs (blocks of
        // their own while it stands, but for the half at the top).
        Kind::RopePulley(a) => {
            want.push(("create:hz_shaft", Some(("axis", a.name())), id, a, standard, 0.0));
            want.push(("create:hz_part_rope_coil", None, rotate_to_face(Dir::positive_of(a)), a, 0.0, 0.0));
            let running = super::create_contraptions::pulley_offset(pos);
            let offset = running.unwrap_or_else(|| {
                let below = game.world.block(pos[0], pos[1] - 1, pos[2]);
                let name = game.pack.assets.registry.state(below).map(|s| s.name.as_str()).unwrap_or("");
                if name == "create:rope" || name == "create:pulley_magnet" { 1.0 } else { 0.0 }
            });
            if running.is_some() || offset == 0.0 {
                let magnet = if offset > 0.25 { "create:pulley_magnet" } else { "create:hz_part_rope_half_magnet" };
                want.push((magnet, None, id, a, 0.0, offset));
            }
            let f = offset % 1.0;
            if offset > 0.75 && !(0.25..=0.75).contains(&f) {
                want.push(("create:hz_part_rope_half", None, id, a, 0.0, if f > 0.75 { f - 1.0 } else { f }));
            }
            if running.is_some() {
                let mut i = 0.0;
                while i < offset - 1.25 {
                    want.push(("create:rope", None, id, a, 0.0, offset - i - 1.0));
                    i += 1.0;
                }
            }
        }
        // HosePulleyRenderer: AbstractPulleyRenderer with its own parts.
        Kind::HosePulley(_) => {
            let a = view.kind.rotation_axis();
            want.push(("create:hz_shaft", Some(("axis", a.name())), id, a, standard, 0.0));
            want.push(("create:hz_part_hose_coil", None, rotate_to_face(Dir::positive_of(a)), a, 0.0, 0.0));
            let offset = super::create_hose_glue::client_offset(pos);
            let magnet = if offset > 0.25 { "create:hz_part_hose_magnet" } else { "create:hz_part_hose_half_magnet" };
            want.push((magnet, None, id, a, 0.0, offset));
            let f = offset % 1.0;
            if offset > 0.75 && !(0.25..=0.75).contains(&f) {
                want.push(("create:hz_part_hose_half", None, id, a, 0.0, if f > 0.75 { f - 1.0 } else { f }));
            }
            let mut i = 0.0;
            while i < offset - 1.25 {
                want.push(("create:hz_part_hose_rope", None, id, a, 0.0, offset - i - 1.0));
                i += 1.0;
            }
        }
        // BearingRenderer: the shaft half behind, the top turned with the
        // structure (a windmill's wooden).
        Kind::MechanicalBearing(f) | Kind::WindmillBearing(f) => {
            want.push(("create:hz_part_shaft_half", None, rotate_to_face(f.opposite()), f.axis(), standard, 0.0));
            let top = if matches!(view.kind, Kind::WindmillBearing(_)) {
                "create:hz_part_bearing_top_wooden"
            } else {
                "create:hz_part_bearing_top"
            };
            let angle = super::create_contraptions::bearing_angle(pos);
            want.push((top, None, rotate_to_face_vertical(f), f.axis(), angle, 0.0));
        }
    }
    scrolls.resize(want.len(), None);
    want.into_iter()
        .zip(scrolls)
        .filter_map(|((block, prop, orient, axis, angle, drop), scroll)| {
            let state = holder(game, c, block, prop)?;
            Some(Part {
                state,
                orient,
                axis,
                angle,
                drop,
                scroll,
            })
        })
        .collect()
}

/// Before the transient blocks: every rotating part near the camera, at
/// its angle (`KineticBlockEntityRenderer.standardKineticRotationTransform`).
pub(super) fn draw(
    game: &Game<'_>,
    camera: glam::DVec3,
    out: &mut Vec<horizon_client::mesher::Vertex>,
) {
    let mut guard = client();
    let Some(c) = guard.as_mut() else {
        drop(guard);
        draw_items(game, camera, out);
        return;
    };
    if c.view.dim != dim_code(game.dim.id()) || c.view.blocks.is_empty() {
        drop(guard);
        draw_items(game, camera, out);
        return;
    }
    let time = render_time();
    let blocks: Vec<(Pos, View)> = c.view.blocks.iter().map(|(p, v)| (*p, *v)).collect();
    let half = glam::Vec3::splat(0.5);
    for (pos, view) in blocks {
        let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
        if (corner + glam::DVec3::splat(0.5)).distance_squared(camera)
            > VIEW_DISTANCE * VIEW_DISTANCE
        {
            continue;
        }
        let origin = (corner - camera).as_vec3();
        // The brightest of the cell and its neighbours: a shaft inside a
        // press or a casing still shows lit.
        let light = [[0, 0, 0], [0, 1, 0], [0, -1, 0], [1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]]
            .iter()
            .map(|d| game.world.light(pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]))
            .fold(0u8, |a, l| (a & 0xF0).max(l & 0xF0) | (a & 0x0F).max(l & 0x0F));
        let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
        let seed = horizon_core::random::position_seed(horizon_core::position::BlockPos::new(
            pos[0], pos[1], pos[2],
        ));
        for part in parts(game, c, pos, view, time) {
            let m = glam::Mat4::from_translation(origin + half - glam::Vec3::Y * part.drop)
                * glam::Mat4::from_axis_angle(unit(part.axis), part.angle.to_radians())
                * part.orient
                * glam::Mat4::from_translation(-half);
            let start = out.len();
            game.block_model_vertices(part.state, seed, m, packed, false, out);
            if let Some((original, target, scroll)) = part.scroll {
                shift_uv(game, &mut out[start..], original, &target, scroll);
            }
        }
        if let Kind::WaterWheel(f) = view.kind {
            let angle = render_angle(view.kind, pos, view.speed, time);
            let m = glam::Mat4::from_translation(origin + half)
                * glam::Mat4::from_axis_angle(unit(f.axis()), angle.to_radians())
                * rotate_to_face_vertical(f)
                * glam::Mat4::from_translation(-half);
            draw_wheel(game, c, m, packed, view.aux.max(0) as u8, false, false, out);
        }
        // ValveHandleBlockEntity.getIndependentAngle, on HandCrankRenderer.
        if let Kind::ValveHandle(f, colour) = view.kind {
            let a = f.axis();
            let aux = i32::from(view.aux) & 0xFFFF;
            let target = ((aux >> 9) - 2) * 90;
            let back = aux & 256 != 0;
            let done = (aux & 255) as f32 / 255.0;
            let angle = if view.speed != 0.0 && view.source.is_some() {
                render_angle(view.kind, pos, view.speed, time)
            } else {
                target as f32 * done * if back { -1.0 } else { 1.0 } * if f.positive() { 1.0 } else { -1.0 }
            };
            let m = glam::Mat4::from_translation(origin + half)
                * glam::Mat4::from_axis_angle(unit(a), angle.to_radians())
                * rotate_to_face_vertical(f)
                * glam::Mat4::from_translation(-half);
            draw_valve_handle(game, c, m, packed, colour, out);
        }
        if let Kind::LargeWaterWheel(a) = view.kind {
            let angle = render_angle(view.kind, pos, view.speed, time);
            let m = glam::Mat4::from_translation(origin + half)
                * glam::Mat4::from_axis_angle(unit(a), angle.to_radians())
                * rotate_to_face_vertical(Dir::positive_of(a))
                * glam::Mat4::from_translation(-half);
            // LargeWaterWheelBlock.EXTENSION (`updateShape`): another large
            // wheel on its negative side draws the longer axle
            // (`WaterWheelRenderer`'s LARGE_EXTENSION).
            let n = Dir::positive_of(a).normal();
            let behind = [pos[0] - n[0], pos[1] - n[1], pos[2] - n[2]];
            let extension = game
                .pack
                .assets
                .registry
                .state(game.world.block(behind[0], behind[1], behind[2]))
                .is_some_and(|s| s.name == "create:large_water_wheel");
            draw_wheel(game, c, m, packed, view.aux.max(0) as u8, true, extension, out);
        }
    }
    drop(guard);
    draw_items(game, camera, out);
}

/// The model holder of a belt partial (`AllPartialModels.BELT_*`).
fn casing_holder(name: &str) -> &'static str {
    match name {
        "horizontal_start" => "create:hz_part_belt_casing_horizontal_start",
        "horizontal_middle" => "create:hz_part_belt_casing_horizontal_middle",
        "horizontal_end" => "create:hz_part_belt_casing_horizontal_end",
        "horizontal_pulley" => "create:hz_part_belt_casing_horizontal_pulley",
        "diagonal_start" => "create:hz_part_belt_casing_diagonal_start",
        "diagonal_middle" => "create:hz_part_belt_casing_diagonal_middle",
        "diagonal_end" => "create:hz_part_belt_casing_diagonal_end",
        _ => "create:hz_part_belt_casing_diagonal_pulley",
    }
}

fn belt_holder(name: &str) -> &'static str {
    match name {
        "belt_start" => "create:hz_part_belt_start",
        "belt_middle" => "create:hz_part_belt_middle",
        "belt_end" => "create:hz_part_belt_end",
        "belt_start_bottom" => "create:hz_part_belt_start_bottom",
        "belt_middle_bottom" => "create:hz_part_belt_middle_bottom",
        "belt_end_bottom" => "create:hz_part_belt_end_bottom",
        "belt_diagonal_start" => "create:hz_part_belt_diagonal_start",
        "belt_diagonal_end" => "create:hz_part_belt_diagonal_end",
        _ => "create:hz_part_belt_diagonal_middle",
    }
}

/// `SuperByteBuffer.shiftUVScrolling`: the vertices textured from
/// `original` take the same place on `target`, `scroll` of its height on.
fn shift_uv(
    game: &Game<'_>,
    vertices: &mut [horizon_client::mesher::Vertex],
    original: &str,
    target: &str,
    scroll: f32,
) {
    let sprites = &game.gui.atlas.sprites;
    let (Some(o), Some(t)) = (sprites.get(original), sprites.get(target)) else {
        return;
    };
    let height = t.v1 - t.v0;
    let eps = 1e-6;
    for v in vertices {
        let [u, w] = v.uv;
        let inside = u >= o.u0.min(o.u1) - eps
            && u <= o.u0.max(o.u1) + eps
            && w >= o.v0.min(o.v1) - eps
            && w <= o.v0.max(o.v1) + eps;
        if inside {
            v.uv = [u - o.u0 + t.u0, w - o.v0 + t.v0 + scroll * height];
        }
    }
}

/// What depots, basins, chutes and belts carry.
fn draw_items(game: &Game<'_>, camera: glam::DVec3, out: &mut Vec<horizon_client::mesher::Vertex>) {
    super::create_machines::draw_depots(game, camera, out);
    super::create_engine_glue::draw(game, camera, render_time(), out);
    super::create_ejector_glue::draw(game, camera, render_time(), out);
    super::create_whistle_glue::draw(game, camera, render_time(), out);
    super::create_cuckoo_glue::draw(game, camera, render_time(), out);
    super::create_logistics::draw_belts(game, camera, out);
    super::create_fluid_glue::draw(game, camera, out);
    super::create_arms::draw(game, camera, out);
    super::create_contraptions::draw(game, camera, out);
    draw_outlines(game, camera, out);
}

// ---- Outlines (catnip's Outliner) ---------------------------------------------

/// Outlines for a player's client (format O1).
pub(super) const OUTLINES: &str = "create:outlines";
/// The white sprite outlines are drawn with (`ponder:special/blank`'s
/// stand-in, made by prepare_content.py).
const BLANK: &str = "create:block/hz_blank";

/// The outlines this client shows.
static CLIENT_OUTLINES: Mutex<Option<super::create_outline::Outliner>> = Mutex::new(None);

/// Tells `player`'s client to show an outline in `slot` for `ticks` ticks
/// (`Outliner.showAABB` on Create's client side).
pub(super) fn send_outline(
    game: &mut Game<'_>,
    player: u128,
    slot: &str,
    bounds: [f64; 6],
    params: super::create_outline::Params,
    ticks: i32,
) {
    let mut out = Vec::from(&b"O1"[..]);
    let id = slot.as_bytes();
    out.push(id.len().min(255) as u8);
    out.extend_from_slice(&id[..id.len().min(255)]);
    for v in bounds {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&params.rgb.to_le_bytes());
    out.extend_from_slice(&params.width.to_le_bytes());
    out.extend_from_slice(&ticks.to_le_bytes());
    if let Err(e) = game.native_net_send_to_player(player, OUTLINES, &out) {
        log::debug!("Create: outline: {e}");
    }
}

fn outlines_from_server(data: &[u8]) {
    let Some(rest) = data.strip_prefix(b"O1") else {
        return;
    };
    let Some((&n, rest)) = rest.split_first() else {
        return;
    };
    let n = usize::from(n);
    let (Some(id), Some(tail)) = (rest.get(..n), rest.get(n..n + 60)) else {
        return;
    };
    let f = |i: usize| f64::from_le_bytes(tail[i * 8..i * 8 + 8].try_into().unwrap_or([0; 8]));
    let bounds = [f(0), f(1), f(2), f(3), f(4), f(5)];
    let int = |i: usize| u32::from_le_bytes(tail[i..i + 4].try_into().unwrap_or([0; 4]));
    let params = super::create_outline::Params {
        rgb: int(48),
        width: f32::from_bits(int(52)),
    };
    let ticks = int(56) as i32;
    let slot = String::from_utf8_lossy(id).into_owned();
    CLIENT_OUTLINES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(Default::default)
        .show_aabb(&slot, bounds, params, ticks);
}

/// `Outliner.showAABB` on the window's own side (its client handlers).
pub(super) fn show_outline(slot: &str, bounds: [f64; 6], params: super::create_outline::Params) {
    CLIENT_OUTLINES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(Default::default)
        .show_aabb(slot, bounds, params, 1);
}

/// `Outliner.showCluster` on the window's own side, and `keep` / `remove`.
pub(super) fn with_outliner<R>(f: impl FnOnce(&mut super::create_outline::Outliner) -> R) -> R {
    f(CLIENT_OUTLINES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default))
}

/// `Outliner.tickOutlines`, each tick of the window's game.
pub(super) fn outlines_tick(game: &Game<'_>) {
    if !game.native_window() {
        return;
    }
    if let Some(o) = CLIENT_OUTLINES.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        o.tick();
    }
}

/// `Outliner.renderOutlines`: each outline's edges as thin boxes, full
/// bright, in its colour (a fading one until it is gone: the frame's
/// vertices take no alpha).
fn draw_outlines(game: &Game<'_>, camera: glam::DVec3, out: &mut Vec<horizon_client::mesher::Vertex>) {
    let boxes: Vec<([f64; 6], u32)> = {
        let guard = CLIENT_OUTLINES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(o) = guard.as_ref() else {
            return;
        };
        o.entries
            .values()
            .flat_map(|e| {
                let boxes = match &e.cluster {
                    Some(blocks) => super::create_outline::cluster_boxes(blocks, e.params.width),
                    None => super::create_outline::edge_boxes(e.bounds, e.params.width),
                };
                boxes.into_iter().map(|b| (b, e.params.rgb))
            })
            .collect()
    };
    if boxes.is_empty() {
        return;
    }
    let Some(s) = game.gui.atlas.sprites.get(BLANK) else {
        return;
    };
    let light = 0xF0 | 0xF0 << 8;
    for (b, rgb) in boxes {
        let origin = (glam::DVec3::new(b[0], b[1], b[2]) - camera).as_vec3();
        let size = [(b[3] - b[0]) as f32, (b[4] - b[1]) as f32, (b[5] - b[2]) as f32];
        let color = 0xFF00_0000 | (rgb >> 16 & 0xFF) | (rgb >> 8 & 0xFF) << 8 | (rgb & 0xFF) << 16;
        // Six faces, wound counter-clockwise seen from outside.
        let [x, y, z] = size;
        let faces: [[[f32; 3]; 4]; 6] = [
            [[0.0, y, 0.0], [0.0, y, z], [x, y, z], [x, y, 0.0]],
            [[0.0, 0.0, 0.0], [x, 0.0, 0.0], [x, 0.0, z], [0.0, 0.0, z]],
            [[0.0, 0.0, 0.0], [0.0, y, 0.0], [x, y, 0.0], [x, 0.0, 0.0]],
            [[0.0, 0.0, z], [x, 0.0, z], [x, y, z], [0.0, y, z]],
            [[0.0, 0.0, 0.0], [0.0, 0.0, z], [0.0, y, z], [0.0, y, 0.0]],
            [[x, 0.0, 0.0], [x, y, 0.0], [x, y, z], [x, 0.0, z]],
        ];
        let uv = [[s.u0, s.v0], [s.u0, s.v1], [s.u1, s.v1], [s.u1, s.v0]];
        for face in faces {
            for (k, p) in face.iter().enumerate() {
                out.push(horizon_client::mesher::Vertex {
                    normal: [0; 4],
                    pos: (origin + glam::Vec3::from_array(*p)).to_array(),
                    uv: uv[k],
                    layer: 0,
                    color,
                    light,
                    glint_uv: [0.0, 0.0],
                });
            }
        }
    }
}

/// `WaterWheelBlockEntity.material`'s default, spruce planks, as
/// `WaterWheelRenderer.generateModel` swaps the oak template sprites
/// (`OAK_PLANKS_TEMPLATE`, `OAK_LOG_TEMPLATE`) for its planks and log.
fn spruce(texture: &str) -> String {
    let plain = texture.strip_prefix("minecraft:").unwrap_or(texture);
    match plain {
        "block/oak_planks" => "block/spruce_planks".to_owned(),
        "block/oak_log" => "block/spruce_log".to_owned(),
        "block/oak_log_top" => "block/spruce_log_top".to_owned(),
        _ => texture.to_owned(),
    }
}

/// `AllPartialModels.WATER_WHEEL` (`create:block/water_wheel/wheel`, an
/// OBJ model) from the game's resources: the content mod made from the
/// player's Create jar.
fn load_wheel(game: &Game<'_>, id: &str) -> Option<Vec<super::create_obj::Quad>> {
    use super::create_obj::{asset_file, model_file, parse_model_json, parse_mtl, parse_obj, quads};
    let root = &game.options.resources;
    let read = |file: &str| std::fs::read_to_string(root.join(file)).ok();
    let json = |id: &str| -> Option<serde_json::Value> { serde_json::from_str(&read(&model_file(id))?).ok() };
    let mut model = parse_model_json(&json(id)?);
    for _ in 0..8 {
        let Some(parent) = model.parent.clone() else {
            break;
        };
        match json(&parent) {
            Some(p) => model.inherit(&parse_model_json(&p)),
            None => break,
        }
    }
    let obj_file = asset_file(model.model.as_deref()?);
    let obj = match parse_obj(&read(&obj_file)?) {
        Ok(obj) => obj,
        Err(e) => {
            log::warn!("Create: {obj_file}: {e}");
            return None;
        }
    };
    let dir = obj_file.rsplit_once('/').map_or("", |(d, _)| d);
    let materials = obj
        .mtllib
        .as_ref()
        .and_then(|m| read(&format!("{dir}/{m}")))
        .map(|t| parse_mtl(&t))
        .unwrap_or_default();
    let list = quads(&obj, &materials, &model.textures, model.flip_v.unwrap_or(false), spruce);
    (!list.is_empty()).then_some(list)
}

/// The water wheel's OBJ quads through `m`, shaded by the way each faces
/// (as `block_model_vertices` shades a block's faces), lit with `light`.
/// `valve_handle`'s OBJ quads through `m`, in its colour's texture.
fn draw_valve_handle(game: &Game<'_>, c: &mut Client, m: glam::Mat4, light: u32, colour: u8, out: &mut Vec<horizon_client::mesher::Vertex>) {
    let name = super::create_kinetics::VALVE_HANDLES[usize::from(colour).min(16)].trim_start_matches("create:");
    let quads = c.valve_handles.entry(colour).or_insert_with(|| {
        let loaded = load_wheel(game, &format!("create:block/{name}"));
        if loaded.is_none() {
            log::warn!("Create: no valve handle model for {name}");
        }
        loaded
    });
    let Some(quads) = quads else {
        return;
    };
    draw_quads(game, quads, m, light, |s| s.to_owned(), out);
}

/// OBJ quads through `m`, shaded by the way each faces, `sprite` naming each's
/// sprite.
fn draw_quads(
    game: &Game<'_>,
    quads: &[super::create_obj::Quad],
    m: glam::Mat4,
    light: u32,
    sprite: impl Fn(&str) -> String,
    out: &mut Vec<horizon_client::mesher::Vertex>,
) {
    for q in quads {
        let Some(s) = game.gui.atlas.sprites.get(&sprite(&q.sprite)) else {
            continue;
        };
        let n = m.transform_vector3(glam::Vec3::from_array(q.normal));
        let (ax, ay, az) = (n.x.abs(), n.y.abs(), n.z.abs());
        let shade: u32 = if ay >= ax && ay >= az {
            if n.y >= 0.0 { 255 } else { 127 }
        } else if az >= ax {
            204
        } else {
            153
        };
        let color = 0xFF00_0000 | shade << 16 | shade << 8 | shade;
        for k in 0..4 {
            let uv = q.uvs[k];
            out.push(horizon_client::mesher::Vertex {
                normal: [0; 4],
                pos: m.transform_point3(glam::Vec3::from_array(q.positions[k])).to_array(),
                uv: [s.u0 + uv[0] * (s.u1 - s.u0), s.v0 + uv[1] * (s.v1 - s.v0)],
                layer: 0,
                color,
                light,
                glint_uv: [0.0, 0.0],
            });
        }
    }
}

fn draw_wheel(
    game: &Game<'_>,
    c: &mut Client,
    m: glam::Mat4,
    light: u32,
    material: u8,
    large: bool,
    extension: bool,
    out: &mut Vec<horizon_client::mesher::Vertex>,
) {
    let (slot, id) = if large && extension {
        (&mut c.large_wheel_extension, "create:block/large_water_wheel/block_extension")
    } else if large {
        (&mut c.large_wheel, "create:block/large_water_wheel/block")
    } else {
        (&mut c.wheel, "create:block/water_wheel/wheel")
    };
    let quads = slot.get_or_insert_with(|| {
        let loaded = load_wheel(game, id);
        if loaded.is_none() {
            log::warn!("Create: no water wheel model ({id} and its OBJ)");
        }
        loaded
    });
    let Some(quads) = quads else {
        return;
    };
    let sprite = |s: &str| super::create_kinetics::wheel_sprite(s, material);
    draw_quads(game, quads, m, light, sprite, out);
}

// ---- Engineer's goggles -------------------------------------------------

const WHITE: u32 = 0xFFFFFF;
const GRAY: u32 = 0xAAAAAA;
const DARK_GRAY: u32 = 0x555555;
const AQUA: u32 = 0x55FFFF;
const GOLD: u32 = 0xFFAA00;

/// `native_hud`: the goggles' overlay for the block under the crosshair
/// (`GoggleOverlayRenderer` over `IHaveGoggleInformation`).
pub(super) fn hud(game: &Game<'_>, batch: &mut horizon_client::hud::HudBatch) {
    value_tip(game, batch);
    let wearing = game.armor[0]
        .as_ref()
        .is_some_and(|s| &*s.item == "create:goggles");
    if !wearing {
        return;
    }
    let Some(hit) = game.pick() else {
        return;
    };
    let view = {
        let guard = client();
        let Some(c) = guard.as_ref() else {
            return;
        };
        if c.view.dim != dim_code(game.dim.id()) {
            return;
        }
        // SteamEngineBlockEntity shows its shaft's numbers, and the shaft none
        // of its own.
        let at = super::create_engine_glue::goggle_shaft(game, hit.pos).unwrap_or(hit.pos);
        c.view.blocks.get(&at).copied()
    };
    if block_name(game, hit.pos) == super::create_engine_glue::POWERED_SHAFT {
        return;
    }
    // WhistleBlockEntity.addToGoggleTooltip.
    let note = super::create_whistle_glue::goggle_note(game, hit.pos);
    let lines = match view {
        _ if note.is_some() => vec![(format!("Pitch: {}", note.unwrap_or_default()), WHITE)],
        Some(view) => {
            let mut lines = super::create_arms::deployer_goggle_lines(game, hit.pos).unwrap_or_default();
            let kinetic = goggle_lines(&view);
            if !lines.is_empty() && !kinetic.is_empty() {
                lines.push((String::new(), WHITE));
            }
            lines.extend(kinetic);
            lines
        }
        None => match super::create_fluid_glue::client_fluids(dim_code(game.dim.id()), hit.pos) {
            // FluidTankBlockEntity.addToGoggleTooltip: the boiler's, if it is one.
            Some(_) if super::create_fluid_glue::client_boiler(dim_code(game.dim.id()), hit.pos).is_some() => {
                let (b, size) = super::create_fluid_glue::client_boiler(dim_code(game.dim.id()), hit.pos).unwrap_or_default();
                boiler_lines(&b, size)
            }
            Some((tanks, count)) => fluid_lines(&tanks, count),
            None => match super::create_logistics::chute_goggle_lines(game, hit.pos)
                .or_else(|| super::create_logistics::tunnel_goggle_lines(game, hit.pos))
            {
                Some(lines) => lines,
                None => return,
            },
        },
    };
    if lines.is_empty() {
        return;
    }
    // Beside the crosshair, in a tooltip's dark box with its purple frame.
    let width = lines
        .iter()
        .map(|(t, _)| game.native_text_width(t))
        .fold(0.0f32, f32::max)
        + 8.0;
    let height = lines.len() as f32 * 10.0 + 6.0;
    let x = batch.width / 2.0 + 20.0;
    let y = batch.height / 2.0 - height / 2.0;
    game.native_fill(batch, [x, y, width, height], 0xF010_0010);
    game.native_fill(batch, [x, y, width, 1.0], 0xFF50_00FF);
    game.native_fill(batch, [x, y + height - 1.0, width, 1.0], 0xFF28_007F);
    for (i, (text, rgb)) in lines.iter().enumerate() {
        game.native_text(batch, text, [x + 4.0, y + 4.0 + i as f32 * 10.0], *rgb, true);
    }
}

/// `ValueSettingsClient.render`: over the hotbar, the value box looked at,
/// its value as `KineticScrollValueBehaviour.formatSettings` writes it
/// (⟳ turning clockwise, ⟲ the other way), and how to change it here.
fn value_tip(game: &Game<'_>, batch: &mut horizon_client::hud::HudBatch) {
    if !can_interact(game) {
        return;
    }
    let Some((_, value)) = looked_at_value_box(game) else {
        return;
    };
    let arrow = if value < 0 { "\u{27f3}" } else { "\u{27f2}" };
    let lines = [
        ("Rotation Speed".to_owned(), 0xFBDC7D),
        (format!("{} {arrow}", value.abs().max(1)), WHITE),
        ("Scroll to change".to_owned(), GRAY),
    ];
    let mut y = batch.height - 75.0 - lines.len() as f32 * 12.0;
    for (text, rgb) in &lines {
        let x = batch.width / 2.0 - game.native_text_width(text) / 2.0;
        game.native_text(batch, text, [x, y], *rgb, true);
        y += 12.0;
    }
}

/// `BoilerData.addToGoggleTooltip`, the bars in one colour each.
fn boiler_lines(b: &super::create_boiler::Boiler, size: i32) -> Vec<(String, u32)> {
    let bars = |n: i32| "|".repeat(n.max(0) as usize);
    let heat = if b.passive_heat { 1 } else { b.active_heat };
    let (for_size, for_water) = (b.max_for_size(size), b.max_for_water());
    let mut lines = vec![
        (format!("Boiler Status:  {}", b.status(size)), WHITE),
        (format!(" Size....... {}", bars(for_size)), GRAY),
        (format!(" Water... {}", bars(for_water)), GRAY),
        (format!(" Heat...... {}", bars(heat)), GRAY),
    ];
    if b.attached_engines == 0 {
        return lines;
    }
    lines.push((String::new(), WHITE));
    if for_size > 0 && for_water == 0 && heat > 0 {
        lines.push(("Water input rate".to_owned(), GRAY));
        lines.push((format!(" {}mB / 10mB per Tick", b.water_supply), AQUA));
        return lines;
    }
    lines.push(("Kinetic Stress Capacity:".to_owned(), GRAY));
    let via = if b.attached_engines == 1 { "via 1 engine".to_owned() } else { format!("via {} engines", b.attached_engines) };
    lines.push((format!(" {}su {via}", b.total_capacity(size)), AQUA));
    lines
}

/// `IHaveGoggleInformation.containedFluidTooltip` for a fluid handler of
/// `count` tanks: each fluid held, its amount and its tank's capacity.
fn fluid_lines(tanks: &[(super::create_fluids::FluidStack, i32)], count: usize) -> Vec<(String, u32)> {
    let held: Vec<_> = tanks.iter().filter(|(f, _)| f.amount > 0 && !f.fluid.is_empty()).collect();
    if held.is_empty() && count > 1 {
        return Vec::new();
    }
    let mut lines = vec![("Fluid Container".to_owned(), WHITE)];
    for (f, capacity) in &held {
        // The fluid's name from its id (Create's are only names here).
        let path = f.fluid.rsplit(':').next().unwrap_or(&f.fluid);
        let name: Vec<String> = path
            .split('_')
            .map(|w| {
                let mut c = w.chars();
                c.next().map(|h| h.to_uppercase().chain(c).collect()).unwrap_or_default()
            })
            .collect();
        lines.push((format!(" {}", name.join(" ")), GRAY));
        lines.push((format!(" {}mB / {}mB", f.amount, capacity), GOLD));
    }
    if held.is_empty() {
        let capacity = tanks.first().map_or(0, |(_, c)| *c);
        lines.push((format!(" Capacity: {capacity}mB"), GRAY));
    }
    lines
}

/// `KineticBlockEntity.addToTooltip` and `addToGoggleTooltip`, and the
/// generators' `GeneratingKineticBlockEntity.addToGoggleTooltip`, in
/// Create's English (`create.gui.goggles.*`, `create.tooltip.*`).
/// `SpeedGaugeBlockEntity` and `StressGaugeBlockEntity`'s goggle tooltips.
fn gauge_lines(stress_gauge: bool, view: &View) -> Vec<(String, u32)> {
    let bar = |level: usize| {
        (0..3).map(|i| if i < level { '\u{2588}' } else { '\u{2592}' }).collect::<String>()
    };
    let mut lines = vec![("Gauge Information:".to_owned(), WHITE)];
    if !stress_gauge {
        lines.push(("Rotation Speed".to_owned(), GRAY));
        // SpeedLevel.of: none, slow, moderate (30) and fast (100).
        let speed = view.speed.abs();
        let (level, rgb) = if view.over_stressed() {
            (3, 0xFF5555)
        } else if speed >= 100.0 {
            (3, 0xFF55FF)
        } else if speed >= 30.0 {
            (2, 0x55FFFF)
        } else if speed > 0.0 {
            (1, 0x55FF55)
        } else {
            (0, DARK_GRAY)
        };
        let text = if view.over_stressed() { "Overstressed".to_owned() } else { format!("{speed:.0} RPM") };
        lines.push((format!("{} {text}", bar(level)), rgb));
        return lines;
    }
    lines.push(("Network Stress".to_owned(), GRAY));
    let fraction = if view.capacity == 0.0 { 0.0 } else { view.stress / view.capacity };
    if view.speed == 0.0 && !view.over_stressed() {
        lines.push((format!("{} No Rotation", bar(0)), DARK_GRAY));
        return lines;
    }
    // StressImpact.of: low under half, moderate under the capacity, high at it.
    let (level, rgb, word) = if view.over_stressed() {
        (3, 0xFF5555, "Overstressed")
    } else if fraction < 0.5 {
        (1, 0x55FF55, "Low")
    } else if fraction < 1.0 {
        (2, 0xFFFF55, "Moderate")
    } else {
        (3, 0xFF5555, "High")
    };
    lines.push((format!("{} {word}", bar(level)), rgb));
    lines.push(("Remaining Capacity".to_owned(), GRAY));
    let remaining = (view.capacity - view.stress).max(0.0);
    let su = |v: f32| if v.fract() == 0.0 { format!("{v:.0}su") } else { format!("{v:.1}su") };
    let text = if remaining != view.capacity {
        format!(" {} / {}", su(remaining), su(view.capacity))
    } else {
        format!(" {}", su(remaining))
    };
    lines.push((text, rgb));
    lines
}

fn goggle_lines(view: &View) -> Vec<(String, u32)> {
    let mut lines = Vec::new();
    if let Kind::Gauge(stress_gauge, ..) = view.kind {
        return gauge_lines(stress_gauge, view);
    }
    if view.over_stressed() {
        lines.push(("Overstressed".to_owned(), GOLD));
        lines.push(("This network is overstressed.".to_owned(), GRAY));
        lines.push(("Remove machines or add more sources.".to_owned(), GRAY));
        return lines;
    }
    let su = |v: f32| {
        let v = v.abs();
        if v.fract() == 0.0 {
            format!("{v:.0}su")
        } else {
            format!("{v:.1}su")
        }
    };
    let capacity = view.generator_capacity();
    if capacity != 0.0 && view.speed != 0.0 {
        lines.push(("Generator Stats:".to_owned(), WHITE));
        lines.push(("Kinetic Stress Capacity:".to_owned(), GRAY));
        lines.push((format!(" {} at current speed", su(capacity * view.speed)), AQUA));
    }
    let impact = view.kind.stress_impact();
    if impact != 0.0 {
        lines.push(("Kinetic Stats:".to_owned(), WHITE));
        lines.push(("Kinetic Stress Impact:".to_owned(), GRAY));
        lines.push((format!(" {} at current speed", su(impact * view.speed)), AQUA));
    }
    if !lines.is_empty() && view.capacity > 0.0 {
        lines.push((format!("Network: {} / {}", su(view.stress), su(view.capacity)), DARK_GRAY));
    }
    lines
}
