// Horizon's side of contraptions (contraption.rs), injected as
// `crate::game::create_contraptions`: each mechanical piston's
// `LinearActuatorBlockEntity` tick after the kinetics', its structure taken
// out of the world (`removeBlocksFromWorld`) and put back
// (`addBlocksToWorld`), riders carried along, the moving blocks drawn on
// clients from `create:contraptions`, and running pistons saved beside the
// world (`data/create_contraptions_*.txt`).

use super::Game;
use super::create_contraption::{
    Column, Elevator, ElevatorColumn, ElevatorError,
    Actuator, ActuatorStep, Bearing, BearingContraption, BearingStep, Chassis, GantryContraption, GantryStep, Pulley,
    PulleyContraption, PulleyStep, ShaftUnder, ChassisKind, Facts, HEAD, MAX_CHASSIS_RANGE, POLE, PistonContraption,
    PistonPart, PistonState, STICKY_HEAD, StructureWorld, Sticky, attached_chassis, chassis_block, chassis_of,
    glue_group, in_span, piston_block, piston_of,
};
use super::create_actor_glue::{Mover, stop_actors, tick_actors};
use super::create_kinetics::{Dir, Kind, Pos};
use horizon_world::registry::BlockStateId;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

pub(super) const CONTRAPTIONS: &str = "create:contraptions";
/// Client to server: a chassis' range scrolled with the wrench (`R1`).
pub(super) const CHASSIS: &str = "create:chassis";
const SUPER_GLUE: &str = "create:super_glue";
/// `ChassisBlockEntity`: `range.setValue(max / 2)`.
const DEFAULT_RANGE: i32 = MAX_CHASSIS_RANGE / 2;
/// `SuperGlueSelectionHandler`: a selection's corners at most this far
/// apart.
const GLUE_REACH: i32 = 24;
const SAVE_EVERY: u64 = 1200;
const VIEW_DISTANCE: f64 = 128.0;

/// A moved block's state as text (`name[k=v,...]`), to save it.
fn state_text(game: &Game<'_>, id: BlockStateId) -> String {
    let Some(s) = game.pack.assets.registry.state(id) else {
        return "minecraft:air".to_owned();
    };
    let mut props: Vec<String> = s.properties.iter().map(|(k, v)| format!("{k}={v}")).collect();
    props.sort();
    format!("{}[{}]", s.name, props.join(","))
}

fn parse_state(game: &Game<'_>, text: &str) -> Option<BlockStateId> {
    let (name, rest) = text.split_once('[').unwrap_or((text, "]"));
    let base = game.pack.assets.registry.default_state(name)?;
    let props: Vec<(&str, &str)> = rest
        .trim_end_matches(']')
        .split(',')
        .filter_map(|kv| kv.split_once('='))
        .collect();
    if props.is_empty() {
        return Some(base);
    }
    game.with_props(base, &props)
}

/// A running piston's structure, out of the world.
struct Running {
    contraption: PistonContraption<BlockStateId>,
    /// Each block's state as text and its block entity's saved data
    /// (`StructureBlockInfo.nbt`), by the blocks' index.
    texts: Vec<String>,
    entities: Vec<Option<horizon_world::nbt::Compound>>,
    /// The moved chassis' ranges, by the blocks' index.
    ranges: Vec<(usize, i32)>,
    /// The glue moved with it (`Contraption.superglue`), where it was when
    /// the structure was assembled.
    glue: Vec<(Pos, Pos)>,
}

#[derive(Default)]
struct Piston {
    actuator: Actuator,
    running: Option<Running>,
}

#[derive(Default)]
struct State {
    root: Option<PathBuf>,
    loaded: std::collections::HashSet<String>,
    pistons: HashMap<(String, Pos), Piston>,
    ticks: u64,
    /// Pistons whose blocks clients must learn again.
    changed: std::collections::HashSet<(String, Pos)>,
    /// Chassis ranges (`ChassisBlockEntity.range`) other than the default.
    ranges: HashMap<(String, Pos), i32>,
    /// Super glue by dimension, as the blocks at its two corners
    /// (`SuperGlueEntity`'s box).
    glue: HashMap<String, Vec<(Pos, Pos)>>,
    /// Players' first glue corner (`SuperGlueSelectionHandler.firstPos`).
    selecting: HashMap<u128, (String, Pos)>,
    /// Action bar lines for players, sent at the next tick (a message from
    /// a client can't show one).
    told: Vec<(u128, horizon_client::text::Text)>,
    /// Mechanical and windmill bearings.
    bearings: HashMap<(String, Pos), BearingEntry>,
    /// Bearings clicked with an empty hand (`useItemOn`), handled at the
    /// next tick.
    clicked: Vec<(String, Pos)>,
    /// Rope pulleys.
    pulleys: HashMap<(String, Pos), PulleyEntry>,
    /// Moving gantries, by dimension.
    gantries: Vec<(String, GantryMove)>,
    /// Carriages to try assembling at the next tick (`queueAssembly`).
    gantry_queue: Vec<(String, Pos)>,
    /// Gantry shafts' speeds last tick (`onSpeedChanged`).
    shaft_rpm: HashMap<(String, Pos), f32>,
    next_gantry: u32,
    /// Elevator pulleys at work and their speed last tick.
    elevators: HashMap<(String, Pos), ElevatorEntry>,
    elevator_rpm: HashMap<(String, Pos), f32>,
    /// Elevator contact columns (`ElevatorColumn`), and the contacts given a
    /// signal last tick.
    columns: HashMap<(String, Column), ElevatorColumn>,
    contact_inputs: std::collections::HashSet<(String, Pos)>,
    /// Elevator pulleys clicked with an empty hand (`clicked`).
    elevator_clicks: Vec<(String, Pos)>,
}

/// A moving gantry (`GantryContraptionEntity`).
struct GantryMove {
    id: u32,
    contraption: GantryContraption<BlockStateId>,
    /// The anchor's corner now.
    at: [f64; 3],
    /// Its motion last tick.
    motion: [f64; 3],
    ticks: u32,
    texts: Vec<String>,
    entities: Vec<Option<horizon_world::nbt::Compound>>,
    ranges: Vec<(usize, i32)>,
    glue: Vec<(Pos, Pos)>,
}

/// What hangs on a pulley's rope, out of the world.
struct Hanging {
    contraption: PulleyContraption<BlockStateId>,
    texts: Vec<String>,
    entities: Vec<Option<horizon_world::nbt::Compound>>,
    ranges: Vec<(usize, i32)>,
    glue: Vec<(Pos, Pos)>,
}

#[derive(Default)]
struct PulleyEntry {
    pulley: Pulley,
    hanging: Option<Hanging>,
    /// An elevator's cabin on its way (elevators_tick moves it).
    elevator: bool,
}

/// An elevator pulley at work (`ElevatorPulleyBlockEntity`, assembled):
/// its cabin stands in the world at `elevator.offset` below it between
/// floors, and is taken out (as a pulley's load) while it moves.
struct ElevatorEntry {
    elevator: Elevator,
    column: Column,
}

/// A bearing's turning structure, out of the world.
struct Turning {
    contraption: BearingContraption<BlockStateId>,
    texts: Vec<String>,
    entities: Vec<Option<horizon_world::nbt::Compound>>,
    ranges: Vec<(usize, i32)>,
    glue: Vec<(Pos, Pos)>,
}

