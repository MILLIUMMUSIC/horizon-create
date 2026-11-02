// Horizon's side of actors (actors.rs), injected as
// `crate::game::create_actor_glue`: the drills, harvesters and ploughs on
// moving structures (`tickActors` for each piston, bearing, pulley and
// gantry, before it moves; a structure stalls while one breaks a block),
// what they do to the world, and the standing drill's breaking
// (`DrillBlockEntity`) and the saw's (`SawBlockEntity`) with the trees it
// fells (`TreeCutter`).

use super::Game;
use super::create_actors::{
    ActorContext, ActorKind, BreakEvent, BreakWorld, Breaker, Cut, LAZY_TICK_RATE, PloughFacts, PlantFacts,
    Psi, cancel_stall, containing, deployer_strikes, drill_can_break, extract_one, harvest, insert_item, psi_tick, is_breakable, moving_break_speed,
    plough_can_break, saw_visit_end, should_trigger, tick_breaker, tick_waiting, tilled, visit_breaker,
};
use super::create_saw::{SawFacts, TreeBlock, TreeWorld, find_tree, is_sawable};
use super::create_kinetics::{Dir, Pos};
use horizon_client::items::ItemStack;
use horizon_core::random::RandomSource;
use horizon_world::nbt::{Compound, Tag};
use horizon_world::registry::BlockStateId;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

/// A moving structure, as its controller is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Mover {
    Piston(Pos),
    Bearing(Pos),
    Pulley(Pos),
    Gantry(u32),
}

/// One actor of a structure: its block's place in the structure, the way
/// it faces there, and its `MovementContext`.
struct Slot {
    local: Pos,
    kind: ActorKind,
    facing: Dir,
    ctx: ActorContext,
    /// `BreakerId`.
    id: i32,
    /// The block's index in the structure (its data, a deployer's state).
    index: usize,
    /// Where a deployer's hand works and the way it faces there, or the
    /// stationary interface a portable one found (`WorkingPos`).
    target: Option<(Pos, Dir)>,
}

#[derive(Default)]
struct Moving {
    slots: Vec<Slot>,
    stalled: bool,
}

/// A standing drill or saw (`BlockBreakingKineticBlockEntity`).
struct Drill {
    breaker: Breaker,
    rpm: f32,
    /// The block in front when it last broke one: a change wakes it
    /// (`neighborChanged`).
    front: BlockStateId,
    id: i32,
}

