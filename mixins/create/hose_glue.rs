// Horizon's side of the hose pulley (rules in hose.rs).

use super::Game;
use super::create_fluids::{FluidHandler, FluidStack, FluidTank};
use super::create_hose::{self as rules, Cell, Drainer, Filler, Handler, HoseWorld};
use super::create_kinetics::{Axis, Dir, Pos};
use horizon_client::containers::{BlockEntity, ModBlockEntity};
use horizon_world::nbt::{JavaString, Tag};
use std::collections::HashMap;
use std::sync::Mutex;

pub(super) const BLOCK: &str = "create:hose_pulley";

/// Its behaviours, by dimension and position (`rootPos` and the searches start
/// over after a reload, which only costs a search).
static HOSES: Mutex<Option<HashMap<(String, Pos), (Drainer, Filler)>>> = Mutex::new(None);

struct World<'a, 'g> {
    game: &'a mut Game<'g>,
}

impl HoseWorld for World<'_, '_> {
    fn loaded(&self, pos: Pos) -> bool {
        let w = &self.game.world;
        w.loaded(pos[0], pos[2]) && pos[1] >= w.shape.min_y && pos[1] < w.shape.max_y()
    }
    fn cell(&self, pos: Pos) -> Cell {
        let game = &*self.game;
        let id = game.world.block(pos[0], pos[1], pos[2]);
        let Some(s) = game.pack.assets.registry.state(id) else {
            return Cell::default();
        };
        let prop = |k: &str| s.properties.get(k).map(String::as_str);
        let waterlogged = prop("waterlogged").map(|v| v == "true");
        let liquid = matches!(s.name.as_str(), "minecraft:water" | "minecraft:lava");
        let level = if liquid { prop("level").and_then(|l| l.parse().ok()).or(Some(0)) } else { None };
        let fluid = if liquid {
            Some(s.name.clone())
        } else if waterlogged == Some(true) {
            Some("minecraft:water".to_owned())
        } else {
            None
        };
        let no_collision = game.block_collision_shape(pos).boxes().is_empty();
        // FluidFillingBehaviour.canBeReplacedByFluid.
        let kept = s.name.ends_with("_door")
            || s.name.ends_with("_sign")
            || matches!(
                s.name.as_str(),
                "minecraft:ladder" | "minecraft:sugar_cane" | "minecraft:bubble_column" | "minecraft:nether_portal" | "minecraft:end_portal" | "minecraft:end_gateway" | "minecraft:structure_void"
            );
        Cell {
            waterlogged,
            liquid_level: level,
            flowing: level.is_some_and(|l| l != 0),
            fluid,
            no_collision,
            replaceable_by_fluid: !kept && no_collision,
        }
    }
    fn set_air(&mut self, pos: Pos) {
        if let Some(air) = self.game.pack.assets.registry.default_state("minecraft:air") {
            let dimension = self.game.dim.id().to_owned();
            self.game.native_set_block(&dimension, pos, air, 2 | 16);
        }
    }
    fn set_waterlogged(&mut self, pos: Pos, on: bool) {
        self.game.native_set_block_property(pos, "waterlogged", if on { "true" } else { "false" });
    }
    fn place_source(&mut self, pos: Pos, fluid: &str) {
        let dimension = self.game.dim.id().to_owned();
        // replaceBlock: what stood there drops.
        if !self.game.world.is_air(self.game.world.block(pos[0], pos[1], pos[2])) && self.cell(pos).fluid.is_none() {
            let [x, y, z] = pos;
            self.game.native_run_command(&format!("execute in {dimension} run setblock {x} {y} {z} air destroy"));
        }
        if let Some(id) = self.game.pack.assets.registry.default_state(fluid) {
            self.game.native_set_block(&dimension, pos, id, 2 | 16);
        }
    }
    fn effect(&mut self, pos: Pos, fluid: &str, fill_sound: bool) {
        let sound = match (fluid == "minecraft:lava", fill_sound) {
            (true, true) => "minecraft:item.bucket.fill_lava",
            (true, false) => "minecraft:item.bucket.empty_lava",
            (false, true) => "minecraft:item.bucket.fill",
            (false, false) => "minecraft:item.bucket.empty",
        };
        let at = pos.map(|v| f64::from(v) + 0.5);
        self.game.with_level(|_, level, _| {
            level.native_play_sound(sound, at, super::create_sounds::BLOCKS, 0.3, 1.0);
        });
    }
    fn ultra_warm(&self) -> bool {
        self.game.dim.id() == "minecraft:the_nether"
    }
}

fn int(be: &ModBlockEntity, key: &str) -> i32 {
    match be.nbt.get(&JavaString::from(key)) {
        Some(Tag::Int(v)) => *v,
        _ => 0,
    }
}

fn offset_of(be: &ModBlockEntity) -> f32 {
    match be.nbt.get(&JavaString::from("Offset")) {
        Some(Tag::Float(v)) => f32::from_bits(*v),
        _ => 0.0,
    }
}

fn tank_of(be: &ModBlockEntity) -> FluidTank {
    let mut tank = FluidTank::new(rules::TANK);
    if let Some(Tag::String(t)) = be.nbt.get(&JavaString::from("Tank"))
        && let Some((name, amount)) = t.to_string_lossy().rsplit_once(' ').map(|(n, a)| (n.to_owned(), a.parse().unwrap_or(0)))
        && amount > 0
    {
        tank.fluid = FluidStack::new(&name, amount);
    }
    tank
}