#[derive(Default)]
struct BearingEntry {
    bearing: Bearing,
    turning: Option<Turning>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn state() -> MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- The world as the structure search sees it -----------------------------

struct Scan<'a, 'g> {
    game: &'a Game<'g>,
    ranges: HashMap<Pos, i32>,
    glue: Vec<(Pos, Pos)>,
}

/// The world of `game`'s dimension, with its chassis ranges and glue.
fn scan<'a, 'g>(game: &'a Game<'g>) -> Scan<'a, 'g> {
    let dimension = game.dim.id();
    let guard = state();
    let (ranges, glue) = guard.as_ref().map_or_else(Default::default, |s| {
        let ranges = s
            .ranges
            .iter()
            .filter(|((d, _), _)| d == dimension)
            .map(|((_, p), r)| (*p, *r))
            .collect();
        (ranges, s.glue.get(dimension).cloned().unwrap_or_default())
    });
    Scan { game, ranges, glue }
}

/// The chassis at `pos`, if any, with its range.
fn chassis_at(game: &Game<'_>, pos: Pos, range: i32) -> Option<Chassis> {
    let id = game.world.block(pos[0], pos[1], pos[2]);
    let name = &game.res.facts().get(id).name;
    let (kind, sticky) = chassis_of(name)?;
    let axis = prop(game, id, "axis").and_then(|a| super::create_kinetics::Axis::parse(&a))?;
    Some(Chassis {
        kind,
        axis,
        sticky,
        range,
    })
}

fn prop(game: &Game<'_>, id: BlockStateId, key: &str) -> Option<String> {
    game.pack.assets.registry.state(id)?.properties.get(key).cloned()
}

impl Scan<'_, '_> {
    fn id(&self, pos: Pos) -> BlockStateId {
        self.game.world.block(pos[0], pos[1], pos[2])
    }
    fn named(&self, name: &str, props: &[(&str, &str)]) -> BlockStateId {
        let registry = self.game.pack.assets.registry;
        registry
            .default_state(name)
            .and_then(|s| self.game.with_props(s, props))
            .unwrap_or_else(|| self.game.res.facts().air)
    }
}

/// The part of the piston's a state is, if any.
fn part_of(game: &Game<'_>, id: BlockStateId) -> Option<PistonPart> {
    let facts = game.res.facts().get(id);
    let facing = || prop(game, id, "facing").and_then(|f| Dir::parse(&f));
    match &*facts.name {
        POLE => Some(PistonPart::Pole { facing: facing()? }),
        HEAD => Some(PistonPart::Head {
            facing: facing()?,
            sticky: false,
        }),
        STICKY_HEAD => Some(PistonPart::Head {
            facing: facing()?,
            sticky: true,
        }),
        name => {
            let (sticky, _, state) = piston_of(name)?;
            Some(PistonPart::Piston {
                facing: facing()?,
                sticky,
                state,
            })
        }
    }
}

impl StructureWorld for Scan<'_, '_> {
    type State = BlockStateId;
    fn loaded(&self, pos: Pos) -> bool {
        self.game.world.loaded(pos[0], pos[2])
    }
    fn outside_height(&self, pos: Pos) -> bool {
        let w = &self.game.world;
        pos[1] < w.shape.min_y || pos[1] >= w.shape.max_y()
    }
    fn facts(&self, pos: Pos) -> Facts {
        let game = self.game;
        let id = self.id(pos);
        let f = game.res.facts().get(id);
        let class = |c: &str| f.classes.iter().any(|k| &**k == c);
        let has = |p: &str| prop(game, id, p);
        let facing = || has("facing").and_then(|d| Dir::parse(&d));
        let solid = !game.block_collision_shape(pos).boxes().is_empty();
        let carpet = class("WoolCarpetBlock");
        // isBrittleFallback.
        let brittle = has("hanging").is_some()
            || class("LadderBlock")
            || class("BaseTorchBlock")
            || class("SignBlock")
            || class("BasePressurePlateBlock")
            || (class("FaceAttachedHorizontalDirectionalBlock") && !class("GrindstoneBlock"))
            || class("BaseRailBlock")
            || class("DiodeBlock")
            || class("RedStoneWireBlock")
            || carpet
            || game.block_in_tag("create:brittle", &f.name);
        // isMovementNecessaryFallback.
        let necessary = brittle
            || game.block_in_tag("create:movable_empty_collider", &f.name)
            || (solid && !f.replaceable);
        let push = game.res.behaviours().facts(id).push;
        use horizon_world::level::data::PushReaction;
        // isMovementAllowedFallback (pistons are judged in contraption.rs).
        let allowed = f.hardness != -1.0
            && !game.block_in_tag("c:relocation_not_supported", &f.name)
            && !game.block_in_tag("create:non_movable", &f.name)
            && (class("GrindstoneBlock") || push != PushReaction::Immoveable);
        // isBlockAttachedTowardsFallback, for the blocks vanilla has.
        let mut attached = [false; 6];
        for d in Dir::ALL {
            let wall = || facing() == Some(d.opposite());
            attached[d as usize] = if class("LadderBlock")
                || class("WallTorchBlock")
                || class("WallSignBlock")
                || class("RedstoneWallTorchBlock")
            {
                wall()
            } else if class("DoorBlock") {
                (has("half").as_deref() == Some("lower") && d == Dir::Up) || d == Dir::Down
            } else if class("FaceAttachedHorizontalDirectionalBlock") {
                match has("face").as_deref() {
                    Some("ceiling") => d == Dir::Up,
                    Some("floor") => d == Dir::Down,
                    _ => wall(),
                }
            } else if let Some(h) = has("hanging") {
                d == if h == "true" { Dir::Up } else { Dir::Down }
            } else {
                (class("StandingSignBlock")
                    || class("BasePressurePlateBlock")
                    || class("FlowerPotBlock")
                    || class("DiodeBlock")
                    || class("RedStoneWireBlock")
                    || carpet
                    || class("BaseTorchBlock")
                    || class("BaseRailBlock"))
                    && d == Dir::Down
            };
        }
        // isNotSupportiveFallback.
        let mut not_supportive = [brittle; 6];
        if carpet {
            not_supportive = std::array::from_fn(|i| Dir::ALL[i] == Dir::Up);
        }
        Facts {
            necessary,
            allowed,
            brittle,
            carpet,
            push_only: push == PushReaction::Push,
            sticky: match &*f.name {
                "minecraft:slime_block" => Some(Sticky::Slime),
                "minecraft:honey_block" => Some(Sticky::Honey),
                _ => None,
            },
            attached,
            not_supportive,
            solid,
            replaceable: f.replaceable,
            part: part_of(game, id),
            chassis: chassis_at(game, pos, self.ranges.get(&pos).copied().unwrap_or(DEFAULT_RANGE)),
            sail: game.block_in_tag("create:windmill_sails", &f.name),
            magnet: &*f.name == MAGNET,
        }
    }
    fn glue(&self, pos: Pos, dir: Dir) -> Option<usize> {
        let to = [0, 1, 2].map(|k| pos[k] + dir.normal()[k]);
        self.glue
            .iter()
            .position(|(a, b)| in_span(*a, *b, pos) && in_span(*a, *b, to))
    }
    fn state(&self, pos: Pos) -> BlockStateId {
        self.id(pos)
    }
    fn head(&self, facing: Dir, sticky: bool) -> BlockStateId {
        self.named(if sticky { STICKY_HEAD } else { HEAD }, &[("facing", facing.name())])
    }
    fn pole(&self, facing: Dir) -> BlockStateId {
        self.named(POLE, &[("facing", facing.name())])
    }
}

fn set(game: &mut Game<'_>, pos: Pos, id: BlockStateId, flags: i32) {
    let dimension = game.dim.id().to_owned();
    game.native_set_block(&dimension, pos, id, flags);
}

/// The piston at `pos` in `state`, its facing and axis kept.
fn set_piston(game: &mut Game<'_>, pos: Pos, state: PistonState) {
    let id = game.world.block(pos[0], pos[1], pos[2]);
    let Some(PistonPart::Piston { facing, sticky, .. }) = part_of(game, id) else {
        return;
    };
    let along = game
        .pack
        .assets
        .registry
        .state(id)
        .and_then(|s| piston_of(&s.name))
        .is_some_and(|(_, along, _)| along);
    let scan = Scan {
        game,
        ranges: HashMap::new(),
        glue: Vec::new(),
    };
    let to = scan.named(piston_block(sticky, along, state), &[("facing", facing.name())]);
    use super::native_hooks::flags;
    set(game, pos, to, flags::UPDATE_ALL | flags::UPDATE_KNOWN_SHAPE);
}

// ---- Assembly and disassembly ------------------------------------------------

/// The saved data of the block entity at `at` (`StructureBlockInfo.nbt`),
/// taken out of the world.
fn capture(game: &mut Game<'_>, at: Pos, state: BlockStateId) -> Option<horizon_world::nbt::Compound> {
    let key = (at[0], at[1], at[2]);
    let facts = game.res.facts().get(state);
    let name = facts.name.to_string();
    let has_entity = facts.block_entity;
    // A deployer's state (kept by the native mod) goes with its block.
    if name == super::create_kinetics::DEPLOYER || name == super::create_kinetics::DEPLOYER_ALONG {
        let text = super::create_arms::take_deployer(game.dim.id(), at);
        let mut c = horizon_world::nbt::Compound::new();
        c.insert("HzDeployer".into(), horizon_world::nbt::Tag::String(text.as_str().into()));
        return Some(c);
    }
    let entity = game.world.block_entities.remove(&key).filter(|_| has_entity)?;
    match horizon_client::save::encode_block_entity(key, &entity, &name) {
        horizon_world::nbt::Tag::Compound(c) => Some(c),
        _ => None,
    }
}

/// `removeBlocksFromWorld`: the blocks gone without drops.
fn remove_blocks(game: &mut Game<'_>, at: &[Pos]) {
    use super::native_hooks::flags;
    let air = game.res.facts().air;
    for &p in at {
        set(
            game,
            p,
            air,
            flags::UPDATE_MOVE_BY_PISTON | flags::UPDATE_SUPPRESS_DROPS | flags::UPDATE_KNOWN_SHAPE | flags::UPDATE_ALL,
        );
    }
}

/// `addBlocksToWorld` for one block: put at `at` with its block entity's
/// data, or dropped where it can't go (bedrock, or a solid block in the way
/// of one without collision).
fn put_back(game: &mut Game<'_>, at: Pos, state: BlockStateId, solid: bool, nbt: Option<horizon_world::nbt::Compound>) {
    use super::native_hooks::flags;
    let here = game.world.block(at[0], at[1], at[2]);
    let here_facts = game.res.facts().get(here);
    let here_solid = !game.block_collision_shape(at).boxes().is_empty();
    if here_facts.hardness == -1.0 || (!solid && here_solid) {
        let item = game.res.facts().get(state).item.to_string();
        if item != "minecraft:air" {
            let centre = at.map(|v| f64::from(v) + 0.5);
            game.spawn_item(horizon_client::items::ItemStack::new(&item, 1), centre, [0.0, 0.1, 0.0], 10);
        }
        return;
    }
    if !game.world.is_air(here) && !here_facts.replaceable {
        let _ = game.destroy_block_dropping(at);
    }
    set(game, at, state, flags::UPDATE_MOVE_BY_PISTON | flags::UPDATE_ALL);
    // A deployer's state back with the native mod.
    if let Some(nbt) = &nbt
        && let Some(horizon_world::nbt::Tag::String(text)) = nbt.get(&"HzDeployer".into())
    {
        let dim = game.dim.id().to_owned();
        super::create_arms::put_deployer(&dim, at, &text.to_string_lossy());
        return;
    }
    // The block entity's data back, at its new place (`loadWithComponents`).
    if let Some(mut nbt) = nbt {
        use horizon_world::nbt::Tag;
        for (k, v) in [("x", at[0]), ("y", at[1]), ("z", at[2])] {
            nbt.insert(k.into(), Tag::Int(v));
        }
        if let Some((key, entity)) = horizon_client::save::decode_block_entity(&Tag::Compound(nbt)) {
            game.world.block_entities.insert(key, entity);
        }
    }
}

/// `assemble`: the structure built, checked and taken out of the world.
fn assemble(game: &mut Game<'_>, pos: Pos, facing: Dir, rpm: f32, actuator: &mut Actuator) -> Option<Running> {
    let retract = actuator.movement_speed(rpm, facing) < 0.0;
    let contraption = {
        let scan = scan(game);
        let c = match PistonContraption::assemble(&scan, pos, facing, retract) {
            Ok(Some(c)) => c,
            Ok(None) => return None,
            Err(e) => {
                log::debug!("Create: piston at {pos:?} can't move: {e:?}");
                return None;
            }
        };
        if !actuator.start(&c, rpm, facing, |g, d| c.colliding_with_world(&scan, g, d)) {
            return None;
        }
        c
    };
    // removeBlocksFromWorld: block entities' data kept, the blocks gone
    // without drops (UPDATE_MOVE_BY_PISTON), the piston MOVING.
    let mut texts = Vec::with_capacity(contraption.blocks.len());
    let mut entities = Vec::with_capacity(contraption.blocks.len());
    let piston = contraption.piston_pos();
    for b in &contraption.blocks {
        texts.push(state_text(game, b.state));
        let at = contraption.world_pos(b.local, contraption.initial_extension_progress);
        entities.push(if at == piston { None } else { capture(game, at, b.state) });
    }
    remove_blocks(game, &contraption.removals());
    set_piston(game, pos, PistonState::Moving);
    // The chassis' ranges and the glue go with the blocks.
    let dimension = game.dim.id().to_owned();
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    let mut ranges = Vec::new();
    for (i, b) in contraption.blocks.iter().enumerate() {
        let at = contraption.world_pos(b.local, contraption.initial_extension_progress);
        if let Some(r) = s.ranges.remove(&(dimension.clone(), at)) {
            ranges.push((i, r));
        }
    }
    let all = s.glue.entry(dimension).or_default();
    let glue: Vec<(Pos, Pos)> = contraption.glue.iter().filter_map(|&g| all.get(g).copied()).collect();
    all.retain(|g| !glue.contains(g));
    Some(Running {
        contraption,
        texts,
        entities,
        ranges,
        glue,
    })
}

/// `disassemble` at `grid_offset`: the piston EXTENDED (RETRACTED when its
/// head lands back in it), the blocks put back, those that can't go where
/// they land dropped.
fn disassemble(game: &mut Game<'_>, pos: Pos, running: Running, grid_offset: i32) {
    set_piston(game, pos, PistonState::Extended);
    let c = &running.contraption;
    let (placements, retracted) = c.placements(grid_offset);
    for (at, i) in placements {
        put_back(game, at, c.blocks[i].state, c.blocks[i].solid, running.entities[i].clone());
    }
    if retracted {
        set_piston(game, pos, PistonState::Retracted);
    }
    let n = c.orientation.normal();
    let moved = grid_offset - c.initial_extension_progress;
    let shift = |p: Pos| [0, 1, 2].map(|k| p[k] + n[k] * moved);
    let dimension = game.dim.id().to_owned();
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    for &(i, r) in &running.ranges {
        if let Some(b) = c.blocks.get(i) {
            s.ranges.insert((dimension.clone(), c.world_pos(b.local, grid_offset)), r);
        }
    }
    s.glue
        .entry(dimension)
        .or_default()
        .extend(running.glue.iter().map(|&(a, b)| (shift(a), shift(b))));
}

// ---- Riders --------------------------------------------------------------------

/// Entities on or in the moved blocks go with them (`ContraptionCollider`
/// carrying what stands on a contraption): `blocks` are the solid blocks'
/// world places before the move, `delta` the move. What stands on one is set
/// on its top after the move, moving as it does; what is inside one moves by
/// `delta`. Horizon has no collision with the moving blocks themselves, so
/// this is what holds the player up.
fn carry(game: &mut Game<'_>, dimension: Option<&str>, blocks: &[[f64; 3]], delta: [f64; 3]) {
    if blocks.is_empty() || delta == [0.0; 3] {
        return;
    }
    // The top of the block an entity of half width `half` at `p` stands on
    // (its feet near that top), or whether it is inside one.
    let standing = |p: [f64; 3], half: f64| -> Option<f64> {
        blocks
            .iter()
            .filter(|b| {
                p[0] + half > b[0] && p[0] - half < b[0] + 1.0 && p[2] + half > b[2] && p[2] - half < b[2] + 1.0
            })
            .map(|b| b[1] + 1.0)
            .filter(|top| p[1] >= top - 0.6 && p[1] <= top + 0.3)
            .reduce(f64::max)
    };
    let inside = |p: [f64; 3], half: f64| {
        blocks.iter().any(|b| {
            p[0] + half > b[0]
                && p[0] - half < b[0] + 1.0
                && p[2] + half > b[2]
                && p[2] - half < b[2] + 1.0
                && p[1] + 1.8 > b[1]
                && p[1] < b[1] + 1.0
        })
    };
    // The window's own player, on its side (as Create's client collides it);
    // the server carries mobs and items.
    let Some(dimension) = dimension else {
        let Some(p) = game.player.as_mut() else {
            return;
        };
        let at = [p.position.x, p.position.y, p.position.z];
        if let Some(top) = standing(at, 0.3) {
            p.position.x += delta[0];
            p.position.z += delta[2];
            p.position.y = top + delta[1];
            // Its own physics moves it on with the block until the next tick.
            p.velocity.y = delta[1];
            p.ground.on_ground = true;
            p.fall_distance = 0.0;
        } else if inside(at, 0.3) {
            p.position.x += delta[0];
            p.position.y += delta[1];
            p.position.z += delta[2];
        }
        return;
    };
    for d in game.dropped.iter_mut() {
        let at = [d.position.x, d.position.y, d.position.z];
        if let Some(top) = standing(at, 0.125) {
            d.position.x += delta[0];
            d.position.z += delta[2];
            d.position.y = top + delta[1];
            d.velocity.y = delta[1];
        } else if inside(at, 0.125) {
            d.position.x += delta[0];
            d.position.y += delta[1];
            d.position.z += delta[2];
        }
    }
    let mobs: Vec<(u32, [f64; 3])> = game
        .native_mobs(dimension)
        .into_iter()
        .filter_map(|m| {
            let to = match standing(m.pos, 0.3) {
                Some(top) => [m.pos[0] + delta[0], top + delta[1], m.pos[2] + delta[2]],
                None if inside(m.pos, 0.3) => [0, 1, 2].map(|i| m.pos[i] + delta[i]),
                None => return None,
            };
            Some((m.id, to))
        })
        .collect();
    for (id, to) in mobs {
        game.native_teleport_mob(dimension, id, to);
    }
}

// ---- The tick ------------------------------------------------------------------

fn save_path(root: &Path, dimension: &str) -> PathBuf {
    root.join("data")
        .join(format!("create_contraptions_{}.txt", dimension.replace(':', "_")))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn limits() -> horizon_world::nbt::Limits {
    horizon_world::nbt::Limits::default()
}

/// `range x y z r` per chassis and `glue ax ay az bx by bz` per glue, then
/// one piston per paragraph: `piston x y z offset length running`, then for
/// a running one `c ax ay az facing sticky length progress`, a `b lx ly lz
/// solid brittle pole state nbt` line per block, `r index range` per chassis
/// and `g ax ay az bx by bz` per glue moved with it.
fn save_text(s: &State, dimension: &str) -> String {
    let mut out = String::from("create-contraptions 1\n");
    let corners = |(a, b): &(Pos, Pos)| format!("{} {} {} {} {} {}", a[0], a[1], a[2], b[0], b[1], b[2]);
    for ((d, p), r) in &s.ranges {
        if d == dimension {
            out += &format!("range {} {} {} {r}\n", p[0], p[1], p[2]);
        }
    }
    for g in s.glue.get(dimension).into_iter().flatten() {
        out += &format!("glue {}\n", corners(g));
    }
    for ((d, p), piston) in &s.pistons {
        if d != dimension {
            continue;
        }
        let a = &piston.actuator;
        out += &format!(
            "piston {} {} {} {} {} {}\n",
            p[0],
            p[1],
            p[2],
            a.offset,
            a.extension_length,
            u8::from(a.running)
        );
        let Some(r) = &piston.running else {
            continue;
        };
        let c = &r.contraption;
        out += &format!(
            "c {} {} {} {} {} {} {}\n",
            c.anchor[0],
            c.anchor[1],
            c.anchor[2],
            c.orientation.name(),
            u8::from(c.sticky),
            c.extension_length,
            c.initial_extension_progress
        );
        out += &cargo_text(&c.blocks, &r.texts, &r.entities, &r.ranges, &r.glue);
    }
    // `bearing x y z angle running speed`, then for a turning one `t facing
    // sails` and its blocks as a piston's.
    for ((d, p), b) in &s.bearings {
        if d != dimension {
            continue;
        }
        let bearing = &b.bearing;
        out += &format!(
            "bearing {} {} {} {} {} {}\n",
            p[0],
            p[1],
            p[2],
            bearing.angle,
            u8::from(bearing.running),
            bearing.last_rpm
        );
        if let Some(t) = &b.turning {
            let c = &t.contraption;
            out += &format!("t {} {}\n", c.facing.name(), c.sails);
            out += &cargo_text(&c.blocks, &t.texts, &t.entities, &t.ranges, &t.glue);
        }
    }
    // `pulley x y z offset running carrying speed`, then for a loaded one
    // `h ax ay az initial` and its blocks.
    for ((d, p), e) in &s.pulleys {
        if d != dimension {
            continue;
        }
        let q = &e.pulley;
        out += &format!(
            "pulley {} {} {} {} {} {} {} {}\n",
            p[0],
            p[1],
            p[2],
            q.offset,
            u8::from(q.running),
            u8::from(q.carrying),
            q.last_rpm,
            u8::from(e.elevator)
        );
        if let Some(h) = &e.hanging {
            let c = &h.contraption;
            out += &format!("h {} {} {} {}\n", c.anchor[0], c.anchor[1], c.anchor[2], c.initial_offset);
            out += &cargo_text(&c.blocks, &h.texts, &h.entities, &h.ranges, &h.glue);
        }
    }
    for ((d, p), e) in &s.elevators {
        if d != dimension {
            continue;
        }
        let el = &e.elevator;
        out += &format!(
            "elevator {} {} {} {} {} {} {} {} {} {}\n",
            p[0],
            p[1],
            p[2],
            el.offset,
            el.contact_y_offset,
            el.min_contact_y,
            el.max_contact_y,
            e.column.x,
            e.column.z,
            e.column.side.name()
        );
    }
    // `gantry ax ay az ticks facing cx cy cz`, then its blocks.
    for (d, g) in &s.gantries {
        if d != dimension {
            continue;
        }
        let c = &g.contraption;
        out += &format!(
            "gantry {} {} {} {} {} {} {} {}\n",
            g.at[0],
            g.at[1],
            g.at[2],
            g.ticks,
            c.facing.name(),
            c.anchor[0],
            c.anchor[1],
            c.anchor[2]
        );
        out += &cargo_text(&c.blocks, &g.texts, &g.entities, &g.ranges, &g.glue);
    }
    out
}

/// A moved structure's `b`, `r` and `g` lines.
fn cargo_text(
    blocks: &[super::create_contraption::Moved<BlockStateId>],
    texts: &[String],
    entities: &[Option<horizon_world::nbt::Compound>],
    ranges: &[(usize, i32)],
    glue: &[(Pos, Pos)],
) -> String {
    let mut out = String::new();
    for (i, b) in blocks.iter().enumerate() {
        let nbt = entities[i]
            .as_ref()
            .and_then(|n| horizon_world::nbt::write_network(&horizon_world::nbt::Tag::Compound(n.clone()), limits()).ok())
            .map_or_else(|| "-".to_owned(), |b| hex(&b));
        out += &format!(
            "b {} {} {} {} {} {} {} {}\n",
            b.local[0],
            b.local[1],
            b.local[2],
            u8::from(b.solid),
            u8::from(b.brittle),
            u8::from(b.pole),
            texts[i],
            nbt
        );
    }
    for (i, range) in ranges {
        out += &format!("r {i} {range}\n");
    }
    for (a, b) in glue {
        out += &format!("g {} {} {} {} {} {}\n", a[0], a[1], a[2], b[0], b[1], b[2]);
    }
    out
}

fn save(s: &State) {
    let Some(root) = &s.root else {
        return;
    };
    let _ = std::fs::create_dir_all(root.join("data"));
    let dims: std::collections::HashSet<&String> = (s.pistons.keys().map(|(d, _)| d))
        .chain(s.ranges.keys().map(|(d, _)| d))
        .chain(s.glue.keys())
        .chain(s.bearings.keys().map(|(d, _)| d))
        .chain(s.pulleys.keys().map(|(d, _)| d))
        .chain(s.gantries.iter().map(|(d, _)| d))
        .collect();
    for d in dims {
        if let Err(e) = std::fs::write(save_path(root, d), save_text(s, d)) {
            log::warn!("Create: can't save the contraptions of {d}: {e}");
        }
    }
}

/// Reads a saved dimension: the actuators now, contraptions' blocks parsed
/// against the registry.
fn load(game: &Game<'_>, s: &mut State, dimension: &str) {
    let Some(root) = &s.root else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(save_path(root, dimension)) else {
        return;
    };
    let mut current: Option<Target> = None;
    for line in text.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        match f.first().copied() {
            Some("piston") if f.len() >= 7 => {
                let (Ok(x), Ok(y), Ok(z)) = (f[1].parse(), f[2].parse(), f[3].parse()) else {
                    continue;
                };
                let pos = [x, y, z];
                let entry = s.pistons.entry((dimension.to_owned(), pos)).or_default();
                entry.actuator.offset = f[4].parse().unwrap_or(0.0);
                entry.actuator.extension_length = f[5].parse().unwrap_or(0);
                entry.actuator.running = f[6] == "1";
                entry.actuator.assemble_next_tick = false;
                current = Some(Target::Piston(pos));
            }
            Some("bearing") if f.len() >= 6 => {
                let (Ok(x), Ok(y), Ok(z)) = (f[1].parse(), f[2].parse(), f[3].parse()) else {
                    continue;
                };
                let pos = [x, y, z];
                let entry = s.bearings.entry((dimension.to_owned(), pos)).or_default();
                entry.bearing.angle = f[4].parse().unwrap_or(0.0);
                entry.bearing.running = f[5] == "1";
                // KineticBlockEntity.read: a loaded speed isn't a change.
                entry.bearing.last_rpm = f.get(6).and_then(|v| v.parse().ok()).unwrap_or(0.0);
                current = Some(Target::Bearing(pos));
            }
            Some("t") if f.len() >= 3 => {
                let Some(Target::Bearing(pos)) = current else { continue };
                let Some(facing) = Dir::parse(f[1]) else { continue };
                let entry = s.bearings.entry((dimension.to_owned(), pos)).or_default();
                entry.turning = Some(Turning {
                    contraption: BearingContraption {
                        facing,
                        anchor: [0, 1, 2].map(|k| pos[k] + facing.normal()[k]),
                        blocks: Vec::new(),
                        sails: f[2].parse().unwrap_or(0),
                        glue: Vec::new(),
                    },
                    texts: Vec::new(),
                    entities: Vec::new(),
                    ranges: Vec::new(),
                    glue: Vec::new(),
                });
            }
            // `elevator x y z offset contact_y min max column_x column_z side`.
            Some("elevator") if f.len() >= 11 => {
                let n: Vec<i32> = [1, 2, 3, 5, 6, 7, 8, 9].iter().filter_map(|&i| f[i].parse().ok()).collect();
                let (Some(side), Ok(offset), [x, y, z, cy, min, max, cx, cz]) =
                    (Dir::parse(f[10]), f[4].parse::<f32>(), n.as_slice())
                else {
                    continue;
                };
                s.elevators.insert(
                    (dimension.to_owned(), [*x, *y, *z]),
                    ElevatorEntry {
                        elevator: Elevator {
                            offset,
                            prev_speed: 0.0,
                            contact_y_offset: *cy,
                            min_contact_y: *min,
                            max_contact_y: *max,
                        },
                        column: Column { x: *cx, z: *cz, side },
                    },
                );
            }
            Some("pulley") if f.len() >= 8 => {
                let (Ok(x), Ok(y), Ok(z)) = (f[1].parse(), f[2].parse(), f[3].parse()) else {
                    continue;
                };
                let pos = [x, y, z];
                let entry = s.pulleys.entry((dimension.to_owned(), pos)).or_default();
                entry.pulley.offset = f[4].parse().unwrap_or(0.0);
                entry.pulley.running = f[5] == "1";
                entry.pulley.carrying = f[6] == "1";
                entry.pulley.last_rpm = f[7].parse().unwrap_or(0.0);
                entry.pulley.assemble_next_tick = false;
                entry.elevator = f.get(8) == Some(&"1");
                current = Some(Target::Pulley(pos));
            }
            Some("gantry") if f.len() >= 9 => {
                let at: Vec<f64> = f[1..4].iter().filter_map(|v| v.parse().ok()).collect();
                let anchor: Vec<i32> = f[6..9].iter().filter_map(|v| v.parse().ok()).collect();
                let Some(facing) = Dir::parse(f[5]) else { continue };
                if at.len() != 3 || anchor.len() != 3 {
                    continue;
                }
                let id = s.next_gantry;
                s.next_gantry = s.next_gantry.wrapping_add(1);
                s.gantries.push((
                    dimension.to_owned(),
                    GantryMove {
                        id,
                        contraption: GantryContraption {
                            facing,
                            anchor: [anchor[0], anchor[1], anchor[2]],
                            blocks: Vec::new(),
                            glue: Vec::new(),
                        },
                        at: [at[0], at[1], at[2]],
                        motion: [0.0; 3],
                        ticks: f[4].parse().unwrap_or(3),
                        texts: Vec::new(),
                        entities: Vec::new(),
                        ranges: Vec::new(),
                        glue: Vec::new(),
                    },
                ));
                current = Some(Target::Gantry(s.gantries.len() - 1));
            }
            Some("h") if f.len() >= 5 => {
                let Some(Target::Pulley(pos)) = current else { continue };
                let n: Vec<i32> = f[1..5].iter().filter_map(|v| v.parse().ok()).collect();
                if n.len() != 4 {
                    continue;
                }
                let entry = s.pulleys.entry((dimension.to_owned(), pos)).or_default();
                entry.hanging = Some(Hanging {
                    contraption: PulleyContraption {
                        anchor: [n[0], n[1], n[2]],
                        initial_offset: n[3],
                        blocks: Vec::new(),
                        glue: Vec::new(),
                    },
                    texts: Vec::new(),
                    entities: Vec::new(),
                    ranges: Vec::new(),
                    glue: Vec::new(),
                });
            }
            Some("c") if f.len() >= 8 => {
                let Some(Target::Piston(pos)) = current else { continue };
                let (Ok(x), Ok(y), Ok(z)) = (f[1].parse(), f[2].parse(), f[3].parse()) else {
                    continue;
                };
                let Some(orientation) = Dir::parse(f[4]) else { continue };
                let entry = s.pistons.entry((dimension.to_owned(), pos)).or_default();
                entry.running = Some(Running {
                    contraption: PistonContraption {
                        orientation,
                        sticky: f[5] == "1",
                        anchor: [x, y, z],
                        blocks: Vec::new(),
                        extension_length: f[6].parse().unwrap_or(0),
                        initial_extension_progress: f[7].parse().unwrap_or(0),
                        glue: Vec::new(),
                    },
                    texts: Vec::new(),
                    entities: Vec::new(),
                    ranges: Vec::new(),
                    glue: Vec::new(),
                });
            }
            Some("range") if f.len() >= 5 => {
                let n: Vec<i32> = f[1..5].iter().filter_map(|v| v.parse().ok()).collect();
                if n.len() == 4 {
                    s.ranges.insert((dimension.to_owned(), [n[0], n[1], n[2]]), n[3]);
                }
            }
            Some(tag @ ("glue" | "g")) if f.len() >= 7 => {
                let n: Vec<i32> = f[1..7].iter().filter_map(|v| v.parse().ok()).collect();
                if n.len() != 6 {
                    continue;
                }
                let g = ([n[0], n[1], n[2]], [n[3], n[4], n[5]]);
                if tag == "glue" {
                    s.glue.entry(dimension.to_owned()).or_default().push(g);
                } else if let Some(c) = cargo(s, dimension, current) {
                    c.glue.push(g);
                }
            }
            Some("r") if f.len() >= 3 => {
                let (Ok(i), Ok(range)) = (f[1].parse(), f[2].parse()) else {
                    continue;
                };
                if let Some(c) = cargo(s, dimension, current) {
                    c.ranges.push((i, range));
                }
            }
            Some("b") if f.len() >= 9 => {
                let Some(r) = cargo(s, dimension, current) else {
                    continue;
                };
                let (Ok(x), Ok(y), Ok(z)) = (f[1].parse(), f[2].parse(), f[3].parse()) else {
                    continue;
                };
                let state = parse_state(game, f[7]).unwrap_or_else(|| game.res.facts().air);
                let nbt = unhex(f[8])
                    .and_then(|b| horizon_world::nbt::read_network(&b, limits()).ok())
                    .and_then(|(t, _)| match t {
                        horizon_world::nbt::Tag::Compound(c) => Some(c),
                        _ => None,
                    });
                r.blocks.push(super::create_contraption::Moved {
                    local: [x, y, z],
                    state,
                    solid: f[4] == "1",
                    brittle: f[5] == "1",
                    pole: f[6] == "1",
                });
                r.texts.push(f[7].to_owned());
                r.entities.push(nbt);
            }
            _ => {}
        }
    }
    // Running pistons without their blocks (an older file) stop.
    for ((d, _), p) in s.pistons.iter_mut() {
        if d == dimension && p.actuator.running && p.running.is_none() {
            p.actuator.running = false;
        }
    }
    for ((d, _), b) in s.bearings.iter_mut() {
        if d == dimension && b.bearing.running && b.turning.is_none() {
            b.bearing.running = false;
        }
    }
}

/// What the `b`, `r` and `g` lines of a save belong to.
#[derive(Clone, Copy)]
enum Target {
    Piston(Pos),
    Bearing(Pos),
    Pulley(Pos),
    Gantry(usize),
}

/// A moved structure's parts, a piston's or a bearing's.
struct Cargo<'a> {
    blocks: &'a mut Vec<super::create_contraption::Moved<BlockStateId>>,
    texts: &'a mut Vec<String>,
    entities: &'a mut Vec<Option<horizon_world::nbt::Compound>>,
    ranges: &'a mut Vec<(usize, i32)>,
    glue: &'a mut Vec<(Pos, Pos)>,
}