#[derive(Default)]
struct State {
    moving: HashMap<(String, Mover), Moving>,
    drills: HashMap<(String, Pos), Drill>,
    /// Stationary portable storage interfaces at work.
    psis: HashMap<(String, Pos), Psi<Mover>>,
    next_id: i32,
    ticks: u64,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn state() -> MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// `NEXT_BREAKER_ID`: breakers' ids are negative, apart from players'.
fn next_id(s: &mut State) -> i32 {
    s.next_id += 1;
    -1000 - s.next_id
}

fn prop(game: &Game<'_>, id: BlockStateId, key: &str) -> Option<String> {
    game.pack.assets.registry.state(id)?.properties.get(key).cloned()
}

fn set(game: &mut Game<'_>, pos: Pos, id: BlockStateId, flags: i32) {
    let dimension = game.dim.id().to_owned();
    game.native_set_block(&dimension, pos, id, flags);
}

fn block_at(game: &Game<'_>, pos: Pos) -> BlockStateId {
    game.world.block(pos[0], pos[1], pos[2])
}

/// The world as breaking actors see it.
struct World<'a, 'g> {
    game: &'a Game<'g>,
}

impl World<'_, '_> {
    fn class(&self, pos: Pos, name: &str) -> bool {
        let f = self.game.res.facts().get(block_at(self.game, pos));
        f.classes.iter().any(|c| &**c == name)
    }
    fn tag(&self, pos: Pos, tag: &str) -> bool {
        let f = self.game.res.facts().get(block_at(self.game, pos));
        self.game.block_in_tag(tag, &f.name)
    }
    fn collision_empty(&self, pos: Pos) -> bool {
        self.game.block_collision_shape(pos).boxes().is_empty()
    }
    fn air(&self, pos: Pos) -> bool {
        self.class(pos, "AirBlock")
    }
    fn name_is(&self, pos: Pos, name: &str) -> bool {
        self.game.res.facts().get(block_at(self.game, pos)).name.as_ref() == name
    }
    /// `SawBlockEntity.isSawable`.
    fn sawable(&self, pos: Pos) -> bool {
        let t = self.tree_block(pos);
        is_sawable(&SawFacts {
            sapling: self.tag(pos, "minecraft:saplings"),
            log: t.log,
            leaves: self.tag(pos, "minecraft:leaves"),
            root: t.root,
            plant: [
                "BambooStalkBlock",
                "CactusBlock",
                "SugarCaneBlock",
                "KelpPlantBlock",
                "KelpBlock",
                "ChorusPlantBlock",
            ]
            .iter()
            .any(|c| self.class(pos, c))
                || self.tag(pos, "create:solid_crops"),
        })
    }
    /// `BlockBreakingKineticBlockEntity.isBreakable`.
    fn breakable(&self, pos: Pos) -> bool {
        is_breakable(
            self.class(pos, "LiquidBlock"),
            self.air(pos),
            self.hardness(pos),
            self.tag(pos, "create:non_breakable"),
        )
    }
}

impl BreakWorld for World<'_, '_> {
    fn hardness(&self, pos: Pos) -> f32 {
        self.game.res.facts().get(block_at(self.game, pos)).hardness
    }
    fn can_break(&self, kind: ActorKind, pos: Pos) -> bool {
        match kind {
            ActorKind::Drill => {
                drill_can_break(self.breakable(pos), self.collision_empty(pos), self.tag(pos, "create:tracks"))
            }
            ActorKind::Plough => plough_can_break(&PloughFacts {
                air: self.air(pos),
                farmland_below: self.class([pos[0], pos[1] - 1, pos[2]], "FarmlandBlock"),
                liquid: self.class(pos, "LiquidBlock"),
                bubble_column: self.class(pos, "BubbleColumnBlock"),
                blacklisted: self.tag(pos, "create:plough_blacklist"),
                whitelisted: self.tag(pos, "create:plough_whitelist"),
                collision_empty: self.collision_empty(pos),
            }),
            ActorKind::Saw => self.breakable(pos) && self.sawable(pos),
            ActorKind::Harvester | ActorKind::Deployer | ActorKind::Psi => false,
        }
    }
    fn falling(&self, pos: Pos) -> bool {
        self.class(pos, "FallingBlock")
    }
}

impl TreeWorld for World<'_, '_> {
    fn tree_block(&self, pos: Pos) -> TreeBlock {
        let id = block_at(self.game, pos);
        let distance = prop(self.game, id, "distance").and_then(|d| d.parse::<i32>().ok());
        let non_decaying = if self.name_is(pos, "minecraft:red_mushroom_block") {
            2
        } else if self.name_is(pos, "minecraft:brown_mushroom_block")
            || self.tag(pos, "minecraft:wart_blocks")
            || self.name_is(pos, "minecraft:weeping_vines")
            || self.name_is(pos, "minecraft:weeping_vines_plant")
        {
            3
        } else {
            -1
        };
        TreeBlock {
            log: self.tag(pos, "minecraft:logs")
                || self.tag(pos, "tconstruct:slimy_logs")
                || self.name_is(pos, "minecraft:mushroom_stem"),
            root: self.tag(pos, "create:roots"),
            leaf: distance.filter(|_| !self.class(pos, "ScaffoldingBlock")),
            vertical_plant: ["BambooStalkBlock", "CactusBlock", "SugarCaneBlock", "KelpPlantBlock", "KelpBlock"]
                .iter()
                .any(|c| self.class(pos, c)),
            chorus: self.class(pos, "ChorusPlantBlock") || self.class(pos, "ChorusFlowerBlock"),
            attachment: self.tag(pos, "create:tree_attachments"),
            non_decaying,
        }
    }
    fn height(&self) -> i32 {
        self.game.world.shape.height
    }
}

/// `TreeCutter.findTree(...).destroyBlocks`: what fell with the block cut at
/// `pos` (`broken` as it was), each block's drops handed to `drop`.
fn fell(game: &mut Game<'_>, pos: Pos, broken: TreeBlock, mut drop: impl FnMut(&mut Game<'_>, Pos, ItemStack)) {
    let tree = find_tree(&World { game }, pos, broken);
    for (at, _effects) in tree.in_order() {
        if !game.world.loaded(at[0], at[2]) {
            continue;
        }
        for stack in destroy(game, at, None) {
            drop(game, at, stack);
        }
    }
}

/// `dropItemFromCutTree`: from the block's centre, thrown the way the
/// cut went, the further the faster.
fn throw_from(game: &mut Game<'_>, at: Pos, from: [f64; 3], way: [f64; 3], stack: ItemStack) {
    let c = at.map(|v| f64::from(v) + 0.5);
    let distance = (0..3).map(|k| (c[k] - from[k]).powi(2)).sum::<f64>().sqrt();
    let v = way.map(|w| w * distance / 20.0);
    game.spawn_item(stack, c, v, 10);
}

