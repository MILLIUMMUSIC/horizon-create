// Create's actors on contraptions, injected as `crate::game::create_actors`:
// `AbstractContraptionEntity.tickActors` (when an actor visits a new block),
// `BlockBreakingMovementBehaviour` (the drill and the plough break what they
// move into, stalling the structure until it breaks),
// `HarvesterMovementBehaviour`, `PloughMovementBehaviour`, and the
// standing drill's `BlockBreakingKineticBlockEntity`; the deployer and the
// portable storage interface on the move (`DeployerMovementBehaviour`,
// `PortableStorageInterfaceMovement` and the stationary interface's timer),
// and the structure's storage (`MountedStorageManager.getAllItems`, as
// `ItemHandlerHelper.insertItem` and `ItemHelper.extract` use it). The world
// is asked through small facts and traits. No Horizon types.

use super::create_belt::Stack;
use super::create_kinetics::{Axis, DEPLOYER, DEPLOYER_ALONG, Dir, Pos};

pub const DRILL: &str = "create:mechanical_drill";
pub const HARVESTER: &str = "create:mechanical_harvester";
pub const PLOUGH: &str = "create:mechanical_plough";
pub const SAW: &str = "create:mechanical_saw";
/// The saw with `AXIS_ALONG_FIRST_COORDINATE` (a block of its own here).
pub const SAW_ALONG: &str = "create:hz_mechanical_saw_along";
pub const PSI: &str = "create:portable_storage_interface";

/// `SmartBlockEntity`'s default `lazyTickRate`.
pub const LAZY_TICK_RATE: u64 = 10;

/// The blocks with a `MovementBehaviour` here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ActorKind {
    /// `DrillMovementBehaviour`.
    Drill,
    /// `HarvesterMovementBehaviour`.
    Harvester,
    /// `PloughMovementBehaviour`.
    Plough,
    /// `SawMovementBehaviour`.
    Saw,
    /// `DeployerMovementBehaviour`.
    Deployer,
    /// `PortableStorageInterfaceMovement`.
    Psi,
}

impl ActorKind {
    pub fn of(name: &str) -> Option<ActorKind> {
        match name {
            DRILL => Some(ActorKind::Drill),
            HARVESTER => Some(ActorKind::Harvester),
            PLOUGH => Some(ActorKind::Plough),
            SAW | SAW_ALONG => Some(ActorKind::Saw),
            DEPLOYER | DEPLOYER_ALONG => Some(ActorKind::Deployer),
            PSI => Some(ActorKind::Psi),
            _ => None,
        }
    }

    pub fn block(self) -> &'static str {
        match self {
            ActorKind::Drill => DRILL,
            ActorKind::Harvester => HARVESTER,
            ActorKind::Plough => PLOUGH,
            ActorKind::Saw => SAW,
            ActorKind::Deployer => DEPLOYER,
            ActorKind::Psi => PSI,
        }
    }

    /// `getActiveAreaOffset`: a little in front of the block.
    pub fn active_area_offset(self, facing: Dir) -> [f64; 3] {
        let scale = match self {
            ActorKind::Drill | ActorKind::Saw => 0.65,
            ActorKind::Harvester | ActorKind::Plough => 0.45,
            ActorKind::Deployer => 2.0,
            ActorKind::Psi => 1.85,
        };
        facing.normal().map(|c| f64::from(c) * scale)
    }

    /// Whether it is a `BlockBreakingMovementBehaviour`.
    pub fn breaks(self) -> bool {
        matches!(self, ActorKind::Drill | ActorKind::Plough | ActorKind::Saw)
    }
}

fn length(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// `VecHelper.isVecPointingTowards`: a little tolerance lets actors moving
/// sideways work.
pub fn pointing_towards(v: [f64; 3], d: Dir) -> bool {
    let l = length(v);
    if l == 0.0 {
        return false;
    }
    let n = d.normal();
    (0..3).map(|k| f64::from(n[k]) * v[k] / l).sum::<f64>() > 0.125
}

/// `isActive` of the drill, saw, harvester and plough: not while moving
/// away from where they face.
pub fn is_active(facing: Dir, relative_motion: [f64; 3]) -> bool {
    !pointing_towards(relative_motion, facing.opposite())
}

impl ActorKind {
    /// `MovementBehaviour.isActive` (the deployer and the interface always).
    pub fn active(self, facing: Dir, relative_motion: [f64; 3]) -> bool {
        match self {
            ActorKind::Deployer | ActorKind::Psi => true,
            _ => is_active(facing, relative_motion),
        }
    }
}

/// `ItemHandlerHelper.insertItem` over the structure's slots (each holds a
/// stack up to its `max_stack`): what doesn't fit.
pub fn insert_item<S: Stack>(
    slots: &mut [Option<S>],
    stack: S,
    same: impl Fn(&S, &S) -> bool,
) -> Option<S> {
    let mut left = stack;
    for slot in slots.iter_mut() {
        let limit = left.max_stack().min(64);
        match slot {
            Some(existing) => {
                if !same(existing, &left) || existing.count() >= limit {
                    continue;
                }
                let moved = left.count().min(limit - existing.count());
                existing.set_count(existing.count() + moved);
                left.set_count(left.count() - moved);
            }
            None => {
                let moved = left.count().min(limit);
                let mut put = left.clone();
                put.set_count(moved);
                *slot = Some(put);
                left.set_count(left.count() - moved);
            }
        }
        if left.count() == 0 {
            return None;
        }
    }
    Some(left)
}

/// `ItemHelper.extract(inv, test, 1, false)`: one item of the first stack
/// that passes.
pub fn extract_one<S: Stack>(slots: &mut [Option<S>], test: impl Fn(&S) -> bool) -> Option<S> {
    for slot in slots.iter_mut() {
        let Some(existing) = slot else {
            continue;
        };
        if !test(existing) {
            continue;
        }
        let mut out = existing.clone();
        out.set_count(1);
        existing.set_count(existing.count() - 1);
        if existing.count() == 0 {
            *slot = None;
        }
        return Some(out);
    }
    None
}

/// `PortableStorageInterfaceBlockEntity.ANIMATION`, and `CLogistics.psiTimeout`.
pub const PSI_ANIMATION: i32 = 4;
pub const PSI_TIMEOUT: i32 = 60;

/// A stationary portable storage interface (`PortableStorageInterfaceBlockEntity`
/// on the server): `connected` is the structure it hands items to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Psi<E> {
    pub transfer_timer: i32,
    pub distance: f32,
    pub keep_alive: i32,
    pub powered: bool,
    pub connected: Option<E>,
}

