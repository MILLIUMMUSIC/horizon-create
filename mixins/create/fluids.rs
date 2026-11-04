// Create's fluid transport, injected as `crate::game::create_fluids`: fluid
// stacks and tanks (`FluidStack`, NeoForge's `FluidTank`), pipes' connections
// (`PipeConnection`, `FluidTransportBehaviour`), the networks that move fluid
// between endpoints (`FluidNetwork`), pressure from mechanical pumps
// (`PumpBlockEntity.distributePressureTo`, `FluidPropagator`), open pipe ends
// that draw from and pour into the world (`OpenEndedPipe`), and the fluid
// tank's rules (`FluidTankBlockEntity`, `FluidTankRenderer`). No Horizon
// types: the world is the `FluidWorld` trait, and every block that holds
// fluid is reached through `FluidAccess` (NeoForge's
// `Capabilities.FluidHandler.BLOCK`), the one place a future
// `block_capability` hook plugs into.

use super::create_kinetics::{Axis, Dir, Pos};
use std::collections::{HashMap, HashSet};

/// `AllConfigs.server().fluids.mechanicalPumpRange`'s default.
pub const PUMP_RANGE: i32 = 16;
/// `AllConfigs.server().fluids.fluidTankCapacity`'s default, in mB per block
/// (`FluidTankBlockEntity.getCapacityMultiplier`).
pub const TANK_CAPACITY: i32 = 8000;
/// `OpenEndedPipe.OpenEndFluidHandler`'s tank.
const OPEN_END_CAPACITY: i32 = 1000;
/// `FluidNetwork.CYCLES_PER_TICK`.
const CYCLES_PER_TICK: usize = 16;
pub const WATER: &str = "minecraft:water";
pub const LAVA: &str = "minecraft:lava";
const FLOWING_WATER: &str = "minecraft:flowing_water";
const FLOWING_LAVA: &str = "minecraft:flowing_lava";

fn offset(p: Pos, d: Dir) -> Pos {
    let n = d.normal();
    [p[0] + n[0], p[1] + n[1], p[2] + n[2]]
}

// ---- Fluid stacks and tanks ---------------------------------------------------

/// NeoForge's `FluidStack`: a still fluid's id and an amount in mB (no data
/// components: potions and their kin aren't ported).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FluidStack {
    pub fluid: String,
    pub amount: i32,
}

impl FluidStack {
    pub fn new(fluid: &str, amount: i32) -> Self {
        FluidStack {
            fluid: fluid.to_owned(),
            amount,
        }
    }
    pub fn empty() -> Self {
        FluidStack::default()
    }
    pub fn is_empty(&self) -> bool {
        self.fluid.is_empty() || self.amount <= 0
    }
    /// `FluidStack.isSameFluidSameComponents` (an empty stack is the empty
    /// fluid).
    pub fn same(&self, other: &FluidStack) -> bool {
        let a = if self.is_empty() { "" } else { &self.fluid };
        let b = if other.is_empty() { "" } else { &other.fluid };
        a == b
    }
    /// `FluidHelper.copyStackWithAmount`.
    pub fn with_amount(&self, amount: i32) -> FluidStack {
        if self.is_empty() || amount <= 0 {
            return FluidStack::empty();
        }
        FluidStack::new(&self.fluid, amount)
    }
}

/// NeoForge's `IFluidHandler`: what a block that holds fluid answers (its
/// tanks' contents are each holder's own: `FluidAccess.handler_tanks`).
pub trait FluidHandler {
    /// How much of `resource` it takes (or would take, simulating).
    fn fill(&mut self, resource: &FluidStack, simulate: bool) -> i32;
    /// Up to `resource`'s amount of that fluid.
    fn drain(&mut self, resource: &FluidStack, simulate: bool) -> FluidStack;
    /// Up to `max` of whatever it holds.
    fn drain_any(&mut self, max: i32, simulate: bool) -> FluidStack;
}

/// NeoForge's `FluidTank`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FluidTank {
    pub fluid: FluidStack,
    pub capacity: i32,
}

impl FluidTank {
    pub fn new(capacity: i32) -> Self {
        FluidTank {
            fluid: FluidStack::empty(),
            capacity,
        }
    }
    pub fn amount(&self) -> i32 {
        if self.fluid.is_empty() {
            0
        } else {
            self.fluid.amount
        }
    }
    /// `FluidTank.setFluid` keeps an empty stack as the empty fluid.
    fn normalize(&mut self) {
        if self.fluid.is_empty() {
            self.fluid = FluidStack::empty();
        }
    }
}

impl FluidHandler for FluidTank {
    fn fill(&mut self, resource: &FluidStack, simulate: bool) -> i32 {
        if resource.is_empty() {
            return 0;
        }
        if self.fluid.is_empty() {
            let filled = self.capacity.min(resource.amount);
            if !simulate && filled > 0 {
                self.fluid = resource.with_amount(filled);
            }
            return filled.max(0);
        }
        if !self.fluid.same(resource) {
            return 0;
        }
        let filled = (self.capacity - self.fluid.amount)
            .min(resource.amount)
            .max(0);
        if !simulate {
            self.fluid.amount += filled;
        }
        filled
    }
    fn drain(&mut self, resource: &FluidStack, simulate: bool) -> FluidStack {
        if resource.is_empty() || !resource.same(&self.fluid) {
            return FluidStack::empty();
        }
        self.drain_any(resource.amount, simulate)
    }
    fn drain_any(&mut self, max: i32, simulate: bool) -> FluidStack {
        let drained = max.min(self.amount());
        if drained <= 0 {
            return FluidStack::empty();
        }
        let out = self.fluid.with_amount(drained);
        if !simulate {
            self.fluid.amount -= drained;
            self.normalize();
        }
        out
    }
}

/// Fluid access at a block face: NeoForge's `Capabilities.FluidHandler.BLOCK`
/// (`level.getCapability(FLUID_HANDLER, pos, side)`). Tanks, basins and any
/// other mod's fluid holders answer here; a `block_capability` hook from the
/// engine would take this trait's place.
pub trait FluidAccess {
    /// Whether the block at `pos` offers a fluid handler on `side`
    /// (`FluidPropagator.hasFluidCapability`).
    fn has_handler(&self, pos: Pos, side: Dir) -> bool;
    /// Its tanks' contents (`getFluidInTank` of each).
    fn handler_tanks(&self, pos: Pos, side: Dir) -> Vec<FluidStack>;
    fn fill(&mut self, pos: Pos, side: Dir, resource: &FluidStack, simulate: bool) -> i32;
    fn drain(&mut self, pos: Pos, side: Dir, resource: &FluidStack, simulate: bool) -> FluidStack;
    fn drain_any(&mut self, pos: Pos, side: Dir, max: i32, simulate: bool) -> FluidStack;
}

// ---- The world at a pipe's open end --------------------------------------------

/// `VanillaFluidTargets`: blocks a pipe drains with no fluid handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VanillaTarget {
    /// A beehive or bee nest's `honey_level`.
    Honey(u8),
    Cauldron,
    LavaCauldron,
    /// A water cauldron, and whether it is full (`LayeredCauldronBlock.isFull`).
    WaterCauldron(bool),
}

/// What a pipe's open end sees in the block in front of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Space {
    /// Its fluid state's type (`minecraft:water`, `minecraft:flowing_lava`),
    /// none when empty.
    pub fluid: Option<String>,
    /// `FluidState.isSource`.
    pub source: bool,
    /// Has a `waterlogged` property.
    pub waterloggable: bool,
    /// `BlockState.canBeReplaced`.
    pub replaceable: bool,
    /// `getDestroySpeed != -1`.
    pub breakable: bool,
    /// `BlockHelper.hasBlockSolidSide` on the face toward the pipe.
    pub solid_side: bool,
    /// `AllBlockTags.FAN_TRANSPARENT`.
    pub fan_transparent: bool,
    pub vanilla: Option<VanillaTarget>,
    /// `dimensionType().ultraWarm()` (the nether: water evaporates).
    pub ultra_warm: bool,
}

/// A change an open end makes to the block in front of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpaceChange {
    /// `setValue(WATERLOGGED, ...)`, with a water tick.
    Waterlogged(bool),
    /// The source drained: the fluid's block at `LEVEL` 14.
    Lowered(String),
    /// A source of the fluid placed (`defaultFluidState().createLegacyBlock()`).
    Source(String),
    /// A lava or water cauldron emptied to a cauldron.
    EmptyCauldron,
    /// A beehive's `honey_level` back to 0.
    EmptyHive,
    /// `FluidReactions`: the block becomes this one.
    Block(&'static str),
}

/// Fluids that have a block here (`FluidHelper.hasBlockState`): water and
/// lava. Create's own fluids (honey, chocolate) have no block in Horizon.
pub fn has_block_state(fluid: &str) -> bool {
    fluid == WATER || fluid == LAVA
}

/// `FluidHelper.convertToStill`.
pub fn still(fluid: &str) -> &str {
    match fluid {
        FLOWING_WATER => WATER,
        FLOWING_LAVA => LAVA,
        f => f,
    }
}

/// `FluidReactions.handlePipeFlowCollisionFallback`: two fluids meeting in a
/// pipe break it and leave this block.
pub fn flow_collision_block(a: &str, b: &str) -> Option<&'static str> {
    if (a == WATER && b == LAVA) || (a == LAVA && b == WATER) {
        return Some("minecraft:cobblestone");
    }
    // AllFluids.getLavaInteraction: Create's honey and chocolate have none
    // here (no block).
    None
}

/// `FluidReactions.handlePipeSpillCollisionFallback`: a pipe pouring `pipe`
/// into the world fluid `world` (`minecraft:flowing_lava`...).
pub fn spill_collision_block(pipe: &str, world: &str) -> Option<&'static str> {
    let pf = still(pipe);
    if pf == WATER && world == LAVA {
        Some("minecraft:obsidian")
    } else if pf == WATER && world == FLOWING_LAVA {
        Some("minecraft:cobblestone")
    } else if pf == LAVA && world == WATER {
        Some("minecraft:stone")
    } else if pf == LAVA && world == FLOWING_WATER {
        Some("minecraft:cobblestone")
    } else {
        None
    }
}

/// `VanillaFluidTargets.canProvideFluidWithoutCapability`.
fn provides_without_capability(space: &Space) -> bool {
    space.vanilla.is_some()
}

/// `OpenEndedPipe.removeFluidFromSpace`: what a pipe draws from the block in
/// front, and how that block changes.
///
/// Create keeps a drained source when vanilla would refill it at once (an
/// infinite water pool, `getNewLiquid`); here it is always lowered and the
/// game's own flow refills it.
pub fn remove_from_space(space: &Space) -> (FluidStack, Option<SpaceChange>) {
    // VanillaFluidTargets.drainBlock
    match space.vanilla {
        Some(VanillaTarget::Honey(level)) if level >= 5 => {
            return (
                FluidStack::new("create:honey", 250),
                Some(SpaceChange::EmptyHive),
            );
        }
        Some(VanillaTarget::LavaCauldron) => {
            return (
                FluidStack::new(LAVA, 1000),
                Some(SpaceChange::EmptyCauldron),
            );
        }
        Some(VanillaTarget::WaterCauldron(true)) => {
            return (
                FluidStack::new(WATER, 1000),
                Some(SpaceChange::EmptyCauldron),
            );
        }
        _ => {}
    }
    if !space.waterloggable && !space.replaceable {
        return (FluidStack::empty(), None);
    }
    let Some(fluid) = space.fluid.as_deref() else {
        return (FluidStack::empty(), None);
    };
    if !space.source {
        return (FluidStack::empty(), None);
    }
    let stack = FluidStack::new(still(fluid), 1000);
    let change = if space.waterloggable {
        SpaceChange::Waterlogged(false)
    } else {
        SpaceChange::Lowered(still(fluid).to_owned())
    };
    (stack, Some(change))
}

/// `OpenEndedPipe.provideFluidToSpace`: whether `fluid` can be (or, not
/// simulating, is) poured into the block in front, and the change. A
/// collision with another fluid there changes the block even when
/// simulating, as in Create.
pub fn provide_to_space(
    space: &Space,
    fluid: &FluidStack,
    simulate: bool,
) -> (bool, Option<SpaceChange>) {
    if !space.waterloggable && !space.replaceable {
        return (false, None);
    }
    if fluid.is_empty() {
        return (false, None);
    }
    // `instanceof FlowingFluid`: only water and lava are in Horizon.
    if !has_block_state(&fluid.fluid) {
        return (false, None);
    }
    if let Some(world) = space.fluid.as_deref()
        && still(world) != fluid.fluid
    {
        return (
            false,
            spill_collision_block(&fluid.fluid, world).map(SpaceChange::Block),
        );
    }
    if space.fluid.is_some() && space.source {
        return (false, None);
    }
    if space.waterloggable && fluid.fluid != WATER {
        return (false, None);
    }
    if simulate {
        return (true, None);
    }
    // pipesPlaceFluidSourceBlocks is on by default.
    if space.ultra_warm && fluid.fluid == WATER {
        return (true, None);
    }
    if space.waterloggable {
        return (true, Some(SpaceChange::Waterlogged(true)));
    }
    (true, Some(SpaceChange::Source(fluid.fluid.clone())))
}

// ---- Pipes ------------------------------------------------------------------

/// A block with a `FluidTransportBehaviour`, as its state says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipeBlock {
    /// `FluidPipeBlock`: the sides it is open at (`PROPERTY_BY_DIRECTION`,
    /// indexed by `Dir`).
    Pipe([bool; 6]),
    /// `PumpBlock`: its `FACING` (where it pushes to).
    Pump(Dir),
    /// `EncasedPipeBlock`: its six sides, set when encased and kept.
    Encased([bool; 6]),
    /// `GlassFluidPipeBlock`: open along its `AXIS`.
    Glass(Axis),
    /// `FluidValveBlock`: `getPipeAxis` and `ENABLED`.
    Valve(Axis, bool),
    /// `SmartFluidPipeBlock`: `getPipeAxis`; its filter is the world's.
    Smart(Axis),
}