fn cargo<'a>(s: &'a mut State, dimension: &str, target: Option<Target>) -> Option<Cargo<'a>> {
    match target? {
        Target::Piston(pos) => {
            let r = s.pistons.get_mut(&(dimension.to_owned(), pos))?.running.as_mut()?;
            Some(Cargo {
                blocks: &mut r.contraption.blocks,
                texts: &mut r.texts,
                entities: &mut r.entities,
                ranges: &mut r.ranges,
                glue: &mut r.glue,
            })
        }
        Target::Gantry(i) => {
            let g = &mut s.gantries.get_mut(i)?.1;
            Some(Cargo {
                blocks: &mut g.contraption.blocks,
                texts: &mut g.texts,
                entities: &mut g.entities,
                ranges: &mut g.ranges,
                glue: &mut g.glue,
            })
        }
        Target::Pulley(pos) => {
            let h = s.pulleys.get_mut(&(dimension.to_owned(), pos))?.hanging.as_mut()?;
            Some(Cargo {
                blocks: &mut h.contraption.blocks,
                texts: &mut h.texts,
                entities: &mut h.entities,
                ranges: &mut h.ranges,
                glue: &mut h.glue,
            })
        }
        Target::Bearing(pos) => {
            let t = s.bearings.get_mut(&(dimension.to_owned(), pos))?.turning.as_mut()?;
            Some(Cargo {
                blocks: &mut t.contraption.blocks,
                texts: &mut t.texts,
                entities: &mut t.entities,
                ranges: &mut t.ranges,
                glue: &mut t.glue,
            })
        }
    }
}

/// After the kinetics' tick of `dimension`: every mechanical piston's
/// `LinearActuatorBlockEntity.tick`.
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str) {
    if game.dim.id() != dimension {
        return;
    }
    let found: Vec<(Pos, Dir, f32)> = super::create_glue::with_kinetics(game, |k, level| {
        k.positions()
            .into_iter()
            .filter_map(|p| match k.get(p).map(|e| e.kind) {
                Some(Kind::MechanicalPiston(f, ..)) => Some((p, f, k.speed_of(level, p))),
                _ => None,
            })
            .collect()
    });
    let mut taken: HashMap<Pos, Piston> = {
        let mut guard = state();
        let s = guard.get_or_insert_with(State::default);
        if s.root.is_none() {
            s.root = Some(game.save.root.clone());
        }
        if s.loaded.insert(dimension.to_owned()) {
            load(game, s, dimension);
        }
        s.ticks += 1;
        let keys: Vec<(String, Pos)> = s.pistons.keys().filter(|(d, _)| d == dimension).cloned().collect();
        keys.into_iter()
            .filter_map(|k| s.pistons.remove(&k).map(|p| (k.1, p)))
            .collect()
    };
    let mut changed = Vec::new();
    for (pos, facing, rpm) in &found {
        let (pos, facing, rpm) = (*pos, *facing, *rpm);
        if !game.world.loaded(pos[0], pos[2]) {
            continue;
        }
        let mut piston = taken.remove(&pos).unwrap_or_default();
        piston.actuator.speed(rpm);
        let anchor = piston
            .running
            .as_ref()
            .map_or([pos[0] + facing.normal()[0], pos[1] + facing.normal()[1], pos[2] + facing.normal()[2]], |r| {
                r.contraption.anchor
            });
        let before = piston.actuator.offset;
        // The structure's actors, before it moves (`tickActors`).
        let stalled = match &mut piston.running {
            Some(r) if piston.actuator.running => {
                let c = &r.contraption;
                let entities = &mut r.entities;
                let n = facing.normal().map(f64::from);
                let off = f64::from(before);
                let blocks = || c.blocks.iter().map(|b| (b.local, b.state)).collect();
                let place = |l: Pos, v: [f64; 3]| {
                    let w = c.world_pos(l, 0);
                    [0, 1, 2].map(|k| f64::from(w[k]) + 0.5 + v[k] + n[k] * off)
                };
                tick_actors(game, Mover::Piston(pos), &blocks, &place, &|v| v, &|v| v, None, entities)
            }
            _ => false,
        };
        let step = {
            let scan = scan(game);
            let running = piston.running.as_ref();
            piston.actuator.tick(rpm, facing, anchor, stalled, |g, d| {
                running.is_some_and(|r| r.contraption.colliding_with_world(&scan, g, d))
            })
        };
        match step {
            ActuatorStep::Idle => {}
            ActuatorStep::Assemble => {
                if let Some(r) = assemble(game, pos, facing, rpm, &mut piston.actuator) {
                    piston.running = Some(r);
                    changed.push(pos);
                }
            }
            ActuatorStep::Moved => {
                if let Some(r) = &piston.running {
                    let n = facing.normal();
                    let step = f64::from(piston.actuator.offset - before);
                    let c = &r.contraption;
                    let blocks: Vec<[f64; 3]> = c
                        .blocks
                        .iter()
                        .filter(|b| b.solid)
                        .map(|b| {
                            let w = c.world_pos(b.local, 0);
                            [0, 1, 2].map(|i| f64::from(w[i]) + f64::from(n[i]) * f64::from(before))
                        })
                        .collect();
                    let delta = [0, 1, 2].map(|i| f64::from(n[i]) * step);
                    carry(game, Some(dimension), &blocks, delta);
                }
            }
            ActuatorStep::Disassemble { grid_offset } => {
                stop_actors(game, Mover::Piston(pos));
                if let Some(r) = piston.running.take() {
                    disassemble(game, pos, r, grid_offset);
                    changed.push(pos);
                }
                piston.actuator.stopped(grid_offset);
            }
        }
        taken.insert(pos, piston);
    }
    // Pistons that are gone drop their structure where it is (`remove`).
    let gone: Vec<Pos> = taken
        .iter()
        .filter(|(p, _)| !found.iter().any(|(q, ..)| q == *p) && game.world.loaded(p[0], p[2]))
        .map(|(p, _)| *p)
        .collect();
    for p in gone {
        stop_actors(game, Mover::Piston(p));
        if let Some(mut piston) = taken.remove(&p)
            && let Some(r) = piston.running.take()
        {
            let grid = piston.actuator.grid_offset(piston.actuator.offset);
            disassemble(game, p, r, grid);
            changed.push(p);
        }
    }
    bearings_tick(game, dimension, &mut changed);
    pulleys_tick(game, dimension, &mut changed);
    elevators_tick(game, dimension, &mut changed);
    gantries_tick(game, dimension);
    let message = {
        let mut guard = state();
        let s = guard.get_or_insert_with(State::default);
        for (p, piston) in taken {
            s.pistons.insert((dimension.to_owned(), p), piston);
        }
        for p in changed {
            s.changed.insert((dimension.to_owned(), p));
        }
        // A chassis' range goes with its block entity: none where no chassis
        // stands any more.
        if s.ticks % 20 == 0 {
            s.ranges.retain(|(d, p), _| {
                d != dimension || !game.world.loaded(p[0], p[2]) || chassis_at(game, *p, DEFAULT_RANGE).is_some()
            });
        }
        if s.ticks % SAVE_EVERY == 0 {
            save(s);
        }
        (encode(game, s, dimension), std::mem::take(&mut s.told))
    };
    let (message, told) = message;
    for (player, text) in told {
        game.native_action_bar(player, text);
    }
    for player in game.native_server_players() {
        if player.dimension != dimension {
            continue;
        }
        if let Err(e) = game.native_net_send_to_player(player.uuid, CONTRAPTIONS, &message) {
            log::debug!("Create: contraptions to {}: {e}", player.name);
        }
    }
}