/// `BlockHelper.destroyBlockAs`: the block's drops (as with `held`), its
/// break effects, and air (or the fluid it held) left behind.
fn destroy(game: &mut Game<'_>, pos: Pos, held: Option<&ItemStack>) -> Vec<ItemStack> {
    let state = block_at(game, pos);
    let items = match &game.world_tables {
        Some(t) => &t.items,
        None => game.res.items(),
    };
    let drops = items.block_drops(game.pack.assets.registry, state, held, &mut game.world.random);
    let at = horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]);
    game.with_level(|b, level, ticks| b.destroy_block(level, ticks, at, false));
    let _ = game.process_level_events();
    drops
}

/// A moving structure's storage: its blocks' places and states, and their
/// block entities' data, by the blocks' index (`MountedStorageManager`).
pub(super) struct Store<'s> {
    pub blocks: &'s dyn Fn() -> Vec<(Pos, BlockStateId)>,
    pub entities: &'s mut [Option<Compound>],
}

impl Store<'_> {
    fn states(&self) -> Vec<BlockStateId> {
        (self.blocks)().into_iter().map(|b| b.1).collect()
    }
}

/// `getAllItems()`: the slots of the structure's chests, barrels, shulker
/// boxes and item vaults (each block's own slots) in its blocks' order,
/// handed to `f` and kept unless simulating.
pub(super) fn storage_items<R>(
    game: &Game<'_>,
    states: &[BlockStateId],
    entities: &mut [Option<Compound>],
    simulate: bool,
    f: impl FnOnce(&mut Vec<Option<ItemStack>>) -> R,
) -> R {
    use horizon_client::containers::BlockEntity;
    let mut parts = Vec::new();
    let mut all: Vec<Option<ItemStack>> = Vec::new();
    for (i, st) in states.iter().enumerate() {
        let name = game.res.facts().get(*st).name.to_string();
        let vault = name == super::create_arms::VAULT;
        if !vault
            && !game.block_in_tag("create:chest_mounted_storage", &name)
            && !game.block_in_tag("create:simple_mounted_storage", &name)
        {
            continue;
        }
        let Some(Some(c)) = entities.get(i) else {
            continue;
        };
        let items = match horizon_client::save::decode_block_entity(&Tag::Compound(c.clone())) {
            Some((key, BlockEntity::Chest(items))) => {
                parts.push((i, key, name, items.len(), None));
                items
            }
            Some((key, BlockEntity::Mod(m))) if vault => {
                let items = m.items.clone();
                parts.push((i, key, name, items.len(), Some(m)));
                items
            }
            _ => continue,
        };
        all.extend(items);
    }
    let before = all.clone();
    let r = f(&mut all);
    if !simulate && all != before {
        let mut rest = all.into_iter();
        for (i, key, name, len, vault) in parts {
            let items: Vec<Option<ItemStack>> = rest.by_ref().take(len).collect();
            let entity = match vault {
                Some(mut m) => {
                    m.items = items;
                    BlockEntity::Mod(m)
                }
                None => BlockEntity::Chest(items),
            };
            if let Tag::Compound(c) = horizon_client::save::encode_block_entity(key, &entity, &name) {
                entities[i] = Some(c);
            }
        }
    }
    r
}

/// `MovementBehaviour.collectOrDropItem`: into the structure's storage,
/// what doesn't fit dropped where the actor is.
fn collect(game: &mut Game<'_>, ctx: &ActorContext, store: &mut Store<'_>, stack: ItemStack) {
    let states = store.states();
    let left = storage_items(game, &states, store.entities, false, |items| {
        insert_item(items, stack, |a, b| a.same_item_same_components(b))
    });
    if let Some(left) = left {
        drop_from(game, ctx, left);
    }
}

/// Dropped where the actor is.
fn drop_from(game: &mut Game<'_>, ctx: &ActorContext, stack: ItemStack) {
    let Some(at) = ctx.position else {
        return;
    };
    let scale = f64::from(game.world.random.next_float()) * 0.3;
    let v = [ctx.motion[0] * scale, (ctx.motion[1] + 0.5) * scale, ctx.motion[2] * scale];
    game.spawn_item(stack, at, v, 10);
}

fn show(game: &mut Game<'_>, id: i32, event: BreakEvent) {
    match event {
        BreakEvent::Progress { pos, stage } => game.destroy_block_progress(id, pos, stage),
        BreakEvent::Cleared { pos } | BreakEvent::Broken { pos, .. } => game.destroy_block_progress(id, pos, -1),
    }
}