impl PipeBlock {
    /// A straight pipe along `axis`.
    #[allow(dead_code)]
    pub fn straight(axis: Axis) -> PipeBlock {
        let mut open = [false; 6];
        for d in Dir::ALL {
            open[d as usize] = d.axis() == axis;
        }
        PipeBlock::Pipe(open)
    }
    /// `canHaveFlowToward`: `isOpenAt` for pipes, `isSideAccessible` for
    /// pumps.
    pub fn open(self, d: Dir) -> bool {
        match self {
            PipeBlock::Pipe(open) | PipeBlock::Encased(open) => open[d as usize],
            PipeBlock::Pump(f) => f.axis() == d.axis(),
            PipeBlock::Glass(a) | PipeBlock::Valve(a, _) | PipeBlock::Smart(a) => a == d.axis(),
        }
    }
    /// `FluidPropagator.validateNeighbourChange`'s check of the pipe.
    fn hears_neighbours(self) -> bool {
        self.straight_axis().is_some() || matches!(self, PipeBlock::Encased(_))
    }
    /// `FluidPropagator.getStraightPipeAxis`.
    pub fn straight_axis(self) -> Option<Axis> {
        match self {
            PipeBlock::Pump(f) => Some(f.axis()),
            PipeBlock::Glass(a) => Some(a),
            // Neither a `PumpBlock`, an `AxisPipeBlock` nor a `FluidPipeBlock`.
            PipeBlock::Encased(_) | PipeBlock::Valve(..) | PipeBlock::Smart(_) => None,
            PipeBlock::Pipe(_) => {
                let mut found = None;
                let mut connections = 0;
                for axis in Axis::ALL {
                    let a = self.open(
                        Dir::ALL
                            .into_iter()
                            .find(|d| d.axis() == axis && !d.positive())?,
                    );
                    let b = self.open(Dir::positive_of(axis));
                    connections += i32::from(a) + i32::from(b);
                    if a && b {
                        if found.is_some() {
                            return None;
                        }
                        found = Some(axis);
                    }
                }
                if connections == 2 { found } else { None }
            }
        }
    }
}

/// What a fluid world answers: the blocks pipes connect through and the
/// blocks at their open ends. Fluid holders answer through `FluidAccess`.
pub trait FluidWorld: FluidAccess {
    fn is_loaded(&self, pos: Pos) -> bool;
    /// The pipe or pump at `pos`, by its block state.
    fn pipe_block(&self, pos: Pos) -> Option<PipeBlock>;
    /// The block at `pos`, seen from a pipe on its side `face`.
    fn space(&self, pos: Pos, face: Dir) -> Space;
    fn change_space(&mut self, pos: Pos, change: SpaceChange);
    /// `FluidReactions.handlePipeFlowCollision`: the pipe at `pos` breaks
    /// (dropping itself) and `block`, if any, takes its place.
    fn pipe_collision(&mut self, pos: Pos, block: Option<&'static str>);
    /// The filter of the smart pipe at `pos`.
    fn pipe_filter(&self, _pos: Pos) -> PipeFilter {
        PipeFilter::Any
    }
}

/// `FilterItemStack.test` for fluids, as a smart pipe's filter answers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipeFilter {
    /// An empty filter.
    Any,
    /// An item holding this fluid.
    Only(String),
    /// An item holding no fluid: `filterFluidStack` is empty.
    Nothing,
}

impl PipeFilter {
    /// The filter of `item` (a bucket is the only item here that holds a
    /// fluid: `GenericItemEmptying`).
    pub fn of(item: Option<&str>) -> PipeFilter {
        match item {
            None => PipeFilter::Any,
            Some(item) => {
                bucket_fluid(item).map_or(PipeFilter::Nothing, |f| PipeFilter::Only(f.to_owned()))
            }
        }
    }
    /// `SmartPipeBehaviour.canPullFluidFrom`: empty passes too.
    pub fn passes(&self, fluid: &FluidStack) -> bool {
        fluid.is_empty()
            || match self {
                PipeFilter::Any => true,
                PipeFilter::Only(f) => *f == fluid.fluid,
                PipeFilter::Nothing => false,
            }
    }
}

/// `SmartFluidPipeBlock.getStateForPlacement` by
/// `FaceAttachedHorizontalDirectionalBlock`'s: the placed `face` and `facing`
/// for a click on `clicked`, `horizontal` the placer's facing and `look` their
/// looking direction along each axis, `connects` the sides with something to
/// connect to.
pub fn smart_placement(
    clicked: Dir,
    horizontal: Dir,
    look: impl Fn(Axis) -> Dir,
    connects: impl Fn(Dir) -> bool,
) -> (SmartFace, Dir) {
    let first = clicked.opposite();
    let (mut face, mut facing) = match first {
        Dir::Up => (SmartFace::Ceiling, horizontal),
        Dir::Down => (SmartFace::Floor, horizontal),
        d => (SmartFace::Wall, d.opposite()),
    };
    let mut preferred = None;
    for side in Dir::ALL {
        if !connects(side) {
            continue;
        }
        if preferred.is_some_and(|p| p != side.axis()) {
            preferred = None;
            break;
        }
        preferred = Some(side.axis());
    }
    match preferred {
        Some(Axis::Y) => {
            face = SmartFace::Wall;
            facing = facing.opposite();
        }
        Some(axis) => {
            if face == SmartFace::Wall {
                face = SmartFace::Floor;
            }
            facing = look(axis);
        }
        None => {}
    }
    (face, facing)
}

/// `AttachFace`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmartFace {
    Floor,
    Wall,
    Ceiling,
}

impl SmartFace {
    pub fn name(self) -> &'static str {
        match self {
            SmartFace::Floor => "floor",
            SmartFace::Wall => "wall",
            SmartFace::Ceiling => "ceiling",
        }
    }
    pub fn parse(s: &str) -> Option<SmartFace> {
        match s {
            "floor" => Some(SmartFace::Floor),
            "wall" => Some(SmartFace::Wall),
            "ceiling" => Some(SmartFace::Ceiling),
            _ => None,
        }
    }
}

/// `SmartFluidPipeBlock.getPipeAxis`.
pub fn smart_pipe_axis(face: SmartFace, facing: Dir) -> Axis {
    if face == SmartFace::Wall {
        Axis::Y
    } else {
        facing.axis()
    }
}

/// `SmartPipeFilterSlot`: the slot's centre in the block, before its turn
/// about Y (`smart_slot_angle`).
pub fn smart_slot(face: SmartFace) -> [f32; 3] {
    let (y, z) = match face {
        SmartFace::Ceiling => (0.55, 4.6),
        SmartFace::Wall => (11.4, 0.55),
        SmartFace::Floor => (15.45, 4.625),
    };
    [0.5, y / 16.0, z / 16.0]
}

/// `SmartPipeFilterSlot.angleY`, from the facing's horizontal angle.
pub fn smart_slot_angle(face: SmartFace, horizontal_angle: f32) -> f32 {
    if face == SmartFace::Wall {
        horizontal_angle + 180.0
    } else {
        horizontal_angle
    }
}

/// `FlowSource.OpenEndedPipe`'s state: its 1000 mB buffer and whether it was
/// last pulling.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenEnd {
    pub tank: FluidTank,
    pub was_pulling: bool,
}

impl OpenEnd {
    fn new() -> Self {
        OpenEnd {
            tank: FluidTank::new(OPEN_END_CAPACITY),
            was_pulling: false,
        }
    }
}

/// `FlowSource`: what lies beyond a connection.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    OpenEnd(OpenEnd),
    /// `FlowSource.FluidHandler`: a block with a fluid handler.
    Handler,
    /// `FlowSource.OtherPipe`.
    OtherPipe,
    Blocked,
}

impl Source {
    fn is_endpoint(&self) -> bool {
        matches!(self, Source::OpenEnd(_) | Source::Handler)
    }
}

/// `PipeConnection.Flow`.
#[derive(Clone, Debug, PartialEq)]
pub struct Flow {
    pub complete: bool,
    pub inbound: bool,
    pub progress: f32,
    pub fluid: FluidStack,
}

/// `PipeConnection`: one side of a pipe.
#[derive(Clone, Debug, PartialEq)]
pub struct Connection {
    pub side: Dir,
    /// `[inbound, outward]`.
    pub pressure: [f32; 2],
    pub source: Option<Source>,
    pub previous_source: Option<Source>,
    pub flow: Option<Flow>,
}

fn idx(inbound: bool) -> usize {
    usize::from(!inbound)
}

impl Connection {
    pub fn new(side: Dir) -> Self {
        Connection {
            side,
            pressure: [0.0; 2],
            source: None,
            previous_source: None,
            flow: None,
        }
    }
    fn inbound_pressure(&self) -> f32 {
        self.pressure[0]
    }
    fn outward_pressure(&self) -> f32 {
        self.pressure[1]
    }
    /// `comparePressure`: positive when the outward pressure is the larger.
    pub fn compare_pressure(&self) -> f32 {
        self.outward_pressure() - self.inbound_pressure()
    }
    pub fn has_pressure(&self) -> bool {
        self.inbound_pressure() != 0.0 || self.outward_pressure() != 0.0
    }
    pub fn add_pressure(&mut self, inbound: bool, pressure: f32) {
        self.pressure[idx(inbound)] += pressure;
    }
    /// `getProvidedFluid`: a complete inbound flow's fluid.
    pub fn provided_fluid(&self) -> FluidStack {
        match &self.flow {
            Some(f) if f.inbound && f.complete => f.fluid.clone(),
            _ => FluidStack::empty(),
        }
    }
    /// `provideOutboundFlow`.
    pub fn outbound_flow(&self) -> FluidStack {
        match &self.flow {
            Some(f) if f.complete && !f.inbound => f.fluid.clone(),
            _ => FluidStack::empty(),
        }
    }
    /// `flipFlowsIfPressureReversed`.
    fn flip_flows_if_pressure_reversed(&mut self) -> bool {
        let cmp = self.compare_pressure();
        let single =
            cmp != 0.0 && (self.inbound_pressure() == 0.0 || self.outward_pressure() == 0.0);
        let Some(flow) = self.flow.as_mut() else {
            return false;
        };
        if !single || (cmp < 0.0) == flow.inbound {
            return false;
        }
        flow.inbound = !flow.inbound;
        if !flow.complete {
            self.flow = None;
        }
        true
    }
    /// `tryStartingNewFlow`.
    fn try_starting_new_flow(&mut self, inbound: bool, provided: FluidStack) -> bool {
        if provided.is_empty() {
            return false;
        }
        self.flow = Some(Flow {
            complete: false,
            inbound,
            progress: 0.0,
            fluid: provided,
        });
        true
    }
    /// `tickFlowProgress` (server side; particles aren't ported).
    fn tick_flow_progress(&mut self) {
        let pressure = self.pressure;
        let Some(flow) = self.flow.as_mut() else {
            return;
        };
        if flow.fluid.is_empty() {
            return;
        }
        let speed =
            1.0 / 32.0 + (pressure[idx(flow.inbound)] / 128.0).clamp(0.0, 1.0) * 31.0 / 32.0;
        flow.progress = (flow.progress + speed).min(1.0);
        if flow.progress >= 1.0 {
            flow.complete = true;
        }
    }
}

/// `FluidTransportBehaviour.UpdatePhase`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    WaitForPumps,
    FlipFlows,
    Idle,
}

/// `FluidTransportBehaviour` of a pipe or pump.
#[derive(Clone, Debug, PartialEq)]
pub struct Transport {
    /// The block it was made for (`onRemove` walks from its old state).
    pub block: PipeBlock,
    pub phase: Phase,
    /// Its connections, by side.
    pub interfaces: [Option<Connection>; 6],
}

impl Transport {
    pub fn new(block: PipeBlock) -> Self {
        let mut t = Transport {
            block,
            phase: Phase::WaitForPumps,
            interfaces: Default::default(),
        };
        t.create_connection_data();
        t
    }
    fn create_connection_data(&mut self) {
        for d in Dir::ALL {
            if self.block.open(d) {
                self.interfaces[d as usize] = Some(Connection::new(d));
            }
        }
    }
    pub fn connection(&self, d: Dir) -> Option<&Connection> {
        self.interfaces[d as usize].as_ref()
    }
    fn connection_mut(&mut self, d: Dir) -> Option<&mut Connection> {
        self.interfaces[d as usize].as_mut()
    }
    /// `hasAnyPressure`.
    pub fn has_any_pressure(&self) -> bool {
        self.interfaces
            .iter()
            .flatten()
            .any(Connection::has_pressure)
    }
    fn sides(&self) -> Vec<Dir> {
        Dir::ALL
            .into_iter()
            .filter(|d| self.interfaces[*d as usize].is_some())
            .collect()
    }
}

/// `PumpBlockEntity`'s own state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pump {
    /// `sidesToUpdate`: `[back, front]`.
    pub sides_to_update: [bool; 2],
    /// The speed it last had (`onSpeedChanged`), none before the first.
    pub speed: Option<f32>,
}

/// `FluidNetwork`: from an endpoint feeding a pipe, the endpoints its flows
/// reach, and the transfer between them.
#[derive(Clone, Debug, PartialEq)]
pub struct Network {
    start: (Pos, Dir),
    transfer_speed: i32,
    pause_before_propagation: i32,
    queued: Vec<(Pos, Dir)>,
    frontier: Vec<(Pos, Dir)>,
    visited: HashSet<Pos>,
    fluid: FluidStack,
    targets: Vec<(Pos, Dir)>,
}

