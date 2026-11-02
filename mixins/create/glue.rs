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
pub(super) fn placed(game: &Game<'_>, pos: [i32; 3]) {
    super::create_logistics::placed(game, pos);
    super::create_arms::placed(game, pos);
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
    super::create_fluid_glue::level_tick(game, dimension, &pumps);
    super::create_arms::level_tick(game, dimension);
    super::create_actor_glue::breakers_tick(game, dimension, &drills);
    super::create_machines::large_wheels_tick(game, dimension, &large_wheels);
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

fn right_click_hand(
    game: &mut Game<'_>,
    pos: [i32; 3],
    face: [i32; 3],
    canceled: &mut bool,
) {
    if super::create_contraptions::right_click(game, pos, face)
        || super::create_arms::right_click(game, pos, face)
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
            || name == super::create_arms::ARM;
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
fn wrench(game: &mut Game<'_>, pos: Pos, face: [i32; 3], sneaking: bool) -> bool {
    // Player.mayBuild: not in adventure or spectator mode.
    let mode = game.native_acting_player().map_or(0, |p| p.game_mode);
    if mode == 2 || mode == 3 {
        return false;
    }
    let name = block_name(game, pos);
    let kind = kind_at(game, pos);
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
    let machine = name == super::create_machines::DEPOT
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
        if game.native_set_block(&dimension, pos, air, super::native_hooks::flags::UPDATE_ALL)
            && mode != 1
            && game.items().get(super::create_logistics::item_of(&name)).is_some()
        {
            game.give_or_drop(horizon_client::items::ItemStack::new(super::create_logistics::item_of(&name), 1));
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
            game.native_set_block_property(pos, key, value);
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
    let max = super::create_kinetics::MAX_ROTATION_SPEED as i32;
    if value == 0 || value.abs() > max {
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
    let Kind::CreativeMotor(facing) = view.kind else {
        return None;
    };
    let local = [0, 1, 2].map(|k| (hit.point[k] - f64::from(hit.pos[k])) as f32);
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

/// `AnimationTickHolder.getRenderTime`, from the wall clock.
fn render_time() -> f32 {
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

/// What Create's renderer for `view` draws at `pos`.
fn parts(game: &Game<'_>, c: &mut Client, pos: Pos, view: View, time: f32) -> Vec<Part> {
    let standard = render_angle(view.kind, pos, view.speed, time);
    let id = glam::Mat4::IDENTITY;
    // (holder, property, orientation, spin axis, angle, drop)
    let mut want: Vec<Want> = Vec::new();
    let mut scrolls: Vec<Option<(&'static str, String, f32)>> = Vec::new();
    match view.kind {
        Kind::Shaft(a) => want.push(("create:hz_shaft", Some(("axis", a.name())), id, a, standard, 0.0)),
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
        Kind::WaterWheel(_) | Kind::LargeWaterWheel(_) => {}
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
        // ElevatorPulleyRenderer: a shaft across it (its coil and belt aren't drawn).
        Kind::ElevatorPulley(_) => {
            let a = view.kind.rotation_axis();
            want.push(("create:hz_shaft", Some(("axis", a.name())), id, a, standard, 0.0));
        }
        Kind::Drill(f) => {
            // DrillRenderer: its head, turned to face its way.
            want.push(("create:hz_part_drill_head", None, rotate_to_face(f), f.axis(), standard, 0.0));
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
                super::create_outline::edge_boxes(e.bounds, e.params.width)
                    .into_iter()
                    .map(|b| (b, e.params.rgb))
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
    for q in quads.iter() {
        let sprite = super::create_kinetics::wheel_sprite(&q.sprite, material);
        let Some(s) = game.gui.atlas.sprites.get(&sprite) else {
            continue;
        };
        let n = m.transform_vector3(glam::Vec3::from_array(q.normal));
        // The shade of the nearest face: up, down, north/south, west/east.
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
        c.view.blocks.get(&hit.pos).copied()
    };
    let lines = match view {
        Some(view) => goggle_lines(&view),
        None => match super::create_fluid_glue::client_fluids(dim_code(game.dim.id()), hit.pos) {
            Some((tanks, count)) => fluid_lines(&tanks, count),
            None => return,
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
fn goggle_lines(view: &View) -> Vec<(String, u32)> {
    let mut lines = Vec::new();
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
    let capacity = view.kind.stress_capacity();
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