/// `HarvesterMovementBehaviour.visitNewPosition` on the server.
fn harvest_at(game: &mut Game<'_>, ctx: &ActorContext, store: &mut Store<'_>, pos: Pos) {
    let state = block_at(game, pos);
    let world = World { game };
    let f = game.res.facts().get(state);
    let class = |c: &str| f.classes.iter().any(|k| &**k == c);
    let name = f.name.clone();
    let max_of = |key: &str| {
        game.pack
            .assets
            .registry
            .property_values(&name, key)
            .iter()
            .filter_map(|v| v.parse::<i32>().ok())
            .max()
    };
    let age = prop(game, state, "age").and_then(|a| a.parse::<i32>().ok()).zip(max_of("age"));
    // CropBlock.getMaxAge: the torchflower's (2) is past its last age.
    let crop = class("CropBlock").then(|| {
        let max = if class("TorchflowerCropBlock") { 2 } else { age.map_or(7, |a| a.1) };
        (age.map_or(0, |a| a.0), max)
    });
    let facts = PlantFacts {
        air: world.air(pos),
        non_harvestable: world.tag(pos, "create:non_harvestable"),
        crop,
        collision_empty: world.collision_empty(pos),
        cocoa: class("CocoaBlock"),
        age,
        sweet_berry_bush: class("SweetBerryBushBlock"),
        sugar_cane: class("SugarCaneBlock"),
        sugar_cane_variant: world.tag(pos, "create:sugar_cane_variants"),
        leaves: class("LeavesBlock"),
        growing_plant: class("GrowingPlantBlock"),
        bush: class("VegetationBlock") || class("BushBlock"),
        fluid: f.fluid_level > 0,
    };
    let seed = f.item.clone();
    let Some(h) = harvest(&facts) else {
        return;
    };
    let shears = h.shears.then(|| ItemStack::new("minecraft:shears", 1));
    let drops = destroy(game, pos, shears.as_ref());
    let mut seed_kept = h.seed_kept;
    for mut stack in drops {
        if !seed_kept && stack.item == seed {
            stack.count -= 1;
            seed_kept = true;
        }
        if stack.count > 0 {
            collect(game, ctx, store, stack);
        }
    }
    let left = match h.cut {
        Cut::Age(a) => game.with_props(state, &[("age", &a.to_string())]),
        Cut::Gone if facts.fluid => game.pack.assets.registry.default_state("minecraft:water"),
        Cut::Gone => None,
    };
    if let Some(left) = left {
        set(game, pos, left, super::native_hooks::flags::UPDATE_ALL);
    }
}

/// The hoe of `PloughMovementBehaviour.visitNewPosition`: the block under
/// an empty spot is tilled.
fn till(game: &mut Game<'_>, ctx: &ActorContext, pos: Pos) {
    let below = [pos[0], pos[1] - 1, pos[2]];
    if !game.world.loaded(below[0], below[2]) {
        return;
    }
    // The ray down from the spot's centre stops on whatever is there.
    let here = game.res.facts().get(block_at(game, pos));
    if !here.outline.is_empty() {
        return;
    }
    let air_above = here.classes.iter().any(|c| &**c == "AirBlock");
    let name = game.res.facts().get(block_at(game, below)).name.clone();
    let Some((new, item)) = tilled(&name, air_above) else {
        return;
    };
    let Some(state) = game.pack.assets.registry.default_state(new) else {
        return;
    };
    set(game, below, state, super::native_hooks::flags::UPDATE_ALL);
    if let Some(item) = item {
        let _ = ctx;
        let c = [f64::from(below[0]) + 0.5, f64::from(below[1]) + 1.25, f64::from(below[2]) + 0.5];
        game.spawn_item(ItemStack::new(item, 1), c, [0.0, 0.1, 0.0], 10);
    }
}

/// `BlockBreakingMovementBehaviour.destroyBlock` (and the plough's snow,
/// dug as with an iron shovel).
fn broken(game: &mut Game<'_>, ctx: &ActorContext, store: &mut Store<'_>, kind: ActorKind, pos: Pos) {
    if kind == ActorKind::Saw {
        // SawMovementBehaviour.onBlockBroken: leaves fell nothing.
        let w = World { game };
        let leaves = w.tag(pos, "minecraft:leaves");
        let cut = w.tree_block(pos);
        for stack in destroy(game, pos, None) {
            collect(game, ctx, store, stack);
        }
        if !leaves {
            // dropItemFromCutTree: into the storage first.
            let from = ctx.position.unwrap_or(pos.map(|v| f64::from(v) + 0.5));
            let way = ctx.relative_motion;
            fell(game, pos, cut, |game, at, stack| {
                let states = store.states();
                let left = storage_items(game, &states, store.entities, false, |items| {
                    insert_item(items, stack, |a, b| a.same_item_same_components(b))
                });
                if let Some(left) = left {
                    throw_from(game, at, from, way, left);
                }
            });
        }
        return;
    }
    let snow = kind == ActorKind::Plough && game.res.facts().get(block_at(game, pos)).name.as_ref() == "minecraft:snow";
    let shovel = snow.then(|| ItemStack::new("minecraft:iron_shovel", 1));
    for stack in destroy(game, pos, shovel.as_ref()) {
        collect(game, ctx, store, stack);
    }
}