/// `native_world_lifecycle`: saved when a world closes, emptied when one
/// opens.
pub(super) fn world_phase(closing: bool) {
    let mut guard = state();
    if closing && let Some(s) = guard.as_ref() {
        save(s);
    }
    *guard = None;
    super::create_actor_glue::reset();
    *client() = None;
    CLIENT_GLUE.lock().unwrap_or_else(|e| e.into_inner()).clear();
    *CLIENT_BEARINGS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *CLIENT_PULLEYS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    ENDED_PULLEYS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    *CLIENT_GANTRIES.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

// ---- Clients -------------------------------------------------------------------

/// C1: every running piston of the dimension each tick (its place, facing,
/// anchor, offset, speed and range), its blocks (local place, state id)
/// only when they changed, and the pistons that stopped; then the glue's
/// corners.
fn encode(game: &Game<'_>, s: &mut State, dimension: &str) -> Vec<u8> {
    let mut out = b"C1".to_vec();
    let push_i = |o: &mut Vec<u8>, v: i32| o.extend_from_slice(&v.to_le_bytes());
    let push_f = |o: &mut Vec<u8>, v: f32| o.extend_from_slice(&v.to_le_bytes());
    let running: Vec<(&Pos, &Piston)> = s
        .pistons
        .iter()
        .filter(|((d, _), p)| d == dimension && p.running.is_some())
        .map(|((_, pos), p)| (pos, p))
        .collect();
    out.extend_from_slice(&(running.len() as u32).to_le_bytes());
    for (pos, p) in &running {
        let r = p.running.as_ref().expect("running");
        let c = &r.contraption;
        for v in **pos {
            push_i(&mut out, v);
        }
        out.push(c.orientation as u8);
        for v in c.anchor {
            push_i(&mut out, v);
        }
        push_f(&mut out, p.actuator.offset);
        let rpm = p.actuator.last_rpm;
        let still = super::create_actor_glue::stalled(dimension, Mover::Piston(**pos));
        push_f(&mut out, if still { 0.0 } else { p.actuator.movement_speed(rpm, c.orientation) });
        push_i(&mut out, p.actuator.extension_length);
        let full = s.changed.contains(&(dimension.to_owned(), **pos)) || s.ticks % 100 == 0;
        if full {
            out.extend_from_slice(&(c.blocks.len() as u32).to_le_bytes());
            for b in &c.blocks {
                for v in b.local {
                    push_i(&mut out, v);
                }
                out.extend_from_slice(&b.state.0.to_le_bytes());
                out.push(u8::from(b.solid));
            }
        } else {
            out.extend_from_slice(&u32::MAX.to_le_bytes());
        }
    }
    // Then the dimension's glue, for players holding super glue.
    let glue = s.glue.get(dimension).map_or(&[][..], Vec::as_slice);
    out.extend_from_slice(&(glue.len() as u32).to_le_bytes());
    for (a, b) in glue {
        for v in a.iter().chain(b) {
            push_i(&mut out, *v);
        }
    }
    // Then the turning bearings: place, facing, angle, degrees a tick, and
    // their blocks as the pistons'.
    let turning: Vec<(&Pos, &BearingEntry)> = s
        .bearings
        .iter()
        .filter(|((d, _), b)| d == dimension && b.turning.is_some())
        .map(|((_, p), b)| (p, b))
        .collect();
    out.extend_from_slice(&(turning.len() as u32).to_le_bytes());
    for (pos, b) in &turning {
        let c = &b.turning.as_ref().expect("turning").contraption;
        for v in **pos {
            push_i(&mut out, v);
        }
        out.push(c.facing as u8);
        push_f(&mut out, b.bearing.angle);
        let still = super::create_actor_glue::stalled(dimension, Mover::Bearing(**pos));
        push_f(&mut out, if still { 0.0 } else { super::create_contraption::convert_to_angular(b.bearing.last_rpm) });
        let full = s.changed.contains(&(dimension.to_owned(), **pos)) || s.ticks % 100 == 0;
        if full {
            out.extend_from_slice(&(c.blocks.len() as u32).to_le_bytes());
            for m in &c.blocks {
                for v in m.local {
                    push_i(&mut out, v);
                }
                out.extend_from_slice(&m.state.0.to_le_bytes());
                out.push(u8::from(m.solid));
            }
        } else {
            out.extend_from_slice(&u32::MAX.to_le_bytes());
        }
    }
    // Then the running pulleys: place, rope length, blocks a tick down,
    // range, what hangs (anchor, initial length, blocks as above).
    let running: Vec<(&Pos, &PulleyEntry)> = s
        .pulleys
        .iter()
        .filter(|((d, _), e)| d == dimension && e.pulley.running)
        .map(|((_, p), e)| (p, e))
        .collect();
    out.extend_from_slice(&(running.len() as u32).to_le_bytes());
    for (pos, e) in &running {
        for v in **pos {
            push_i(&mut out, v);
        }
        push_f(&mut out, e.pulley.offset);
        let still = super::create_actor_glue::stalled(dimension, Mover::Pulley(**pos));
        push_f(&mut out, if still { 0.0 } else { Pulley::movement_speed(e.pulley.last_rpm) });
        push_i(&mut out, e.pulley.range);
        let (anchor, initial, blocks) = e
            .hanging
            .as_ref()
            .map_or(([0; 3], 0, &[][..]), |h| (h.contraption.anchor, h.contraption.initial_offset, &h.contraption.blocks[..]));
        for v in anchor {
            push_i(&mut out, v);
        }
        push_i(&mut out, initial);
        let full = s.changed.contains(&(dimension.to_owned(), **pos)) || s.ticks % 100 == 0;
        if full {
            out.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
            for m in blocks {
                for v in m.local {
                    push_i(&mut out, v);
                }
                out.extend_from_slice(&m.state.0.to_le_bytes());
                out.push(u8::from(m.solid));
            }
        } else {
            out.extend_from_slice(&u32::MAX.to_le_bytes());
        }
    }
    // Then the moving gantries: id, anchor corner and motion, and their
    // blocks on their first ticks and every 20.
    let moving: Vec<&GantryMove> = s.gantries.iter().filter(|(d, _)| d == dimension).map(|(_, g)| g).collect();
    out.extend_from_slice(&(moving.len() as u32).to_le_bytes());
    for g in moving {
        out.extend_from_slice(&g.id.to_le_bytes());
        for v in g.at.iter().chain(&g.motion) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        if g.ticks <= 3 || s.ticks % 20 == 0 {
            let blocks = &g.contraption.blocks;
            out.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
            for m in blocks {
                for v in m.local {
                    push_i(&mut out, v);
                }
                out.extend_from_slice(&m.state.0.to_le_bytes());
                out.push(u8::from(m.solid));
            }
        } else {
            out.extend_from_slice(&u32::MAX.to_le_bytes());
        }
    }
    s.changed.retain(|(d, _)| d != dimension);
    let _ = game;
    out
}

#[derive(Clone)]
struct ClientView {
    facing: Dir,
    anchor: Pos,
    offset: f32,
    speed: f32,
    range: i32,
    blocks: Vec<(Pos, BlockStateId, bool)>,
    at: Instant,
}

static CLIENT: Mutex<Option<HashMap<Pos, ClientView>>> = Mutex::new(None);
/// The glue of the window's dimension, as the server last sent it.
static CLIENT_GLUE: Mutex<Vec<(Pos, Pos)>> = Mutex::new(Vec::new());

fn client() -> MutexGuard<'static, Option<HashMap<Pos, ClientView>>> {
    CLIENT.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn from_server(data: &[u8]) {
    let mut r = Reader { data, at: 0 };
    if r.take(2) != Some(b"C1".as_slice()) {
        return;
    }
    let Some(n) = r.u32() else { return };
    let mut guard = client();
    let views = guard.get_or_insert_with(HashMap::new);
    let mut seen = Vec::new();
    for _ in 0..n {
        let (Some(x), Some(y), Some(z)) = (r.i32(), r.i32(), r.i32()) else { return };
        let Some(facing) = r.u8().and_then(|d| Dir::ALL.get(usize::from(d)).copied()) else { return };
        let (Some(ax), Some(ay), Some(az)) = (r.i32(), r.i32(), r.i32()) else { return };
        let (Some(offset), Some(speed), Some(range), Some(count)) = (r.f32(), r.f32(), r.i32(), r.u32()) else {
            return;
        };
        let pos = [x, y, z];
        let mut blocks = None;
        if count != u32::MAX {
            let mut list = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let (Some(lx), Some(ly), Some(lz), Some(id), Some(solid)) =
                    (r.i32(), r.i32(), r.i32(), r.u32(), r.u8())
                else {
                    return;
                };
                list.push(([lx, ly, lz], BlockStateId(id), solid != 0));
            }
            blocks = Some(list);
        }
        let old = views.get(&pos).map(|v| v.blocks.clone());
        let Some(blocks) = blocks.or(old) else { continue };
        views.insert(
            pos,
            ClientView {
                facing,
                anchor: [ax, ay, az],
                offset,
                speed,
                range,
                blocks,
                at: Instant::now(),
            },
        );
        seen.push(pos);
    }
    views.retain(|p, _| seen.contains(p));
    let Some(n) = r.u32() else { return };
    let mut glue = Vec::with_capacity(n.min(4096) as usize);
    for _ in 0..n {
        let v: Vec<i32> = (0..6).filter_map(|_| r.i32()).collect();
        if v.len() != 6 {
            return;
        }
        glue.push(([v[0], v[1], v[2]], [v[3], v[4], v[5]]));
    }
    *CLIENT_GLUE.lock().unwrap_or_else(|e| e.into_inner()) = glue;
    let Some(n) = r.u32() else { return };
    let mut guard = CLIENT_BEARINGS.lock().unwrap_or_else(|e| e.into_inner());
    let views = guard.get_or_insert_with(HashMap::new);
    let mut seen = Vec::new();
    for _ in 0..n {
        let (Some(x), Some(y), Some(z)) = (r.i32(), r.i32(), r.i32()) else { return };
        let Some(facing) = r.u8().and_then(|d| Dir::ALL.get(usize::from(d)).copied()) else { return };
        let (Some(angle), Some(speed), Some(count)) = (r.f32(), r.f32(), r.u32()) else { return };
        let pos = [x, y, z];
        let mut blocks = None;
        if count != u32::MAX {
            let mut list = Vec::with_capacity(count.min(4096) as usize);
            for _ in 0..count {
                let (Some(lx), Some(ly), Some(lz), Some(id), Some(solid)) = (r.i32(), r.i32(), r.i32(), r.u32(), r.u8())
                else {
                    return;
                };
                list.push(([lx, ly, lz], BlockStateId(id), solid != 0));
            }
            blocks = Some(list);
        }
        let old = views.get(&pos).map(|v| v.blocks.clone());
        let Some(blocks) = blocks.or(old) else { continue };
        views.insert(
            pos,
            BearingView {
                facing,
                angle,
                speed,
                blocks,
                at: Instant::now(),
            },
        );
        seen.push(pos);
    }
    views.retain(|p, _| seen.contains(p));
    drop(guard);
    let Some(n) = r.u32() else { return };
    let mut guard = CLIENT_PULLEYS.lock().unwrap_or_else(|e| e.into_inner());
    let views = guard.get_or_insert_with(HashMap::new);
    let mut seen = Vec::new();
    for _ in 0..n {
        let (Some(x), Some(y), Some(z)) = (r.i32(), r.i32(), r.i32()) else { return };
        let (Some(offset), Some(speed), Some(range)) = (r.f32(), r.f32(), r.i32()) else { return };
        let (Some(ax), Some(ay), Some(az), Some(initial), Some(count)) = (r.i32(), r.i32(), r.i32(), r.i32(), r.u32())
        else {
            return;
        };
        let pos = [x, y, z];
        let mut blocks = None;
        if count != u32::MAX {
            let mut list = Vec::with_capacity(count.min(4096) as usize);
            for _ in 0..count {
                let (Some(lx), Some(ly), Some(lz), Some(id), Some(solid)) = (r.i32(), r.i32(), r.i32(), r.u32(), r.u8())
                else {
                    return;
                };
                list.push(([lx, ly, lz], BlockStateId(id), solid != 0));
            }
            blocks = Some(list);
        }
        let old = views.get(&pos).map(|v| v.blocks.clone());
        let Some(blocks) = blocks.or(old) else { continue };
        views.insert(
            pos,
            PulleyView {
                offset,
                speed,
                range,
                anchor: [ax, ay, az],
                initial,
                blocks,
                at: Instant::now(),
            },
        );
        seen.push(pos);
    }
    // Loads put back: what stood on them is set on their blocks.
    let ended: Vec<PulleyView> = views.iter().filter(|(p, _)| !seen.contains(p)).map(|(_, v)| v.clone()).collect();
    ENDED_PULLEYS.lock().unwrap_or_else(|e| e.into_inner()).extend(ended);
    views.retain(|p, _| seen.contains(p));
    drop(guard);
    let Some(n) = r.u32() else { return };
    let mut guard = CLIENT_GANTRIES.lock().unwrap_or_else(|e| e.into_inner());
    let views = guard.get_or_insert_with(HashMap::new);
    let mut seen = Vec::new();
    for _ in 0..n {
        let Some(id) = r.u32() else { return };
        let v: Vec<f64> = (0..6).filter_map(|_| r.f64()).collect();
        let Some(count) = r.u32() else { return };
        if v.len() != 6 {
            return;
        }
        let mut blocks = None;
        if count != u32::MAX {
            let mut list = Vec::with_capacity(count.min(4096) as usize);
            for _ in 0..count {
                let (Some(lx), Some(ly), Some(lz), Some(state), Some(solid)) = (r.i32(), r.i32(), r.i32(), r.u32(), r.u8())
                else {
                    return;
                };
                list.push(([lx, ly, lz], BlockStateId(state), solid != 0));
            }
            blocks = Some(list);
        }
        let old = views.get(&id).map(|g| g.blocks.clone());
        let Some(blocks) = blocks.or(old) else { continue };
        views.insert(
            id,
            GantryView {
                at: [v[0], v[1], v[2]],
                motion: [v[3], v[4], v[5]],
                blocks,
                received: Instant::now(),
            },
        );
        seen.push(id);
    }
    views.retain(|id, _| seen.contains(id));
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.data.get(self.at..self.at + n)?;
        self.at += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }
    fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
    }
    fn f64(&mut self) -> Option<f64> {
        self.take(8).map(|b| f64::from_le_bytes(b.try_into().expect("8 bytes")))
    }
}

/// `native_tick` of the window: its player carried by the contraptions it
/// stands on, as the server last placed them and moving since.
pub(super) fn client_tick(game: &mut Game<'_>) {
    if !game.native_window() {
        return;
    }
    let turning: Vec<(Pos, BearingView)> = CLIENT_BEARINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|m| m.iter().map(|(p, v)| (*p, v.clone())).collect())
        .unwrap_or_default();
    // What stands on a pulley's load goes up and down with it.
    let hanging: Vec<PulleyView> = CLIENT_PULLEYS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default();
    for v in hanging {
        let offset = v.offset_now();
        let moving = offset > 0.0 && offset < v.range as f32;
        let y = f64::from(v.initial) - f64::from(offset);
        let blocks: Vec<[f64; 3]> = v
            .blocks
            .iter()
            .filter(|b| b.2)
            .map(|(l, ..)| [f64::from(l[0] + v.anchor[0]), f64::from(l[1] + v.anchor[1]) + y, f64::from(l[2] + v.anchor[2])])
            .collect();
        let speed = if moving { v.speed } else { 0.0 };
        carry(game, None, &blocks, [0.0, -f64::from(speed), 0.0]);
    }
    // A load put back on the grid: the window's player standing on it is
    // set on top of its blocks (it may have lagged a little below).
    let ended = std::mem::take(&mut *ENDED_PULLEYS.lock().unwrap_or_else(|e| e.into_inner()));
    for v in ended {
        let y = f64::from(v.initial) - f64::from(v.offset_now().round());
        let Some(p) = game.player.as_mut() else {
            break;
        };
        let at = [p.position.x, p.position.y, p.position.z];
        let top = v
            .blocks
            .iter()
            .filter(|b| b.2)
            .map(|(l, ..)| [f64::from(l[0] + v.anchor[0]), f64::from(l[1] + v.anchor[1]) + y, f64::from(l[2] + v.anchor[2])])
            .filter(|b| at[0] + 0.3 > b[0] && at[0] - 0.3 < b[0] + 1.0 && at[2] + 0.3 > b[2] && at[2] - 0.3 < b[2] + 1.0)
            .map(|b| b[1] + 1.0)
            .filter(|top| at[1] >= top - 0.6 && at[1] <= top + 0.3)
            .reduce(f64::max);
        if let Some(top) = top {
            p.position.y = top;
            p.velocity.y = 0.0;
            p.ground.on_ground = true;
            p.fall_distance = 0.0;
        }
    }
    // And on a moving gantry.
    let gantries: Vec<GantryView> = CLIENT_GANTRIES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default();
    for v in gantries {
        let at = v.at_now();
        let blocks: Vec<[f64; 3]> = v
            .blocks
            .iter()
            .filter(|b| b.2)
            .map(|(l, ..)| [0, 1, 2].map(|k| f64::from(l[k]) + at[k]))
            .collect();
        carry(game, None, &blocks, v.motion);
    }
    // Riders turn by what the drawn structure turned since the last tick.
    let mut last = RIDDEN.lock().unwrap_or_else(|e| e.into_inner());
    let previous = std::mem::take(&mut *last);
    for (pos, v) in turning {
        if v.facing.axis() != super::create_kinetics::Axis::Y {
            continue;
        }
        let anchor = [0, 1, 2].map(|k| pos[k] + v.facing.normal()[k]);
        let angle = v.angle + v.speed * v.at.elapsed().as_secs_f32().min(0.5) * 20.0;
        last.push((pos, angle));
        let Some((_, before)) = previous.iter().find(|(p, _)| *p == pos) else {
            continue;
        };
        let delta = (angle - before + 540.0).rem_euclid(360.0) - 180.0;
        let solid: Vec<Pos> = v.blocks.iter().filter(|b| b.2).map(|b| b.0).collect();
        let centres = turned_centres(anchor, v.facing.axis(), *before, &solid);
        carry_turn(game, None, anchor, &centres, delta);
    }
    drop(last);
    let views: Vec<ClientView> = match client().as_ref() {
        Some(v) => v.values().cloned().collect(),
        None => return,
    };
    for v in views {
        let ticks = v.at.elapsed().as_secs_f32() * 20.0;
        let offset = (v.offset + v.speed * ticks).clamp(0.0, v.range as f32);
        let speed = if (offset <= 0.0 && v.speed < 0.0) || (offset >= v.range as f32 && v.speed > 0.0) {
            0.0
        } else {
            v.speed
        };
        let n = v.facing.normal();
        let blocks: Vec<[f64; 3]> = v
            .blocks
            .iter()
            .filter(|b| b.2)
            .map(|(local, ..)| [0, 1, 2].map(|i| f64::from(local[i] + v.anchor[i]) + f64::from(n[i]) * f64::from(offset)))
            .collect();
        let delta = [0, 1, 2].map(|i| f64::from(n[i]) * f64::from(speed));
        carry(game, None, &blocks, delta);
    }
}

/// `ContraptionRenderer`: every moving block where its contraption is now,
/// the offset running on between the server's updates
/// (`getInterpolatedOffset`).
/// `f` on a moving structure's blocks' states and their block entities'
/// data (its storage), if it still moves.
pub(super) fn with_storage<R>(
    game: &Game<'_>,
    mover: Mover,
    f: impl FnOnce(&[BlockStateId], &mut [Option<horizon_world::nbt::Compound>]) -> R,
) -> Option<R> {
    let dim = game.dim.id().to_owned();
    let mut guard = state();
    let s = guard.as_mut()?;
    let (states, entities): (Vec<BlockStateId>, &mut Vec<Option<horizon_world::nbt::Compound>>) = match mover {
        Mover::Piston(p) => {
            let r = s.pistons.get_mut(&(dim, p))?.running.as_mut()?;
            (r.contraption.blocks.iter().map(|b| b.state).collect(), &mut r.entities)
        }
        Mover::Bearing(p) => {
            let t = s.bearings.get_mut(&(dim, p))?.turning.as_mut()?;
            (t.contraption.blocks.iter().map(|b| b.state).collect(), &mut t.entities)
        }
        Mover::Pulley(p) => {
            let h = s.pulleys.get_mut(&(dim, p))?.hanging.as_mut()?;
            (h.contraption.blocks.iter().map(|b| b.state).collect(), &mut h.entities)
        }
        Mover::Gantry(id) => {
            let g = s.gantries.iter_mut().find(|(d, g)| *d == dim && g.id == id).map(|(_, g)| g)?;
            (g.contraption.blocks.iter().map(|b| b.state).collect(), &mut g.entities)
        }
    };
    Some(f(&states, entities))
}