impl<E> Default for Psi<E> {
    fn default() -> Self {
        Psi {
            transfer_timer: 0,
            distance: 0.0,
            keep_alive: 0,
            powered: false,
            connected: None,
        }
    }
}

impl<E: Copy + PartialEq> Psi<E> {
    /// `startTransferringTo`.
    pub fn start_transferring_to(&mut self, entity: E, distance: f32) {
        if self.connected == Some(entity) {
            return;
        }
        self.distance = distance.min(2.0);
        self.connected = Some(entity);
        self.transfer_timer = 6 + PSI_ANIMATION * 2;
    }

    /// `stopTransferring`.
    pub fn stop_transferring(&mut self) {
        self.connected = None;
    }

    /// `isConnected`.
    pub fn is_connected(&self) -> bool {
        self.transfer_timer >= PSI_ANIMATION && self.transfer_timer <= PSI_TIMEOUT + PSI_ANIMATION
    }

    /// `canTransfer`, `alive` telling whether the structure still moves.
    pub fn can_transfer(&mut self, alive: bool) -> bool {
        if self.connected.is_some() && !alive {
            self.stop_transferring();
        }
        self.connected.is_some() && self.is_connected()
    }

    /// `onContentTransferred`.
    pub fn transferred(&mut self) {
        self.transfer_timer = PSI_TIMEOUT + PSI_ANIMATION;
    }

    /// `tick` on the server.
    pub fn tick(&mut self) {
        if self.keep_alive > 0 {
            self.keep_alive -= 1;
            if self.keep_alive == 0 {
                self.stop_transferring();
                self.transfer_timer = PSI_ANIMATION - 1;
                return;
            }
        }
        self.transfer_timer = self.transfer_timer.min(PSI_ANIMATION * 2 + PSI_TIMEOUT);
        let can_decrement =
            self.transfer_timer > PSI_ANIMATION || self.transfer_timer > 0 && self.keep_alive == 0;
        if can_decrement {
            self.transfer_timer -= 1;
            if self.transfer_timer <= 0 || self.powered {
                self.stop_transferring();
            }
        }
    }

    /// Nothing left to keep for it.
    pub fn idle(&self) -> bool {
        self.connected.is_none() && self.transfer_timer <= 0 && self.keep_alive == 0
    }
}

/// `PortableStorageInterfaceMovement.tick` on the server, once the actor
/// found a stationary interface at `target` (`WorkingPos`; `None` once
/// lost): the structure stalls when its next step would take the actor
/// away from it, and goes on once the interface's timer has run down.
/// `timer` is the interface's `transferTimer` (the glue sets `keepAlive`).
pub fn psi_tick(ctx: &mut ActorContext, target: Option<[f64; 3]>, timer: Option<i32>) {
    let Some(target) = target else {
        if ctx.stall {
            ctx.stall = false;
        }
        return;
    };
    let Some(position) = ctx.position else {
        return;
    };
    let dist = |a: [f64; 3]| ((0..3).map(|k| (a[k] - target[k]).powi(2)).sum::<f64>()).sqrt();
    let next = [0, 1, 2].map(|k| position[k] + ctx.motion[k]);
    if !ctx.stall && dist(position) < dist(next) {
        ctx.stall = true;
    }
    let Some(timer) = timer else {
        ctx.stall = false;
        return;
    };
    if ctx.stall && timer <= PSI_ANIMATION {
        ctx.stall = false;
    }
}

/// `DeployerMovementBehaviour.tick` while it stalls breaking a block: true
/// when the hand strikes again (every 21 ticks, `Timer`).
pub fn deployer_strikes(ctx: &mut ActorContext) -> bool {
    if ctx.stationary_timer < 20 {
        ctx.stationary_timer += 1;
        return false;
    }
    ctx.stationary_timer = 0;
    true
}

/// A block an actor is breaking (`BreakingPos`, `Progress`,
/// `TicksUntilNextProgress` in `MovementContext.data`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Breaking {
    pub pos: Pos,
    pub progress: i32,
    pub ticks_until_next: i32,
}