/// An actor's visit to a new block (`visitNewPosition`).
/// `Direction.getNearest` of a vector, and how far the vector is from it.
fn nearest(v: [f64; 3]) -> (Dir, f64) {
    let mut best = (Dir::Up, f64::MAX);
    for d in Dir::ALL {
        let n = d.normal();
        let dist = (0..3).map(|k| (v[k] - f64::from(n[k])).powi(2)).sum::<f64>().sqrt();
        if dist < best.1 {
            best = (d, dist);
        }
    }
    best
}

/// A deployer's state, as its block carries it (`HzDeployer`).
fn deployer_of(store: &Store<'_>, index: usize) -> super::create_deployer::Deployer<ItemStack> {
    let text = store
        .entities
        .get(index)
        .and_then(|e| e.as_ref())
        .and_then(|c| match c.get(&"HzDeployer".into()) {
            Some(Tag::String(t)) => Some(t.to_string_lossy()),
            _ => None,
        })
        .unwrap_or_default();
    super::create_arms::moving_deployer(&text)
}

fn set_deployer(store: &mut Store<'_>, index: usize, d: &super::create_deployer::Deployer<ItemStack>) {
    if let Some(e) = store.entities.get_mut(index) {
        let text = super::create_arms::moving_deployer_text(d);
        e.get_or_insert_with(Compound::new)
            .insert("HzDeployer".into(), Tag::String(text.as_str().into()));
    }
}

/// `DeployerMovementBehaviour.activate` and `tryDisposeOfExcess`: true
/// while it breaks a block.
fn deploy(game: &mut Game<'_>, slot: &mut Slot, store: &mut Store<'_>, at: Pos, facing: Dir) -> bool {
    let mut d = deployer_of(store, slot.index);
    let breaking = super::create_arms::deploy_moving(game, at, facing, &mut d);
    for stack in std::mem::take(&mut d.inventory) {
        collect(game, &slot.ctx, store, stack);
    }
    set_deployer(store, slot.index, &d);
    breaking
}

/// `PortableStorageInterfaceMovement.getCurrentFacingIfValid`.
fn psi_facing(slot: &Slot, turn: &dyn Fn([f64; 3]) -> [f64; 3]) -> Option<Dir> {
    let (d, dist) = nearest(turn(slot.facing.normal().map(f64::from)));
    (dist <= 0.5).then_some(d)
}

/// `getStationaryInterfaceAt`: an interface facing back the other way.
fn stationary_at(game: &Game<'_>, pos: Pos, facing: Dir) -> bool {
    let id = block_at(game, pos);
    game.res.facts().get(id).name.as_ref() == super::create_actors::PSI
        && prop(game, id, "facing").and_then(|f| Dir::parse(&f)) == Some(facing.opposite())
}

/// An actor's visit to a new block (`visitNewPosition`).
fn visit(
    game: &mut Game<'_>,
    mover: Mover,
    slot: &mut Slot,
    store: &mut Store<'_>,
    pos: Pos,
    turn: &dyn Fn([f64; 3]) -> [f64; 3],
) {
    if !game.world.loaded(pos[0], pos[2]) {
        return;
    }
    match slot.kind {
        ActorKind::Drill => visit_breaker(&mut slot.ctx, slot.kind, &World { game }, pos),
        ActorKind::Plough => {
            visit_breaker(&mut slot.ctx, slot.kind, &World { game }, pos);
            till(game, &slot.ctx, pos);
        }
        ActorKind::Harvester => harvest_at(game, &slot.ctx, store, pos),
        ActorKind::Saw => {
            visit_breaker(&mut slot.ctx, slot.kind, &World { game }, pos);
            saw_visit_end(&mut slot.ctx, turn(slot.facing.normal().map(f64::from)));
        }
        ActorKind::Deployer => {
            // tryGrabbingItem: one item of the storage into an empty hand.
            let mut d = deployer_of(store, slot.index);
            if d.held.is_none() {
                let states = store.states();
                d.held = storage_items(game, &states, store.entities, false, |items| extract_one(items, |_| true));
                set_deployer(store, slot.index, &d);
            }
            let (facing, _) = nearest(turn(slot.facing.normal().map(f64::from)));
            slot.target = Some((pos, facing));
            slot.ctx.stall = deploy(game, slot, store, pos, facing);
        }
        ActorKind::Psi => {
            // findInterface: one or two blocks ahead.
            slot.target = None;
            let Some(facing) = psi_facing(slot, turn) else {
                return;
            };
            let n = facing.normal();
            let found = (0..2)
                .map(|i| [pos[0] + n[0] * i, pos[1] + n[1] * i, pos[2] + n[2] * i])
                .find(|p| stationary_at(game, *p, facing));
            let Some(at) = found else {
                return;
            };
            let dim = game.dim.id().to_owned();
            let mut guard = state();
            let s = guard.get_or_insert_with(State::default);
            let powered = s.psis.get(&(dim.clone(), at)).is_some_and(|p| p.powered);
            drop(guard);
            // getStationaryInterfaceAt: a powered one isn't found.
            if powered || super::create_glue::signal(game, at) {
                return;
            }
            let mut guard = state();
            let s = guard.get_or_insert_with(State::default);
            let psi = s.psis.entry((dim, at)).or_default();
            let position = slot.ctx.position.unwrap_or_default();
            let diff: f64 = (0..3).map(|k| (f64::from(at[k]) + 0.5 - position[k]) * f64::from(n[k])).sum();
            psi.start_transferring_to(mover, (diff.abs() + 1.85 - 1.0) as f32);
            slot.target = Some((at, facing));
        }
    }
}