fn put_tank(be: &mut ModBlockEntity, tank: &FluidTank) {
    let text = if tank.fluid.is_empty() { String::new() } else { format!("{} {}", tank.fluid.fluid, tank.fluid.amount) };
    be.nbt.insert(JavaString::from("Tank"), Tag::String(text.as_str().into()));
}

fn facing_at(game: &Game<'_>, pos: Pos) -> Option<Dir> {
    let s = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2]))?;
    if s.name != BLOCK {
        return None;
    }
    s.properties.get("facing").and_then(|f| Dir::parse(f))
}

/// `hasPipeTowards`: its fluid handler is on one side, where a pipe goes.
pub(super) fn has_pipe_side(game: &Game<'_>, pos: Pos, side: Dir) -> bool {
    facing_at(game, pos).is_some_and(|f| f.clockwise(Axis::Y).opposite() == side)
}

/// `hasPipeTowards`, from the level a shape update has.
pub(super) fn level_pipe_side(facing: Option<&str>, side: Dir) -> bool {
    facing.and_then(Dir::parse).is_some_and(|f| f.clockwise(Axis::Y).opposite() == side)
}

fn root(pos: Pos, offset: f32) -> Pos {
    [pos[0], pos[1] - offset.ceil() as i32, pos[2]]
}

/// `HosePulleyBlockEntity.tick`, `onSpeedChanged` and `lazyTick`.
pub(super) fn tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let speed = super::create_glue::with_kinetics(game, |k, level| k.speed_of(level, pos));
    let last = match entity.nbt.get(&JavaString::from("LastSpeed")) {
        Some(Tag::Float(v)) => f32::from_bits(*v),
        _ => 0.0,
    };
    let mut offset = offset_of(entity);
    let mut moving = int(entity, "Moving") != 0;
    let dimension = game.dim.id().to_owned();
    let key = (dimension, pos);
    let can_go = |game: &Game<'_>, below: i32| {
        let id = game.world.block(pos[0], pos[1] - below, pos[2]);
        game.res.facts().get(id).replaceable
    };
    let mut reset = false;
    let movement = speed / 512.0;
    if speed != last {
        moving = true;
        if speed == 0.0 {
            offset = offset.round();
            moving = false;
        } else {
            let next = offset + movement;
            if next < 0.0 || !can_go(game, next.ceil() as i32) {
                moving = false;
            }
            reset = moving;
        }
    }
    let (next, still_going) = rules::offset_tick(offset, speed, |d| can_go(game, d));
    offset = next;
    moving &= still_going;
    if !moving && movement > 0.0 && game.world.game_time % 10 == 0 && can_go(game, (offset + movement).ceil() as i32) {
        moving = true;
        reset = true;
    }
    {
        let mut guard = HOSES.lock().unwrap_or_else(|e| e.into_inner());
        let (mut drainer, mut filler) = guard.get_or_insert_with(HashMap::new).remove(&key).unwrap_or_default();
        drop(guard);
        if reset {
            drainer.reset();
            filler.reset();
        }
        let world = World { game };
        drainer.tick(&world, pos);
        filler.tick(&world);
        HOSES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(key, (drainer, filler));
    }
    entity.nbt.insert(JavaString::from("Offset"), Tag::Float(offset.to_bits()));
    entity.nbt.insert(JavaString::from("LastSpeed"), Tag::Float(speed.to_bits()));
    entity.nbt.insert(JavaString::from("Moving"), Tag::Int(i32::from(moving)));
    let shown = (offset * 256.0) as u32;
    super::create_machines::note_view(
        game,
        pos,
        vec![(String::new(), shown & 255), (String::new(), shown >> 8 & 255), (String::new(), shown >> 16 & 255)],
    );
}

/// The fluid handler of the hose pulley at `pos`.
pub(super) fn with_handler<R>(game: &mut Game<'_>, pos: Pos, op: impl FnOnce(&mut dyn FluidHandler) -> R) -> Option<R> {
    let (mut tank, offset, moving) = match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(BlockEntity::Mod(be)) => (tank_of(be), offset_of(be), int(be, "Moving") != 0),
        _ => return None,
    };
    let key = (game.dim.id().to_owned(), pos);
    let (mut drainer, mut filler) =
        HOSES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).remove(&key).unwrap_or_default();
    let r = {
        let mut world = World { game };
        let mut handler = Handler {
            tank: &mut tank,
            filler: &mut filler,
            drainer: &mut drainer,
            world: &mut world,
            root: root(pos, offset),
            own: pos,
            still: !moving,
        };
        op(&mut handler)
    };
    HOSES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(key, (drainer, filler));
    game.native_with_mod_block_entity(pos, |be| put_tank(be, &tank));
    Some(r)
}

/// `getFluidInTank`: its tank, as far as a shared look can tell.
pub(super) fn tanks(game: &Game<'_>, pos: Pos) -> Vec<FluidStack> {
    match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(BlockEntity::Mod(be)) => vec![tank_of(be).fluid],
        _ => Vec::new(),
    }
}

/// How far down a client was told its hose is (`getInterpolatedOffset`'s floor).
pub(super) fn client_offset(pos: Pos) -> f32 {
    let slots = super::create_machines::client_slots(pos).unwrap_or_default();
    let byte = |i: usize| slots.get(i).map_or(0, |s| s.1);
    ((byte(0) | byte(1) << 8 | byte(2) << 16) as f32 / 256.0).max(3.0 / 16.0)
}