/// A moving block's model, and what Create draws beside it
/// (`renderInContraption`): the drill's head, still; the saw's blade.
fn draw_block(
    game: &Game<'_>,
    state: BlockStateId,
    seed: i64,
    m: glam::Mat4,
    packed: u32,
    out: &mut Vec<horizon_client::mesher::Vertex>,
) {
    game.block_model_vertices(state, seed, m, packed, false, out);
    let registry = game.pack.assets.registry;
    let Some(s) = registry.state(state) else {
        return;
    };
    if s.name == super::create_actors::DRILL
        && let Some(f) = s.properties.get("facing").and_then(|f| Dir::parse(f))
        && let Some(head) = registry.default_state("create:hz_part_drill_head")
    {
        let c = glam::Vec3::splat(0.5);
        let orient =
            glam::Mat4::from_translation(c) * super::create_glue::rotate_to_face(f) * glam::Mat4::from_translation(-c);
        game.block_model_vertices(head, seed, m * orient, packed, false, out);
    }
    let saw = s.name == super::create_actors::SAW || s.name == super::create_actors::SAW_ALONG;
    if saw && let Some(f) = s.properties.get("facing").and_then(|f| Dir::parse(f)) {
        let vertical = f.axis() == super::create_kinetics::Axis::Y;
        let name = if vertical {
            "create:hz_part_saw_blade_vertical_active"
        } else {
            "create:hz_part_saw_blade_horizontal_active"
        };
        let Some(blade) = registry.default_state(name) else {
            return;
        };
        let c = glam::Vec3::splat(0.5);
        let mut turn = super::create_glue::rotate_to_face(f);
        if vertical && s.name == super::create_actors::SAW_ALONG {
            turn = glam::Mat4::from_rotation_y(90f32.to_radians()) * turn;
        }
        let orient = glam::Mat4::from_translation(c) * turn * glam::Mat4::from_translation(-c);
        game.block_model_vertices(blade, seed, m * orient, packed, false, out);
    }
}

pub(super) fn draw(game: &Game<'_>, camera: glam::DVec3, out: &mut Vec<horizon_client::mesher::Vertex>) {
    draw_glue(game, camera, out);
    draw_bearings(game, camera, out);
    draw_pulleys(game, camera, out);
    draw_gantries(game, camera, out);
    let views: Vec<ClientView> = match client().as_ref() {
        Some(v) => v.values().cloned().collect(),
        None => return,
    };
    for v in views {
        let ticks = v.at.elapsed().as_secs_f32() * 20.0;
        let offset = (v.offset + v.speed * ticks.min(1.0)).clamp(0.0, v.range as f32);
        let n = v.facing.normal();
        let shift = glam::DVec3::new(f64::from(n[0]), f64::from(n[1]), f64::from(n[2])) * f64::from(offset);
        for (local, state, _) in &v.blocks {
            let cell = [0, 1, 2].map(|i| local[i] + v.anchor[i]);
            let corner = glam::DVec3::new(f64::from(cell[0]), f64::from(cell[1]), f64::from(cell[2])) + shift;
            if (corner + glam::DVec3::splat(0.5)).distance_squared(camera) > VIEW_DISTANCE * VIEW_DISTANCE {
                continue;
            }
            let lit = [0, 1, 2].map(|i| (corner.to_array()[i] + 0.5).floor() as i32);
            let light = game.world.light(lit[0], lit[1], lit[2]);
            let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
            let seed = horizon_core::random::position_seed(horizon_core::position::BlockPos::new(
                cell[0], cell[1], cell[2],
            ));
            let m = glam::Mat4::from_translation((corner - camera).as_vec3());
            draw_block(game, *state, seed, m, packed, out);
        }
    }
}

// ---- Chassis and super glue in players' hands -------------------------------------

fn set_named(game: &mut Game<'_>, pos: Pos, name: &str, props: &[(&str, &str)]) {
    let Some(id) = game
        .pack
        .assets
        .registry
        .default_state(name)
        .and_then(|s| game.with_props(s, props))
    else {
        return;
    };
    use super::native_hooks::flags;
    set(game, pos, id, flags::UPDATE_ALL);
}

/// The chassis at `pos` given new sticky sides, its axis kept.
fn set_chassis(game: &mut Game<'_>, pos: Pos, c: &Chassis, sticky: u8) {
    set_named(game, pos, &chassis_block(c.kind, sticky), &[("axis", c.axis.name())]);
}

fn add(p: Pos, d: Dir) -> Pos {
    [0, 1, 2].map(|k| p[k] + d.normal()[k])
}

fn tell(game: &mut Game<'_>, text: horizon_client::text::Text) {
    if let Some(player) = game.native_acting_player() {
        game.native_action_bar(player.uuid, text);
    }
}

fn tell_key(game: &mut Game<'_>, key: &str) {
    tell(game, horizon_client::text::Text::translate(key, &[]));
}

/// `native_right_click_block`: slime (or super glue) on a chassis' side
/// (`AbstractChassisBlock.useItemOn`), and super glue's two corners
/// (`SuperGlueSelectionHandler`, chosen on the server here). True when it
/// took the click.
pub(super) fn right_click(game: &mut Game<'_>, pos: Pos, face: [i32; 3]) -> bool {
    let held = game.held().map(|s| s.item.to_string());
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    let face = Dir::nearest(face);
    let glue = held.as_deref() == Some(SUPER_GLUE);
    if click_bearing(game, pos) || click_carriage(game, pos) || click_elevator(game, pos) {
        return true;
    }
    if let Some(c) = chassis_at(game, pos, DEFAULT_RANGE) {
        let slime = glue || held.as_deref().is_some_and(|i| game.items().item_in_tag("c:slimeballs", i));
        let side = super::create_contraption::glueable_side(c.kind, c.axis, face);
        if side.is_some() && (slime || (held.is_none() && sneaking)) {
            let next = c.use_item(face, slime, |d| chassis_at(game, add(pos, d), DEFAULT_RANGE));
            if let Some(bits) = next {
                set_chassis(game, pos, &c, bits);
            }
            return next.is_some() || glue;
        }
        // SuperGlueItem leaves a chassis' glueable side to the chassis.
        if glue && side.is_some() {
            return false;
        }
    }
    if !glue {
        return false;
    }
    let Some(player) = game.native_acting_player() else {
        return true;
    };
    let dimension = game.dim.id().to_owned();
    let first = {
        let mut guard = state();
        let s = guard.get_or_insert_with(State::default);
        if sneaking {
            if s.selecting.remove(&player.uuid).is_some() {
                drop(guard);
                tell_key(game, "create.super_glue.abort");
            }
            return true;
        }
        match s.selecting.get(&player.uuid) {
            Some((d, p)) if *d == dimension => Some(*p),
            _ => None,
        }
    };
    let Some(first) = first else {
        state()
            .get_or_insert_with(State::default)
            .selecting
            .insert(player.uuid, (dimension, pos));
        tell_key(game, "create.super_glue.first_pos");
        return true;
    };
    if (0..3).any(|k| (first[k] - pos[k]).abs() > GLUE_REACH) {
        tell_key(game, "create.super_glue.too_far");
        return true;
    }
    // SuperGlueSelectionPacket.handle.
    let reached = glue_group(&scan(game), first, pos).is_some_and(|g| g.contains(&pos));
    if !reached {
        tell_key(game, "create.super_glue.cannot_reach");
        return true;
    }
    if player.game_mode != 1 {
        // collectGlueFromInventory: one use of the held glue.
        let max = game.items().get(SUPER_GLUE).map_or(0, |i| i.max_damage);
        let hand = game.held_mut();
        if let Some(h) = hand.as_mut() {
            h.damage += 1;
            if max > 0 && h.damage >= max {
                *hand = None;
            }
        }
    }
    {
        let mut guard = state();
        let s = guard.get_or_insert_with(State::default);
        s.selecting.remove(&player.uuid);
        s.glue.entry(dimension).or_default().push((first, pos));
    }
    tell_key(game, "create.super_glue.success");
    true
}

/// `native_left_click_block` with super glue: the glue around the block
/// (or its face) comes off (`SuperGlueRemovalPacket`); the glue never breaks
/// blocks (`canAttackBlock`). True when it took the click.
pub(super) fn left_click(game: &mut Game<'_>, pos: Pos, face: [i32; 3]) -> bool {
    if game.held().is_none_or(|s| &*s.item != SUPER_GLUE) {
        return false;
    }
    let out = [0, 1, 2].map(|k| pos[k] + face[k]);
    let dimension = game.dim.id().to_owned();
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    if let Some(all) = s.glue.get_mut(&dimension) {
        // The smallest box around the block hit.
        let size = |(a, b): &(Pos, Pos)| (0..3).map(|k| (a[k] - b[k]).abs() + 1).product::<i32>();
        let found = (all.iter().enumerate())
            .filter(|(_, (a, b))| in_span(*a, *b, pos) || in_span(*a, *b, out))
            .min_by_key(|(_, g)| size(g))
            .map(|(i, _)| i);
        if let Some(i) = found {
            all.remove(i);
        }
    }
    true
}

/// `native_place_block`, a tick later: `LinearChassisBlock.getStateForPlacement`
/// (the axis of a chassis beside it, else the way the player looks; a
/// sneaking player's click as a log's) and `updateShape` around it.
pub(super) fn placed_chassis(game: &mut Game<'_>, pos: Pos, sneaking: bool, looking: Dir) {
    let Some(c) = chassis_at(game, pos, DEFAULT_RANGE) else {
        return;
    };
    let mut c = c;
    if c.kind != ChassisKind::Radial && !sneaking {
        // The hook doesn't say which block was clicked: a lone linear
        // chassis beside it stands for it.
        let beside: Vec<Chassis> = Dir::ALL
            .iter()
            .filter_map(|d| chassis_at(game, add(pos, *d), DEFAULT_RANGE))
            .filter(|o| o.kind != ChassisKind::Radial)
            .collect();
        let axis = if beside.len() == 1 {
            beside[0].axis
        } else {
            looking.axis()
        };
        if axis != c.axis {
            c.axis = axis;
            set_chassis(game, pos, &c, c.sticky);
        }
    }
    for d in Dir::ALL {
        let other = add(pos, d);
        let Some(o) = chassis_at(game, other, DEFAULT_RANGE) else {
            continue;
        };
        let mine = c.update_shape(d, Some(&o));
        if mine != c.sticky {
            c.sticky = mine;
            set_chassis(game, pos, &c, mine);
        }
        let theirs = o.update_shape(d.opposite(), Some(&c));
        if theirs != o.sticky {
            set_chassis(game, other, &o, theirs);
        }
    }
}

/// `native_wheel` on this client: the wrench over a chassis scrolls its
/// range (`ChassisScrollValueBehaviour`, `requiresWrench`; Ctrl for its
/// whole group). True when it took the wheel.
pub(super) fn wheel(game: &mut Game<'_>, steps: f32) -> bool {
    if steps == 0.0 || game.held().is_none_or(|s| &*s.item != super::create_glue::WRENCH) {
        return false;
    }
    let Some(hit) = game.pick() else {
        return false;
    };
    if chassis_at(game, hit.pos, DEFAULT_RANGE).is_none() {
        return false;
    }
    let bulk = game.keys.contains(&winit::keyboard::KeyCode::ControlLeft);
    let delta = (steps.abs().round() as i32).max(1) * if steps > 0.0 { 1 } else { -1 };
    let mut data = b"R1".to_vec();
    for v in hit.pos.into_iter().chain([delta, i32::from(bulk)]) {
        data.extend_from_slice(&v.to_le_bytes());
    }
    if let Err(e) = game.native_net_send_to_server(CHASSIS, &data) {
        log::debug!("Create: chassis range: {e}");
    }
    true
}

/// `native_net_from_client` on `create:chassis`: the range stepped (the
/// chassis' group with Ctrl) and told on the action bar.
pub(super) fn from_client(game: &mut Game<'_>, player: u128, data: &[u8]) {
    let Some(rest) = data.strip_prefix(b"R1") else {
        return;
    };
    if rest.len() != 20 {
        return;
    }
    let int = |i: usize| i32::from_le_bytes([rest[i], rest[i + 1], rest[i + 2], rest[i + 3]]);
    let (pos, delta, bulk) = ([int(0), int(4), int(8)], int(12), int(16) != 0);
    let Some(p) = game.native_server_players().into_iter().find(|p| p.uuid == player) else {
        return;
    };
    if p.game_mode == 2 || p.game_mode == 3 || p.dimension != game.dim.id() {
        return;
    }
    let c = pos.map(|v| f64::from(v) + 0.5);
    if (0..3).map(|k| (p.position[k] - c[k]).powi(2)).sum::<f64>() > 64.0 {
        return;
    }
    let dimension = game.dim.id().to_owned();
    let (group, range, radial) = {
        let world = scan(game);
        let Some(chassis) = world.facts(pos).chassis else {
            return;
        };
        let range = (chassis.range + delta).clamp(1, MAX_CHASSIS_RANGE);
        // collectChassisGroup.
        let mut group = vec![pos];
        if bulk {
            let mut i = 0;
            while i < group.len() {
                let at = group[i];
                if let Some(c) = world.facts(at).chassis {
                    for n in attached_chassis(&world, at, &c) {
                        if !group.contains(&n) {
                            group.push(n);
                        }
                    }
                }
                i += 1;
            }
        }
        (group, range, chassis.kind == ChassisKind::Radial)
    };
    let label = if radial { "create.contraptions.chassis.radius" } else { "create.contraptions.chassis.range" };
    let text = horizon_client::text::Text::translate(label, &[])
        .append(horizon_client::text::Text::literal(format!(": {range}")));
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    for at in group {
        s.ranges.insert((dimension.clone(), at), range);
    }
    s.told.push((player, text));
}

/// `SuperGlueSelectionHandler.tick`'s outlines: with super glue in hand,
/// every glue box nearby drawn as thin slime edges (Horizon has no outliner).
fn draw_glue(game: &Game<'_>, camera: glam::DVec3, out: &mut Vec<horizon_client::mesher::Vertex>) {
    if game.held().is_none_or(|s| &*s.item != SUPER_GLUE) {
        return;
    }
    let glue = CLIENT_GLUE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(slime) = game.pack.assets.registry.default_state("minecraft:slime_block") else {
        return;
    };
    const THICK: f64 = 1.0 / 32.0;
    for (a, b) in glue {
        let lo = glam::DVec3::new(
            f64::from(a[0].min(b[0])),
            f64::from(a[1].min(b[1])),
            f64::from(a[2].min(b[2])),
        );
        let hi = glam::DVec3::new(
            f64::from(a[0].max(b[0]) + 1),
            f64::from(a[1].max(b[1]) + 1),
            f64::from(a[2].max(b[2]) + 1),
        );
        if ((lo + hi) * 0.5).distance_squared(camera) > 48.0 * 48.0 {
            continue;
        }
        let size = hi - lo;
        // Four edges along each axis.
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for (su, sv) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                let mut corner = lo;
                corner[u] += size[u] * su - THICK * 0.5;
                corner[v] += size[v] * sv - THICK * 0.5;
                let mut scale = glam::DVec3::splat(THICK);
                scale[axis] = size[axis];
                let m = glam::Mat4::from_translation((corner - camera).as_vec3())
                    * glam::Mat4::from_scale(scale.as_vec3());
                game.block_model_vertices(slime, 0, m, 0xF0F0, false, out);
            }
        }
    }
}

// ---- Bearings ------------------------------------------------------------------

/// The structure in front of the bearing at `pos` taken out of the world
/// (`MechanicalBearingBlockEntity.assemble`).
fn assemble_bearing(game: &mut Game<'_>, pos: Pos, facing: Dir, windmill: bool) -> Option<Turning> {
    let contraption = {
        let scan = scan(game);
        match BearingContraption::assemble(&scan, pos, facing, windmill) {
            Ok(Some(c)) => c,
            Ok(None) => return None,
            Err(e) => {
                log::debug!("Create: bearing at {pos:?} can't turn: {e:?}");
                return None;
            }
        }
    };
    let mut texts = Vec::with_capacity(contraption.blocks.len());
    let mut entities = Vec::with_capacity(contraption.blocks.len());
    let at: Vec<Pos> = contraption
        .blocks
        .iter()
        .map(|b| [0, 1, 2].map(|k| b.local[k] + contraption.anchor[k]))
        .collect();
    for (b, &p) in contraption.blocks.iter().zip(&at) {
        texts.push(state_text(game, b.state));
        entities.push(capture(game, p, b.state));
    }
    remove_blocks(game, &at);
    let dimension = game.dim.id().to_owned();
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    let mut ranges = Vec::new();
    for (i, p) in at.iter().enumerate() {
        if let Some(r) = s.ranges.remove(&(dimension.clone(), *p)) {
            ranges.push((i, r));
        }
    }
    let all = s.glue.entry(dimension).or_default();
    let glue: Vec<(Pos, Pos)> = contraption.glue.iter().filter_map(|&g| all.get(g).copied()).collect();
    all.retain(|g| !glue.contains(g));
    Some(Turning {
        contraption,
        texts,
        entities,
        ranges,
        glue,
    })
}

/// `ControlledContraptionEntity.disassemble`: the blocks put back turned
/// by `angle` rounded to a quarter (`StructureTransform`), their states
/// turned with them.
fn disassemble_bearing(game: &mut Game<'_>, t: Turning, angle: f32) {
    use super::create_contraption::{TurnShape, rounded_angle, turn_props, turn_pos};
    let c = &t.contraption;
    let axis = c.facing.axis();
    let quarter = rounded_angle(angle);
    let registry = game.pack.assets.registry;
    let mut placed = vec![[0; 3]; c.blocks.len()];
    for i in super::create_contraption::placing_order(&c.blocks) {
        let b = &c.blocks[i];
        let at = c.placed_at(b.local, angle);
        let mut state = b.state;
        if quarter != 0
            && let Some(info) = registry.state(b.state)
        {
            let facts = game.res.facts().get(b.state);
            let class = |name: &str| facts.classes.iter().any(|k| &**k == name);
            let shape = if class("StairBlock") {
                TurnShape::Stairs
            } else if class("FaceAttachedHorizontalDirectionalBlock") {
                TurnShape::FaceAttached
            } else {
                TurnShape::Plain
            };
            let base = registry.default_state(&info.name);
            let six = base.and_then(|s| game.with_props(s, &[("facing", "up")])).is_some()
                && base.and_then(|s| game.with_props(s, &[("facing", "north")])).is_some();
            let props: Vec<(String, String)> = info.properties.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            let turned = turn_props(&props, shape, six, axis, quarter);
            let pairs: Vec<(&str, &str)> = turned.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            if let Some(id) = base.and_then(|s| game.with_props(s, &pairs)) {
                state = id;
            }
        }
        put_back(game, at, state, b.solid, t.entities[i].clone());
        placed[i] = at;
    }
    let dimension = game.dim.id().to_owned();
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    for &(i, r) in &t.ranges {
        if let Some(at) = placed.get(i) {
            s.ranges.insert((dimension.clone(), *at), r);
        }
    }
    let turn = |p: Pos| {
        let local = [0, 1, 2].map(|k| p[k] - c.anchor[k]);
        let v = turn_pos(local, quarter, axis);
        [0, 1, 2].map(|k| v[k] + c.anchor[k])
    };
    s.glue
        .entry(dimension)
        .or_default()
        .extend(t.glue.iter().map(|&(a, b)| (turn(a), turn(b))));
}