/// `PortableStorageInterfaceMovement.tick` on the server.
fn psi_actor_tick(game: &Game<'_>, mover: Mover, slot: &mut Slot, turn: &dyn Fn([f64; 3]) -> [f64; 3]) {
    let Some((at, _)) = slot.target else {
        psi_tick(&mut slot.ctx, None, None);
        return;
    };
    let target = at.map(|v| f64::from(v) + 0.5);
    let valid = psi_facing(slot, turn).filter(|f| stationary_at(game, at, *f));
    let dim = game.dim.id().to_owned();
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    let timer = valid.and_then(|_| {
        let psi = s.psis.entry((dim, at)).or_default();
        if psi.connected.is_none() {
            let distance = psi.distance;
            psi.start_transferring_to(mover, distance);
        }
        let timer = psi.transfer_timer;
        psi.keep_alive = 2;
        Some(timer)
    });
    if timer.is_none() {
        slot.target = None;
    }
    psi_tick(&mut slot.ctx, Some(target), timer);
}

fn slots_of(game: &Game<'_>, s: &mut State, blocks: &[(Pos, BlockStateId)]) -> Vec<Slot> {
    let mut slots = Vec::new();
    for (index, &(local, state)) in blocks.iter().enumerate() {
        let Some(name) = game.pack.assets.registry.state(state).map(|b| b.name.clone()) else {
            continue;
        };
        let Some(kind) = ActorKind::of(&name) else {
            continue;
        };
        let Some(facing) = prop(game, state, "facing").and_then(|f| Dir::parse(&f)) else {
            continue;
        };
        slots.push(Slot {
            local,
            kind,
            facing,
            ctx: ActorContext::default(),
            id: next_id(s),
            index,
            target: None,
        });
    }
    slots
}

/// `AbstractContraptionEntity.tickActors` for one structure, before it
/// moves this tick: `blocks` are its blocks' places and states (read when
/// it is first seen), `place(local, v)` the world position of `v` from the
/// centre of the block at `local`, `unrotate` a world vector in the
/// structure's frame and `turn` back. `centre`: a bearing's facing and the degrees it
/// turned last tick, for actors on its axis. True when it is stalled.
pub(super) fn tick_actors(
    game: &mut Game<'_>,
    mover: Mover,
    blocks: &dyn Fn() -> Vec<(Pos, BlockStateId)>,
    place: &dyn Fn(Pos, [f64; 3]) -> [f64; 3],
    unrotate: &dyn Fn([f64; 3]) -> [f64; 3],
    turn: &dyn Fn([f64; 3]) -> [f64; 3],
    centre: Option<(Dir, f32)>,
    entities: &mut [Option<Compound>],
) -> bool {
    let mut store = Store { blocks, entities };
    let key = (game.dim.id().to_owned(), mover);
    let mut moving = {
        let mut guard = state();
        let s = guard.get_or_insert_with(State::default);
        match s.moving.remove(&key) {
            Some(m) => m,
            None => Moving {
                slots: slots_of(game, s, &blocks()),
                stalled: false,
            },
        }
    };
    let mut stalled = false;
    for slot in &mut moving.slots {
        let offset = slot.kind.active_area_offset(slot.facing);
        let actor_pos = place(slot.local, offset);
        let grid = containing(actor_pos);
        let mut visited = !slot.ctx.stall && should_trigger(&mut slot.ctx, actor_pos, unrotate);
        if !visited
            && !slot.ctx.stall
            && slot.ctx.position.is_some()
            && let Some((facing, delta)) = centre
        {
            visited = super::create_actors::bearing_centre_trigger(&mut slot.ctx, slot.local, offset, facing, delta);
        }
        slot.ctx.position = Some(actor_pos);
        if !slot.kind.active(slot.facing, slot.ctx.relative_motion) {
            continue;
        }
        if visited && !slot.ctx.stall {
            visit(game, mover, slot, &mut store, grid, turn);
            slot.ctx.first_movement = false;
        }
        // DeployerMovementBehaviour.tick: it strikes the block again.
        if slot.kind == ActorKind::Deployer
            && slot.ctx.stall
            && deployer_strikes(&mut slot.ctx)
            && let Some((at, facing)) = slot.target
        {
            slot.ctx.stall = deploy(game, slot, &mut store, at, facing);
        }
        if slot.kind == ActorKind::Psi {
            psi_actor_tick(game, mover, slot, turn);
        }
        if slot.kind.breaks() {
            let speed = moving_break_speed(slot.ctx.motion);
            let event = tick_breaker(&mut slot.ctx, slot.kind, &World { game }, speed);
            if let Some(e) = event {
                show(game, slot.id, e);
                if let BreakEvent::Broken { pos, .. } = e {
                    broken(game, &slot.ctx, &mut store, slot.kind, pos);
                }
            }
            if let Some(again) = tick_waiting(&mut slot.ctx) {
                visit(game, mover, slot, &mut store, again, turn);
            }
        }
        stalled |= slot.ctx.stall;
    }
    moving.stalled = stalled;
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    s.moving.insert(key, moving);
    stalled
}

