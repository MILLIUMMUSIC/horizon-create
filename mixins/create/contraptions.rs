// Horizon's side of contraptions (contraption.rs), injected as
// `crate::game::create_contraptions`: each mechanical piston's
// `LinearActuatorBlockEntity` tick after the kinetics', its structure taken
// out of the world (`removeBlocksFromWorld`) and put back
// (`addBlocksToWorld`), riders carried along, the moving blocks drawn on
// clients from `create:contraptions`, and running pistons saved beside the
// world (`data/create_contraptions_*.txt`).

use super::Game;
use super::create_contraption::{
    Actuator, ActuatorStep, Chassis, ChassisKind, Facts, HEAD, MAX_CHASSIS_RANGE, POLE, PistonContraption,
    PistonPart, PistonState, STICKY_HEAD, StructureWorld, Sticky, attached_chassis, chassis_block, chassis_of,
    glue_group, in_span, piston_block, piston_of,
};
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
        if at == piston {
            entities.push(None);
            continue;
        }
        let key = (at[0], at[1], at[2]);
        let facts = game.res.facts().get(b.state);
        let name = facts.name.to_string();
        let has_entity = facts.block_entity;
        let nbt = game.world.block_entities.remove(&key).filter(|_| has_entity).and_then(|e| {
            match horizon_client::save::encode_block_entity(key, &e, &name) {
                horizon_world::nbt::Tag::Compound(c) => Some(c),
                _ => None,
            }
        });
        entities.push(nbt);
    }
    use super::native_hooks::flags;
    let air = game.res.facts().air;
    for at in contraption.removals() {
        set(
            game,
            at,
            air,
            flags::UPDATE_MOVE_BY_PISTON | flags::UPDATE_SUPPRESS_DROPS | flags::UPDATE_KNOWN_SHAPE | flags::UPDATE_ALL,
        );
    }
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
    use super::native_hooks::flags;
    set_piston(game, pos, PistonState::Extended);
    let c = &running.contraption;
    let (placements, retracted) = c.placements(grid_offset);
    for (at, i) in placements {
        let state = c.blocks[i].state;
        let here = game.world.block(at[0], at[1], at[2]);
        let here_facts = game.res.facts().get(here);
        let here_solid = !game.block_collision_shape(at).boxes().is_empty();
        if here_facts.hardness == -1.0 || (!c.blocks[i].solid && here_solid) {
            let item = game.res.facts().get(state).item.to_string();
            if item != "minecraft:air" {
                let centre = at.map(|v| f64::from(v) + 0.5);
                game.spawn_item(horizon_client::items::ItemStack::new(&item, 1), centre, [0.0, 0.1, 0.0], 10);
            }
            continue;
        }
        if !game.world.is_air(here) && !here_facts.replaceable {
            let _ = game.destroy_block_dropping(at);
        }
        set(game, at, state, flags::UPDATE_MOVE_BY_PISTON | flags::UPDATE_ALL);
        // The block entity's data back, at its new place (`loadWithComponents`).
        if let Some(mut nbt) = running.entities[i].clone() {
            use horizon_world::nbt::Tag;
            for (k, v) in [("x", at[0]), ("y", at[1]), ("z", at[2])] {
                nbt.insert(k.into(), Tag::Int(v));
            }
            if let Some((key, entity)) = horizon_client::save::decode_block_entity(&Tag::Compound(nbt)) {
                game.world.block_entities.insert(key, entity);
            }
        }
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
        for (i, b) in c.blocks.iter().enumerate() {
            let nbt = r.entities[i]
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
                r.texts[i],
                nbt
            );
        }
        for (i, range) in &r.ranges {
            out += &format!("r {i} {range}\n");
        }
        for g in &r.glue {
            out += &format!("g {}\n", corners(g));
        }
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
    let mut current: Option<Pos> = None;
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
                current = Some(pos);
            }
            Some("c") if f.len() >= 8 => {
                let Some(pos) = current else { continue };
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
                } else if let Some(r) = current
                    .and_then(|pos| s.pistons.get_mut(&(dimension.to_owned(), pos)))
                    .and_then(|p| p.running.as_mut())
                {
                    r.glue.push(g);
                }
            }
            Some("r") if f.len() >= 3 => {
                let (Ok(i), Ok(range)) = (f[1].parse(), f[2].parse()) else {
                    continue;
                };
                if let Some(r) = current
                    .and_then(|pos| s.pistons.get_mut(&(dimension.to_owned(), pos)))
                    .and_then(|p| p.running.as_mut())
                {
                    r.ranges.push((i, range));
                }
            }
            Some("b") if f.len() >= 9 => {
                let Some(pos) = current else { continue };
                let Some(r) = s.pistons.get_mut(&(dimension.to_owned(), pos)).and_then(|p| p.running.as_mut()) else {
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
                r.contraption.blocks.push(super::create_contraption::Moved {
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
        let step = {
            let scan = scan(game);
            let running = piston.running.as_ref();
            piston.actuator.tick(rpm, facing, anchor, |g, d| {
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
        if let Some(mut piston) = taken.remove(&p)
            && let Some(r) = piston.running.take()
        {
            let grid = piston.actuator.grid_offset(piston.actuator.offset);
            disassemble(game, p, r, grid);
            changed.push(p);
        }
    }
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
    *client() = None;
    CLIENT_GLUE.lock().unwrap_or_else(|e| e.into_inner()).clear();
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
        push_f(&mut out, p.actuator.movement_speed(rpm, c.orientation));
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
    s.changed.retain(|(d, _)| d != dimension);
    // Then the dimension's glue, for players holding super glue.
    let glue = s.glue.get(dimension).map_or(&[][..], Vec::as_slice);
    out.extend_from_slice(&(glue.len() as u32).to_le_bytes());
    for (a, b) in glue {
        for v in a.iter().chain(b) {
            push_i(&mut out, *v);
        }
    }
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
}

/// `native_tick` of the window: its player carried by the contraptions it
/// stands on, as the server last placed them and moving since.
pub(super) fn client_tick(game: &mut Game<'_>) {
    if !game.native_window() {
        return;
    }
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
pub(super) fn draw(game: &Game<'_>, camera: glam::DVec3, out: &mut Vec<horizon_client::mesher::Vertex>) {
    draw_glue(game, camera, out);
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
            game.block_model_vertices(*state, seed, m, packed, false, out);
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