/// `MovementContext`, what an actor keeps while it moves.
#[derive(Clone, Debug, PartialEq)]
pub struct ActorContext {
    pub position: Option<[f64; 3]>,
    pub motion: [f64; 3],
    pub relative_motion: [f64; 3],
    pub first_movement: bool,
    pub stall: bool,
    pub breaking: Option<Breaking>,
    /// `WaitingTicks` and `LastPos`: falling blocks settling.
    pub waiting: Option<(i32, Pos)>,
    /// `StationaryTimer`: actors at a bearing's centre.
    pub stationary_timer: i32,
}

impl Default for ActorContext {
    fn default() -> Self {
        ActorContext {
            position: None,
            motion: [0.0; 3],
            relative_motion: [0.0; 3],
            first_movement: true,
            stall: false,
            breaking: None,
            waiting: None,
            stationary_timer: 0,
        }
    }
}

/// `BlockPos.containing`.
pub fn containing(v: [f64; 3]) -> Pos {
    v.map(|c| c.floor() as i32)
}

/// `AbstractContraptionEntity.shouldActorTrigger`: the actor, now at
/// `actor_pos`, entered a new block (or moved for the first time).
/// `unrotate` is `reverseRotation`, turning a world vector into the
/// structure's own frame.
pub fn should_trigger(
    ctx: &mut ActorContext,
    actor_pos: [f64; 3],
    unrotate: impl Fn([f64; 3]) -> [f64; 3],
) -> bool {
    let Some(previous) = ctx.position else {
        return false;
    };
    ctx.motion = [0, 1, 2].map(|k| actor_pos[k] - previous[k]);
    ctx.relative_motion = unrotate(ctx.motion);
    containing(previous) != containing(actor_pos)
        || length(ctx.relative_motion) > 0.0 && ctx.first_movement
}

/// The rest of `ControlledContraptionEntity.shouldActorTrigger`: an actor
/// on a bearing's axis, working along it, visits its block every 20 ticks
/// as the structure turns. `local` is its place from the anchor.
pub fn bearing_centre_trigger(
    ctx: &mut ActorContext,
    local: Pos,
    offset: [f64; 3],
    facing: Dir,
    angle_delta: f32,
) -> bool {
    let along = match facing.axis() {
        Axis::X => 0,
        Axis::Y => 1,
        Axis::Z => 2,
    };
    if (0..3).any(|k| k != along && offset[k] != 0.0) {
        return false;
    }
    if (0..3).any(|k| k != along && local[k] != 0) {
        return false;
    }
    let n = facing.normal();
    ctx.motion = n.map(|c| f64::from(c) * f64::from(angle_delta) / 360.0);
    ctx.relative_motion = ctx.motion;
    if ctx.stationary_timer > 0 {
        ctx.stationary_timer -= 1;
        return false;
    }
    ctx.stationary_timer = 20;
    true
}

/// `MovementContext.getAnimationSpeed` on the server.
pub fn animation_speed(motion: [f64; 3]) -> f32 {
    let l = -length(motion);
    if l.abs() < 1.0 / 512.0 {
        return 0.0;
    }
    (((l * 1000.0 + 100.0 * l.signum()) as i32 / 100) * 100) as f32
}

/// `BlockBreakingMovementBehaviour.getBlockBreakingSpeed` for structures
/// moved by pistons, pulleys, bearings and gantries.
pub fn moving_break_speed(motion: [f64; 3]) -> f32 {
    (animation_speed(motion).abs() / 500.0).clamp(1.0 / 128.0, 16.0)
}

/// What breaking actors ask of the world.
pub trait BreakWorld {
    /// `getDestroySpeed`.
    fn hardness(&self, pos: Pos) -> f32;
    /// The actor's `canBreak`.
    fn can_break(&self, kind: ActorKind, pos: Pos) -> bool;
    /// `instanceof FallingBlock`.
    fn falling(&self, pos: Pos) -> bool;
}

/// What a breaker's tick asks the glue to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakEvent {
    /// `destroyBlockProgress(id, pos, stage)`, with the hit sound.
    Progress { pos: Pos, stage: i32 },
    /// `destroyBlockProgress(id, pos, -1)`.
    Cleared { pos: Pos },
    /// The block at `pos` breaks (its drops collected or dropped): the top
    /// of a column of falling blocks over `from`.
    Broken { pos: Pos, from: Pos },
}

/// `BlockBreakingMovementBehaviour.visitNewPosition` on the server, after
/// the damage to entities: a block it can break stalls the structure.
pub fn visit_breaker(ctx: &mut ActorContext, kind: ActorKind, world: &impl BreakWorld, pos: Pos) {
    if !world.can_break(kind, pos) {
        return;
    }
    let (progress, ticks_until_next) = ctx
        .breaking
        .map_or((0, 0), |b| (b.progress, b.ticks_until_next));
    ctx.breaking = Some(Breaking {
        pos,
        progress,
        ticks_until_next,
    });
    ctx.stall = true;
}

/// The end of `SawMovementBehaviour.visitNewPosition`: a saw facing up or
/// down in the world cuts nothing as it moves.
pub fn saw_visit_end(ctx: &mut ActorContext, facing_in_world: [f64; 3]) {
    let [x, y, z] = facing_in_world.map(f64::abs);
    let vertical = y > x && y > z;
    if vertical && ctx.breaking.is_some() {
        ctx.breaking = None;
        ctx.stall = false;
    }
}

/// `BlockBreakingMovementBehaviour.cancelStall` (and `stopMoving`).
pub fn cancel_stall(ctx: &mut ActorContext) -> Option<BreakEvent> {
    let b = ctx.breaking.take()?;
    ctx.stall = false;
    Some(BreakEvent::Cleared { pos: b.pos })
}