impl Network {
    fn new(start: (Pos, Dir)) -> Self {
        let mut n = Network {
            start,
            transfer_speed: 0,
            pause_before_propagation: 0,
            queued: Vec::new(),
            frontier: Vec::new(),
            visited: HashSet::new(),
            fluid: FluidStack::empty(),
            targets: Vec::new(),
        };
        n.reset();
        n
    }
    /// `FluidNetwork.reset`.
    pub fn reset(&mut self) {
        self.frontier.clear();
        self.visited.clear();
        self.targets.clear();
        self.queued.clear();
        self.fluid = FluidStack::empty();
        self.queued.push(self.start);
        self.pause_before_propagation = 2;
    }
}

/// Where fluid goes in or out: the handler of a block beyond a pipe's side,
/// or a pipe's open end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Target {
    /// The block at the position, reached on that side.
    Handler(Pos, Dir),
    /// The open end of the pipe at the position, on that side.
    OpenEnd(Pos, Dir),
}

impl Target {
    /// The handler's identity (`accumulatedFill` is an identity map: a
    /// block's handler is the same from every side).
    fn key(self) -> (Pos, Option<Dir>) {
        match self {
            Target::Handler(p, _) => (p, None),
            Target::OpenEnd(p, d) => (p, Some(d)),
        }
    }
}

/// Every pipe and pump of a dimension: their transport behaviours, the
/// networks their connections run, and the pumps' own state.
#[derive(Clone, Debug, Default)]
pub struct Fluids {
    pub transports: HashMap<Pos, Transport>,
    networks: HashMap<(Pos, Dir), Network>,
    pub pumps: HashMap<Pos, Pump>,
}

impl Fluids {
    pub fn new() -> Self {
        Self::default()
    }

    /// A pipe or pump appeared (`onPlace`'s scheduled tick), changed state
    /// or went away (`onRemove`), as the world now says.
    pub fn block_changed(&mut self, world: &mut dyn FluidWorld, pos: Pos) {
        let now = world.pipe_block(pos);
        let before = self.transports.get(&pos).map(|t| t.block);
        match (before, now) {
            (None, None) => self.neighbour_changed(world, pos),
            (Some(old), None) => {
                self.propagate_changed_pipe(world, pos, Some(old));
                self.remove(pos);
                self.neighbour_changed(world, pos);
            }
            // `onPlace`'s tick, or `onFilterChanged`.
            (Some(old), Some(new)) if old == new => {
                self.propagate_changed_pipe(world, pos, Some(new))
            }
            (Some(old), Some(new)) => {
                let pump = |b| matches!(b, PipeBlock::Pump(_));
                if pump(old) != pump(new) {
                    self.remove(pos);
                    self.transports.insert(pos, Transport::new(new));
                } else if !pump(new) {
                    // The same block entity, or `cacheFlows` and `loadFlows`
                    // between a pipe, its glass and its casing; the sides
                    // follow as `wipePressure` has them.
                    if let Some(t) = self.transports.get_mut(&pos) {
                        t.block = new;
                        for d in Dir::ALL {
                            if !new.open(d) {
                                t.interfaces[d as usize] = None;
                            } else if t.interfaces[d as usize].is_none() {
                                t.interfaces[d as usize] = Some(Connection::new(d));
                            }
                        }
                    }
                } else if let Some(t) = self.transports.get_mut(&pos) {
                    t.block = new;
                }
                self.propagate_changed_pipe(world, pos, Some(new));
                if let PipeBlock::Pump(_) = new {
                    // A pump turned round: `onSpeedChanged` won't hear it.
                    self.update_pressure_change(world, pos);
                }
            }
            (None, Some(new)) => {
                self.transports.insert(pos, Transport::new(new));
                if let PipeBlock::Pump(_) = new {
                    self.pumps.entry(pos).or_default();
                }
                self.propagate_changed_pipe(world, pos, Some(new));
            }
        }
    }

    /// The flows of the glass pipes that have any.
    pub fn glass_streams(&self) -> Vec<(Pos, Vec<PipeStream>)> {
        self.transports
            .iter()
            .filter(|(_, t)| matches!(t.block, PipeBlock::Glass(_)))
            .filter_map(|(p, t)| {
                let streams: Vec<PipeStream> = t
                    .interfaces
                    .iter()
                    .flatten()
                    .filter_map(|c| {
                        let f = c.flow.as_ref().filter(|f| !f.fluid.is_empty())?;
                        Some(PipeStream {
                            side: c.side,
                            inbound: f.inbound,
                            progress: (f.progress.clamp(0.0, 1.0) * 255.0) as u8,
                            fluid: FluidStack::new(&f.fluid.fluid, 1),
                        })
                    })
                    .collect();
                (!streams.is_empty()).then_some((*p, streams))
            })
            .collect()
    }

    fn remove(&mut self, pos: Pos) {
        self.transports.remove(&pos);
        self.pumps.remove(&pos);
        self.networks.retain(|(p, _), _| *p != pos);
    }

    /// `FluidPipeBlock.neighborChanged` / `PumpBlock.neighborChanged` with
    /// `validateNeighbourChange`: a block that is no pipe, pump or liquid
    /// changed at `pos`; straight pipes open toward it look again. The
    /// caller skips liquid changes.
    pub fn neighbour_changed(&mut self, world: &mut dyn FluidWorld, pos: Pos) {
        if world.pipe_block(pos).is_some() {
            return;
        }
        for d in Dir::ALL {
            let pipe = offset(pos, d);
            let Some(block) = self.transports.get(&pipe).map(|t| t.block) else {
                continue;
            };
            if !block.hears_neighbours() || !block.open(d.opposite()) {
                continue;
            }
            self.propagate_changed_pipe(world, pipe, None);
        }
    }

    /// The pipe-like block at `pos` with a transport behaviour
    /// (`FluidPropagator.getPipe`), by its state now.
    fn pipe(&self, world: &dyn FluidWorld, pos: Pos) -> Option<PipeBlock> {
        self.transports.get(&pos)?;
        world.pipe_block(pos)
    }

    /// `FluidPropagator.propagateChangedPipe`: every pipe within pump range
    /// of `start` loses its pressure, and the pumps found there
    /// redistribute theirs. `state` stands for the block at `start`.
    pub fn propagate_changed_pipe(
        &mut self,
        world: &mut dyn FluidWorld,
        start: Pos,
        state: Option<PipeBlock>,
    ) {
        let mut frontier: Vec<(i32, Pos)> = vec![(0, start)];
        let mut visited: HashSet<Pos> = HashSet::new();
        let mut pumps: Vec<(Pos, Dir)> = Vec::new();
        while !frontier.is_empty() {
            let (distance, current) = frontier.remove(0);
            if !visited.insert(current) {
                continue;
            }
            let current_state = if current == start {
                state.or_else(|| world.pipe_block(current))
            } else {
                world.pipe_block(current)
            };
            if !self.transports.contains_key(&current) {
                continue;
            }
            let Some(current_state) = current_state else {
                continue;
            };
            self.wipe_pressure(world, current);
            for d in Dir::ALL.into_iter().filter(|d| current_state.open(*d)) {
                let target = offset(current, d);
                if !world.is_loaded(target) {
                    continue;
                }
                if self.pumps.contains_key(&target) {
                    let facing_along = matches!(world.pipe_block(target), Some(PipeBlock::Pump(f)) if f.axis() == d.axis());
                    if facing_along && !pumps.contains(&(target, d.opposite())) {
                        pumps.push((target, d.opposite()));
                    }
                    continue;
                }
                if visited.contains(&target) {
                    continue;
                }
                let Some(target_state) = self.pipe(world, target) else {
                    continue;
                };
                let pressured = self
                    .transports
                    .get(&target)
                    .is_some_and(Transport::has_any_pressure);
                if distance >= PUMP_RANGE && !pressured {
                    continue;
                }
                if target_state.open(d.opposite()) {
                    frontier.push((distance + 1, target));
                }
            }
        }
        for (pump, side) in pumps {
            self.update_pipes_on_side(world, pump, side);
        }
    }

    /// `FluidTransportBehaviour.wipePressure`.
    fn wipe_pressure(&mut self, world: &dyn FluidWorld, pos: Pos) {
        let block = world.pipe_block(pos);
        let Some(t) = self.transports.get_mut(&pos) else {
            return;
        };
        if let Some(block) = block {
            t.block = block;
            for d in Dir::ALL {
                if !block.open(d) {
                    t.interfaces[d as usize] = None;
                } else if t.interfaces[d as usize].is_none() {
                    t.interfaces[d as usize] = Some(Connection::new(d));
                }
            }
        }
        t.phase = Phase::WaitForPumps;
        for c in t.interfaces.iter_mut().flatten() {
            // PipeConnection.wipePressure
            c.pressure = [0.0; 2];
            if c.source.is_some() {
                c.previous_source = c.source.take();
            }
            if let Some(n) = self.networks.get_mut(&(pos, c.side)) {
                n.reset();
            }
        }
    }

    /// `FluidPropagator.resetAffectedFluidNetworks`.
    fn reset_affected_networks(&mut self, start: Pos, side: Dir) {
        let mut frontier = vec![start];
        let mut visited = HashSet::new();
        while !frontier.is_empty() {
            let pos = frontier.remove(0);
            if !visited.insert(pos) {
                continue;
            }
            let Some(t) = self.transports.get(&pos) else {
                continue;
            };
            let mut reset = Vec::new();
            for d in Dir::ALL {
                if pos == start && d != side {
                    continue;
                }
                let target = offset(pos, d);
                if visited.contains(&target) {
                    continue;
                }
                let Some(c) = t.connection(d) else {
                    continue;
                };
                if !c.flow.as_ref().is_some_and(|f| f.inbound) {
                    continue;
                }
                reset.push(d);
                frontier.push(target);
            }
            for d in reset {
                if let Some(n) = self.networks.get_mut(&(pos, d)) {
                    n.reset();
                }
            }
        }
    }

    /// `FluidPropagator.isOpenEnd`.
    fn is_open_end(&self, world: &dyn FluidWorld, pos: Pos, side: Dir) -> bool {
        let connected = offset(pos, side);
        if self
            .pipe(world, connected)
            .is_some_and(|b| b.open(side.opposite()))
        {
            return false;
        }
        if let Some(PipeBlock::Pump(f)) = world.pipe_block(connected)
            && f.axis() == side.axis()
        {
            return false;
        }
        let space = world.space(connected, side.opposite());
        if provides_without_capability(&space) {
            return true;
        }
        if space.solid_side && !space.fan_transparent {
            return false;
        }
        if world.has_handler(connected, side.opposite()) {
            return false;
        }
        if !(space.replaceable && space.breakable) && !space.waterloggable {
            return false;
        }
        true
    }

    /// `PipeConnection.determineSource`.
    fn determine_source(&mut self, world: &dyn FluidWorld, pos: Pos, side: Dir) -> bool {
        let relative = offset(pos, side);
        if !world.is_loaded(relative) {
            return false;
        }
        let source = if self.is_open_end(world, pos, side) {
            let previous = self
                .transports
                .get(&pos)
                .and_then(|t| t.connection(side))
                .and_then(|c| c.previous_source.clone());
            match previous {
                Some(s @ Source::OpenEnd(_)) => s,
                _ => Source::OpenEnd(OpenEnd::new()),
            }
        } else if world.has_handler(relative, side.opposite()) {
            Source::Handler
        } else if self.transports.contains_key(&relative) {
            Source::OtherPipe
        } else {
            Source::Blocked
        };
        if let Some(c) = self
            .transports
            .get_mut(&pos)
            .and_then(|t| t.connection_mut(side))
        {
            c.source = Some(source);
        }
        true
    }

    /// `FlowSource.provideHandler` of the connection at `pos`, `side`.
    fn handler_of(&self, world: &dyn FluidWorld, pos: Pos, side: Dir) -> Option<Target> {
        let c = self.transports.get(&pos)?.connection(side)?;
        match c.source.as_ref()? {
            Source::OpenEnd(_) => Some(Target::OpenEnd(pos, side)),
            Source::Handler => {
                let connected = offset(pos, side);
                world
                    .has_handler(connected, side.opposite())
                    .then_some(Target::Handler(connected, side.opposite()))
            }
            _ => None,
        }
    }

    fn open_end_mut(&mut self, pos: Pos, side: Dir) -> Option<&mut OpenEnd> {
        match self
            .transports
            .get_mut(&pos)?
            .connection_mut(side)?
            .source
            .as_mut()?
        {
            Source::OpenEnd(o) => Some(o),
            _ => None,
        }
    }

    fn target_tanks(&self, world: &dyn FluidWorld, t: Target) -> Vec<FluidStack> {
        match t {
            Target::Handler(p, s) => world.handler_tanks(p, s),
            Target::OpenEnd(p, s) => {
                let c = self.transports.get(&p).and_then(|t| t.connection(s));
                match c.and_then(|c| c.source.as_ref()) {
                    Some(Source::OpenEnd(o)) => vec![o.tank.fluid.clone()],
                    _ => Vec::new(),
                }
            }
        }
    }

    fn target_fill(
        &mut self,
        world: &mut dyn FluidWorld,
        t: Target,
        resource: &FluidStack,
        simulate: bool,
    ) -> i32 {
        match t {
            Target::Handler(p, s) => world.fill(p, s, resource, simulate),
            Target::OpenEnd(p, s) => {
                let Some(mut o) = self.open_end_mut(p, s).map(std::mem::take) else {
                    return 0;
                };
                let r = open_end_fill(
                    &mut o,
                    world,
                    offset(p, s),
                    s.opposite(),
                    resource,
                    simulate,
                );
                if let Some(slot) = self.open_end_mut(p, s) {
                    *slot = o;
                }
                r
            }
        }
    }