/// The places of a turning structure's solid blocks, turned by `angle`
/// about its anchor (their centres).
fn turned_centres(anchor: Pos, axis: super::create_kinetics::Axis, angle: f32, blocks: &[Pos]) -> Vec<[f64; 3]> {
    blocks
        .iter()
        .map(|l| {
            let v = super::create_kinetics::rotate_vec(l.map(|c| c as f32), angle, axis);
            [0, 1, 2].map(|k| f64::from(anchor[k]) + 0.5 + f64::from(v[k]))
        })
        .collect()
}

/// What stands on a structure turning about Y is turned with it by
/// `delta` degrees about the anchor's centre (`ContraptionCollider`
/// carrying riders); `centres` are its solid blocks' centres now.
fn carry_turn(game: &mut Game<'_>, dimension: Option<&str>, anchor: Pos, centres: &[[f64; 3]], delta: f32) {
    if centres.is_empty() || delta == 0.0 {
        return;
    }
    let pivot = [f64::from(anchor[0]) + 0.5, f64::from(anchor[2]) + 0.5];
    // The top of the block something at `p` stands on (its feet near it).
    let on = |p: [f64; 3]| {
        centres
            .iter()
            .filter(|c| (p[0] - c[0]).abs() < 0.8 && (p[2] - c[2]).abs() < 0.8)
            .map(|c| c[1] + 0.5)
            .filter(|top| p[1] >= top - 0.6 && p[1] <= top + 0.3)
            .reduce(f64::max)
    };
    // Turned about the pivot, standing on that top (Horizon has no
    // collision with the moving blocks: this holds it up).
    let turn = |p: [f64; 3], top: f64| {
        let v = super::create_kinetics::rotate_vec(
            [(p[0] - pivot[0]) as f32, 0.0, (p[2] - pivot[1]) as f32],
            delta,
            super::create_kinetics::Axis::Y,
        );
        [pivot[0] + f64::from(v[0]), top, pivot[1] + f64::from(v[2])]
    };
    let Some(dimension) = dimension else {
        if let Some(p) = game.player.as_mut() {
            let at = [p.position.x, p.position.y, p.position.z];
            if let Some(top) = on(at) {
                let to = turn(at, top);
                p.position.x = to[0];
                p.position.y = to[1];
                p.position.z = to[2];
                p.velocity.y = 0.0;
                p.ground.on_ground = true;
                p.fall_distance = 0.0;
            }
        }
        return;
    };
    for d in game.dropped.iter_mut() {
        let at = [d.position.x, d.position.y, d.position.z];
        if let Some(top) = on(at) {
            let to = turn(at, top);
            d.position.x = to[0];
            d.position.y = to[1];
            d.position.z = to[2];
            d.velocity.y = 0.0;
        }
    }
    let mobs: Vec<(u32, [f64; 3])> = game
        .native_mobs(dimension)
        .into_iter()
        .filter_map(|m| on(m.pos).map(|top| (m.id, turn(m.pos, top))))
        .collect();
    for (id, to) in mobs {
        game.native_teleport_mob(dimension, id, to);
    }
}

/// After the kinetics' tick of `dimension`: every bearing's tick
/// (`MechanicalBearingBlockEntity.tick`); a windmill's generated speed set
/// from its sails. Pistons' ticks come first.
fn bearings_tick(game: &mut Game<'_>, dimension: &str, changed: &mut Vec<Pos>) {
    let found: Vec<(Pos, Dir, bool, f32)> = super::create_glue::with_kinetics(game, |k, level| {
        k.positions()
            .into_iter()
            .filter_map(|p| match k.get(p).map(|e| e.kind) {
                Some(Kind::MechanicalBearing(f)) => Some((p, f, false, k.speed_of(level, p))),
                Some(Kind::WindmillBearing(f)) => Some((p, f, true, k.speed_of(level, p))),
                _ => None,
            })
            .collect()
    });
    let (mut taken, clicked) = {
        let mut guard = state();
        let s = guard.get_or_insert_with(State::default);
        let keys: Vec<(String, Pos)> = s.bearings.keys().filter(|(d, _)| d == dimension).cloned().collect();
        let taken: HashMap<Pos, BearingEntry> =
            keys.into_iter().filter_map(|k| s.bearings.remove(&k).map(|b| (k.1, b))).collect();
        let clicked: Vec<Pos> = s.clicked.iter().filter(|(d, _)| d == dimension).map(|(_, p)| *p).collect();
        s.clicked.retain(|(d, _)| d != dimension);
        (taken, clicked)
    };
    let mut generated: Vec<(Pos, i32)> = Vec::new();
    for &(pos, facing, windmill, rpm) in &found {
        if !game.world.loaded(pos[0], pos[2]) {
            continue;
        }
        let mut entry = taken.remove(&pos).unwrap_or_default();
        let mut b = entry.bearing;
        if clicked.contains(&pos) && b.clicked() {
            // Put back at once: a windmill as it was built.
            stop_actors(game, Mover::Bearing(pos));
            if let Some(t) = entry.turning.take() {
                disassemble_bearing(game, t, if windmill { 0.0 } else { b.angle });
                changed.push(pos);
            }
            b.stopped();
            if windmill {
                generated.push((pos, 0));
            }
        }
        b.speed(rpm, windmill);
        let before = b.angle;
        let mut turning = entry.turning.take();
        let stalled = match &mut turning {
            Some(t) if b.running => {
                let c = &t.contraption;
                let entities = &mut t.entities;
                let axis = facing.axis();
                let anchor = c.anchor;
                let blocks = || c.blocks.iter().map(|m| (m.local, m.state)).collect();
                let place = |l: Pos, v: [f64; 3]| {
                    let local = [0, 1, 2].map(|k| l[k] as f32 + v[k] as f32);
                    let w = super::create_kinetics::rotate_vec(local, before, axis);
                    [0, 1, 2].map(|k| f64::from(anchor[k]) + 0.5 + f64::from(w[k]))
                };
                let unrotate = |v: [f64; 3]| {
                    super::create_kinetics::rotate_vec(v.map(|c| c as f32), -before, axis).map(f64::from)
                };
                let delta = if super::create_actor_glue::stalled(dimension, Mover::Bearing(pos)) {
                    0.0
                } else {
                    super::create_contraption::convert_to_angular(rpm)
                };
                let turn = |v: [f64; 3]| {
                    super::create_kinetics::rotate_vec(v.map(|c| c as f32), before, axis).map(f64::from)
                };
                tick_actors(
                    game,
                    Mover::Bearing(pos),
                    &blocks,
                    &place,
                    &unrotate,
                    &turn,
                    Some((facing, delta)),
                    entities,
                )
            }
            _ => false,
        };
        let step = b.tick(rpm, windmill, stalled, || {
            turning = assemble_bearing(game, pos, facing, windmill);
            turning.is_some()
        });
        match step {
            BearingStep::Idle => {}
            BearingStep::Assembled | BearingStep::Turned => {
                if step == BearingStep::Assembled {
                    changed.push(pos);
                }
                if let Some(t) = &turning {
                    let c = &t.contraption;
                    if windmill {
                        generated.push((pos, super::create_contraption::windmill_speed(c.sails)));
                    }
                    if facing.axis() == super::create_kinetics::Axis::Y {
                        let solid: Vec<Pos> = c.blocks.iter().filter(|b| b.solid).map(|b| b.local).collect();
                        let centres = turned_centres(c.anchor, facing.axis(), before, &solid);
                        carry_turn(game, Some(dimension), c.anchor, &centres, b.angle - before);
                    }
                }
            }
            BearingStep::Disassemble { angle } => {
                stop_actors(game, Mover::Bearing(pos));
                if let Some(t) = turning.take() {
                    disassemble_bearing(game, t, angle);
                    changed.push(pos);
                }
                b.stopped();
                if windmill {
                    generated.push((pos, 0));
                }
            }
        }
        entry.bearing = b;
        entry.turning = turning;
        taken.insert(pos, entry);
    }
    // Bearings that are gone put their structure back where it is.
    let gone: Vec<Pos> = taken
        .keys()
        .filter(|p| !found.iter().any(|(q, ..)| q == *p) && game.world.loaded(p[0], p[2]))
        .copied()
        .collect();
    for p in gone {
        stop_actors(game, Mover::Bearing(p));
        if let Some(mut entry) = taken.remove(&p)
            && let Some(t) = entry.turning.take()
        {
            disassemble_bearing(game, t, entry.bearing.angle);
            changed.push(p);
        }
    }
    if !generated.is_empty() {
        super::create_glue::with_kinetics(game, |k, level| {
            for (p, rpm) in generated {
                if k.get(p).is_some_and(|e| e.flow_score != rpm) {
                    k.set_flow_score(level, p, rpm);
                }
            }
        });
    }
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    for (p, entry) in taken {
        s.bearings.insert((dimension.to_owned(), p), entry);
    }
}

/// `MechanicalBearingBlock.useItemOn` / `WindmillBearingBlock.useItemOn`
/// with an empty hand, not sneaking: true when it took the click.
fn click_bearing(game: &mut Game<'_>, pos: Pos) -> bool {
    let name = &game.res.facts().get(game.world.block(pos[0], pos[1], pos[2])).name;
    if name.as_ref() != "create:mechanical_bearing" && name.as_ref() != "create:windmill_bearing" {
        return false;
    }
    if game.held().is_some() || game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft) {
        return false;
    }
    let dimension = game.dim.id().to_owned();
    state().get_or_insert_with(State::default).clicked.push((dimension, pos));
    true
}

/// `ElevatorPulleyBlock.useItemOn` with an empty hand, not sneaking.
fn click_elevator(game: &mut Game<'_>, pos: Pos) -> bool {
    if name_at(game, pos) != "create:elevator_pulley" {
        return false;
    }
    if game.held().is_some() || game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft) {
        return false;
    }
    let dimension = game.dim.id().to_owned();
    state().get_or_insert_with(State::default).elevator_clicks.push((dimension, pos));
    true
}

/// `RedstoneContactBlock.updateShape` for the blocks changed and their
/// neighbours: a redstone contact gives a signal while another faces it
/// (`hasValidContact`).
pub(super) fn contacts_changed(game: &mut Game<'_>, changed: &[Pos]) {
    let mut seen = std::collections::HashSet::new();
    for &p in changed {
        for at in std::iter::once(p).chain(Dir::ALL.into_iter().map(|d| add(p, d))) {
            if !seen.insert(at) || !game.world.loaded(at[0], at[2]) {
                continue;
            }
            let Some((name, facing)) = contact_at(game, at) else {
                continue;
            };
            if name != REDSTONE_CONTACT {
                continue;
            }
            let front = add(at, facing);
            let valid = contact_at(game, front).is_some_and(|(_, f)| f == facing.opposite());
            let id = game.world.block(at[0], at[1], at[2]);
            let want = if valid { "true" } else { "false" };
            if prop(game, id, "powered").as_deref() != Some(want) {
                set_named(game, at, REDSTONE_CONTACT, &[("facing", facing.name()), ("powered", want)]);
            }
        }
    }
}

/// The angle a client draws the bearing at `pos`'s top at (degrees).
pub(super) fn bearing_angle(pos: Pos) -> f32 {
    CLIENT_BEARINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(&pos))
        .map_or(0.0, |v| v.angle + v.speed * v.at.elapsed().as_secs_f32().min(0.5) * 20.0)
}

#[derive(Clone)]
struct BearingView {
    facing: Dir,
    angle: f32,
    /// Degrees a tick.
    speed: f32,
    blocks: Vec<(Pos, BlockStateId, bool)>,
    at: Instant,
}

static CLIENT_BEARINGS: Mutex<Option<HashMap<Pos, BearingView>>> = Mutex::new(None);
/// The angle each turning bearing was drawn at on the window's last tick.
static RIDDEN: Mutex<Vec<(Pos, f32)>> = Mutex::new(Vec::new());

/// `ContraptionRenderer` for bearings: every block turned about the
/// anchor's centre by the angle running on between the server's updates.
fn draw_bearings(game: &Game<'_>, camera: glam::DVec3, out: &mut Vec<horizon_client::mesher::Vertex>) {
    let views: Vec<(Pos, BearingView)> = CLIENT_BEARINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|m| m.iter().map(|(p, v)| (*p, v.clone())).collect())
        .unwrap_or_default();
    for (pos, v) in views {
        let anchor = [0, 1, 2].map(|k| pos[k] + v.facing.normal()[k]);
        let centre = glam::DVec3::new(f64::from(anchor[0]), f64::from(anchor[1]), f64::from(anchor[2])) + 0.5;
        if centre.distance_squared(camera) > VIEW_DISTANCE * VIEW_DISTANCE {
            continue;
        }
        let angle = v.angle + v.speed * v.at.elapsed().as_secs_f32().min(0.5) * 20.0;
        let axis = match v.facing.axis() {
            super::create_kinetics::Axis::X => glam::Vec3::X,
            super::create_kinetics::Axis::Y => glam::Vec3::Y,
            super::create_kinetics::Axis::Z => glam::Vec3::Z,
        };
        let turn = glam::Mat4::from_translation((centre - camera).as_vec3()) * glam::Mat4::from_axis_angle(axis, angle.to_radians());
        for (local, state, _) in &v.blocks {
            let r = super::create_kinetics::rotate_vec(local.map(|c| c as f32), angle, v.facing.axis());
            let lit = [0, 1, 2].map(|k| (centre[k] + f64::from(r[k])).floor() as i32);
            let light = game.world.light(lit[0], lit[1], lit[2]);
            let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
            let cell = [0, 1, 2].map(|k| local[k] + anchor[k]);
            let seed = horizon_core::random::position_seed(horizon_core::position::BlockPos::new(cell[0], cell[1], cell[2]));
            let m = turn * glam::Mat4::from_translation(glam::Vec3::new(local[0] as f32, local[1] as f32, local[2] as f32) - 0.5);
            draw_block(game, *state, seed, m, packed, out);
        }
    }
}

// ---- Rope pulleys ----------------------------------------------------------------

const ROPE: &str = "create:rope";
const MAGNET: &str = "create:pulley_magnet";

fn below(pos: Pos, n: i32) -> Pos {
    [pos[0], pos[1] - n, pos[2]]
}

fn name_at(game: &Game<'_>, pos: Pos) -> String {
    game.res.facts().get(game.world.block(pos[0], pos[1], pos[2])).name.to_string()
}

/// `PulleyBlockEntity.assemble`'s rope count: rope and magnet blocks below.
fn count_ropes(game: &Game<'_>, pos: Pos, range: i32) -> i32 {
    let mut i = 1;
    while i <= range {
        let n = name_at(game, below(pos, i));
        if n != ROPE && n != MAGNET {
            break;
        }
        i += 1;
    }
    i - 1
}

/// `PulleyBlockEntity.assemble`: the rope counted and taken in, what hangs
/// below taken out of the world. False when the pulley doesn't start.
fn assemble_pulley(game: &mut Game<'_>, pos: Pos, rpm: f32, entry: &mut PulleyEntry) -> bool {
    let ropes = count_ropes(game, pos, entry.pulley.range);
    let built = {
        let scan = scan(game);
        match PulleyContraption::assemble(&scan, pos, ropes) {
            Ok(Some(c)) => {
                let dir = if rpm > 0.0 { Dir::Down } else { Dir::Up };
                let next = [0, 1, 2].map(|k| c.anchor[k] + dir.normal()[k]);
                if super::create_contraption::colliding_with_world(&c.blocks, &scan, next, dir) {
                    (None, Some(c))
                } else {
                    (Some(true), Some(c))
                }
            }
            Ok(None) => (Some(false), None),
            Err(e) => {
                log::debug!("Create: pulley at {pos:?} can't take its load: {e:?}");
                (None, None)
            }
        }
    };
    let (mut built, contraption) = built;
    // Going up, what hangs there is taken even when it couldn't move.
    if rpm < 0.0 && contraption.is_some() {
        built = Some(true);
    }
    if !entry.pulley.start(rpm, ropes, built) {
        return false;
    }
    // removeRopes.
    let air = game.res.facts().air;
    use super::native_hooks::flags;
    for i in 1..=ropes {
        set(game, below(pos, i), air, flags::UPDATE_MOVE_BY_PISTON | flags::UPDATE_ALL);
    }
    let Some(contraption) = contraption.filter(|_| built == Some(true)) else {
        return true;
    };
    entry.hanging = Some(take_hanging(game, contraption));
    true
}

/// What hangs on a pulley taken out of the world (`removeBlocksFromWorld`),
/// with its block entities, chassis ranges and glue.
fn take_hanging(game: &mut Game<'_>, contraption: PulleyContraption<BlockStateId>) -> Hanging {
    let at: Vec<Pos> = contraption
        .blocks
        .iter()
        .map(|b| [0, 1, 2].map(|k| b.local[k] + contraption.anchor[k]))
        .collect();
    let mut texts = Vec::with_capacity(at.len());
    let mut entities = Vec::with_capacity(at.len());
    for (b, &p) in contraption.blocks.iter().zip(&at) {
        texts.push(state_text(game, b.state));
        entities.push(capture(game, p, b.state));
    }
    remove_blocks(game, &at);
    let dimension = game.dim.id().to_owned();
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    let mut ranges = Vec::new();
    for (i, p) in at.iter().enumerate() {
        if let Some(r) = s.ranges.remove(&(dimension.clone(), *p)) {
            ranges.push((i, r));
        }
    }
    let all = s.glue.entry(dimension).or_default();
    let glue: Vec<(Pos, Pos)> = contraption.glue.iter().filter_map(|&g| all.get(g).copied()).collect();
    all.retain(|g| !glue.contains(g));
    Hanging {
        contraption,
        texts,
        entities,
        ranges,
        glue,
    }
}