/// `native_world_lifecycle`: a new world starts with no actors.
pub(super) fn reset() {
    *state() = None;
}

/// Whether a structure stood still last tick for its actors.
pub(super) fn stalled(dimension: &str, mover: Mover) -> bool {
    state()
        .as_ref()
        .and_then(|s| s.moving.get(&(dimension.to_owned(), mover)))
        .is_some_and(|m| m.stalled)
}

/// `stopMoving`: the structure is put back; its actors' cracks go.
pub(super) fn stop_actors(game: &mut Game<'_>, mover: Mover) {
    let key = (game.dim.id().to_owned(), mover);
    let Some(mut moving) = state().as_mut().and_then(|s| s.moving.remove(&key)) else {
        return;
    };
    for slot in &mut moving.slots {
        if let Some(e) = cancel_stall(&mut slot.ctx) {
            show(game, slot.id, e);
        }
        if slot.kind == ActorKind::Deployer
            && let Some((at, facing)) = slot.target
        {
            super::create_arms::stop_deploying(game, at, facing);
        }
    }
    // Interfaces it was connected to let go (its entity is gone).
    if let Some(s) = state().as_mut() {
        for ((d, _), psi) in s.psis.iter_mut() {
            if *d == key.0 && psi.connected == Some(mover) {
                psi.stop_transferring();
            }
        }
    }
}

/// `PortableStorageInterfaceBlockEntity.tick` for the dimension's
/// interfaces at work (after the structures moved).
pub(super) fn psis_tick(game: &mut Game<'_>, dimension: &str) {
    let here: Vec<Pos> = state()
        .as_ref()
        .map(|s| s.psis.keys().filter(|(d, _)| d == dimension).map(|(_, p)| *p).collect())
        .unwrap_or_default();
    // neighbourChanged: a signal stops it.
    let powered: Vec<(Pos, bool)> = here.into_iter().map(|p| (p, super::create_glue::signal(game, p))).collect();
    let mut guard = state();
    let Some(s) = guard.as_mut() else {
        return;
    };
    for (p, on) in powered {
        if let Some(psi) = s.psis.get_mut(&(dimension.to_owned(), p)) {
            psi.powered = on;
        }
    }
    s.psis.retain(|(d, p), psi| {
        if d != dimension {
            return true;
        }
        if game.res.facts().get(block_at(game, *p)).name.as_ref() != super::create_actors::PSI {
            return false;
        }
        psi.tick();
        !psi.idle()
    });
}

/// The stationary interface at `pos` and the structure it can hand items
/// to now (`canTransfer`).
fn psi_mover(game: &Game<'_>, pos: Pos) -> Option<Mover> {
    let dim = game.dim.id().to_owned();
    let connected = state().as_ref()?.psis.get(&(dim.clone(), pos))?.connected?;
    let alive = super::create_contraptions::with_storage(game, connected, |_, _| ()).is_some();
    let mut guard = state();
    let psi = guard.as_mut()?.psis.get_mut(&(dim, pos))?;
    psi.can_transfer(alive).then_some(connected)
}

/// The slots an interface offers (`InterfaceItemHandler`): none unless it
/// is connected.
pub(super) fn psi_slots(game: &Game<'_>, pos: Pos) -> usize {
    let Some(mover) = psi_mover(game, pos) else {
        return 0;
    };
    super::create_contraptions::with_storage(game, mover, |states, entities| {
        storage_items(game, states, entities, true, |items| items.len())
    })
    .unwrap_or(0)
}