    fn target_drain(
        &mut self,
        world: &mut dyn FluidWorld,
        t: Target,
        amount: i32,
        filter: Option<&FluidStack>,
        simulate: bool,
    ) -> FluidStack {
        match t {
            Target::Handler(p, s) => match filter {
                Some(f) => world.drain(p, s, &f.with_amount(amount), simulate),
                None => world.drain_any(p, s, amount, simulate),
            },
            Target::OpenEnd(p, s) => {
                let Some(mut o) = self.open_end_mut(p, s).map(std::mem::take) else {
                    return FluidStack::empty();
                };
                let r = open_end_drain(
                    &mut o,
                    world,
                    offset(p, s),
                    s.opposite(),
                    amount,
                    filter,
                    simulate,
                );
                if let Some(slot) = self.open_end_mut(p, s) {
                    *slot = o;
                }
                r
            }
        }
    }

    /// `FlowSource.provideFluid` of the connection at `pos`, `side`, through
    /// `canPullFluidFrom`: anything, but nothing through a closed valve.
    fn provide_fluid(&mut self, world: &mut dyn FluidWorld, pos: Pos, side: Dir) -> FluidStack {
        let block = self.transports.get(&pos).map(|t| t.block);
        if matches!(block, Some(PipeBlock::Valve(_, false))) {
            return FluidStack::empty();
        }
        let filter = if matches!(block, Some(PipeBlock::Smart(_))) {
            world.pipe_filter(pos)
        } else {
            PipeFilter::Any
        };
        let source = self
            .transports
            .get(&pos)
            .and_then(|t| t.connection(side))
            .and_then(|c| c.source.as_ref())
            .map(|s| match s {
                Source::OpenEnd(_) => 0,
                Source::Handler => 1,
                Source::OtherPipe => 2,
                Source::Blocked => 3,
            });
        match source {
            Some(2) => {
                // OtherPipe: what the pipe beyond sends out toward us.
                let other = offset(pos, side);
                let fluid = self
                    .transports
                    .get(&other)
                    .and_then(|t| t.connection(side.opposite()))
                    .map_or_else(FluidStack::empty, Connection::outbound_flow);
                if filter.passes(&fluid) {
                    fluid
                } else {
                    FluidStack::empty()
                }
            }
            Some(0 | 1) => {
                let Some(t) = self.handler_of(world, pos, side) else {
                    return FluidStack::empty();
                };
                // `tank.drain(1, SIMULATE)` if the predicate accepts it, else
                // the first of its tanks' fluids that it does.
                let fluid = self.target_drain(world, t, 1, None, true);
                if filter.passes(&fluid) {
                    return fluid;
                }
                match &filter {
                    PipeFilter::Only(f) => {
                        let wanted = FluidStack::new(f, 1);
                        self.target_drain(world, t, 1, Some(&wanted), true)
                    }
                    _ => FluidStack::empty(),
                }
            }
            _ => FluidStack::empty(),
        }
    }

    // ---- Pumps --------------------------------------------------------------

    /// `PumpBlockEntity.onSpeedChanged` with the pump's speed this tick.
    pub fn pump_speed(&mut self, world: &mut dyn FluidWorld, pos: Pos, speed: f32) {
        if let std::collections::hash_map::Entry::Vacant(e) = self.transports.entry(pos) {
            let Some(block @ PipeBlock::Pump(_)) = world.pipe_block(pos) else {
                return;
            };
            e.insert(Transport::new(block));
        }
        let pump = self.pumps.entry(pos).or_default();
        let previous = pump.speed.replace(speed);
        if previous.is_some_and(|p| p.abs() == speed.abs()) {
            return;
        }
        if previous.is_none() && speed == 0.0 {
            return;
        }
        self.update_pressure_change(world, pos);
    }

    fn pump_speed_of(&self, pos: Pos) -> f32 {
        self.pumps.get(&pos).and_then(|p| p.speed).unwrap_or(0.0)
    }

    /// `PumpBlockEntity.updatePressureChange`.
    fn update_pressure_change(&mut self, world: &mut dyn FluidWorld, pos: Pos) {
        let Some(PipeBlock::Pump(front)) = world.pipe_block(pos) else {
            return;
        };
        let front_pos = offset(pos, front);
        let back_pos = offset(pos, front.opposite());
        self.propagate_changed_pipe(world, front_pos, None);
        self.propagate_changed_pipe(world, back_pos, None);
        self.wipe_pressure(world, pos);
        if let Some(p) = self.pumps.get_mut(&pos) {
            p.sides_to_update = [true, true];
        }
    }

    /// `PumpBlockEntity.updatePipesOnSide`.
    fn update_pipes_on_side(&mut self, world: &mut dyn FluidWorld, pos: Pos, side: Dir) {
        let Some(PipeBlock::Pump(front)) = world.pipe_block(pos) else {
            return;
        };
        if side.axis() != front.axis() {
            return;
        }
        if let Some(p) = self.pumps.get_mut(&pos) {
            p.sides_to_update[usize::from(side == front)] = true;
        }
        self.wipe_pressure(world, pos);
    }

    /// `PumpBlockEntity.hasReachedValidEndpoint`.
    fn reached_valid_endpoint(
        &self,
        world: &dyn FluidWorld,
        pos: Pos,
        face: Dir,
        pull: bool,
    ) -> bool {
        let connected = offset(pos, face);
        if let Some(PipeBlock::Pump(f)) = world.pipe_block(connected)
            && f.axis() == face.axis()
            && self.pumps.contains_key(&connected)
        {
            // isPullingOnSide(isFront(opposite)) != pull
            return (face.opposite() != f) != pull;
        }
        if self
            .pipe(world, connected)
            .is_some_and(|b| b.open(face.opposite()))
        {
            return false;
        }
        if world.has_handler(connected, face.opposite()) {
            return true;
        }
        self.is_open_end(world, pos, face)
    }

    /// `PumpBlockEntity.distributePressureTo`: pressure on every face on a
    /// way from the pump's `side` to an endpoint, split between parallel
    /// branches.
    fn distribute_pressure_to(&mut self, world: &mut dyn FluidWorld, pos: Pos, side: Dir) {
        let speed = self.pump_speed_of(pos);
        if speed == 0.0 {
            return;
        }
        let Some(PipeBlock::Pump(front)) = world.pipe_block(pos) else {
            return;
        };
        let start = (pos, side);
        let start_connected = offset(pos, side);
        // isPullingOnSide(isFront(side)): pumps pull at the back.
        let pull = side != front;
        let mut targets: HashSet<(Pos, Dir)> = HashSet::new();
        let mut graph: HashMap<Pos, (i32, [Option<bool>; 6])> = HashMap::new();
        let note = |graph: &mut HashMap<Pos, (i32, [Option<bool>; 6])>,
                    p: Pos,
                    distance: i32,
                    d: Dir,
                    v: bool| {
            graph.entry(p).or_insert((distance, [None; 6])).1[d as usize] = Some(v);
        };
        if !pull {
            self.reset_affected_networks(pos, side.opposite());
        }
        if !self.reached_valid_endpoint(world, pos, side, pull) {
            note(&mut graph, pos, 0, side, pull);
            note(&mut graph, start_connected, 1, side.opposite(), !pull);
            let mut frontier: Vec<(i32, Pos)> = vec![(1, start_connected)];
            let mut visited: HashSet<Pos> = HashSet::new();
            while !frontier.is_empty() {
                let (distance, current) = frontier.remove(0);
                if !world.is_loaded(current) || !visited.insert(current) {
                    continue;
                }
                let Some(current_state) = self.pipe(world, current) else {
                    continue;
                };
                for face in Dir::ALL.into_iter().filter(|d| current_state.open(*d)) {
                    let connected = offset(current, face);
                    if !world.is_loaded(connected) {
                        continue;
                    }
                    // blockFace.isEquivalent(start)
                    if (current, face) == start || (connected, face.opposite()) == start {
                        continue;
                    }
                    if self.reached_valid_endpoint(world, current, face, pull) {
                        note(&mut graph, current, distance, face, pull);
                        targets.insert((current, face));
                        continue;
                    }
                    if !self.transports.contains_key(&connected)
                        || self.pumps.contains_key(&connected)
                    {
                        continue;
                    }
                    if visited.contains(&connected) {
                        continue;
                    }
                    if distance + 1 >= PUMP_RANGE {
                        note(&mut graph, current, distance, face, pull);
                        targets.insert((current, face));
                        continue;
                    }
                    note(&mut graph, current, distance, face, pull);
                    note(&mut graph, connected, distance + 1, face.opposite(), !pull);
                    frontier.push((distance + 1, connected));
                }
            }
        }
        let mut valid: HashMap<i32, HashSet<(Pos, Dir)>> = HashMap::new();
        search_for_endpoint(&graph, &targets, &mut valid, (pos, side.opposite()), pull);
        let pressure = speed.abs();
        for set in valid.values() {
            let parallel = (set.len().max(2) - 1) as f32;
            for &(p, d) in set {
                if p == pos {
                    continue;
                }
                let Some(inbound) = graph.get(&p).and_then(|(_, m)| m[d as usize]) else {
                    continue;
                };
                if let Some(c) = self
                    .transports
                    .get_mut(&p)
                    .and_then(|t| t.connection_mut(d))
                {
                    c.add_pressure(inbound, pressure / parallel);
                }
            }
        }
    }

    // ---- Ticks -----------------------------------------------------------------

    /// One server tick of every pipe and pump (`FluidTransportBehaviour.tick`,
    /// then `PumpFluidTransferBehaviour`'s pressure and the pump's
    /// `sidesToUpdate`), in position order.
    pub fn tick(&mut self, world: &mut dyn FluidWorld) {
        let mut positions: Vec<Pos> = self.transports.keys().copied().collect();
        positions.sort();
        for pos in positions {
            if !world.is_loaded(pos) || !self.transports.contains_key(&pos) {
                continue;
            }
            self.tick_transport(world, pos);
            if let Some(PipeBlock::Pump(front)) = self.transports.get(&pos).map(|t| t.block) {
                let speed = self.pump_speed_of(pos).abs();
                if let Some(t) = self.transports.get_mut(&pos) {
                    for c in t.interfaces.iter_mut().flatten() {
                        let pull = c.side != front;
                        c.pressure[idx(pull)] = speed;
                        c.pressure[idx(!pull)] = 0.0;
                    }
                }
                let update = self
                    .pumps
                    .get_mut(&pos)
                    .map(|p| std::mem::take(&mut p.sides_to_update));
                if let Some([back, front_update]) = update {
                    if back {
                        self.distribute_pressure_to(world, pos, front.opposite());
                    }
                    if front_update {
                        self.distribute_pressure_to(world, pos, front);
                    }
                }
            }
        }
    }

    /// `FluidTransportBehaviour.tick` on the server.
    fn tick_transport(&mut self, world: &mut dyn FluidWorld, pos: Pos) {
        let Some(t) = self.transports.get_mut(&pos) else {
            return;
        };
        if t.phase == Phase::WaitForPumps {
            t.phase = Phase::FlipFlows;
            return;
        }
        let sides = t.sides();
        for &d in &sides {
            if let Some(c) = self
                .transports
                .get_mut(&pos)
                .and_then(|t| t.connection_mut(d))
            {
                c.flip_flows_if_pressure_reversed();
            }
            // manageSource: determine it when unknown (the handlers need no
            // cache here).
            if self
                .transports
                .get(&pos)
                .and_then(|t| t.connection(d))
                .is_some_and(|c| c.source.is_none())
            {
                self.determine_source(world, pos, d);
            }
        }
        if let Some(t) = self.transports.get_mut(&pos)
            && t.phase == Phase::FlipFlows
        {
            t.phase = Phase::Idle;
            return;
        }
        let mut available = FluidStack::empty();
        let mut colliding = FluidStack::empty();
        let mut single: Option<Dir> = None;
        if let Some(t) = self.transports.get(&pos) {
            for c in t.interfaces.iter().flatten() {
                let fluid = c.provided_fluid();
                if fluid.is_empty() {
                    continue;
                }
                if available.is_empty() {
                    single = Some(c.side);
                    available = fluid;
                    continue;
                }
                if available.same(&fluid) {
                    single = None;
                    available = fluid;
                    continue;
                }
                colliding = fluid;
                break;
            }
        }
        if !colliding.is_empty() {
            let block = flow_collision_block(&available.fluid, &colliding.fluid);
            world.pipe_collision(pos, block);
            return;
        }
        for &d in &sides {
            let internal = if single != Some(d) {
                available.clone()
            } else {
                FluidStack::empty()
            };
            self.manage_flows(world, pos, d, internal);
        }
        if let Some(t) = self.transports.get_mut(&pos) {
            for c in t.interfaces.iter_mut().flatten() {
                c.tick_flow_progress();
            }
        }
    }