/// `PulleyBlockEntity.disassemble` at `grid` (rope and magnet put down to
/// there unless the pulley is gone: `ropes`), what hung put back.
fn disassemble_pulley(game: &mut Game<'_>, pos: Pos, hanging: Option<Hanging>, grid: i32, ropes: bool) {
    use super::native_hooks::flags;
    let flag = flags::UPDATE_MOVE_BY_PISTON | flags::UPDATE_ALL;
    if ropes {
        let named = |game: &Game<'_>, name: &str| game.pack.assets.registry.default_state(name);
        if grid > 0 {
            let at = below(pos, grid);
            let here = game.res.facts().get(game.world.block(at[0], at[1], at[2]));
            if here.hardness != -1.0
                && let Some(magnet) = named(game, MAGNET)
            {
                if !game.world.is_air(game.world.block(at[0], at[1], at[2])) {
                    let _ = game.destroy_block_dropping(at);
                }
                set(game, at, magnet, flag);
            }
        }
        for i in 1..grid {
            let at = below(pos, i);
            let here = game.res.facts().get(game.world.block(at[0], at[1], at[2]));
            if here.hardness == -1.0 {
                continue;
            }
            if !game.world.is_air(game.world.block(at[0], at[1], at[2])) {
                let _ = game.destroy_block_dropping(at);
            }
            if let Some(rope) = named(game, ROPE) {
                set(game, at, rope, flag);
            }
        }
    }
    let Some(h) = hanging else {
        return;
    };
    let c = &h.contraption;
    let anchor = c.anchor_at(grid);
    let shift = anchor[1] - c.anchor[1];
    let mut placed = vec![[0; 3]; c.blocks.len()];
    for i in super::create_contraption::placing_order(&c.blocks) {
        let b = &c.blocks[i];
        let at = [0, 1, 2].map(|k| b.local[k] + anchor[k]);
        put_back(game, at, b.state, b.solid, h.entities[i].clone());
        placed[i] = at;
    }
    let dimension = game.dim.id().to_owned();
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    for &(i, r) in &h.ranges {
        if let Some(at) = placed.get(i) {
            s.ranges.insert((dimension.clone(), *at), r);
        }
    }
    let down = |p: Pos| [p[0], p[1] + shift, p[2]];
    s.glue
        .entry(dimension)
        .or_default()
        .extend(h.glue.iter().map(|&(a, b)| (down(a), down(b))));
}

/// After the kinetics' tick of `dimension`: every rope pulley's tick.
fn pulleys_tick(game: &mut Game<'_>, dimension: &str, changed: &mut Vec<Pos>) {
    let found: Vec<(Pos, f32)> = super::create_glue::with_kinetics(game, |k, level| {
        k.positions()
            .into_iter()
            .filter_map(|p| match k.get(p).map(|e| e.kind) {
                Some(Kind::RopePulley(_)) => Some((p, k.speed_of(level, p))),
                _ => None,
            })
            .collect()
    });
    let mut taken: HashMap<Pos, PulleyEntry> = {
        let mut guard = state();
        let s = guard.get_or_insert_with(State::default);
        let keys: Vec<(String, Pos)> = s.pulleys.keys().filter(|(d, _)| d == dimension).cloned().collect();
        keys.into_iter().filter_map(|k| s.pulleys.remove(&k).map(|e| (k.1, e))).collect()
    };
    let min_y = game.world.shape.min_y;
    for &(pos, rpm) in &found {
        if !game.world.loaded(pos[0], pos[2]) {
            continue;
        }
        let mut entry = taken.remove(&pos).unwrap_or_default();
        entry.pulley.range = Pulley::range_at(pos[1], min_y);
        entry.pulley.speed(rpm);
        let before = entry.pulley.offset;
        let stalled = match &mut entry.hanging {
            Some(h) if entry.pulley.running => {
                let c = &h.contraption;
                let entities = &mut h.entities;
                let y = f64::from(c.anchor[1]) + f64::from(c.initial_offset) - f64::from(before);
                let blocks = || c.blocks.iter().map(|m| (m.local, m.state)).collect();
                let place = |l: Pos, v: [f64; 3]| {
                    [
                        f64::from(l[0] + c.anchor[0]) + 0.5 + v[0],
                        f64::from(l[1]) + y + 0.5 + v[1],
                        f64::from(l[2] + c.anchor[2]) + 0.5 + v[2],
                    ]
                };
                tick_actors(game, Mover::Pulley(pos), &blocks, &place, &|v| v, &|v| v, None, entities)
            }
            _ => false,
        };
        let step = {
            let scan = scan(game);
            let hanging = entry.hanging.as_ref();
            entry.pulley.tick(
                rpm,
                stalled,
                |depth| {
                    let f = scan.facts(below(pos, depth));
                    f.necessary && !f.brittle
                },
                |grid, dir| {
                    hanging.is_some_and(|h| {
                        super::create_contraption::colliding_with_world(&h.contraption.blocks, &scan, grid, dir)
                    })
                },
                |offset| {
                    hanging.map(|h| {
                        let a = h.contraption.anchor;
                        [a[0] as f32, a[1] as f32 + h.contraption.initial_offset as f32 - offset, a[2] as f32]
                    })
                },
            )
        };
        match step {
            PulleyStep::Idle => {}
            PulleyStep::Assemble => {
                if assemble_pulley(game, pos, rpm, &mut entry) {
                    changed.push(pos);
                }
            }
            PulleyStep::Moved => {
                if let Some(h) = &entry.hanging {
                    let c = &h.contraption;
                    let y = f64::from(c.anchor[1]) + f64::from(c.initial_offset) - f64::from(before);
                    let blocks: Vec<[f64; 3]> = c
                        .blocks
                        .iter()
                        .filter(|b| b.solid)
                        .map(|b| [f64::from(b.local[0] + c.anchor[0]), f64::from(b.local[1]) + y, f64::from(b.local[2] + c.anchor[2])])
                        .collect();
                    let delta = [0.0, -f64::from(entry.pulley.offset - before), 0.0];
                    carry(game, Some(dimension), &blocks, delta);
                }
            }
            PulleyStep::Disassemble { grid_offset, reassemble } => {
                stop_actors(game, Mover::Pulley(pos));
                let hanging = entry.hanging.take();
                disassemble_pulley(game, pos, hanging, grid_offset, true);
                entry.pulley.stopped(grid_offset, reassemble);
                changed.push(pos);
            }
        }
        taken.insert(pos, entry);
    }
    // Pulleys that are gone put down what hung there, without rope.
    let gone: Vec<Pos> = taken
        .iter()
        .filter(|(p, e)| !e.elevator && !found.iter().any(|(q, _)| q == *p) && game.world.loaded(p[0], p[2]))
        .map(|(p, _)| *p)
        .collect();
    for p in gone {
        stop_actors(game, Mover::Pulley(p));
        if let Some(entry) = taken.remove(&p)
            && entry.pulley.running
        {
            let grid = entry.pulley.grid_offset(entry.pulley.offset);
            disassemble_pulley(game, p, entry.hanging, grid, false);
            changed.push(p);
        }
    }
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    for (p, entry) in taken {
        s.pulleys.insert((dimension.to_owned(), p), entry);
    }
}

#[derive(Clone)]
struct PulleyView {
    offset: f32,
    /// Blocks a tick, down.
    speed: f32,
    range: i32,
    anchor: Pos,
    initial: i32,
    blocks: Vec<(Pos, BlockStateId, bool)>,
    at: Instant,
}

static CLIENT_PULLEYS: Mutex<Option<HashMap<Pos, PulleyView>>> = Mutex::new(None);
/// Pulley loads (and elevator cabins) just put back, for the window's player
/// standing on them.
static ENDED_PULLEYS: Mutex<Vec<PulleyView>> = Mutex::new(Vec::new());

impl PulleyView {
    /// `getInterpolatedOffset`, running on between the server's updates.
    fn offset_now(&self) -> f32 {
        let ticks = self.at.elapsed().as_secs_f32().min(0.5) * 20.0;
        (self.offset + self.speed * ticks).clamp(0.0, self.range as f32)
    }
}

/// A running pulley's rope length on this client, none when it isn't
/// running (its rope and magnet are blocks then).
pub(super) fn pulley_offset(pos: Pos) -> Option<f32> {
    CLIENT_PULLEYS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()?
        .get(&pos)
        .map(PulleyView::offset_now)
}

/// `ContraptionRenderer` for pulleys: what hangs, where the rope's end is.
fn draw_pulleys(game: &Game<'_>, camera: glam::DVec3, out: &mut Vec<horizon_client::mesher::Vertex>) {
    let views: Vec<PulleyView> = CLIENT_PULLEYS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default();
    for v in views {
        let y = f64::from(v.initial) - f64::from(v.offset_now());
        for (local, state, _) in &v.blocks {
            let cell = [0, 1, 2].map(|k| local[k] + v.anchor[k]);
            let corner = glam::DVec3::new(f64::from(cell[0]), f64::from(cell[1]) + y, f64::from(cell[2]));
            if (corner + glam::DVec3::splat(0.5)).distance_squared(camera) > VIEW_DISTANCE * VIEW_DISTANCE {
                continue;
            }
            let lit = [cell[0], (corner.y + 0.5).floor() as i32, cell[2]];
            let light = game.world.light(lit[0], lit[1], lit[2]);
            let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
            let seed = horizon_core::random::position_seed(horizon_core::position::BlockPos::new(cell[0], cell[1], cell[2]));
            let m = glam::Mat4::from_translation((corner - camera).as_vec3());
            draw_block(game, *state, seed, m, packed, out);
        }
    }
}

// ---- Gantries ---------------------------------------------------------------------

const GANTRY_CARRIAGE: &str = "create:gantry_carriage";

/// The gantry shaft at `pos`: its facing and part.
fn shaft_at(game: &Game<'_>, pos: Pos) -> Option<(Dir, super::create_contraption::GantryPart)> {
    let id = game.world.block(pos[0], pos[1], pos[2]);
    let part = super::create_contraption::gantry_shaft_of(&game.res.facts().get(id).name)?;
    let facing = prop(game, id, "facing").and_then(|f| Dir::parse(&f))?;
    Some((facing, part))
}

/// The carriage at `pos`: its facing.
fn carriage_at(game: &Game<'_>, pos: Pos) -> Option<Dir> {
    let id = game.world.block(pos[0], pos[1], pos[2]);
    let name = &game.res.facts().get(id).name;
    if &**name != GANTRY_CARRIAGE && &**name != super::create_kinetics::GANTRY_CARRIAGE_ALONG {
        return None;
    }
    prop(game, id, "facing").and_then(|f| Dir::parse(&f))
}

/// `updateShape` for the gantry shaft at `pos`: its part from the shafts
/// of its facing on either side.
pub(super) fn fix_gantry_part(game: &mut Game<'_>, pos: Pos) {
    let Some((facing, part)) = shaft_at(game, pos) else {
        return;
    };
    let joins = |d: Dir| shaft_at(game, add(pos, d)).is_some_and(|(f, _)| f == facing);
    let next = super::create_contraption::GantryPart::of(joins(facing), joins(facing.opposite()));
    if next != part {
        set_named(game, pos, &super::create_contraption::gantry_shaft_block(next), &[("facing", facing.name())]);
    }
}

/// `GantryShaftBlock.getStateForPlacement` a tick after a player placed
/// one: facing the way they look (away; towards when sneaking), or as a
/// shaft along that axis behind it; then the parts around it.
pub(super) fn placed_gantry_shaft(game: &mut Game<'_>, pos: Pos, sneaking: bool, looking: Dir) {
    let Some((current, _)) = shaft_at(game, pos) else {
        return;
    };
    let mut facing = if sneaking { looking } else { looking.opposite() };
    let beside = [facing.opposite(), facing]
        .into_iter()
        .find_map(|d| shaft_at(game, add(pos, d)).filter(|(f, _)| f.axis() == facing.axis()));
    if let Some((f, _)) = beside {
        facing = if sneaking { f.opposite() } else { f };
    }
    if facing != current {
        let name = name_at(game, pos);
        set_named(game, pos, &name, &[("facing", facing.name())]);
    }
    for at in [pos, add(pos, facing), add(pos, facing.opposite())] {
        fix_gantry_part(game, at);
    }
}

/// A carriage placed, or clicked with an empty hand
/// (`checkValidGantryShaft`): it tries to assemble next tick.
pub(super) fn queue_carriage(game: &Game<'_>, pos: Pos) {
    let dimension = game.dim.id().to_owned();
    state().get_or_insert_with(State::default).gantry_queue.push((dimension, pos));
}

fn click_carriage(game: &mut Game<'_>, pos: Pos) -> bool {
    if carriage_at(game, pos).is_none()
        || game.held().is_some()
        || game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft)
    {
        return false;
    }
    queue_carriage(game, pos);
    true
}

/// `GantryCarriageBlockEntity.tryAssemble`.
fn assemble_gantry(game: &mut Game<'_>, pos: Pos, rpm: &HashMap<Pos, f32>, id: u32) -> Option<GantryMove> {
    let facing = carriage_at(game, pos)?;
    let shaft = add(pos, facing.opposite());
    let (shaft_facing, part) = shaft_at(game, shaft)?;
    if shaft_facing.axis() == facing.axis() {
        return None;
    }
    let speed = super::create_contraption::pinion_speed(rpm.get(&shaft).copied().unwrap_or(0.0));
    // shouldAssemble.
    if !part.can_assemble_on(speed) {
        return None;
    }
    let dir = if speed < 0.0 { shaft_facing.opposite() } else { shaft_facing };
    let contraption = {
        let scan = scan(game);
        let c = match GantryContraption::assemble(&scan, pos, facing) {
            Ok(Some(c)) => c,
            Ok(None) => return None,
            Err(e) => {
                log::debug!("Create: gantry at {pos:?} can't move: {e:?}");
                return None;
            }
        };
        if super::create_contraption::colliding_with_world(&c.blocks, &scan, add(pos, dir), dir) {
            return None;
        }
        c
    };
    let at: Vec<Pos> = contraption
        .blocks
        .iter()
        .map(|b| [0, 1, 2].map(|k| b.local[k] + pos[k]))
        .collect();
    let mut texts = Vec::with_capacity(at.len());
    let mut entities = Vec::with_capacity(at.len());
    for (b, &p) in contraption.blocks.iter().zip(&at) {
        texts.push(state_text(game, b.state));
        entities.push(capture(game, p, b.state));
    }
    remove_blocks(game, &at);
    let dimension = game.dim.id().to_owned();
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    let mut ranges = Vec::new();
    for (i, p) in at.iter().enumerate() {
        if let Some(r) = s.ranges.remove(&(dimension.clone(), *p)) {
            ranges.push((i, r));
        }
    }
    let all = s.glue.entry(dimension).or_default();
    let glue: Vec<(Pos, Pos)> = contraption.glue.iter().filter_map(|&g| all.get(g).copied()).collect();
    all.retain(|g| !glue.contains(g));
    Some(GantryMove {
        id,
        contraption,
        at: pos.map(f64::from),
        motion: [0.0; 3],
        ticks: 0,
        texts,
        entities,
        ranges,
        glue,
    })
}

/// `AbstractContraptionEntity.disassemble` for a gantry: the structure put
/// back with its anchor at `grid`.
fn disassemble_gantry(game: &mut Game<'_>, g: GantryMove, grid: Pos) {
    let c = &g.contraption;
    let mut placed = vec![[0; 3]; c.blocks.len()];
    for i in super::create_contraption::placing_order(&c.blocks) {
        let b = &c.blocks[i];
        let at = [0, 1, 2].map(|k| b.local[k] + grid[k]);
        put_back(game, at, b.state, b.solid, g.entities[i].clone());
        placed[i] = at;
    }
    let shift = [0, 1, 2].map(|k| grid[k] - c.anchor[k]);
    let dimension = game.dim.id().to_owned();
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    for &(i, r) in &g.ranges {
        if let Some(at) = placed.get(i) {
            s.ranges.insert((dimension.clone(), *at), r);
        }
    }
    let moved = |p: Pos| [0, 1, 2].map(|k| p[k] + shift[k]);
    s.glue
        .entry(dimension)
        .or_default()
        .extend(g.glue.iter().map(|&(a, b)| (moved(a), moved(b))));
}

