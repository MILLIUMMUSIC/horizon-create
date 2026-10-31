// Horizon's side of Create, injected as `crate::game::create_glue`. The
// server keeps each dimension's kinetics (`create_kinetics`) and tells its
// players what turns on the `create:kinetics` channel; clients draw the
// rotating parts each frame, as Create's KineticBlockEntityRenderer does.
// Everything Horizon-specific lives here; the rules are in kinetics.rs.

use super::Game;
use super::create_processing::{PressMode, PressStep, press_head_offset};
use super::create_kinetics::{
    Axis, ClientView, Dir, ENTRIES_PER_MESSAGE, Kind, Kinetics, Level, Pos, View,
    encode_update, gearbox_shaft_angle, render_angle,
};
use horizon_world::registry::BlockStateId;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

const CHANNEL: &str = "create:kinetics";
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
}

#[derive(Default)]
struct Client {
    view: ClientView,
    /// Model holder block states by block and property value.
    parts: HashMap<(&'static str, &'static str), Option<BlockStateId>>,
}

static SERVER: Mutex<Option<Server>> = Mutex::new(None);
static CLIENT: Mutex<Option<Client>> = Mutex::new(None);

fn server() -> MutexGuard<'static, Option<Server>> {
    SERVER.lock().unwrap_or_else(|e| e.into_inner())
}

fn client() -> MutexGuard<'static, Option<Client>> {
    CLIENT.lock().unwrap_or_else(|e| e.into_inner())
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
fn kind_at(game: &Game<'_>, pos: Pos) -> Option<Kind> {
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
                    && Kind::from_block(&s.name, |k| s.properties.get(k).map(String::as_str))
                        .is_some()
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
    if s.states.contains(&old.0) || s.states.contains(&new.0) {
        s.pending
            .push((level_dimension_id(dimension).to_owned(), [pos.x, pos.y, pos.z]));
    }
}