    /// `PipeConnection.manageFlows`.
    fn manage_flows(
        &mut self,
        world: &mut dyn FluidWorld,
        pos: Pos,
        side: Dir,
        internal: FluidStack,
    ) -> bool {
        let retained = self.networks.remove(&(pos, side));
        let has_source = self
            .transports
            .get(&pos)
            .and_then(|t| t.connection(side))
            .is_some_and(|c| c.source.is_some());
        if !has_source && !self.determine_source(world, pos, side) {
            return false;
        }
        let Some(c) = self.transports.get(&pos).and_then(|t| t.connection(side)) else {
            return false;
        };
        let (has_pressure, cmp, pressure) = (c.has_pressure(), c.compare_pressure(), c.pressure);
        let flow = c.flow.clone();
        let endpoint = c.source.as_ref().is_some_and(Source::is_endpoint);
        let Some(flow) = flow else {
            if !has_pressure {
                return false;
            }
            let prioritize_inbound = cmp < 0.0;
            for tf in [true, false] {
                let inbound = prioritize_inbound == tf;
                if pressure[idx(inbound)] == 0.0 {
                    continue;
                }
                let provided = if inbound {
                    self.provide_fluid(world, pos, side)
                } else {
                    internal.clone()
                };
                if let Some(c) = self
                    .transports
                    .get_mut(&pos)
                    .and_then(|t| t.connection_mut(side))
                    && c.try_starting_new_flow(inbound, provided)
                {
                    return true;
                }
            }
            return false;
        };
        let provided = if flow.inbound {
            self.provide_fluid(world, pos, side)
        } else {
            internal.clone()
        };
        if !has_pressure || provided.is_empty() || !provided.same(&flow.fluid) {
            if let Some(c) = self
                .transports
                .get_mut(&pos)
                .and_then(|t| t.connection_mut(side))
            {
                c.flow = None;
            }
            return true;
        }
        if flow.inbound != (cmp < 0.0) {
            let inbound = !flow.inbound;
            if (inbound && !provided.is_empty()) || (!inbound && !internal.is_empty()) {
                self.reset_affected_networks(pos, side);
                let provided = if inbound {
                    self.provide_fluid(world, pos, side)
                } else {
                    internal
                };
                if let Some(c) = self
                    .transports
                    .get_mut(&pos)
                    .and_then(|t| t.connection_mut(side))
                {
                    c.try_starting_new_flow(inbound, provided);
                }
                return true;
            }
        }
        if !endpoint || !flow.inbound {
            return false;
        }
        // Layer III
        let mut network = retained.unwrap_or_else(|| Network::new((pos, side)));
        self.tick_network(world, &mut network);
        self.networks.insert((pos, side), network);
        false
    }

    /// `FluidNetwork.tick`.
    fn tick_network(&mut self, world: &mut dyn FluidWorld, n: &mut Network) {
        if n.pause_before_propagation > 0 {
            n.pause_before_propagation -= 1;
            return;
        }
        for _ in 0..CYCLES_PER_TICK {
            let mut should_continue = false;
            for face in std::mem::take(&mut n.queued) {
                if !world.is_loaded(face.0) {
                    // `isPresent` false: it stays queued.
                    n.queued.push(face);
                    continue;
                }
                if let Some(c) = self
                    .transports
                    .get(&face.0)
                    .and_then(|t| t.connection(face.1))
                {
                    if face == n.start {
                        n.transfer_speed = (c.pressure[0] / 2.0).max(1.0) as i32;
                    }
                    if !n.frontier.contains(&face) {
                        n.frontier.push(face);
                    }
                }
            }
            let mut kept = Vec::new();
            for face in std::mem::take(&mut n.frontier) {
                let Some(c) = self
                    .transports
                    .get(&face.0)
                    .and_then(|t| t.connection(face.1))
                else {
                    // The pipe is gone: Create keeps its old connection.
                    continue;
                };
                let Some(flow) = c.flow.clone() else {
                    kept.push(face);
                    continue;
                };
                if !n.fluid.is_empty() && !flow.fluid.same(&n.fluid) {
                    continue;
                }
                if !flow.inbound {
                    if c.compare_pressure() < 0.0 {
                        kept.push(face);
                    }
                    continue;
                }
                if !flow.complete {
                    kept.push(face);
                    continue;
                }
                if n.fluid.is_empty() {
                    n.fluid = flow.fluid.clone();
                }
                let mut can_remove = true;
                for side in Dir::ALL {
                    if side == face.1 {
                        continue;
                    }
                    let adjacent_location = (face.0, side);
                    let Some(adjacent) = self
                        .transports
                        .get(&face.0)
                        .and_then(|t| t.connection(side))
                    else {
                        continue;
                    };
                    let Some(out) = adjacent.flow.as_ref() else {
                        // Branch could potentially still appear
                        if adjacent.has_pressure() && adjacent.pressure[1] > 0.0 {
                            can_remove = false;
                        }
                        continue;
                    };
                    if out.inbound {
                        if adjacent.compare_pressure() > 0.0 {
                            can_remove = false;
                        }
                        continue;
                    }
                    if !out.complete {
                        can_remove = false;
                        continue;
                    }
                    // Give pipe end a chance to init connections
                    if adjacent.source.is_none() && !self.determine_source(world, face.0, side) {
                        can_remove = false;
                        continue;
                    }
                    let endpoint = self
                        .transports
                        .get(&face.0)
                        .and_then(|t| t.connection(side))
                        .and_then(|c| c.source.as_ref())
                        .is_some_and(Source::is_endpoint);
                    if endpoint {
                        n.targets.push(adjacent_location);
                        continue;
                    }
                    let connected = offset(face.0, side);
                    if n.visited.insert(connected) {
                        n.queued.push((connected, side.opposite()));
                        should_continue = true;
                    }
                }
                if !can_remove {
                    kept.push(face);
                }
            }
            n.frontier = kept;
            if !should_continue {
                break;
            }
        }

        let Some(source) = self.handler_of(world, n.start.0, n.start.1) else {
            return;
        };
        if n.targets.is_empty() {
            return;
        }
        let mut flow_speed = n.transfer_speed;
        let mut accumulated: HashMap<(Pos, Option<Dir>), i32> = HashMap::new();
        for simulate in [true, false] {
            let mut transfer = FluidStack::empty();
            for contained in self.target_tanks(world, source) {
                if contained.is_empty() || !contained.same(&n.fluid) {
                    continue;
                }
                transfer = self.target_drain(world, source, flow_speed, Some(&contained), simulate);
                break;
            }
            if transfer.is_empty() {
                let generic = self.target_drain(world, source, flow_speed, None, simulate);
                if !generic.is_empty() && generic.same(&n.fluid) {
                    transfer = generic;
                }
            }
            if transfer.is_empty() {
                return;
            }
            if simulate {
                flow_speed = transfer.amount;
            }
            let mut available: Vec<(Pos, Dir)> = n.targets.clone();
            while !available.is_empty() && transfer.amount > 0 {
                let divided = transfer.amount / available.len() as i32;
                let mut remainder = transfer.amount % available.len() as i32;
                let mut i = 0;
                while i < available.len() {
                    let face = available[i];
                    let mut to_transfer = divided;
                    if remainder > 0 {
                        to_transfer += 1;
                        remainder -= 1;
                    }
                    if transfer.is_empty() {
                        break;
                    }
                    let Some(target) = self.handler_of(world, face.0, face.1) else {
                        available.remove(i);
                        continue;
                    };
                    let mut simulated = to_transfer;
                    if simulate {
                        simulated += accumulated.get(&target.key()).copied().unwrap_or(0);
                    }
                    let mut fill =
                        self.target_fill(world, target, &transfer.with_amount(simulated), simulate);
                    if simulate {
                        accumulated.insert(target.key(), fill);
                        fill -= simulated - to_transfer;
                    }
                    transfer.amount -= fill;
                    if fill < simulated {
                        available.remove(i);
                        continue;
                    }
                    i += 1;
                }
            }
            flow_speed -= transfer.amount;
        }
    }

    // ---- Saving -----------------------------------------------------------

    /// The pipes and pumps there are (their flows and pressure come back
    /// from the pumps after loading).
    pub fn save(&self) -> String {
        let mut lines: Vec<String> = self
            .transports
            .iter()
            .filter(|(p, _)| !self.pumps.contains_key(*p))
            .map(|(p, t)| {
                let open: String = Dir::ALL
                    .into_iter()
                    .map(|d| if t.block.open(d) { '1' } else { '0' })
                    .collect();
                format!("pipe {} {} {} {open}", p[0], p[1], p[2])
            })
            .collect();
        lines.sort();
        lines.join("\n")
    }

    pub fn load(text: &str) -> Fluids {
        let mut f = Fluids::new();
        for line in text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() != 5 || parts[0] != "pipe" {
                continue;
            }
            let (Ok(x), Ok(y), Ok(z)) = (parts[1].parse(), parts[2].parse(), parts[3].parse())
            else {
                continue;
            };
            let mut open = [false; 6];
            for (i, c) in parts[4].chars().take(6).enumerate() {
                open[i] = c == '1';
            }
            f.transports
                .insert([x, y, z], Transport::new(PipeBlock::Pipe(open)));
        }
        f
    }
}

/// What a pipe sees beside it, for `FluidPipeBlock.canConnectTo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipeNeighbour {
    /// Another fluid pipe (no brackets here).
    Pipe,
    /// A block with a fluid handler (`hasFluidCapability`: tanks, basins).
    Holder,
    /// `VanillaFluidTargets.canProvideFluidWithoutCapability`: cauldrons and
    /// honey levels.
    Vanilla,
    /// A pump facing this way (`canHaveFlowToward`: along its axis).
    Pump(Dir),
    /// An encased pipe, and whether it is open towards this block.
    Encased(bool),
    /// A glass pipe along this axis.
    Glass(Axis),
    /// A valve with this pipe axis.
    Valve(Axis),
    /// A hose pulley's pipe side: it takes what comes, with a rim (not a drain).
    Hose,
    Other,
}

/// `FluidPipeBlock.canConnectTo` the neighbour on side `d`.
pub fn pipe_connects(n: PipeNeighbour, d: Dir) -> bool {
    match n {
        PipeNeighbour::Pipe
        | PipeNeighbour::Holder
        | PipeNeighbour::Hose
        | PipeNeighbour::Vanilla => true,
        PipeNeighbour::Pump(f) => f.axis() == d.axis(),
        PipeNeighbour::Encased(open) => open,
        PipeNeighbour::Glass(a) | PipeNeighbour::Valve(a) => a == d.axis(),
        PipeNeighbour::Other => false,
    }
}

/// What a pipe draws on a side (`FluidTransportBehaviour.AttachmentTypes`
/// as `FluidPipeBlockEntity`'s behaviour renders it), kept as the side's
/// state property: `Closed` for a side it isn't open at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipeSide {
    Closed,
    Rim,
    Drain,
    Connection,
    DetailedConnection,
}

impl PipeSide {
    #[allow(dead_code)]
    pub const ALL: [PipeSide; 5] = [
        PipeSide::Closed,
        PipeSide::Rim,
        PipeSide::Drain,
        PipeSide::Connection,
        PipeSide::DetailedConnection,
    ];
    pub fn name(self) -> &'static str {
        match self {
            PipeSide::Closed => "closed",
            PipeSide::Rim => "rim",
            PipeSide::Drain => "drain",
            PipeSide::Connection => "connection",
            PipeSide::DetailedConnection => "detailed",
        }
    }
    #[allow(dead_code)]
    pub fn parse(s: &str) -> Option<PipeSide> {
        PipeSide::ALL.into_iter().find(|p| p.name() == s)
    }
}

/// `getRenderedRimAttachment` of a pipe open as `open` on side `d`, whose
/// neighbour there is `n`.
pub fn pipe_attachment(open: [bool; 6], d: Dir, n: PipeNeighbour) -> PipeSide {
    if !open[d as usize] {
        return PipeSide::Closed;
    }
    let straight = |a: Axis| {
        let along: Vec<Dir> = Dir::ALL.into_iter().filter(|x| open[*x as usize]).collect();
        along.len() == 2 && along.iter().all(|x| x.axis() == a)
    };
    match n {
        // A pump pushing into this pipe: nothing, so a detailed connection.
        PipeNeighbour::Pump(f) if f == d.opposite() => PipeSide::DetailedConnection,
        PipeNeighbour::Holder => PipeSide::Drain,
        // Another pipe it connects to: no rim.
        PipeNeighbour::Pipe => {
            if straight(d.axis()) {
                PipeSide::Connection
            } else {
                PipeSide::DetailedConnection
            }
        }
        // A pump or valve it can flow into.
        PipeNeighbour::Pump(f) if f.axis() == d.axis() => PipeSide::DetailedConnection,
        PipeNeighbour::Valve(a) if a == d.axis() => PipeSide::DetailedConnection,
        _ => PipeSide::Rim,
    }
}

/// What the end of a glass pipe or valve draws
/// (`StraightPipeFluidTransportBehaviour.getRenderedRimAttachment`): no
/// connectors, just `PARTIAL_RIM` or `PARTIAL_DRAIN`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipeEnd {
    None,
    Rim,
    Drain,
}

impl PipeEnd {
    pub fn name(self) -> &'static str {
        match self {
            PipeEnd::None => "none",
            PipeEnd::Rim => "rim",
            PipeEnd::Drain => "drain",
        }
    }
}

/// The end of a glass pipe (or else a valve) along `axis`, on side `d`.
pub fn straight_end(glass: bool, axis: Axis, d: Dir, n: PipeNeighbour) -> PipeEnd {
    if d.axis() != axis {
        return PipeEnd::None;
    }
    match n {
        PipeNeighbour::Pump(f) if f == d.opposite() => PipeEnd::None,
        PipeNeighbour::Holder => PipeEnd::Drain,
        _ if !glass => PipeEnd::None,
        PipeNeighbour::Pipe => PipeEnd::None,
        PipeNeighbour::Glass(a) | PipeNeighbour::Valve(a) if a == axis => PipeEnd::None,
        _ => PipeEnd::Rim,
    }
}

/// An encased pipe's side as its state keeps it: `false` closed, `true`
/// open, `drain` open with a drain (`StandardPipeFluidTransportBehaviour`
/// draws nothing else on one).
pub fn encased_side(open: bool, d: Dir, n: PipeNeighbour) -> &'static str {
    match n {
        _ if !open => "false",
        PipeNeighbour::Pump(f) if f == d.opposite() => "true",
        PipeNeighbour::Holder => "drain",
        _ => "true",
    }
}

/// A flow in a glass pipe as clients draw it (`TransparentStraightPipeRenderer`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipeStream {
    pub side: Dir,
    pub inbound: bool,
    /// The flow's progress, in 255ths.
    pub progress: u8,
    pub fluid: FluidStack,
}