/// `BlockBreakingMovementBehaviour.tickBreaker`.
pub fn tick_breaker(
    ctx: &mut ActorContext,
    kind: ActorKind,
    world: &impl BreakWorld,
    speed: f32,
) -> Option<BreakEvent> {
    let Some(mut b) = ctx.breaking else {
        ctx.stall = false;
        return None;
    };
    if ctx.relative_motion == [0.0; 3] {
        ctx.stall = false;
        return None;
    }
    if b.ticks_until_next > 0 {
        b.ticks_until_next -= 1;
        ctx.breaking = Some(b);
        return None;
    }
    let hardness = world.hardness(b.pos);
    if !world.can_break(kind, b.pos) {
        let mut event = None;
        if b.progress != 0 {
            ctx.breaking = None;
            event = Some(BreakEvent::Cleared { pos: b.pos });
        }
        ctx.stall = false;
        return event;
    }
    b.progress += ((speed / hardness) as i32).clamp(1, 10 - b.progress);
    if b.progress >= 10 {
        // Falling blocks break from the top of their column down.
        let mut top = b.pos;
        while world.falling([top[0], top[1] + 1, top[2]]) {
            top[1] += 1;
        }
        ctx.stall = false;
        ctx.breaking = None;
        // onBlockBroken: a falling block lets what is over it settle (the
        // saw fells a tree instead).
        if kind != ActorKind::Saw && world.falling(top) {
            ctx.waiting = Some((10, b.pos));
            ctx.stall = true;
        }
        return Some(BreakEvent::Broken {
            pos: top,
            from: b.pos,
        });
    }
    b.ticks_until_next = (hardness / speed) as i32;
    ctx.breaking = Some(b);
    Some(BreakEvent::Progress {
        pos: b.pos,
        stage: b.progress,
    })
}

/// The end of `BlockBreakingMovementBehaviour.tick`: when falling blocks
/// have settled, the block is visited again.
pub fn tick_waiting(ctx: &mut ActorContext) -> Option<Pos> {
    let (ticks, pos) = ctx.waiting?;
    if ticks > 0 {
        ctx.waiting = Some((ticks - 1, pos));
        ctx.stall = true;
        return None;
    }
    ctx.waiting = None;
    ctx.stall = false;
    Some(pos)
}

/// `BlockBreakingKineticBlockEntity.isBreakable`, from the block's facts.
pub fn is_breakable(liquid: bool, air: bool, hardness: f32, non_breakable: bool) -> bool {
    !(liquid || air || hardness == -1.0 || non_breakable)
}

/// `BlockBreakingKineticBlockEntity`: a standing drill breaking the block
/// in front of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Breaker {
    pub ticks_until_next: i32,
    pub progress: i32,
    pub pos: Option<Pos>,
}

impl Breaker {
    pub fn destroy_next_tick(&mut self) {
        self.ticks_until_next = 1;
    }

    /// `onSpeedChanged`.
    pub fn speed_changed(&mut self) {
        if self.progress == -1 {
            self.destroy_next_tick();
        }
    }

    /// `lazyTick`, every `LAZY_TICK_RATE` ticks.
    pub fn lazy_tick(&mut self) {
        if self.ticks_until_next == -1 {
            self.destroy_next_tick();
        }
    }

    /// `tick` on the server: `breakable` is `canBreak` of the block at
    /// `target` (its hardness `hardness`).
    pub fn tick(
        &mut self,
        rpm: f32,
        target: Pos,
        hardness: f32,
        breakable: bool,
    ) -> Option<BreakEvent> {
        if rpm == 0.0 {
            return None;
        }
        self.pos = Some(target);
        if self.ticks_until_next < 0 {
            return None;
        }
        if self.ticks_until_next > 0 {
            self.ticks_until_next -= 1;
            return None;
        }
        if !breakable {
            if self.progress != 0 {
                self.progress = 0;
                return Some(BreakEvent::Cleared { pos: target });
            }
            return None;
        }
        let speed = (rpm / 100.0).abs();
        self.progress += ((speed / hardness) as i32).clamp(1, 10 - self.progress);
        if self.progress >= 10 {
            self.progress = 0;
            self.ticks_until_next = -1;
            return Some(BreakEvent::Broken {
                pos: target,
                from: target,
            });
        }
        self.ticks_until_next = (hardness / speed) as i32;
        Some(BreakEvent::Progress {
            pos: target,
            stage: self.progress,
        })
    }
}

/// `DrillBlock.getDamage`.
pub fn drill_damage(rpm: f32) -> f64 {
    let s = f64::from(rpm.abs());
    let sum = (s / 16.0).min(2.0) + (s / 32.0).min(4.0) + (s / 64.0).min(4.0);
    sum.clamp(1.0, 10.0)
}

/// The facts of a block the drill moves into (`DrillMovementBehaviour.canBreak`).
pub fn drill_can_break(breakable: bool, collision_empty: bool, track: bool) -> bool {
    breakable && !collision_empty && !track
}

/// What `PloughMovementBehaviour.canBreak` reads of a block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PloughFacts {
    pub air: bool,
    pub farmland_below: bool,
    /// `LiquidBlock`.
    pub liquid: bool,
    pub bubble_column: bool,
    /// `create:plough_blacklist` / `create:plough_whitelist`.
    pub blacklisted: bool,
    pub whitelisted: bool,
    pub collision_empty: bool,
}