/// `native_place_block`: a player placed a block in their dimension.
pub(super) fn placed(game: &Game<'_>, pos: [i32; 3]) {
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
    let full_update;
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
        let kinetics = s.dims.entry(dimension.to_owned()).or_default();
        let mut level = InPlace {
            game,
            destroyed: Vec::new(),
        };
        for pos in changed {
            let kind = kind_at(game, pos);
            kinetics.on_changed(&mut level, pos, kind);
        }
        // Changes no hook told us of: a loaded block that isn't ours any more.
        for pos in kinetics.positions() {
            if !level.is_loaded(pos) {
                continue;
            }
            let kind = kind_at(game, pos);
            if kinetics.get(pos).map(|e| e.kind) != kind {
                kinetics.on_changed(&mut level, pos, kind);
            }
        }
        kinetics.tick(&mut level);
        // WaterWheelBlockEntity.lazyTick: the flow, every 60 ticks.
        if ticks % 60 == 0 {
            for (p, f) in kinetics.water_wheels() {
                if level.is_loaded(p) {
                    let score = super::create_machines::water_wheel_flow(game, p, f);
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
        if ticks % SAVE_EVERY == 0 {
            save_all(s);
        }
    }
    // Outside the lock: these change blocks and items, which come back to us.
    for [x, y, z] in destroy {
        game.native_run_command(&format!(
            "execute in {dimension} run setblock {x} {y} {z} air destroy"
        ));
    }
    for (p, step, mode) in press_steps {
        super::create_machines::press_world_step(game, p, step, mode);
    }
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

/// `native_right_click_block`: hand cranks turn, creative motors step
/// their speed.
pub(super) fn right_click(
    game: &mut Game<'_>,
    pos: [i32; 3],
    face: [i32; 3],
    canceled: &mut bool,
) {
    if super::create_machines::right_click(game, pos, face) {
        *canceled = true;
        return;
    }
    let Some(kind) = kind_at(game, pos) else {
        return;
    };
    // The press's template is a grindstone: its menu doesn't open.
    if matches!(kind, Kind::MechanicalPress(_)) {
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

/// `native_world_lifecycle`: a world's kinetics start empty and are saved
/// when it closes.
pub(super) fn world_phase(phase: super::native_lifecycle::WorldPhase) {
    use super::native_lifecycle::WorldPhase;
    super::create_machines::reset();
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
fn rotate_to_face(facing: Dir) -> glam::Mat4 {
    let horizontal: f32 = match facing {
        Dir::South | Dir::Up | Dir::Down => 0.0,
        Dir::West => -90.0,
        Dir::North => 180.0,
        Dir::East => -270.0,
    };
    let vertical: f32 = match facing {
        Dir::Up => -90.0,
        Dir::Down => 90.0,
        _ => 0.0,
    };
    glam::Mat4::from_rotation_y(horizontal.to_radians())
        * glam::Mat4::from_rotation_x(vertical.to_radians())
}

/// One part to draw: a model holder state, how it is turned to face its
/// way, and its spin in degrees about `axis` through the block's centre
/// (`kineticRotationTransform`).
struct Part {
    state: BlockStateId,
    orient: glam::Mat4,
    axis: Axis,
    angle: f32,
    /// Moved down this far, after turning (the press's head).
    drop: f32,
}

/// What Create's renderer for `view` draws at `pos`.
fn parts(game: &Game<'_>, c: &mut Client, pos: Pos, view: View, time: f32) -> Vec<Part> {
    let standard = render_angle(view.kind, pos, view.speed, time);
    let id = glam::Mat4::IDENTITY;
    // (holder, property, orientation, spin axis, angle)
    let mut want: Vec<(&'static str, Option<(&'static str, &'static str)>, glam::Mat4, Axis, f32)> =
        Vec::new();
    let mut drop = 0.0;
    match view.kind {
        Kind::Shaft(a) => want.push(("create:hz_shaft", Some(("axis", a.name())), id, a, standard)),
        Kind::Cogwheel(a) => {
            want.push(("create:hz_cogwheel", Some(("axis", a.name())), id, a, standard))
        }
        Kind::LargeCogwheel(a) => {
            want.push(("create:hz_large_cogwheel", Some(("axis", a.name())), id, a, standard))
        }
        Kind::Gearbox(a) => {
            // GearboxRenderer: a shaft half on each side off its axis.
            for side in Dir::ALL.into_iter().filter(|s| s.axis() != a) {
                let angle = gearbox_shaft_angle(pos, &view, side, time);
                want.push(("create:hz_part_shaft_half", None, rotate_to_face(side), side.axis(), angle));
            }
        }
        Kind::CreativeMotor(f) => {
            // CreativeMotorRenderer: the shaft half out of its face.
            want.push(("create:hz_part_shaft_half", None, rotate_to_face(f), f.axis(), standard));
        }
        Kind::HandCrank(f) => {
            // HandCrankRenderer: its own model turning with the network, and
            // the handle at the crank's own chasing angle.
            want.push(("create:hz_hand_crank", Some(("facing", f.name())), id, f.axis(), standard));
            let angle = c.view.crank_angle(pos, view.speed, time);
            want.push((
                "create:hz_part_hand_crank_handle",
                None,
                rotate_to_face(f.opposite()),
                f.axis(),
                angle,
            ));
        }
        Kind::MechanicalPress(f) => {
            // MechanicalPressRenderer: a shaft through it, and the head, at
            // rest until pressing is ported.
            let a = f.axis();
            want.push(("create:hz_shaft", Some(("axis", a.name())), id, a, standard));
            want.push(("create:hz_part_press_head", None, rotate_to_face(f), a, 0.0));
            let mode = if view.press_belt {
                PressMode::Belt
            } else {
                PressMode::World
            };
            drop = press_head_offset(view.press_ticks >= 0, mode, f32::from(view.press_ticks));
        }
        Kind::Millstone => {
            // MillstoneRenderer: the inner cog, turning about Y.
            want.push(("create:hz_part_millstone_inner", None, id, Axis::Y, standard));
        }
        Kind::WaterWheel(f) => {
            // WaterWheelRenderer: the wheel itself (in oak: its planks
            // material isn't ported yet).
            want.push(("create:hz_water_wheel", Some(("facing", f.name())), id, f.axis(), standard));
        }
    }
    want.into_iter()
        .filter_map(|(block, prop, orient, axis, angle)| {
            let state = holder(game, c, block, prop)?;
            // Only the press's head moves down.
            let head = block == "create:hz_part_press_head";
            Some(Part {
                state,
                orient,
                axis,
                angle,
                drop: if head { drop } else { 0.0 },
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
        super::create_machines::draw_depots(game, camera, out);
        return;
    };
    if c.view.dim != dim_code(game.dim.id()) || c.view.blocks.is_empty() {
        drop(guard);
        super::create_machines::draw_depots(game, camera, out);
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
            game.block_model_vertices(part.state, seed, m, packed, false, out);
        }
    }
    drop(guard);
    super::create_machines::draw_depots(game, camera, out);
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
        let Some(v) = c.view.blocks.get(&hit.pos).copied() else {
            return;
        };
        v
    };
    let lines = goggle_lines(&view);
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