/// `FluidRenderer.renderFluidStream`'s box in the block, of radius 3/16: from
/// the side in to the middle when inbound, else from the middle out to it, as
/// far as it has got.
pub fn pipe_stream_box(s: &PipeStream) -> [f32; 6] {
    const RADIUS: f32 = 3.0 / 16.0;
    let progress = f32::from(s.progress) / 255.0;
    let (start, end) = if s.inbound {
        (0.5, 0.5 - progress * 0.5)
    } else {
        (0.0, progress * 0.5)
    };
    let n = s.side.normal();
    let mut low = [0.5 - RADIUS; 3];
    let mut high = [0.5 + RADIUS; 3];
    for k in 0..3 {
        if n[k] != 0 {
            let (a, b) = (0.5 + start * n[k] as f32, 0.5 + end * n[k] as f32);
            low[k] = a.min(b);
            high[k] = a.max(b);
        }
    }
    [low[0], low[1], low[2], high[0], high[1], high[2]]
}

/// `FluidValveBlock.getPipeAxis`: of the two axes not its facing's, the
/// second when `AXIS_ALONG_FIRST_COORDINATE`.
pub fn valve_pipe_axis(facing: Dir, along_first: bool) -> Axis {
    let others: Vec<Axis> = Axis::ALL
        .into_iter()
        .filter(|a| *a != facing.axis())
        .collect();
    others[usize::from(along_first)]
}

/// `FluidValveBlockEntity`: the pointer a tick on, chasing open at a positive
/// speed, closed at a negative one, standing still at none.
pub fn valve_pointer(pointer: f32, speed: f32) -> f32 {
    let target = if speed > 0.0 { 1.0 } else { 0.0 };
    let step = (speed.abs() / 16.0 / 20.0).clamp(0.0, 1.0);
    pointer + (target - pointer).clamp(-step, step)
}

/// `FluidPipeBlock.onWrenched`: the axis of the glass pipe it becomes, from
/// its straight axis or, on a corner, the side nearest the click (`local` in
/// the block); none when the click is on that axis.
pub fn pipe_wrench_axis(straight: Option<Axis>, face: Dir, local: [f64; 3]) -> Option<Axis> {
    let axis = straight.unwrap_or_else(|| {
        let mut closest = f64::MAX;
        let mut found = Dir::Up;
        for d in Dir::ALL {
            if d.axis() == face.axis() {
                continue;
            }
            let n = d.normal();
            let distance: f64 = (0..3)
                .map(|k| (f64::from(n[k]) + 0.5 - local[k]).powi(2))
                .sum();
            if distance < closest {
                closest = distance;
                found = d;
            }
        }
        found.axis()
    });
    (axis != face.axis()).then_some(axis)
}

/// `FluidPipeBlock.updateBlockState`: the sides a pipe opens, from what it
/// was (`prev`), the sides that connect, the side it leaves alone
/// (`ignore`) and the way it prefers when nothing connects.
pub fn update_pipe(
    prev: [bool; 6],
    preferred: Dir,
    ignore: Option<Dir>,
    connects: impl Fn(Dir) -> bool,
) -> [bool; 6] {
    let prev_sides = prev.iter().filter(|o| **o).count();
    let mut open = prev;
    for d in Dir::ALL {
        if Some(d) != ignore {
            open[d as usize] = connects(d);
        }
    }
    // Enough connections: two or more stay as they are.
    let mut connected = None;
    for d in Dir::ALL {
        if open[d as usize] {
            if connected.is_some() {
                return open;
            }
            connected = Some(d);
        }
    }
    // One: the opposite end opens too.
    if let Some(d) = connected {
        open[d.opposite() as usize] = true;
        return open;
    }
    // None, and it was a straight pipe before: as it was.
    if prev_sides == 2 {
        return prev;
    }
    open[preferred as usize] = true;
    open[preferred.opposite() as usize] = true;
    open
}

/// `PumpBlockEntity.searchForEndpointRecursively`.
fn search_for_endpoint(
    graph: &HashMap<Pos, (i32, [Option<bool>; 6])>,
    targets: &HashSet<(Pos, Dir)>,
    valid: &mut HashMap<i32, HashSet<(Pos, Dir)>>,
    current: (Pos, Dir),
    pull: bool,
) -> bool {
    let (pos, came_from) = current;
    let Some((distance, map)) = graph.get(&pos) else {
        return false;
    };
    let mut success = false;
    for next in Dir::ALL {
        if next == came_from {
            continue;
        }
        let Some(value) = map[next as usize] else {
            continue;
        };
        let local = (pos, next);
        if targets.contains(&local) {
            valid.entry(*distance).or_default().insert(local);
            success = true;
            continue;
        }
        if value != pull {
            continue;
        }
        if !search_for_endpoint(
            graph,
            targets,
            valid,
            (offset(pos, next), next.opposite()),
            pull,
        ) {
            continue;
        }
        valid.entry(*distance).or_default().insert(local);
        success = true;
    }
    if success {
        valid.entry(*distance).or_default().insert(current);
    }
    success
}

/// `OpenEndFluidHandler.fill`: fluid poured out of an open end gathers in its
/// buffer and becomes a source block once there is a bucket of it. Effects
/// of fluids without a block (`OpenPipeEffectHandler`: potions, milk...) are
/// not ported.
fn open_end_fill(
    o: &mut OpenEnd,
    world: &mut dyn FluidWorld,
    out: Pos,
    face: Dir,
    resource: &FluidStack,
    simulate: bool,
) -> i32 {
    if !world.is_loaded(out) || resource.is_empty() {
        return 0;
    }
    let space = world.space(out, face);
    let (ok, change) = provide_to_space(&space, resource, true);
    if let Some(c) = change {
        world.change_space(out, c);
    }
    if !ok {
        return 0;
    }
    // `containedFluidStack` is the tank's own stack: it sees this fill when
    // it holds the same fluid, and stays the old one when it is replaced.
    let before = o.tank.fluid.clone();
    let has_block_state = !before.is_empty() && has_block_state(&before.fluid);
    let same = !before.is_empty() && before.same(resource);
    if !before.is_empty() && !same {
        o.tank.fluid = FluidStack::empty();
    }
    o.was_pulling = false;
    let fill = o.tank.fill(resource, simulate);
    if simulate {
        return fill;
    }
    let contained = if before.is_empty() {
        FluidStack::empty()
    } else if same {
        o.tank.fluid.clone()
    } else {
        before
    };
    if o.tank.amount() == OPEN_END_CAPACITY || !has_block_state {
        let space = world.space(out, face);
        let (placed, change) = provide_to_space(&space, &contained, false);
        if let Some(c) = change {
            world.change_space(out, c);
        }
        if placed {
            o.tank.fluid = FluidStack::empty();
        }
    }
    fill
}

/// `OpenEndFluidHandler.drainInner`: from its buffer, else a bucket from the
/// world, the rest kept in the buffer.
fn open_end_drain(
    o: &mut OpenEnd,
    world: &mut dyn FluidWorld,
    out: Pos,
    face: Dir,
    amount: i32,
    filter: Option<&FluidStack>,
    simulate: bool,
) -> FluidStack {
    if !world.is_loaded(out) || amount == 0 {
        return FluidStack::empty();
    }
    let amount = amount.min(1000);
    o.was_pulling = true;
    let internal = match filter {
        Some(f) => o.tank.drain(&f.with_amount(amount), simulate),
        None => o.tank.drain_any(amount, simulate),
    };
    if !internal.is_empty() {
        return internal;
    }
    let space = world.space(out, face);
    let (mut drained, change) = remove_from_space(&space);
    if drained.is_empty() {
        return FluidStack::empty();
    }
    if let Some(f) = filter
        && !drained.same(f)
    {
        return FluidStack::empty();
    }
    if !simulate && let Some(c) = change {
        world.change_space(out, c);
    }
    let remainder = drained.amount - amount;
    drained.amount = amount;
    if !simulate && remainder > 0 {
        if !o.tank.fluid.is_empty() && !o.tank.fluid.same(&drained) {
            o.tank.fluid = FluidStack::empty();
        }
        o.tank.fill(&drained.with_amount(remainder), false);
    }
    drained
}

// ---- The basin's tanks --------------------------------------------------------

/// `BasinBlockEntity`'s fluids: `SmartFluidTankBehaviour` OUTPUT and INPUT,
/// two 1000 mB tanks each, offered as `CombinedTankWrapper(output, input)`.
/// Outputs refuse insertion (`forbidInsertion`); inputs keep their fluids
/// apart (`enforceVariety`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BasinTanks {
    pub output: [FluidTank; 2],
    pub input: [FluidTank; 2],
}

/// `SmartFluidTankBehaviour`'s tank capacity in a basin.
pub const BASIN_TANK: i32 = 1000;

impl Default for BasinTanks {
    fn default() -> Self {
        let t = || FluidTank::new(BASIN_TANK);
        BasinTanks {
            output: [t(), t()],
            input: [t(), t()],
        }
    }
}

/// `CombinedTankWrapper.fill` over single tanks.
fn combined_fill(
    tanks: &mut [FluidTank],
    resource: &FluidStack,
    simulate: bool,
    enforce_variety: bool,
) -> i32 {
    if resource.is_empty() {
        return 0;
    }
    let mut filled = 0;
    let mut resource = resource.clone();
    let mut fitting = false;
    'outer: for search in [true, false] {
        for tank in tanks.iter_mut() {
            if search && tank.fluid.same(&resource) && !tank.fluid.is_empty() {
                fitting = true;
            }
            if search && !fitting {
                continue;
            }
            let into = tank.fill(&resource, simulate);
            resource.amount -= into;
            filled += into;
            if resource.is_empty() {
                break 'outer;
            }
            if fitting && (enforce_variety || into != 0) {
                break 'outer;
            }
        }
    }
    filled
}

/// `CombinedTankWrapper.drain(FluidStack)` over single tanks.
fn combined_drain(tanks: &mut [FluidTank], resource: &FluidStack, simulate: bool) -> FluidStack {
    if resource.is_empty() {
        return FluidStack::empty();
    }
    let mut drained = FluidStack::empty();
    let mut resource = resource.clone();
    for tank in tanks.iter_mut() {
        let from = tank.drain(&resource, simulate);
        resource.amount -= from.amount.max(0);
        if !from.is_empty() && (drained.is_empty() || from.same(&drained)) {
            drained = from.with_amount(from.amount + drained.amount.max(0));
        }
        if resource.is_empty() {
            break;
        }
    }
    drained
}

/// `CombinedTankWrapper.drain(int)` over single tanks.
fn combined_drain_any(tanks: &mut [FluidTank], mut max: i32, simulate: bool) -> FluidStack {
    let mut drained = FluidStack::empty();
    for tank in tanks.iter_mut() {
        let from = tank.drain_any(max, simulate);
        max -= from.amount.max(0);
        if !from.is_empty() && (drained.is_empty() || from.same(&drained)) {
            drained = from.with_amount(from.amount + drained.amount.max(0));
        }
        if max == 0 {
            break;
        }
    }
    drained
}

impl BasinTanks {
    /// `acceptFluidOutputsIntoBasin`: the output tanks' `forceFill`.
    pub fn force_fill_output(&mut self, resource: &FluidStack, simulate: bool) -> i32 {
        combined_fill(&mut self.output, resource, simulate, true)
    }
    /// Every tank's fluid, outputs first (the capability's `getFluidInTank`).
    pub fn stacks(&self) -> Vec<FluidStack> {
        self.output
            .iter()
            .chain(self.input.iter())
            .map(|t| t.fluid.clone())
            .collect()
    }
    /// Puts back what `basin_take_fluids` left of `stacks`.
    pub fn set_stacks(&mut self, stacks: &[FluidStack]) {
        for (t, s) in self
            .output
            .iter_mut()
            .chain(self.input.iter_mut())
            .zip(stacks)
        {
            t.fluid = if s.is_empty() {
                FluidStack::empty()
            } else {
                s.clone()
            };
        }
    }
    pub fn is_empty(&self) -> bool {
        self.output
            .iter()
            .chain(self.input.iter())
            .all(|t| t.fluid.is_empty())
    }
}

impl FluidHandler for BasinTanks {
    fn fill(&mut self, resource: &FluidStack, simulate: bool) -> i32 {
        // The outer wrapper over (output, input): outputs take nothing
        // (`forbidInsertion`), and whichever pass gets there, the inputs
        // take what they take.
        combined_fill(&mut self.input, resource, simulate, true)
    }
    fn drain(&mut self, resource: &FluidStack, simulate: bool) -> FluidStack {
        let from_output = combined_drain(&mut self.output, resource, simulate);
        let rest = resource.with_amount(resource.amount - from_output.amount.max(0));
        if rest.is_empty() {
            return from_output;
        }
        let from_input = combined_drain(&mut self.input, &rest, simulate);
        merge(from_output, from_input)
    }
    fn drain_any(&mut self, max: i32, simulate: bool) -> FluidStack {
        let from_output = combined_drain_any(&mut self.output, max, simulate);
        let rest = max - from_output.amount.max(0);
        if rest == 0 {
            return from_output;
        }
        let from_input = combined_drain_any(&mut self.input, rest, simulate);
        merge(from_output, from_input)
    }
}

/// `CombinedTankWrapper`'s sum: the first fluid drained, and more of it.
fn merge(a: FluidStack, b: FluidStack) -> FluidStack {
    if a.is_empty() {
        return b;
    }
    if !b.is_empty() && b.same(&a) {
        return a.with_amount(a.amount + b.amount);
    }
    a
}

// ---- The fluid tank ---------------------------------------------------------