/// `PloughMovementBehaviour.canBreak` (no tracks here).
pub fn plough_can_break(f: &PloughFacts) -> bool {
    if f.air || f.farmland_below || f.liquid || f.bubble_column || f.blacklisted {
        return false;
    }
    f.whitelisted || f.collision_empty
}

/// `HoeItem.useOn` on the top of `below` (`TILLABLES`), with whether the
/// block over it is air: the block it turns into and an item it drops.
pub fn tilled(below: &str, air_above: bool) -> Option<(&'static str, Option<&'static str>)> {
    match below {
        "minecraft:grass_block" | "minecraft:dirt_path" | "minecraft:dirt" if air_above => {
            Some(("minecraft:farmland", None))
        }
        "minecraft:coarse_dirt" if air_above => Some(("minecraft:dirt", None)),
        "minecraft:rooted_dirt" => Some(("minecraft:dirt", Some("minecraft:hanging_roots"))),
        _ => None,
    }
}

/// What `HarvesterMovementBehaviour` reads of a block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlantFacts {
    pub air: bool,
    /// `create:non_harvestable`.
    pub non_harvestable: bool,
    /// A `CropBlock`: its age and `getMaxAge`.
    pub crop: Option<(i32, i32)>,
    pub collision_empty: bool,
    pub cocoa: bool,
    /// The `age` property and its highest value.
    pub age: Option<(i32, i32)>,
    pub sweet_berry_bush: bool,
    pub sugar_cane: bool,
    /// `create:sugar_cane_variants`.
    pub sugar_cane_variant: bool,
    pub leaves: bool,
    pub growing_plant: bool,
    /// `BushBlock` (`VegetationBlock` since 1.21.5).
    pub bush: bool,
    /// Holds a fluid (waterlogged), left behind when cut.
    pub fluid: bool,
}

/// What a harvested block becomes (`cutCrop`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cut {
    /// Air, or the fluid it held.
    Gone,
    /// The same block with this `age`.
    Age(i32),
}

/// `HarvesterMovementBehaviour.visitNewPosition`'s choice: harvest with
/// shears (leaves), keep one seed back for replanting unless
/// `seed_kept` already, and leave `cut` behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Harvest {
    pub shears: bool,
    pub seed_kept: bool,
    pub cut: Cut,
}

/// `CKinetics` defaults.
const HARVEST_PARTIALLY_GROWN: bool = false;
const HARVESTER_REPLANTS: bool = true;

/// `isValidCrop`.
fn valid_crop(f: &PlantFacts) -> bool {
    if let Some((age, max)) = f.crop {
        if HARVEST_PARTIALLY_GROWN {
            return age != 0 || !HARVESTER_REPLANTS;
        }
        return age >= max;
    }
    if (f.collision_empty || f.cocoa)
        && let Some((age, max)) = f.age
    {
        if f.sweet_berry_bush && age <= 1 && HARVESTER_REPLANTS {
            return false;
        }
        if age == 0 && HARVESTER_REPLANTS || !HARVEST_PARTIALLY_GROWN && max != age {
            return false;
        }
        return true;
    }
    false
}

/// `isValidOther`.
fn valid_other(f: &PlantFacts) -> bool {
    if f.crop.is_some() {
        return false;
    }
    if f.sugar_cane || f.leaves {
        return true;
    }
    if f.cocoa {
        return f.age.is_some_and(|(age, max)| age == max);
    }
    if f.collision_empty {
        if f.growing_plant {
            return true;
        }
        if f.age.is_some() {
            return false;
        }
        return f.bush;
    }
    false
}

/// `cutCrop` with replanting on.
fn cut_crop(f: &PlantFacts) -> Cut {
    if f.crop.is_some() {
        return Cut::Age(0);
    }
    if f.sweet_berry_bush {
        return Cut::Age(1);
    }
    if f.sugar_cane_variant || f.growing_plant {
        return Cut::Gone;
    }
    if (f.collision_empty || f.cocoa) && f.age.is_some() {
        return Cut::Age(0);
    }
    Cut::Gone
}