/// After the kinetics' tick of `dimension`: carriages assembled whose shaft
/// changed speed or that were queued, then every moving gantry's tick.
fn gantries_tick(game: &mut Game<'_>, dimension: &str) {
    let shafts: HashMap<Pos, f32> = super::create_glue::with_kinetics(game, |k, level| {
        k.positions()
            .into_iter()
            .filter_map(|p| match k.get(p).map(|e| e.kind) {
                Some(Kind::GantryShaft(_)) => Some((p, k.speed_of(level, p))),
                _ => None,
            })
            .collect()
    });
    let (queue, mut moving, mut next_id) = {
        let mut guard = state();
        let s = guard.get_or_insert_with(State::default);
        let mut queue: Vec<Pos> = s.gantry_queue.iter().filter(|(d, _)| d == dimension).map(|(_, p)| *p).collect();
        s.gantry_queue.retain(|(d, _)| d != dimension);
        // GantryShaftBlockEntity.onSpeedChanged: checkAttachedCarriageBlocks.
        let mut changed_shafts = Vec::new();
        for (p, rpm) in &shafts {
            let key = (dimension.to_owned(), *p);
            if s.shaft_rpm.get(&key) != Some(rpm) {
                s.shaft_rpm.insert(key, *rpm);
                changed_shafts.push(*p);
            }
        }
        s.shaft_rpm.retain(|(d, p), _| d != dimension || shafts.contains_key(p));
        let all = std::mem::take(&mut s.gantries);
        let (mine, others): (Vec<_>, Vec<_>) = all.into_iter().partition(|(d, _)| d == dimension);
        s.gantries = others;
        drop(guard);
        for p in changed_shafts {
            if let Some((f, _)) = shaft_at(game, p) {
                for d in Dir::ALL {
                    let c = add(p, d);
                    if d.axis() != f.axis() && carriage_at(game, c) == Some(d) && !queue.contains(&c) {
                        queue.push(c);
                    }
                }
            }
        }
        let next = state().as_ref().map_or(0, |s| s.next_gantry);
        (queue, mine.into_iter().map(|(_, g)| g).collect::<Vec<_>>(), next)
    };
    for pos in queue {
        if let Some(g) = assemble_gantry(game, pos, &shafts, next_id) {
            next_id = next_id.wrapping_add(1);
            moving.push(g);
        }
    }
    let mut still = Vec::new();
    for mut g in moving {
        g.ticks += 1;
        let stalled = {
            let c = &g.contraption;
            let entities = &mut g.entities;
            let at = g.at;
            let blocks = || c.blocks.iter().map(|m| (m.local, m.state)).collect();
            let place = |l: Pos, v: [f64; 3]| [0, 1, 2].map(|k| f64::from(l[k]) + at[k] + 0.5 + v[k]);
            tick_actors(game, Mover::Gantry(g.id), &blocks, &place, &|v| v, &|v| v, None, entities)
        };
        let step = {
            let scan = scan(game);
            let blocks = &g.contraption.blocks;
            super::create_contraption::gantry_tick(
                g.at,
                g.contraption.facing,
                g.ticks,
                stalled,
                |p| {
                    let (facing, part) = shaft_at(game, p)?;
                    Some(ShaftUnder {
                        facing,
                        part,
                        rpm: shafts.get(&p).copied().unwrap_or(0.0),
                    })
                },
                |grid, dir| super::create_contraption::colliding_with_world(blocks, &scan, grid, dir),
            )
        };
        match step {
            GantryStep::Moved(m) => {
                let blocks: Vec<[f64; 3]> = g
                    .contraption
                    .blocks
                    .iter()
                    .filter(|b| b.solid)
                    .map(|b| [0, 1, 2].map(|k| f64::from(b.local[k]) + g.at[k]))
                    .collect();
                carry(game, Some(dimension), &blocks, m);
                g.at = [0, 1, 2].map(|k| g.at[k] + m[k]);
                g.motion = m;
                still.push(g);
            }
            GantryStep::Disassemble(grid) => {
                stop_actors(game, Mover::Gantry(g.id));
                disassemble_gantry(game, g, grid);
            }
        }
    }
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    s.next_gantry = next_id;
    s.gantries.extend(still.into_iter().map(|g| (dimension.to_owned(), g)));
}

#[derive(Clone)]
struct GantryView {
    at: [f64; 3],
    motion: [f64; 3],
    blocks: Vec<(Pos, BlockStateId, bool)>,
    received: Instant,
}

static CLIENT_GANTRIES: Mutex<Option<HashMap<u32, GantryView>>> = Mutex::new(None);

impl GantryView {
    fn at_now(&self) -> [f64; 3] {
        let ticks = f64::from(self.received.elapsed().as_secs_f32().min(0.5) * 20.0);
        [0, 1, 2].map(|k| self.at[k] + self.motion[k] * ticks)
    }
}

/// `ContraptionRenderer` for gantries.
fn draw_gantries(game: &Game<'_>, camera: glam::DVec3, out: &mut Vec<horizon_client::mesher::Vertex>) {
    let views: Vec<GantryView> = CLIENT_GANTRIES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default();
    for v in views {
        let at = v.at_now();
        for (local, state, _) in &v.blocks {
            let corner = glam::DVec3::new(
                f64::from(local[0]) + at[0],
                f64::from(local[1]) + at[1],
                f64::from(local[2]) + at[2],
            );
            if (corner + glam::DVec3::splat(0.5)).distance_squared(camera) > VIEW_DISTANCE * VIEW_DISTANCE {
                continue;
            }
            let cell = [0, 1, 2].map(|k| (corner.to_array()[k] + 0.5).floor() as i32);
            let light = game.world.light(cell[0], cell[1], cell[2]);
            let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
            let seed = horizon_core::random::position_seed(horizon_core::position::BlockPos::new(cell[0], cell[1], cell[2]));
            let m = glam::Mat4::from_translation((corner - camera).as_vec3());
            draw_block(game, *state, seed, m, packed, out);
        }
    }
}

// ---- Elevator pulleys --------------------------------------------------------------

const REDSTONE_CONTACT: &str = "create:redstone_contact";
const ELEVATOR_CONTACT: &str = "create:elevator_contact";

/// A redstone or elevator contact at `pos` and its facing.
fn contact_at(game: &Game<'_>, pos: Pos) -> Option<(String, Dir)> {
    let name = name_at(game, pos);
    if name != REDSTONE_CONTACT && name != ELEVATOR_CONTACT {
        return None;
    }
    let id = game.world.block(pos[0], pos[1], pos[2]);
    Some((name, Dir::parse(&prop(game, id, "facing")?)?))
}

/// `ElevatorColumn.gatherAll`: the redstone contacts of the column turn into
/// elevator contacts, and every elevator contact in it is counted.
fn gather_column(game: &mut Game<'_>, column: Column, into: &mut ElevatorColumn) {
    let (lo, hi) = (game.world.shape.min_y, game.world.shape.max_y());
    for y in lo..hi {
        let pos = [column.x, y, column.z];
        if !game.world.loaded(pos[0], pos[2]) {
            return;
        }
        let Some((name, facing)) = contact_at(game, pos) else {
            continue;
        };
        if facing != column.side {
            continue;
        }
        if name == REDSTONE_CONTACT {
            set_named(game, pos, ELEVATOR_CONTACT, &[("facing", facing.name()), ("powered", "false")]);
        }
        into.add(y);
    }
}

/// An elevator contact's signal out (`POWERING`).
fn set_contact_output(game: &mut Game<'_>, pos: Pos, on: bool) {
    let Some((name, facing)) = contact_at(game, pos) else {
        return;
    };
    if name != ELEVATOR_CONTACT {
        return;
    }
    let id = game.world.block(pos[0], pos[1], pos[2]);
    let want = if on { "true" } else { "false" };
    if prop(game, id, "powered").as_deref() != Some(want) {
        set_named(game, pos, ELEVATOR_CONTACT, &[("facing", facing.name()), ("powered", want)]);
    }
}

/// `ElevatorPulleyBlockEntity.assemble`: the cabin under the pulley at `pos`
/// (the first block below that stops a rope), its one sideways redstone
/// contact giving the column; the cabin stays where it is, its floor called.
fn assemble_elevator(game: &mut Game<'_>, pos: Pos) -> Result<Option<ElevatorEntry>, ElevatorError> {
    let mut i = 1;
    while i <= super::create_contraption::MAX_ROPE_LENGTH {
        let p = below(pos, i);
        if p[1] < game.world.shape.min_y {
            break;
        }
        let facts = game.res.facts().get(game.world.block(p[0], p[1], p[2]));
        if !game.block_collision_shape(p).boxes().is_empty() && !facts.replaceable {
            break;
        }
        i += 1;
    }
    let offset = i - 1;
    let contraption = {
        let scan = scan(game);
        match PulleyContraption::assemble(&scan, pos, offset) {
            Ok(Some(c)) => c,
            _ => return Ok(None),
        }
    };
    let contacts: Vec<(Pos, Dir)> = contraption
        .blocks
        .iter()
        .filter_map(|b| {
            let at = [0, 1, 2].map(|k| b.local[k] + contraption.anchor[k]);
            contact_at(game, at).map(|(_, f)| (b.local, f))
        })
        .collect();
    let (column, contact_y) = super::create_contraption::elevator_column(&contacts, contraption.anchor)?;
    let dim = game.dim.id().to_owned();
    let mut col = state()
        .as_mut()
        .and_then(|s| s.columns.remove(&(dim.clone(), column)))
        .unwrap_or_default();
    if col.active {
        state().get_or_insert_with(State::default).columns.insert((dim, column), col);
        return Err(ElevatorError::ColumnConflict);
    }
    let elevator = Elevator::new(pos[1], offset, contact_y);
    // column.target: the floor it stands at, its contact the one calling.
    let level = elevator.level_at(pos[1], offset);
    col.target = Some(level);
    col.calling = Some(level);
    gather_column(game, column, &mut col);
    col.active = true;
    state().get_or_insert_with(State::default).columns.insert((dim, column), col);
    Ok(Some(ElevatorEntry { elevator, column }))
}

/// The elevator pulleys' ticks: assembled when they start turning, their
/// cabins taken out and moved toward the floor called (`getMovementSpeed`)
/// and put back on arrival, the contact there giving a signal; contacts
/// given a signal call the cabin (`ElevatorContactBlock.neighborChanged`).
fn elevators_tick(game: &mut Game<'_>, dimension: &str, changed: &mut Vec<Pos>) {
    let found: Vec<(Pos, f32)> = super::create_glue::with_kinetics(game, |k, level| {
        k.positions()
            .into_iter()
            .filter_map(|p| match k.get(p).map(|e| e.kind) {
                Some(Kind::ElevatorPulley(_)) => Some((p, k.speed_of(level, p))),
                _ => None,
            })
            .collect()
    });
    let dim = dimension.to_owned();
    let clicks: Vec<Pos> = {
        let mut guard = state();
        let s = guard.get_or_insert_with(State::default);
        let mine = s.elevator_clicks.iter().filter(|(d, _)| *d == dim).map(|(_, p)| *p).collect();
        s.elevator_clicks.retain(|(d, _)| *d != dim);
        mine
    };
    // Contacts called by a signal.
    let columns: Vec<Column> = state()
        .as_ref()
        .map(|s| s.columns.keys().filter(|(d, _)| *d == dim).map(|(_, c)| *c).collect())
        .unwrap_or_default();
    for column in columns {
        let ys = state()
            .as_ref()
            .and_then(|s| s.columns.get(&(dim.clone(), column)).map(|c| c.contacts.clone()))
            .unwrap_or_default();
        for y in ys {
            let at = [column.x, y, column.z];
            if !game.world.loaded(at[0], at[2]) || contact_at(game, at).is_none() {
                continue;
            }
            let signal = super::create_glue::signal(game, at);
            let rising = {
                let mut guard = state();
                let s = guard.get_or_insert_with(State::default);
                let was = !s.contact_inputs.insert((dim.clone(), at)) || !signal;
                if !signal {
                    s.contact_inputs.remove(&(dim.clone(), at));
                }
                let col = s.columns.entry((dim.clone(), column)).or_default();
                let call = signal && !was && col.calling != Some(y);
                if call {
                    col.call(y);
                }
                call.then(|| col.contacts.clone())
            };
            // callToContactAndUpdate: the other contacts stop giving a signal.
            if let Some(all) = rising {
                for other in all.into_iter().filter(|o| *o != y) {
                    set_contact_output(game, [column.x, other, column.z], false);
                }
            }
        }
    }
    for (pos, rpm) in found {
        if !game.world.loaded(pos[0], pos[2]) {
            continue;
        }
        let key = (dim.clone(), pos);
        let (entry, started) = {
            let mut guard = state();
            let s = guard.get_or_insert_with(State::default);
            let before = s.elevator_rpm.insert(key.clone(), rpm).unwrap_or(0.0);
            (s.elevators.remove(&key), before == 0.0 && rpm != 0.0)
        };
        let clicked = clicks.contains(&pos);
        // clicked: a running elevator is taken apart (its cabin put down, its
        // column free), a stopped one assembles.
        if clicked && let Some(e) = &entry {
            let (cabin, column) = {
                let mut guard = state();
                let s = guard.get_or_insert_with(State::default);
                if let Some(c) = s.columns.get_mut(&(dim.clone(), e.column)) {
                    c.active = false;
                }
                let cabin = s.pulleys.remove(&key).filter(|c| c.elevator);
                (cabin, e.column)
            };
            let _ = column;
            if let Some(mut cabin) = cabin {
                disassemble_pulley(game, pos, cabin.hanging.take(), e.elevator.offset.round() as i32, false);
            }
            changed.push(pos);
            continue;
        }
        let mut entry = match entry {
            Some(e) => e,
            None if (started || clicked) && rpm != 0.0 => match assemble_elevator(game, pos) {
                Ok(Some(e)) => e,
                Ok(None) => continue,
                Err(e) => {
                    log::debug!("Create: elevator at {pos:?} can't assemble: {e:?}");
                    continue;
                }
            },
            None => continue,
        };
        // A loaded elevator gathers its column again (columns aren't saved),
        // its floor the one it stands at.
        let known = state().as_ref().is_some_and(|s| s.columns.contains_key(&(dim.clone(), entry.column)));
        if !known {
            let mut col = ElevatorColumn::default();
            gather_column(game, entry.column, &mut col);
            col.active = true;
            col.target = Some(entry.elevator.level_at(pos[1], entry.elevator.offset.round() as i32));
            state().get_or_insert_with(State::default).columns.insert((dim.clone(), entry.column), col);
        }
        let target_y = state()
            .as_ref()
            .and_then(|s| s.columns.get(&(dim.clone(), entry.column)).and_then(|c| c.target));
        let el = &mut entry.elevator;
        let target = el.target_offset(pos[1], target_y);
        let moving = state().as_ref().is_some_and(|s| s.pulleys.get(&key).is_some_and(|e| e.elevator));
        if !moving && rpm != 0.0 && target != el.offset as i32 {
            // The cabin is taken out to move.
            let taken = {
                let scan = scan(game);
                PulleyContraption::assemble(&scan, pos, el.offset as i32).ok().flatten()
            };
            if let Some(c) = taken {
                let hanging = take_hanging(game, c);
                el.prev_speed = 0.0;
                let mut pulley = Pulley::default();
                pulley.running = true;
                pulley.carrying = true;
                pulley.offset = el.offset;
                pulley.range = super::create_contraption::MAX_ROPE_LENGTH;
                pulley.assemble_next_tick = false;
                state().get_or_insert_with(State::default).pulleys.insert(
                    key.clone(),
                    PulleyEntry {
                        pulley,
                        hanging: Some(hanging),
                        elevator: true,
                    },
                );
                changed.push(pos);
            }
        }
        let cabin = state().as_mut().and_then(|s| {
            if s.pulleys.get(&key).is_some_and(|e| e.elevator) {
                s.pulleys.remove(&key)
            } else {
                None
            }
        });
        if let Some(mut cabin) = cabin {
            let (mut speed, arrived) = el.movement_speed(target, rpm);
            // Arrived, it eases onto the floor (a quarter of the way a tick)
            // before it is put back.
            let settled = arrived && (target as f32 - el.offset).abs() <= 1.0 / 128.0;
            if arrived && !settled {
                speed = (target as f32 - el.offset) * 0.25;
            }
            if settled || rpm == 0.0 {
                // Put back on the floor (or where it is, stopped).
                let grid = if arrived { target } else { el.offset.round() as i32 };
                disassemble_pulley(game, pos, cabin.hanging.take(), grid, false);
                el.offset = grid as f32;
                el.prev_speed = 0.0;
                changed.push(pos);
                if arrived {
                    // triggerContact.
                    let level = el.level_at(pos[1], grid);
                    set_contact_output(game, [entry.column.x, level, entry.column.z], true);
                }
            } else {
                if let Some(h) = &cabin.hanging {
                    let c = &h.contraption;
                    let y = f64::from(c.anchor[1]) + f64::from(c.initial_offset) - f64::from(el.offset);
                    let blocks: Vec<[f64; 3]> = c
                        .blocks
                        .iter()
                        .filter(|b| b.solid)
                        .map(|b| {
                            [f64::from(b.local[0] + c.anchor[0]), f64::from(b.local[1]) + y, f64::from(b.local[2] + c.anchor[2])]
                        })
                        .collect();
                    carry(game, Some(dimension), &blocks, [0.0, -f64::from(speed), 0.0]);
                }
                el.offset += speed;
                cabin.pulley.offset = el.offset;
                // What clients take as its speed (`movement_speed` of it).
                cabin.pulley.last_rpm = speed * 512.0;
                state().get_or_insert_with(State::default).pulleys.insert(key.clone(), cabin);
            }
        }
        state().get_or_insert_with(State::default).elevators.insert(key, entry);
    }
    // Elevator pulleys that are gone: their column is free again, a moving
    // cabin is put down where it is.
    let gone: Vec<(Pos, Column)> = state()
        .as_ref()
        .map(|s| {
            s.elevators
                .iter()
                .filter(|((d, p), _)| *d == dim && game.world.loaded(p[0], p[2]) && !matches!(
                    super::create_glue::kind_at(game, *p),
                    Some(Kind::ElevatorPulley(_))
                ))
                .map(|((_, p), e)| (*p, e.column))
                .collect()
        })
        .unwrap_or_default();
    for (p, column) in gone {
        let (cabin, offset) = {
            let mut guard = state();
            let s = guard.get_or_insert_with(State::default);
            let e = s.elevators.remove(&(dim.clone(), p));
            if let Some(c) = s.columns.get_mut(&(dim.clone(), column)) {
                c.active = false;
            }
            (s.pulleys.remove(&(dim.clone(), p)), e.map(|e| e.elevator.offset))
        };
        if let (Some(mut cabin), Some(offset)) = (cabin, offset) {
            disassemble_pulley(game, p, cabin.hanging.take(), offset.round() as i32, false);
            changed.push(p);
        }
    }
}