/// `FluidTankRenderer.renderSafe`'s fluid box for a tank `width` wide and
/// `height` high (in blocks) filled to `fill` (0 to 1): `[x0, y0, z0, x1, y1,
/// z1]` within the controller's block, none when too little to show.
pub fn tank_fluid_box(fill: f32, width: f32, height: f32) -> Option<[f32; 6]> {
    let cap_height = 1.0 / 4.0;
    let hull = 1.0 / 16.0 + 1.0 / 128.0;
    let min_puddle = 1.0 / 16.0;
    let total = height - 2.0 * cap_height - min_puddle;
    if fill < 1.0 / (512.0 * total) {
        return None;
    }
    let clamped = (fill * total).clamp(0.0, total);
    let y_min = total + cap_height + min_puddle - clamped;
    let y_max = y_min + clamped;
    // ms.translate(0, clampedLevel - totalHeight, 0)
    let shift = clamped - total;
    let x_min = hull;
    let x_max = x_min + width - 2.0 * hull;
    Some([x_min, y_min + shift, x_min, x_max, y_max + shift, x_max])
}

/// `BasinRenderer.renderFluids`: the basin's fluids (`stacks`: input tanks,
/// then output tanks) side by side along X, together as high as there is
/// fluid (a 2000 mB basin is full, eased).
pub fn basin_fluid_boxes(stacks: &[FluidStack]) -> Vec<(FluidStack, [f32; 6])> {
    let total: i32 = stacks
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| s.amount)
        .sum();
    if total < 1 {
        return Vec::new();
    }
    let total = total as f32;
    let level = (total / 2000.0).clamp(0.0, 1.0);
    let level = 1.0 - (1.0 - level) * (1.0 - level);
    let (mut x_min, mut x_max) = (2.0 / 16.0, 2.0 / 16.0);
    let (y_min, z_min, z_max) = (2.0 / 16.0, 2.0 / 16.0, 14.0 / 16.0);
    let y_max = y_min + 12.0 / 16.0 * level;
    let mut boxes = Vec::new();
    for s in stacks {
        if s.is_empty() || s.amount < 1 {
            continue;
        }
        let partial = (s.amount as f32 / total).clamp(0.0, 1.0);
        x_max += partial * 12.0 / 16.0;
        boxes.push((s.clone(), [x_min, y_min, z_min, x_max, y_max, z_max]));
        x_min = x_max;
    }
    boxes
}

/// `FluidHelper.tryEmptyItemIntoBE` / `tryFillItemFromBE` for a bucket on a
/// tank: what a bucket item holds, or what filled bucket a fluid makes.
pub fn bucket_fluid(item: &str) -> Option<&'static str> {
    match item {
        "minecraft:water_bucket" => Some(WATER),
        "minecraft:lava_bucket" => Some(LAVA),
        _ => None,
    }
}