/// `HarvesterMovementBehaviour.visitNewPosition` on the server.
pub fn harvest(f: &PlantFacts) -> Option<Harvest> {
    if f.air || f.non_harvestable {
        return None;
    }
    let mut not_crop_but_cuttable = false;
    if !valid_crop(f) {
        if valid_other(f) {
            not_crop_but_cuttable = true;
        } else {
            return None;
        }
    }
    Some(Harvest {
        shears: f.leaves,
        seed_kept: not_crop_but_cuttable,
        cut: cut_crop(f),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct World {
        hardness: HashMap<Pos, f32>,
        falling: Vec<Pos>,
    }

    impl BreakWorld for World {
        fn hardness(&self, pos: Pos) -> f32 {
            self.hardness.get(&pos).copied().unwrap_or(0.0)
        }
        fn can_break(&self, _kind: ActorKind, pos: Pos) -> bool {
            self.hardness.contains_key(&pos)
        }
        fn falling(&self, pos: Pos) -> bool {
            self.falling.contains(&pos)
        }
    }

    #[test]
    fn active_area_and_pointing() {
        assert_eq!(
            ActorKind::Drill.active_area_offset(Dir::East),
            [0.65, 0.0, 0.0]
        );
        assert_eq!(
            ActorKind::Plough.active_area_offset(Dir::North),
            [0.0, 0.0, -0.45]
        );
        assert!(pointing_towards([1.0, 0.0, 0.0], Dir::East));
        // Sideways is not away.
        assert!(is_active(Dir::East, [0.0, 0.0, 0.3]));
        assert!(!is_active(Dir::East, [-0.2, 0.0, 0.0]));
        assert!(is_active(Dir::East, [0.0; 3]));
        assert_eq!(ActorKind::of(DRILL), Some(ActorKind::Drill));
        assert!(!ActorKind::Harvester.breaks());
    }

    #[test]
    fn triggers_on_a_new_block() {
        let mut ctx = ActorContext::default();
        let id = |v| v;
        assert!(!should_trigger(&mut ctx, [0.5, 0.5, 0.5], id));
        ctx.position = Some([0.5, 0.5, 0.5]);
        // First movement triggers in the same block.
        assert!(should_trigger(&mut ctx, [0.6, 0.5, 0.5], id));
        ctx.first_movement = false;
        ctx.position = Some([0.6, 0.5, 0.5]);
        assert!(!should_trigger(&mut ctx, [0.9, 0.5, 0.5], id));
        ctx.position = Some([0.9, 0.5, 0.5]);
        assert!(should_trigger(&mut ctx, [1.1, 0.5, 0.5], id));
        assert!((ctx.motion[0] - 0.2).abs() < 1e-9);
    }

    #[test]
    fn bearing_centre_every_20_ticks() {
        let mut ctx = ActorContext::default();
        assert!(bearing_centre_trigger(
            &mut ctx,
            [0, 3, 0],
            [0.0, 0.65, 0.0],
            Dir::Up,
            3.0
        ));
        assert_eq!(ctx.stationary_timer, 20);
        assert!((ctx.motion[1] - 3.0 / 360.0).abs() < 1e-9);
        for _ in 0..20 {
            assert!(!bearing_centre_trigger(
                &mut ctx,
                [0, 3, 0],
                [0.0, 0.65, 0.0],
                Dir::Up,
                3.0
            ));
        }
        assert!(bearing_centre_trigger(
            &mut ctx,
            [0, 3, 0],
            [0.0, 0.65, 0.0],
            Dir::Up,
            3.0
        ));
        // Off the axis, or working across it: no.
        assert!(!bearing_centre_trigger(
            &mut ctx,
            [1, 3, 0],
            [0.0, 0.65, 0.0],
            Dir::Up,
            3.0
        ));
        assert!(!bearing_centre_trigger(
            &mut ctx,
            [0, 3, 0],
            [0.65, 0.0, 0.0],
            Dir::Up,
            3.0
        ));
    }

    #[test]
    fn animation_and_break_speed() {
        // A piston at full speed (0.49 blocks a tick).
        assert_eq!(animation_speed([0.49, 0.0, 0.0]), -500.0);
        assert_eq!(moving_break_speed([0.49, 0.0, 0.0]), 1.0);
        // 16 rpm: 1/32 of a block a tick.
        assert_eq!(animation_speed([0.03125, 0.0, 0.0]), -100.0);
        assert_eq!(moving_break_speed([0.0, 0.0, 0.0]), 1.0 / 128.0);
    }

    #[test]
    fn breaking_stalls_until_broken() {
        let world = World {
            hardness: HashMap::from([([1, 0, 0], 1.5)]),
            falling: vec![],
        };
        let mut ctx = ActorContext {
            relative_motion: [0.49, 0.0, 0.0],
            ..ActorContext::default()
        };
        visit_breaker(&mut ctx, ActorKind::Drill, &world, [5, 0, 0]);
        assert!(!ctx.stall);
        visit_breaker(&mut ctx, ActorKind::Drill, &world, [1, 0, 0]);
        assert!(ctx.stall);
        // Speed 1 against stone's 1.5: a stage a tick and a wait of one.
        let mut events = Vec::new();
        for _ in 0..40 {
            if let Some(e) = tick_breaker(&mut ctx, ActorKind::Drill, &world, 1.0) {
                events.push(e);
            }
            if matches!(events.last(), Some(BreakEvent::Broken { .. })) {
                break;
            }
        }
        assert_eq!(
            events[0],
            BreakEvent::Progress {
                pos: [1, 0, 0],
                stage: 1
            }
        );
        assert_eq!(events.len(), 10);
        assert_eq!(
            events[9],
            BreakEvent::Broken {
                pos: [1, 0, 0],
                from: [1, 0, 0]
            }
        );
        assert!(!ctx.stall);
        assert_eq!(ctx.breaking, None);
    }

    #[test]
    fn falling_blocks_break_from_the_top_and_settle() {
        let world = World {
            hardness: HashMap::from([([1, 0, 0], 0.5)]),
            falling: vec![[1, 0, 0], [1, 1, 0], [1, 2, 0]],
        };
        let mut ctx = ActorContext {
            relative_motion: [0.49, 0.0, 0.0],
            ..ActorContext::default()
        };
        visit_breaker(&mut ctx, ActorKind::Drill, &world, [1, 0, 0]);
        let mut last = None;
        for _ in 0..20 {
            last = tick_breaker(&mut ctx, ActorKind::Drill, &world, 16.0);
            if last.is_some() {
                break;
            }
        }
        assert_eq!(
            last,
            Some(BreakEvent::Broken {
                pos: [1, 2, 0],
                from: [1, 0, 0]
            })
        );
        assert!(ctx.stall);
        for _ in 0..10 {
            assert_eq!(tick_waiting(&mut ctx), None);
        }
        assert_eq!(tick_waiting(&mut ctx), Some([1, 0, 0]));
        assert!(!ctx.stall);
    }

    #[test]
    fn breaker_stops_when_the_block_goes() {
        let world = World {
            hardness: HashMap::new(),
            falling: vec![],
        };
        let mut ctx = ActorContext {
            relative_motion: [0.1, 0.0, 0.0],
            stall: true,
            breaking: Some(Breaking {
                pos: [1, 0, 0],
                progress: 3,
                ticks_until_next: 0,
            }),
            ..ActorContext::default()
        };
        assert_eq!(
            tick_breaker(&mut ctx, ActorKind::Drill, &world, 1.0),
            Some(BreakEvent::Cleared { pos: [1, 0, 0] })
        );
        assert!(!ctx.stall);
        // Not moving: nothing breaks, nothing stalls.
        ctx.breaking = Some(Breaking {
            pos: [1, 0, 0],
            progress: 0,
            ticks_until_next: 0,
        });
        ctx.relative_motion = [0.0; 3];
        ctx.stall = true;
        assert_eq!(tick_breaker(&mut ctx, ActorKind::Drill, &world, 1.0), None);
        assert!(!ctx.stall);
        assert_eq!(
            cancel_stall(&mut ctx),
            Some(BreakEvent::Cleared { pos: [1, 0, 0] })
        );
    }

    #[test]
    fn standing_drill() {
        let mut b = Breaker::default();
        assert_eq!(b.tick(0.0, [0, 0, 1], 1.5, true), None);
        // 64 rpm against stone: 0.64 / 1.5 rounds to a stage, then 2 ticks' wait.
        assert_eq!(
            b.tick(64.0, [0, 0, 1], 1.5, true),
            Some(BreakEvent::Progress {
                pos: [0, 0, 1],
                stage: 1
            })
        );
        assert_eq!(b.ticks_until_next, 2);
        let mut ticks = 1;
        let broken = loop {
            ticks += 1;
            if let Some(e @ BreakEvent::Broken { .. }) = b.tick(64.0, [0, 0, 1], 1.5, true) {
                break e;
            }
        };
        assert_eq!(
            broken,
            BreakEvent::Broken {
                pos: [0, 0, 1],
                from: [0, 0, 1]
            }
        );
        assert_eq!(ticks, 28);
        // It waits for the next lazy tick (or a neighbour's change).
        assert_eq!(b.tick(64.0, [0, 0, 1], 1.5, true), None);
        b.lazy_tick();
        assert_eq!(b.ticks_until_next, 1);
        // Air in front: no progress.
        let mut c = Breaker::default();
        assert_eq!(c.tick(64.0, [0, 0, 1], 0.0, false), None);
        assert!(is_breakable(false, false, 1.5, false));
        assert!(!is_breakable(false, false, -1.0, false));
        assert!(!is_breakable(true, false, 100.0, false));
    }

    #[test]
    fn saws_cut_sideways_only() {
        assert_eq!(ActorKind::of(SAW_ALONG), Some(ActorKind::Saw));
        let world = World {
            hardness: HashMap::from([([1, 0, 0], 2.0)]),
            falling: vec![[1, 0, 0]],
        };
        let mut ctx = ActorContext {
            relative_motion: [0.49, 0.0, 0.0],
            ..ActorContext::default()
        };
        visit_breaker(&mut ctx, ActorKind::Saw, &world, [1, 0, 0]);
        saw_visit_end(&mut ctx, [1.0, 0.0, 0.0]);
        assert!(ctx.stall);
        saw_visit_end(&mut ctx, [0.0, -1.0, 0.0]);
        assert!(!ctx.stall);
        assert_eq!(ctx.breaking, None);
        // A felled log over a falling block doesn't wait for it.
        visit_breaker(&mut ctx, ActorKind::Saw, &world, [1, 0, 0]);
        let e = tick_breaker(&mut ctx, ActorKind::Saw, &world, 32.0);
        assert!(matches!(e, Some(BreakEvent::Broken { .. })));
        assert_eq!(ctx.waiting, None);
    }

    #[derive(Clone, Debug, PartialEq)]
    struct Item(&'static str, u32);

    impl Stack for Item {
        fn count(&self) -> u32 {
            self.1
        }
        fn set_count(&mut self, count: u32) {
            self.1 = count;
        }
        fn max_stack(&self) -> u32 {
            if self.0 == "pearl" { 16 } else { 64 }
        }
    }

    #[test]
    fn storage_fills_slots_in_order() {
        let same = |a: &Item, b: &Item| a.0 == b.0;
        let mut slots = vec![Some(Item("stone", 60)), None, Some(Item("dirt", 1)), None];
        assert_eq!(insert_item(&mut slots, Item("stone", 10), same), None);
        assert_eq!(slots[0], Some(Item("stone", 64)));
        assert_eq!(slots[1], Some(Item("stone", 6)));
        assert_eq!(
            insert_item(&mut slots, Item("pearl", 40), same),
            Some(Item("pearl", 24))
        );
        assert_eq!(slots[3], Some(Item("pearl", 16)));
        assert_eq!(
            extract_one(&mut slots, |s| s.0 == "dirt"),
            Some(Item("dirt", 1))
        );
        assert_eq!(slots[2], None);
        assert_eq!(extract_one(&mut slots, |_| true), Some(Item("stone", 1)));
        assert_eq!(slots[0], Some(Item("stone", 63)));
        assert_eq!(ActorKind::of(DEPLOYER_ALONG), Some(ActorKind::Deployer));
        assert!(ActorKind::Psi.active(Dir::East, [-1.0, 0.0, 0.0]));
    }

    #[test]
    fn interfaces_hold_the_structure_until_idle() {
        let mut psi: Psi<u32> = Psi::default();
        psi.start_transferring_to(7, 3.0);
        assert_eq!((psi.distance, psi.transfer_timer), (2.0, 14));
        assert!(psi.can_transfer(true));
        // The actor keeps it alive while it waits; with nothing moved, the
        // timer runs down to the animation's length and stays there.
        for _ in 0..30 {
            psi.keep_alive = 2;
            psi.tick();
        }
        assert_eq!(psi.transfer_timer, PSI_ANIMATION);
        psi.transferred();
        assert_eq!(psi.transfer_timer, 64);
        // Left alone (no keep alive) it disconnects.
        psi.keep_alive = 1;
        psi.tick();
        assert_eq!(psi.connected, None);
        assert!(!psi.can_transfer(true));

        let mut ctx = ActorContext {
            position: Some([0.5, 0.5, 0.5]),
            motion: [0.1, 0.0, 0.0],
            ..ActorContext::default()
        };
        // Moving away from the interface behind: stall while it transfers.
        psi_tick(&mut ctx, Some([-1.5, 0.5, 0.5]), Some(30));
        assert!(ctx.stall);
        psi_tick(&mut ctx, Some([-1.5, 0.5, 0.5]), Some(PSI_ANIMATION));
        assert!(!ctx.stall);
        // Still coming nearer: no stall.
        let mut ctx2 = ActorContext {
            position: Some([0.5, 0.5, 0.5]),
            motion: [0.1, 0.0, 0.0],
            ..ActorContext::default()
        };
        psi_tick(&mut ctx2, Some([3.5, 0.5, 0.5]), Some(30));
        assert!(!ctx2.stall);
        let mut n = 0;
        while !deployer_strikes(&mut ctx2) {
            n += 1;
        }
        assert_eq!(n, 20);
    }

    #[test]
    fn drill_damage_by_speed() {
        assert_eq!(drill_damage(0.0), 1.0);
        assert_eq!(drill_damage(32.0), 3.5);
        assert_eq!(drill_damage(256.0), 10.0);
        assert!(drill_can_break(true, false, false));
        assert!(!drill_can_break(true, true, false));
    }

    #[test]
    fn plough_rules() {
        let grass = PloughFacts {
            collision_empty: true,
            ..PloughFacts::default()
        };
        assert!(plough_can_break(&grass));
        assert!(!plough_can_break(&PloughFacts {
            farmland_below: true,
            ..grass
        }));
        assert!(!plough_can_break(&PloughFacts::default()));
        assert!(plough_can_break(&PloughFacts {
            whitelisted: true,
            ..PloughFacts::default()
        }));
        assert_eq!(
            tilled("minecraft:grass_block", true),
            Some(("minecraft:farmland", None))
        );
        assert_eq!(tilled("minecraft:dirt", false), None);
        assert_eq!(
            tilled("minecraft:rooted_dirt", false),
            Some(("minecraft:dirt", Some("minecraft:hanging_roots")))
        );
        assert_eq!(tilled("minecraft:stone", true), None);
    }

    #[test]
    fn harvester_rules() {
        let wheat = |age| PlantFacts {
            crop: Some((age, 7)),
            collision_empty: true,
            age: Some((age, 7)),
            bush: true,
            ..PlantFacts::default()
        };
        assert_eq!(harvest(&wheat(3)), None);
        assert_eq!(
            harvest(&wheat(7)),
            Some(Harvest {
                shears: false,
                seed_kept: false,
                cut: Cut::Age(0)
            })
        );
        // Sugar cane: cut, no seed kept back, gone.
        let cane = PlantFacts {
            sugar_cane: true,
            sugar_cane_variant: true,
            collision_empty: true,
            age: Some((3, 15)),
            ..PlantFacts::default()
        };
        assert_eq!(
            harvest(&cane),
            Some(Harvest {
                shears: false,
                seed_kept: true,
                cut: Cut::Gone
            })
        );
        // Berries: ripe at 3, back to 1.
        let berries = |age| PlantFacts {
            sweet_berry_bush: true,
            collision_empty: true,
            age: Some((age, 3)),
            bush: true,
            ..PlantFacts::default()
        };
        assert_eq!(harvest(&berries(2)), None);
        assert_eq!(harvest(&berries(3)).map(|h| h.cut), Some(Cut::Age(1)));
        // Leaves with shears; grass and flowers cut.
        let leaves = PlantFacts {
            leaves: true,
            ..PlantFacts::default()
        };
        assert_eq!(harvest(&leaves).map(|h| h.shears), Some(true));
        let grass = PlantFacts {
            collision_empty: true,
            bush: true,
            ..PlantFacts::default()
        };
        assert_eq!(harvest(&grass).map(|h| h.cut), Some(Cut::Gone));
        // Ripe cocoa (a collision box) back to age 0.
        let cocoa = PlantFacts {
            cocoa: true,
            age: Some((2, 2)),
            ..PlantFacts::default()
        };
        assert_eq!(
            harvest(&cocoa).map(|h| (h.seed_kept, h.cut)),
            Some((false, Cut::Age(0)))
        );
        assert_eq!(harvest(&PlantFacts::default()), None);
        assert_eq!(harvest(&PlantFacts { air: true, ..grass }), None);
    }
}