/// `f` on the connected structure's slots through the interface at `pos`;
/// a change keeps it connected (`onContentTransferred`).
pub(super) fn psi_items<R>(
    game: &mut Game<'_>,
    pos: Pos,
    simulate: bool,
    f: impl FnOnce(&mut Vec<Option<ItemStack>>) -> R,
) -> Option<R> {
    let mover = psi_mover(game, pos)?;
    let mut changed = false;
    let r = super::create_contraptions::with_storage(game, mover, |states, entities| {
        storage_items(game, states, entities, simulate, |items| {
            let before = items.clone();
            let r = f(items);
            changed = *items != before;
            r
        })
    })?;
    if changed && !simulate {
        let dim = game.dim.id().to_owned();
        if let Some(psi) = state().as_mut().and_then(|s| s.psis.get_mut(&(dim, pos))) {
            psi.transferred();
        }
    }
    Some(r)
}

/// `DrillBlockEntity.tick` and `SawBlockEntity.tick` for the dimension's
/// standing drills and saws (place, facing, speed, whether a saw), after
/// the kinetics.
pub(super) fn breakers_tick(game: &mut Game<'_>, dimension: &str, drills: &[(Pos, Dir, f32, bool)]) {
    let mut taken: HashMap<Pos, Drill> = {
        let mut guard = state();
        let s = guard.get_or_insert_with(State::default);
        s.ticks += 1;
        let keys: Vec<(String, Pos)> = s.drills.keys().filter(|(d, _)| d == dimension).cloned().collect();
        let mut taken: HashMap<Pos, Drill> =
            keys.into_iter().filter_map(|k| s.drills.remove(&k).map(|d| (k.1, d))).collect();
        for &(pos, ..) in drills {
            if !taken.contains_key(&pos) {
                let id = next_id(s);
                taken.insert(
                    pos,
                    Drill {
                        breaker: Breaker::default(),
                        rpm: 0.0,
                        front: BlockStateId(0),
                        id,
                    },
                );
            }
        }
        taken
    };
    let lazy = state().as_ref().is_some_and(|s| s.ticks % LAZY_TICK_RATE == 0);
    let mut kept = HashMap::new();
    for &(pos, facing, rpm, saw) in drills {
        let Some(mut d) = taken.remove(&pos) else {
            continue;
        };
        if !game.world.loaded(pos[0], pos[2]) {
            kept.insert(pos, d);
            continue;
        }
        // SawBlockEntity.shouldRun: a saw facing up or down cuts nothing.
        if saw && facing.axis() == super::create_kinetics::Axis::Y {
            kept.insert(pos, d);
            continue;
        }
        if d.rpm != rpm {
            d.rpm = rpm;
            d.breaker.speed_changed();
        }
        // A saw tries again at once (SawBlockEntity.tick).
        if saw && d.breaker.ticks_until_next < 0 {
            d.breaker.destroy_next_tick();
        }
        let n = facing.normal();
        let target = [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]];
        let front = block_at(game, target);
        if d.breaker.ticks_until_next == -1 && front != d.front {
            d.breaker.destroy_next_tick();
        }
        if lazy {
            d.breaker.lazy_tick();
        }
        let (hardness, breakable, cut, leaves) = {
            let w = World { game };
            let breakable = w.breakable(target) && (!saw || w.sawable(target));
            (w.hardness(target), breakable, w.tree_block(target), w.tag(target, "minecraft:leaves"))
        };
        if let Some(event) = d.breaker.tick(rpm, target, hardness, breakable) {
            show(game, d.id, event);
            if let BreakEvent::Broken { pos: at, .. } = event {
                // onBlockBroken: the drops at the block's centre, a little
                // apart, still.
                let drops = destroy(game, at, None);
                for stack in drops {
                    let r = &mut game.world.random;
                    let jitter = [0, 1, 2].map(|_| (f64::from(r.next_float()) - 0.5) * 0.25);
                    let c = [0, 1, 2].map(|k| f64::from(at[k]) + 0.5 + jitter[k]);
                    game.spawn_item(stack, c, [0.0; 3], 10);
                }
                // SawBlockEntity.onBlockBroken: the tree falls away from it.
                if saw && !leaves {
                    let from = at.map(|v| f64::from(v) + 0.5);
                    let way = n.map(f64::from);
                    fell(game, at, cut, |game, p, stack| throw_from(game, p, from, way, stack));
                }
                d.front = block_at(game, target);
            }
        }
        kept.insert(pos, d);
    }
    // Drills that are gone take their cracks with them (`invalidate`).
    for (_, d) in taken {
        if d.breaker.progress != 0
            && let Some(p) = d.breaker.pos
        {
            game.destroy_block_progress(d.id, p, -1);
        }
    }
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    for (p, d) in kept {
        s.drills.insert((dimension.to_owned(), p), d);
    }
}