pub fn filled_bucket(fluid: &str) -> Option<&'static str> {
    match fluid {
        WATER => Some("minecraft:water_bucket"),
        LAVA => Some("minecraft:lava_bucket"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct TestWorld {
        pipes: HashMap<Pos, PipeBlock>,
        tanks: HashMap<Pos, FluidTank>,
        water: HashSet<Pos>,
        solid: HashSet<Pos>,
        collisions: Vec<(Pos, Option<&'static str>)>,
    }

    impl FluidAccess for TestWorld {
        fn has_handler(&self, pos: Pos, _: Dir) -> bool {
            self.tanks.contains_key(&pos)
        }
        fn handler_tanks(&self, pos: Pos, _: Dir) -> Vec<FluidStack> {
            self.tanks
                .get(&pos)
                .map(|t| vec![t.fluid.clone()])
                .unwrap_or_default()
        }
        fn fill(&mut self, pos: Pos, _: Dir, r: &FluidStack, simulate: bool) -> i32 {
            self.tanks.get_mut(&pos).map_or(0, |t| t.fill(r, simulate))
        }
        fn drain(&mut self, pos: Pos, _: Dir, r: &FluidStack, simulate: bool) -> FluidStack {
            self.tanks
                .get_mut(&pos)
                .map_or_else(FluidStack::empty, |t| t.drain(r, simulate))
        }
        fn drain_any(&mut self, pos: Pos, _: Dir, max: i32, simulate: bool) -> FluidStack {
            self.tanks
                .get_mut(&pos)
                .map_or_else(FluidStack::empty, |t| t.drain_any(max, simulate))
        }
    }

    impl FluidWorld for TestWorld {
        fn is_loaded(&self, _: Pos) -> bool {
            true
        }
        fn pipe_block(&self, pos: Pos) -> Option<PipeBlock> {
            self.pipes.get(&pos).copied()
        }
        fn space(&self, pos: Pos, _: Dir) -> Space {
            let solid = self.solid.contains(&pos) || self.tanks.contains_key(&pos);
            Space {
                fluid: self.water.contains(&pos).then(|| WATER.to_owned()),
                source: self.water.contains(&pos),
                replaceable: !solid,
                breakable: true,
                solid_side: solid,
                ..Space::default()
            }
        }
        fn change_space(&mut self, pos: Pos, change: SpaceChange) {
            match change {
                SpaceChange::Source(f) if f == WATER => {
                    self.water.insert(pos);
                }
                SpaceChange::Lowered(_) => {
                    // An infinite pool: the game's flow refills it.
                }
                _ => {}
            }
        }
        fn pipe_collision(&mut self, pos: Pos, block: Option<&'static str>) {
            self.collisions.push((pos, block));
        }
    }

    fn place(f: &mut Fluids, w: &mut TestWorld, pos: Pos, b: PipeBlock) {
        w.pipes.insert(pos, b);
        f.block_changed(w, pos);
    }

    #[test]
    fn tanks_fill_and_drain_like_neoforge() {
        let mut t = FluidTank::new(1000);
        assert_eq!(t.fill(&FluidStack::new(WATER, 600), false), 600);
        assert_eq!(t.fill(&FluidStack::new(LAVA, 100), false), 0);
        assert_eq!(t.fill(&FluidStack::new(WATER, 600), true), 400);
        assert_eq!(t.amount(), 600);
        assert_eq!(
            t.drain(&FluidStack::new(LAVA, 100), false),
            FluidStack::empty()
        );
        assert_eq!(t.drain_any(1000, false), FluidStack::new(WATER, 600));
        assert!(t.fluid.is_empty());
    }

    #[test]
    fn straight_pipes_and_pumps_are_open_along_their_axis() {
        let p = PipeBlock::straight(Axis::X);
        assert!(p.open(Dir::East) && p.open(Dir::West) && !p.open(Dir::Up));
        assert_eq!(p.straight_axis(), Some(Axis::X));
        let pump = PipeBlock::Pump(Dir::Up);
        assert!(pump.open(Dir::Down) && !pump.open(Dir::North));
        let mut corner = [false; 6];
        corner[Dir::Up as usize] = true;
        corner[Dir::East as usize] = true;
        assert_eq!(PipeBlock::Pipe(corner).straight_axis(), None);
    }

    #[test]
    fn open_ends_drain_sources_and_pour_by_the_bucket() {
        let water = Space {
            fluid: Some(WATER.into()),
            source: true,
            replaceable: true,
            breakable: true,
            ..Space::default()
        };
        assert_eq!(
            remove_from_space(&water),
            (
                FluidStack::new(WATER, 1000),
                Some(SpaceChange::Lowered(WATER.into()))
            )
        );
        let air = Space {
            replaceable: true,
            breakable: true,
            ..Space::default()
        };
        assert_eq!(remove_from_space(&air).0, FluidStack::empty());
        let w = FluidStack::new(WATER, 1000);
        assert_eq!(provide_to_space(&air, &w, true), (true, None));
        assert_eq!(
            provide_to_space(&air, &w, false),
            (true, Some(SpaceChange::Source(WATER.into())))
        );
        let lava = Space {
            fluid: Some(LAVA.into()),
            source: true,
            ..air.clone()
        };
        assert_eq!(
            provide_to_space(&lava, &w, true),
            (false, Some(SpaceChange::Block("minecraft:obsidian")))
        );
        let nether = Space {
            ultra_warm: true,
            ..air.clone()
        };
        assert_eq!(provide_to_space(&nether, &w, false), (true, None));
        let honey = FluidStack::new("create:honey", 250);
        assert_eq!(provide_to_space(&air, &honey, true), (false, None));
        let hive = Space {
            vanilla: Some(VanillaTarget::Honey(5)),
            ..Space::default()
        };
        assert_eq!(remove_from_space(&hive).0, honey);
        assert_eq!(
            flow_collision_block(LAVA, WATER),
            Some("minecraft:cobblestone")
        );
        assert_eq!(flow_collision_block(WATER, WATER), None);
    }

    /// Water | pipe | pump (facing east) | pipe | pipe | tank, along X.
    fn rig() -> (Fluids, TestWorld) {
        let mut f = Fluids::new();
        let mut w = TestWorld::default();
        w.water.insert([0, 0, 0]);
        let mut tank = FluidTank::new(TANK_CAPACITY);
        tank.capacity = TANK_CAPACITY;
        w.tanks.insert([5, 0, 0], tank);
        place(&mut f, &mut w, [1, 0, 0], PipeBlock::straight(Axis::X));
        place(&mut f, &mut w, [2, 0, 0], PipeBlock::Pump(Dir::East));
        place(&mut f, &mut w, [3, 0, 0], PipeBlock::straight(Axis::X));
        place(&mut f, &mut w, [4, 0, 0], PipeBlock::straight(Axis::X));
        (f, w)
    }

    #[test]
    fn a_pump_puts_pressure_toward_both_endpoints() {
        let (mut f, mut w) = rig();
        f.pump_speed(&mut w, [2, 0, 0], 32.0);
        for _ in 0..2 {
            f.tick(&mut w);
        }
        let c = |f: &Fluids, p: Pos, d: Dir| f.transports[&p].connection(d).unwrap().pressure;
        // Pulled in at the back: inbound at the open end, outward toward the pump.
        assert_eq!(c(&f, [1, 0, 0], Dir::West), [32.0, 0.0]);
        assert_eq!(c(&f, [1, 0, 0], Dir::East), [0.0, 32.0]);
        // Pushed out at the front, toward the tank.
        assert_eq!(c(&f, [3, 0, 0], Dir::West), [32.0, 0.0]);
        assert_eq!(c(&f, [4, 0, 0], Dir::East), [0.0, 32.0]);
        // The pump's own sides.
        assert_eq!(c(&f, [2, 0, 0], Dir::West), [32.0, 0.0]);
        assert_eq!(c(&f, [2, 0, 0], Dir::East), [0.0, 32.0]);
    }

    #[test]
    fn a_pump_moves_water_from_a_source_into_a_tank() {
        let (mut f, mut w) = rig();
        f.pump_speed(&mut w, [2, 0, 0], 32.0);
        for _ in 0..200 {
            f.tick(&mut w);
        }
        let amount = w.tanks[&[5, 0, 0]].amount();
        assert!(amount > 0, "the tank got nothing");
        assert_eq!(w.tanks[&[5, 0, 0]].fluid.fluid, WATER);
        // Transfer speed is half the pressure a tick, once flowing.
        let before = amount;
        f.tick(&mut w);
        assert_eq!(w.tanks[&[5, 0, 0]].amount() - before, 16);
        assert!(w.collisions.is_empty());
    }

    #[test]
    fn a_valve_holds_the_flow_until_it_opens_and_glass_passes_it() {
        let (mut f, mut w) = rig();
        w.pipes.insert([3, 0, 0], PipeBlock::Valve(Axis::X, false));
        f.block_changed(&mut w, [3, 0, 0]);
        w.pipes.insert([4, 0, 0], PipeBlock::Glass(Axis::X));
        f.block_changed(&mut w, [4, 0, 0]);
        f.pump_speed(&mut w, [2, 0, 0], 32.0);
        for _ in 0..200 {
            f.tick(&mut w);
        }
        assert_eq!(w.tanks[&[5, 0, 0]].amount(), 0);
        w.pipes.insert([3, 0, 0], PipeBlock::Valve(Axis::X, true));
        f.block_changed(&mut w, [3, 0, 0]);
        for _ in 0..200 {
            f.tick(&mut w);
        }
        assert!(w.tanks[&[5, 0, 0]].amount() > 0, "the tank got nothing");
        let before = w.tanks[&[5, 0, 0]].amount();
        w.pipes.insert([3, 0, 0], PipeBlock::Valve(Axis::X, false));
        f.block_changed(&mut w, [3, 0, 0]);
        for _ in 0..50 {
            f.tick(&mut w);
        }
        let closed = w.tanks[&[5, 0, 0]].amount();
        for _ in 0..50 {
            f.tick(&mut w);
        }
        assert_eq!(w.tanks[&[5, 0, 0]].amount(), closed);
        assert!(closed >= before);
    }

    #[test]
    fn a_stopped_pump_moves_nothing() {
        let (mut f, mut w) = rig();
        f.pump_speed(&mut w, [2, 0, 0], 0.0);
        for _ in 0..100 {
            f.tick(&mut w);
        }
        assert_eq!(w.tanks[&[5, 0, 0]].amount(), 0);
    }

    #[test]
    fn a_pump_empties_a_tank_into_the_world() {
        let (mut f, mut w) = rig();
        w.water.clear();
        w.tanks.get_mut(&[5, 0, 0]).unwrap().fluid = FluidStack::new(WATER, 4000);
        // Turned round: from the tank toward the open end.
        w.pipes.insert([2, 0, 0], PipeBlock::Pump(Dir::West));
        f.block_changed(&mut w, [2, 0, 0]);
        f.pump_speed(&mut w, [2, 0, 0], 64.0);
        for _ in 0..300 {
            f.tick(&mut w);
        }
        assert!(w.tanks[&[5, 0, 0]].amount() < 4000);
        assert!(w.water.contains(&[0, 0, 0]), "no water was poured out");
    }

    #[test]
    fn removing_a_pipe_drops_the_pressure() {
        let (mut f, mut w) = rig();
        f.pump_speed(&mut w, [2, 0, 0], 32.0);
        for _ in 0..3 {
            f.tick(&mut w);
        }
        w.pipes.remove(&[4, 0, 0]);
        f.block_changed(&mut w, [4, 0, 0]);
        assert!(!f.transports.contains_key(&[4, 0, 0]));
        assert!(!f.transports[&[3, 0, 0]].has_any_pressure());
    }

    #[test]
    fn pipes_turn_corners_towards_what_they_connect_to() {
        let none = [false; 6];
        // Nothing around: straight along the placer's look.
        let p = update_pipe(none, Dir::North, None, |_| false);
        assert!(p[Dir::North as usize] && p[Dir::South as usize]);
        // A tank to the east and a pipe above: a corner.
        let c = update_pipe(none, Dir::North, None, |d| d == Dir::East || d == Dir::Up);
        assert_eq!(c.iter().filter(|o| **o).count(), 2);
        assert!(c[Dir::East as usize] && c[Dir::Up as usize]);
        // One connection: the far end opens.
        let one = update_pipe(none, Dir::North, None, |d| d == Dir::West);
        assert!(one[Dir::West as usize] && one[Dir::East as usize]);
        // A straight pipe whose neighbours went keeps its way.
        let kept = update_pipe(p, Dir::Up, None, |_| false);
        assert_eq!(kept, p);
        assert!(pipe_connects(PipeNeighbour::Pump(Dir::East), Dir::West));
        // Rims on open ends, drains into tanks, plain joins along a line.
        assert_eq!(
            pipe_attachment(p, Dir::North, PipeNeighbour::Other),
            PipeSide::Rim
        );
        assert_eq!(
            pipe_attachment(p, Dir::North, PipeNeighbour::Holder),
            PipeSide::Drain
        );
        assert_eq!(
            pipe_attachment(p, Dir::North, PipeNeighbour::Pipe),
            PipeSide::Connection
        );
        assert_eq!(
            pipe_attachment(c, Dir::East, PipeNeighbour::Pipe),
            PipeSide::DetailedConnection
        );
        assert_eq!(
            pipe_attachment(p, Dir::East, PipeNeighbour::Pipe),
            PipeSide::Closed
        );
        assert!(!pipe_connects(PipeNeighbour::Pump(Dir::East), Dir::Up));
    }

    #[test]
    fn saving_keeps_pipes() {
        let (f, _) = rig();
        let g = Fluids::load(&f.save());
        assert_eq!(g.transports.len(), 3);
        assert_eq!(g.transports[&[1, 0, 0]].block, PipeBlock::straight(Axis::X));
    }

    #[test]
    fn basins_fill_their_inputs_apart_and_drain_outputs_first() {
        let mut b = BasinTanks::default();
        assert_eq!(b.fill(&FluidStack::new(WATER, 1500), false), 1500);
        assert_eq!(b.input[0].fluid, FluidStack::new(WATER, 1000));
        assert_eq!(b.input[1].fluid, FluidStack::new(WATER, 500));
        // A fitting tank is full: enforceVariety stops there.
        assert_eq!(b.fill(&FluidStack::new(WATER, 100), true), 0);
        assert_eq!(b.fill(&FluidStack::new(LAVA, 100), true), 0);
        let mut b = BasinTanks::default();
        b.fill(&FluidStack::new(WATER, 300), false);
        assert_eq!(b.fill(&FluidStack::new(LAVA, 100), false), 100);
        assert_eq!(b.input[1].fluid, FluidStack::new(LAVA, 100));
        assert_eq!(b.force_fill_output(&FluidStack::new(LAVA, 50), false), 50);
        assert_eq!(
            b.drain(&FluidStack::new(WATER, 1000), true),
            FluidStack::new(WATER, 300)
        );
        // As CombinedTankWrapper.drain(int): what isn't the first fluid
        // drained is drained all the same, and lost.
        assert_eq!(b.drain_any(1000, false), FluidStack::new(LAVA, 50));
        assert!(b.is_empty());
    }

    #[test]
    fn basin_fluids_lie_side_by_side() {
        assert!(basin_fluid_boxes(&vec![FluidStack::empty(); 4]).is_empty());
        let boxes = basin_fluid_boxes(&[
            FluidStack::new(WATER, 1000),
            FluidStack::empty(),
            FluidStack::new(LAVA, 1000),
            FluidStack::empty(),
        ]);
        assert_eq!(boxes.len(), 2);
        // 2000 mB: full, 2/16 to 14/16 high, halves along X.
        let [x0, y0, _, x1, y1, _] = boxes[0].1;
        assert!((x0 - 0.125).abs() < 1e-6 && (x1 - 0.5).abs() < 1e-6);
        assert!((y0 - 0.125).abs() < 1e-6 && (y1 - 0.875).abs() < 1e-6);
        assert!((boxes[1].1[3] - 0.875).abs() < 1e-6);
        let one = basin_fluid_boxes(&[FluidStack::new(WATER, 500)]);
        // 1 - (3/4)^2 of the height.
        assert!((one[0].1[4] - (0.125 + 0.75 * 0.4375)).abs() < 1e-6);
    }

    #[test]
    fn a_closed_valve_passes_nothing_and_an_open_one_does() {
        let mut f = Fluids::default();
        let v = PipeBlock::Valve(Axis::X, false);
        assert!(v.open(Dir::East) && !v.open(Dir::Up));
        assert_eq!(v.straight_axis(), None);
        assert_eq!(PipeBlock::Glass(Axis::Z).straight_axis(), Some(Axis::Z));
        assert!(PipeBlock::Encased([true; 6]).hears_neighbours());
        assert!(!v.hears_neighbours());
        f.transports.insert([0, 0, 0], Transport::new(v));
        assert!(f.transports[&[0, 0, 0]].connection(Dir::East).is_some());
    }

    #[test]
    fn stream_boxes_run_to_and_from_the_side() {
        let s = |side, inbound, progress| PipeStream {
            side,
            inbound,
            progress,
            fluid: FluidStack::new(WATER, 1),
        };
        let b = pipe_stream_box(&s(Dir::East, false, 255));
        assert_eq!([b[0], b[3]], [0.5, 1.0]);
        let b = pipe_stream_box(&s(Dir::East, true, 255));
        assert_eq!([b[0], b[3]], [0.5, 1.0]);
        let b = pipe_stream_box(&s(Dir::West, false, 255));
        assert_eq!([b[0], b[3]], [0.0, 0.5]);
        // Inbound starts at the side, outward at the middle.
        let b = pipe_stream_box(&s(Dir::Up, true, 0));
        assert_eq!([b[1], b[4]], [1.0, 1.0]);
        let b = pipe_stream_box(&s(Dir::Up, false, 0));
        assert_eq!([b[1], b[4]], [0.5, 0.5]);
        assert_eq!([b[0], b[3]], [0.5 - 3.0 / 16.0, 0.5 + 3.0 / 16.0]);
    }

    #[test]
    fn smart_pipe_filter_and_placement() {
        let water = FluidStack::new(WATER, 1);
        assert!(PipeFilter::of(None).passes(&water));
        assert!(PipeFilter::of(Some("minecraft:water_bucket")).passes(&water));
        assert!(!PipeFilter::of(Some("minecraft:lava_bucket")).passes(&water));
        assert!(!PipeFilter::of(Some("minecraft:stone")).passes(&water));
        assert!(PipeFilter::of(Some("minecraft:stone")).passes(&FluidStack::empty()));
        let look = |a: Axis| Dir::positive_of(a);
        // On the floor, nothing beside: as clicked.
        assert_eq!(
            smart_placement(Dir::Up, Dir::North, look, |_| false),
            (SmartFace::Floor, Dir::North)
        );
        assert_eq!(
            smart_placement(Dir::Down, Dir::North, look, |_| false),
            (SmartFace::Ceiling, Dir::North)
        );
        assert_eq!(
            smart_placement(Dir::East, Dir::West, look, |_| false),
            (SmartFace::Wall, Dir::East)
        );
        // A pipe beside it along X: the floor, turned along X.
        assert_eq!(
            smart_placement(Dir::East, Dir::West, look, |d| d == Dir::East),
            (SmartFace::Floor, Dir::East)
        );
        // Above: on a wall, turned round.
        assert_eq!(
            smart_placement(Dir::Up, Dir::North, look, |d| d == Dir::Up),
            (SmartFace::Wall, Dir::South)
        );
        // Two axes: no preference.
        assert_eq!(
            smart_placement(Dir::Up, Dir::North, look, |d| d == Dir::Up
                || d == Dir::East),
            (SmartFace::Floor, Dir::North)
        );
        assert_eq!(smart_pipe_axis(SmartFace::Wall, Dir::North), Axis::Y);
        assert_eq!(smart_pipe_axis(SmartFace::Floor, Dir::North), Axis::Z);
        assert_eq!(PipeBlock::Smart(Axis::X).straight_axis(), None);
        assert!(PipeBlock::Smart(Axis::X).open(Dir::West));
    }

    #[test]
    fn valve_pointer_chases_by_speed() {
        assert_eq!(valve_pointer(0.0, 32.0), 0.1);
        assert_eq!(valve_pointer(0.5, 0.0), 0.5);
        assert_eq!(valve_pointer(0.05, -32.0), 0.0);
        assert_eq!(valve_pointer(0.95, 64.0), 1.0);
    }

    #[test]
    fn valve_axis_is_the_one_not_facing_nor_the_shaft() {
        assert_eq!(valve_pipe_axis(Dir::North, false), Axis::X);
        assert_eq!(valve_pipe_axis(Dir::North, true), Axis::Y);
        assert_eq!(valve_pipe_axis(Dir::Up, false), Axis::X);
        assert_eq!(valve_pipe_axis(Dir::Up, true), Axis::Z);
        assert_eq!(valve_pipe_axis(Dir::East, false), Axis::Y);
        assert_eq!(valve_pipe_axis(Dir::East, true), Axis::Z);
    }

    #[test]
    fn ends_of_straight_pipes() {
        use PipeNeighbour as N;
        assert_eq!(
            straight_end(true, Axis::X, Dir::Up, N::Holder),
            PipeEnd::None
        );
        assert_eq!(
            straight_end(true, Axis::X, Dir::East, N::Holder),
            PipeEnd::Drain
        );
        assert_eq!(
            straight_end(false, Axis::X, Dir::East, N::Holder),
            PipeEnd::Drain
        );
        assert_eq!(
            straight_end(true, Axis::X, Dir::East, N::Other),
            PipeEnd::Rim
        );
        assert_eq!(
            straight_end(false, Axis::X, Dir::East, N::Other),
            PipeEnd::None
        );
        assert_eq!(
            straight_end(true, Axis::X, Dir::East, N::Pipe),
            PipeEnd::None
        );
        assert_eq!(
            straight_end(true, Axis::X, Dir::East, N::Glass(Axis::X)),
            PipeEnd::None
        );
        assert_eq!(
            straight_end(true, Axis::X, Dir::East, N::Pump(Dir::West)),
            PipeEnd::None
        );
        assert_eq!(
            straight_end(true, Axis::X, Dir::East, N::Pump(Dir::East)),
            PipeEnd::Rim
        );
        assert_eq!(encased_side(false, Dir::East, N::Holder), "false");
        assert_eq!(encased_side(true, Dir::East, N::Holder), "drain");
        assert_eq!(encased_side(true, Dir::East, N::Pipe), "true");
    }

    #[test]
    fn pipes_connect_to_their_straight_neighbours_along_them() {
        use PipeNeighbour as N;
        assert!(pipe_connects(N::Glass(Axis::X), Dir::East));
        assert!(!pipe_connects(N::Glass(Axis::X), Dir::Up));
        assert!(pipe_connects(N::Valve(Axis::Y), Dir::Up));
        assert!(pipe_connects(N::Encased(true), Dir::Up));
        assert!(!pipe_connects(N::Encased(false), Dir::Up));
        let mut open = [false; 6];
        open[Dir::East as usize] = true;
        assert_eq!(
            pipe_attachment(open, Dir::East, N::Valve(Axis::X)),
            PipeSide::DetailedConnection
        );
        assert_eq!(
            pipe_attachment(open, Dir::East, N::Glass(Axis::X)),
            PipeSide::Rim
        );
        assert_eq!(
            pipe_attachment(open, Dir::East, N::Encased(true)),
            PipeSide::Rim
        );
    }

    #[test]
    fn wrench_on_a_pipe_picks_the_click_axis() {
        assert_eq!(
            pipe_wrench_axis(Some(Axis::X), Dir::Up, [0.5, 1.0, 0.5]),
            Some(Axis::X)
        );
        assert_eq!(
            pipe_wrench_axis(Some(Axis::X), Dir::East, [1.0, 0.5, 0.5]),
            None
        );
        assert_eq!(
            pipe_wrench_axis(None, Dir::Up, [0.9, 1.0, 0.5]),
            Some(Axis::X)
        );
        assert_eq!(
            pipe_wrench_axis(None, Dir::Up, [0.5, 1.0, 0.1]),
            Some(Axis::Z)
        );
    }

    #[test]
    fn the_tank_fluid_box_follows_its_fill() {
        assert_eq!(tank_fluid_box(0.0, 1.0, 1.0), None);
        let half = tank_fluid_box(0.5, 1.0, 1.0).unwrap();
        let hull = 1.0 / 16.0 + 1.0 / 128.0;
        assert!((half[0] - hull).abs() < 1e-6 && (half[3] - (1.0 - hull)).abs() < 1e-6);
        assert!((half[1] - 0.3125).abs() < 1e-6);
        assert!((half[4] - (0.3125 + 0.4375 / 2.0)).abs() < 1e-6);
        let full = tank_fluid_box(1.0, 1.0, 1.0).unwrap();
        assert!((full[4] - 0.75).abs() < 1e-6);
    }
}
