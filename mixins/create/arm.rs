// Create's mechanical arm, ported from Create 6.0.10 (MIT, The Create Team):
// ArmBlockEntity (its phases, the search for inputs and outputs, the
// selection modes), ArmInteractionPoint and the point types the port has
// blocks for (AllArmInteractionPointTypes: depot, belt, basin, chute,
// deployer, funnel, millstone), ArmAngleTarget and the arm's animation. No
// Horizon types: points are reached through `ArmWorld`, items are any
// `ArmStack`.

use super::create_kinetics::{Axis, Dir, Pos, rotate_vec};

/// `CLogistics.mechanicalArmRange`.
pub const RANGE: i32 = 5;
/// `SmartBlockEntity.lazyTickRate`.
const LAZY_TICK_RATE: i32 = 10;

/// `ArmBlockEntity.Phase`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    SearchInputs,
    MoveToInput,
    SearchOutputs,
    MoveToOutput,
    Dancing,
}

impl Phase {
    pub const ALL: [Phase; 5] = [
        Phase::SearchInputs,
        Phase::MoveToInput,
        Phase::SearchOutputs,
        Phase::MoveToOutput,
        Phase::Dancing,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Phase::SearchInputs => "SEARCH_INPUTS",
            Phase::MoveToInput => "MOVE_TO_INPUT",
            Phase::SearchOutputs => "SEARCH_OUTPUTS",
            Phase::MoveToOutput => "MOVE_TO_OUTPUT",
            Phase::Dancing => "DANCING",
        }
    }
    pub fn parse(s: &str) -> Phase {
        Phase::ALL
            .into_iter()
            .find(|p| p.name() == s)
            .unwrap_or(Phase::SearchInputs)
    }
}

/// `ArmBlockEntity.SelectionMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionMode {
    RoundRobin,
    ForcedRoundRobin,
    PreferFirst,
}

impl SelectionMode {
    pub const ALL: [SelectionMode; 3] = [
        SelectionMode::RoundRobin,
        SelectionMode::ForcedRoundRobin,
        SelectionMode::PreferFirst,
    ];
    /// Its place in the list (`ScrollValue`).
    pub fn code(self) -> i32 {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0) as i32
    }
    pub fn from_code(code: i32) -> SelectionMode {
        Self::ALL
            .get(code.max(0) as usize)
            .copied()
            .unwrap_or(SelectionMode::RoundRobin)
    }
    /// One step along the list, as the value box scrolls (not round).
    pub fn step(self, up: bool) -> SelectionMode {
        Self::from_code((self.code() + if up { 1 } else { -1 }).clamp(0, 2))
    }
    /// `create.mechanical_arm.selection_mode.*`'s English names.
    pub fn label(self) -> &'static str {
        match self {
            SelectionMode::RoundRobin => "Round Robin",
            SelectionMode::ForcedRoundRobin => "Forced Round Robin",
            SelectionMode::PreferFirst => "Prefer First Target",
        }
    }
}

/// `ArmInteractionPoint.Mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Deposit,
    Take,
}

/// The point types the port has blocks for (`AllArmInteractionPointTypes`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointType {
    Basin,
    Belt,
    Chute,
    /// `DeployerPoint`, with the deployer's facing.
    Deployer(Dir),
    /// `DepotPoint` on a depot.
    Depot,
    /// `FunnelPoint`, with `FunnelBlock.getFunnelFacing`.
    Funnel(Dir),
    Millstone,
    /// `CampfirePoint`: food in, nothing out.
    Campfire,
    /// `ComposterPoint`: compostables in, bone meal out.
    Composter,
    /// `RespawnAnchorPoint`: glowstone in.
    RespawnAnchor,
}

impl PointType {
    /// The type's registry name (`create:depot`).
    pub fn name(self) -> &'static str {
        match self {
            PointType::Basin => "create:basin",
            PointType::Belt => "create:belt",
            PointType::Chute => "create:chute",
            PointType::Deployer(_) => "create:deployer",
            PointType::Depot => "create:depot",
            PointType::Funnel(_) => "create:funnel",
            PointType::Millstone => "create:millstone",
            PointType::Campfire => "create:campfire",
            PointType::Composter => "create:composter",
            PointType::RespawnAnchor => "create:respawn_anchor",
        }
    }
    /// `DepositOnlyArmInteractionPoint`.
    pub fn deposit_only(self) -> bool {
        matches!(
            self,
            PointType::Funnel(_) | PointType::Campfire | PointType::RespawnAnchor
        )
    }
    /// `getInteractionPositionVector`, from the block's corner.
    pub fn interaction_offset(self) -> [f64; 3] {
        match self {
            // TopFaceArmInteractionPoint
            PointType::Chute => [0.5, 1.0, 0.5],
            // DepotPoint (and BeltPoint)
            PointType::Depot | PointType::Belt => [0.5, 14.0 / 16.0, 0.5],
            PointType::Deployer(f) => {
                let n = f.opposite().normal();
                [0, 1, 2].map(|i| 0.5 + f64::from(n[i]) * 0.65)
            }
            PointType::Funnel(f) => {
                let n = f.normal();
                [0, 1, 2].map(|i| 0.5 + f64::from(n[i]) * -0.15)
            }
            PointType::Basin | PointType::Millstone | PointType::Campfire => [0.5, 0.5, 0.5],
            PointType::Composter => [0.5, 13.0 / 16.0, 0.5],
            PointType::RespawnAnchor => [0.5, 1.0, 0.5],
        }
    }
    /// `getInteractionDirection`: the way the claw faces.
    pub fn interaction_direction(self) -> Dir {
        match self {
            PointType::Deployer(f) | PointType::Funnel(f) => f.opposite(),
            _ => Dir::Down,
        }
    }
}

/// One `ArmInteractionPoint`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub kind: PointType,
    pub pos: Pos,
    pub mode: Mode,
}

impl Point {
    /// A new point (`ArmInteractionPoint.create`): depositing.
    pub fn new(kind: PointType, pos: Pos) -> Self {
        Point {
            kind,
            pos,
            mode: Mode::Deposit,
        }
    }
    /// `cycleMode` (a no-op for deposit-only points).
    pub fn cycle_mode(&mut self) {
        if self.kind.deposit_only() {
            return;
        }
        self.mode = match self.mode {
            Mode::Deposit => Mode::Take,
            Mode::Take => Mode::Deposit,
        };
    }
    /// `getTargetAngles` of an arm at `arm`.
    pub fn target_angles(&self, arm: Pos, ceiling: bool) -> ArmAngles {
        let o = self.kind.interaction_offset();
        let target = [0, 1, 2].map(|i| f64::from(self.pos[i]) + o[i]);
        ArmAngles::target(arm, target, self.kind.interaction_direction(), ceiling)
    }
    /// `serialize(anchor)`: type, position relative to the arm, mode.
    pub fn serialize(&self, anchor: Pos) -> String {
        let facing = match self.kind {
            PointType::Deployer(f) | PointType::Funnel(f) => f.name(),
            _ => "",
        };
        format!(
            "{} {} {} {} {} {}",
            self.kind.name(),
            self.pos[0] - anchor[0],
            self.pos[1] - anchor[1],
            self.pos[2] - anchor[2],
            if self.mode == Mode::Take {
                "TAKE"
            } else {
                "DEPOSIT"
            },
            facing,
        )
    }
    /// `deserialize`: the point's type comes back from the block now there
    /// (`type.canCreatePoint`), through `kind_at`.
    pub fn deserialize(
        s: &str,
        anchor: Pos,
        kind_at: impl Fn(Pos) -> Option<PointType>,
    ) -> Option<Point> {
        let f: Vec<&str> = s.split(' ').collect();
        if f.len() < 5 {
            return None;
        }
        let rel: Vec<i32> = f[1..4].iter().filter_map(|v| v.parse().ok()).collect();
        if rel.len() != 3 {
            return None;
        }
        let pos = [anchor[0] + rel[0], anchor[1] + rel[1], anchor[2] + rel[2]];
        let kind = kind_at(pos)?;
        if kind.name() != f[0] {
            return None;
        }
        let mode = if f[4] == "TAKE" {
            Mode::Take
        } else {
            Mode::Deposit
        };
        Some(Point { kind, pos, mode })
    }
}

/// `BlockPos.closerThan`: whether a selected point is within the arm's range
/// (`ArmInteractionPointHandler.flushSettings`).
pub fn in_range(point: Pos, arm: Pos) -> bool {
    let d: i64 = (0..3).map(|i| i64::from(point[i] - arm[i]).pow(2)).sum();
    d < i64::from(RANGE * RANGE)
}

/// `ArmAngleTarget`: the arm's four angles for a target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArmAngles {
    pub base: f32,
    pub lower: f32,
    pub upper: f32,
    pub head: f32,
}

/// `ArmAngleTarget.NO_TARGET`: at rest.
pub const NO_TARGET: ArmAngles = ArmAngles {
    base: 0.0,
    lower: 135.0,
    upper: 45.0,
    head: 0.0,
};

fn deg(rad: f64) -> f32 {
    rad.to_degrees() as f32
}

impl ArmAngles {
    /// `new ArmAngleTarget(armPos, pointTarget, clawFacing, ceiling)`.
    pub fn target(arm: Pos, point_target: [f64; 3], claw_facing: Dir, ceiling: bool) -> ArmAngles {
        let origin = [
            f64::from(arm[0]) + 0.5,
            f64::from(arm[1]) + 0.5 + if ceiling { -6.0 / 16.0 } else { 6.0 / 16.0 },
            f64::from(arm[2]) + 0.5,
        ];
        let claw_target = point_target;
        let back = claw_facing.opposite().normal();
        let target = [0, 1, 2].map(|i| point_target[i] + f64::from(back[i]) * 0.5);
        let mut diff = [0, 1, 2].map(|i| target[i] - origin[i]);
        let horizontal = (diff[0] * diff[0] + diff[2] * diff[2]).sqrt() as f32;
        let mut base = deg(diff[0].atan2(diff[2])) + 180.0;
        if ceiling {
            diff[1] = -diff[1];
            base = 180.0 - base;
        }
        let alpha_offset = deg(diff[1].atan2(f64::from(horizontal)));
        let a = 14.0 / 16.0f32;
        let a2 = a * a;
        let b = 15.0 / 16.0f32;
        let b2 = b * b;
        let length = ((diff[1] * diff[1]) as f32 + horizontal * horizontal)
            .sqrt()
            .clamp(1.0 / 8.0, a + b);
        let length2 = length * length;
        let alpha_ratio = (-b2 + a2 + length2) / (2.0 * a * length);
        let mut alpha = deg(f64::from(alpha_ratio).acos()) + alpha_offset;
        let beta_ratio = (-length2 + a2 + b2) / (2.0 * b * a);
        let mut beta = deg(f64::from(beta_ratio).acos());
        if alpha.is_nan() {
            alpha = 0.0;
        }
        if beta.is_nan() {
            beta = 0.0;
        }
        let mut head = rotate_vec([0.0, b, 0.0], beta + 180.0, Axis::X);
        head[1] += a;
        head = rotate_vec(head, alpha - 90.0, Axis::X);
        head = rotate_vec(head, base, Axis::Y);
        head = rotate_vec(head, if ceiling { 180.0 } else { 0.0 }, Axis::X);
        let head_pos = [0, 1, 2].map(|i| f64::from(head[i]) + origin[i]);
        let mut head_diff = [0, 1, 2].map(|i| claw_target[i] - head_pos[i]);
        if ceiling {
            head_diff[1] = -head_diff[1];
        }
        let horizontal_head =
            (head_diff[0] * head_diff[0] + head_diff[2] * head_diff[2]).sqrt() as f32;
        let head_angle = alpha + beta + 135.0 - deg(head_diff[1].atan2(f64::from(horizontal_head)));
        ArmAngles {
            base,
            lower: alpha,
            upper: beta,
            head: -head_angle,
        }
    }
}

/// `AngleHelper.getShortestAngleDiff`.
fn shortest_angle_diff(current: f32, target: f32) -> f32 {
    let current = current % 360.0;
    let target = target % 360.0;
    (((target - current) % 360.0) + 540.0) % 360.0 - 180.0
}

/// `AngleHelper.angleLerp`.
pub fn angle_lerp(pct: f32, current: f32, target: f32) -> f32 {
    current + shortest_angle_diff(current, target) * pct
}

/// An item the arm carries: `ItemStack` as the arm uses it.
pub trait ArmStack: Clone + PartialEq {
    fn count(&self) -> u32;
    /// `ItemStack.isSameItem`: the same item, components aside.
    fn same_item(&self, other: &Self) -> bool;
}

/// The world as the arm's points reach it (their `IItemHandler`, through
/// `Capabilities.ItemHandler.BLOCK` from above).
pub trait ArmWorld<S> {
    /// `isValid`: the point's block is still of its type.
    fn is_valid(&self, point: &Point) -> bool;
    /// `getSlotCount`.
    fn slot_count(&mut self, point: &Point) -> usize;
    /// `extract(arm, slot, amount, simulate)`.
    fn extract(&mut self, point: &Point, slot: usize, amount: u32, simulate: bool) -> Option<S>;
    /// `insert(arm, stack, simulate)`: what is left.
    fn insert(&mut self, point: &Point, stack: S, simulate: bool) -> Option<S>;
    /// `keepAlive` (`BeltPoint`: the first item on the segment waits).
    fn keep_alive(&mut self, point: &Point);
}

/// `ArmBlockEntity`'s fields, as saved and synced.
#[derive(Clone, Debug, PartialEq)]
pub struct Arm<S> {
    pub inputs: Vec<Point>,
    pub outputs: Vec<Point>,
    pub chased_point_progress: f32,
    pub chased_point_index: i32,
    pub held: Option<S>,
    pub phase: Phase,
    pub selection_mode: SelectionMode,
    pub last_input_index: i32,
    pub last_output_index: i32,
    pub redstone_locked: bool,
    /// `goggles`: worn since goggles were used on it (drawn on its claw).
    pub goggles: bool,
    lazy_tick_counter: i32,
    /// Something changed for saving and clients (`setChanged`, `sendData`).
    pub dirty: bool,
    /// The client's animation (`previousTarget`, `previousBaseAngle`, the
    /// angles), kept here so the server can tell clients the angles.
    pub previous_target: ArmAngles,
    pub previous_base_angle: f32,
    pub angles: ArmAngles,
    /// A pick-up sound to play (`collectItem`).
    pub picked_up: bool,
}

impl<S: ArmStack> Default for Arm<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: ArmStack> Arm<S> {
    pub fn new() -> Self {
        Arm {
            inputs: Vec::new(),
            outputs: Vec::new(),
            chased_point_progress: 0.0,
            chased_point_index: 0,
            held: None,
            phase: Phase::SearchInputs,
            selection_mode: SelectionMode::RoundRobin,
            last_input_index: -1,
            last_output_index: -1,
            redstone_locked: false,
            goggles: false,
            lazy_tick_counter: LAZY_TICK_RATE,
            dirty: false,
            previous_target: NO_TARGET,
            previous_base_angle: NO_TARGET.base,
            angles: NO_TARGET,
            picked_up: false,
        }
    }

    /// The points as `initInteractionPoints` sorts them.
    pub fn set_points(&mut self, points: Vec<Point>) {
        self.inputs.clear();
        self.outputs.clear();
        for p in points {
            match p.mode {
                Mode::Deposit => self.outputs.push(p),
                Mode::Take => self.inputs.push(p),
            }
        }
        self.dirty = true;
    }

    /// `getTargetedInteractionPoint`.
    pub fn targeted(&self) -> Option<Point> {
        let i = usize::try_from(self.chased_point_index).ok()?;
        match self.phase {
            Phase::MoveToInput => self.inputs.get(i).copied(),
            Phase::MoveToOutput => self.outputs.get(i).copied(),
            _ => None,
        }
    }

    /// Phase and target change, as `sendData` tells clients: `read` keeps
    /// the point left as the animation's start.
    fn set_phase(&mut self, phase: Phase, index: i32, arm: Pos, ceiling: bool) {
        let previous = self.targeted();
        let changed = phase != self.phase || index != self.chased_point_index;
        self.phase = phase;
        self.chased_point_index = index;
        self.dirty = true;
        if changed {
            self.previous_target = previous.map_or(NO_TARGET, |p| p.target_angles(arm, ceiling));
            if previous.is_some() {
                self.previous_base_angle = self.previous_target.base;
            }
        }
    }

    /// `ArmBlockEntity.tick` on the server, `speed` its `getSpeed`.
    pub fn tick(&mut self, speed: f32, arm: Pos, ceiling: bool, world: &mut dyn ArmWorld<S>) {
        // SmartBlockEntity.tick: the lazy tick first.
        let counter = self.lazy_tick_counter;
        self.lazy_tick_counter -= 1;
        if counter <= 0 {
            self.lazy_tick_counter = LAZY_TICK_RATE;
            self.lazy_tick(arm, ceiling, world);
        }
        let reached = self.tick_movement_progress(speed, arm, ceiling);
        if self.chased_point_progress < 1.0 {
            if self.phase == Phase::MoveToInput
                && let Some(point) = self.targeted()
            {
                world.keep_alive(&point);
            }
            return;
        }
        match self.phase {
            Phase::MoveToInput => self.collect_item(arm, ceiling, world),
            Phase::MoveToOutput => self.deposit_item(arm, ceiling, world),
            Phase::SearchInputs | Phase::Dancing => self.search_for_item(arm, ceiling, world),
            Phase::SearchOutputs => {}
        }
        if reached {
            self.lazy_tick(arm, ceiling, world);
        }
    }

    /// `lazyTick`.
    fn lazy_tick(&mut self, arm: Pos, ceiling: bool, world: &mut dyn ArmWorld<S>) {
        if self.chased_point_progress < 0.5 {
            return;
        }
        // checkForMusic: no jukebox points here.
        if self.phase == Phase::SearchOutputs {
            self.search_for_destination(arm, ceiling, world);
        }
    }

    /// `tickMovementProgress`: whether the target was reached this tick;
    /// and the client's angles for it.
    fn tick_movement_progress(&mut self, speed: f32, arm: Pos, ceiling: bool) -> bool {
        let reached_before = self.chased_point_progress >= 1.0;
        self.chased_point_progress += speed.abs().min(256.0) / 1024.0;
        if self.chased_point_progress > 1.0 {
            self.chased_point_progress = 1.0;
        }
        self.animate(arm, ceiling);
        !reached_before && self.chased_point_progress >= 1.0
    }

    /// The client half of `tickMovementProgress`: the angles now.
    fn animate(&mut self, arm: Pos, ceiling: bool) {
        let progress = self.chased_point_progress;
        let target_point = self.targeted();
        let mut previous = self.previous_target;
        let mut target = target_point.map_or(NO_TARGET, |p| p.target_angles(arm, ceiling));
        let has_target = target_point.is_some();
        let base = angle_lerp(
            progress,
            self.previous_base_angle,
            if has_target {
                target.base
            } else {
                self.previous_base_angle
            },
        );
        // The arm first backs up to rest, then goes on.
        if progress < 0.5 {
            target = NO_TARGET;
        } else {
            previous = NO_TARGET;
        }
        let p = if progress == 1.0 {
            1.0
        } else {
            (progress % 0.5) * 2.0
        };
        self.angles = ArmAngles {
            base,
            lower: previous.lower + (target.lower - previous.lower) * p,
            upper: previous.upper + (target.upper - previous.upper) * p,
            head: angle_lerp(p, previous.head % 360.0, target.head % 360.0),
        };
    }

    /// `searchForItem`.
    fn search_for_item(&mut self, arm: Pos, ceiling: bool, world: &mut dyn ArmWorld<S>) {
        if self.redstone_locked {
            return;
        }
        let mut found = false;
        let start = if self.selection_mode == SelectionMode::PreferFirst {
            0
        } else {
            self.last_input_index + 1
        };
        let mut scan = if self.selection_mode == SelectionMode::ForcedRoundRobin {
            self.last_input_index + 2
        } else {
            self.inputs.len() as i32
        };
        if scan > self.inputs.len() as i32 {
            scan = self.inputs.len() as i32;
        }
        'points: for i in start.max(0)..scan {
            let point = self.inputs[i as usize];
            if !world.is_valid(&point) {
                continue;
            }
            for j in 0..world.slot_count(&point) {
                if self.distributable_amount(&point, j, world) == 0 {
                    continue;
                }
                self.select_index(true, i, arm, ceiling);
                found = true;
                break 'points;
            }
        }
        if !found && self.selection_mode == SelectionMode::RoundRobin {
            self.last_input_index = -1;
        }
        if self.last_input_index == self.inputs.len() as i32 - 1 {
            self.last_input_index = -1;
        }
    }

    /// `searchForDestination`.
    fn search_for_destination(&mut self, arm: Pos, ceiling: bool, world: &mut dyn ArmWorld<S>) {
        let Some(held) = self.held.clone() else {
            // ItemStack.matches(EMPTY, EMPTY): every output "takes" nothing.
            return;
        };
        let mut found = false;
        let start = if self.selection_mode == SelectionMode::PreferFirst {
            0
        } else {
            self.last_output_index + 1
        };
        let mut scan = if self.selection_mode == SelectionMode::ForcedRoundRobin {
            self.last_output_index + 2
        } else {
            self.outputs.len() as i32
        };
        if scan > self.outputs.len() as i32 {
            scan = self.outputs.len() as i32;
        }
        for i in start.max(0)..scan {
            let point = self.outputs[i as usize];
            if !world.is_valid(&point) {
                continue;
            }
            let remainder = world.insert(&point, held.clone(), true);
            if remainder.as_ref() == Some(&held) {
                continue;
            }
            self.select_index(false, i, arm, ceiling);
            found = true;
            break;
        }
        if !found && self.selection_mode == SelectionMode::RoundRobin {
            self.last_output_index = -1;
        }
        if self.last_output_index == self.outputs.len() as i32 - 1 {
            self.last_output_index = -1;
        }
    }

    /// `selectIndex`.
    fn select_index(&mut self, input: bool, index: i32, arm: Pos, ceiling: bool) {
        let phase = if input {
            Phase::MoveToInput
        } else {
            Phase::MoveToOutput
        };
        self.set_phase(phase, index, arm, ceiling);
        self.chased_point_progress = 0.0;
        if input {
            self.last_input_index = index;
        } else {
            self.last_output_index = index;
        }
    }

    /// `getDistributableAmount`.
    fn distributable_amount(&self, point: &Point, slot: usize, world: &mut dyn ArmWorld<S>) -> u32 {
        let Some(stack) = world.extract(point, slot, 64, true) else {
            return 0;
        };
        let remainder = self.simulate_insertion(stack.clone(), world);
        match remainder {
            Some(r) if r.same_item(&stack) => stack.count() - r.count(),
            _ => stack.count(),
        }
    }

    /// `simulateInsertion`.
    fn simulate_insertion(&self, stack: S, world: &mut dyn ArmWorld<S>) -> Option<S> {
        let mut stack = Some(stack);
        for point in &self.outputs {
            if world.is_valid(point)
                && let Some(s) = stack.take()
            {
                stack = world.insert(point, s, true);
            }
            if stack.is_none() {
                break;
            }
        }
        stack
    }

    /// `depositItem`.
    fn deposit_item(&mut self, arm: Pos, ceiling: bool, world: &mut dyn ArmWorld<S>) {
        if let Some(point) = self.targeted()
            && world.is_valid(&point)
            && let Some(held) = self.held.take()
        {
            self.held = world.insert(&point, held, false);
        }
        let phase = if self.held.is_none() {
            Phase::SearchInputs
        } else {
            Phase::SearchOutputs
        };
        self.set_phase(phase, -1, arm, ceiling);
        self.chased_point_progress = 0.0;
    }

    /// `collectItem`.
    fn collect_item(&mut self, arm: Pos, ceiling: bool, world: &mut dyn ArmWorld<S>) {
        if let Some(point) = self.targeted()
            && world.is_valid(&point)
        {
            for i in 0..world.slot_count(&point) {
                let amount = self.distributable_amount(&point, i, world);
                if amount == 0 {
                    continue;
                }
                let previous = self.held.clone();
                self.held = world.extract(&point, i, amount, false);
                self.set_phase(Phase::SearchOutputs, -1, arm, ceiling);
                self.chased_point_progress = 0.0;
                let same = match (&self.held, &previous) {
                    (Some(a), Some(b)) => a.same_item(b),
                    (None, None) => true,
                    _ => false,
                };
                self.picked_up |= !same;
                return;
            }
        }
        self.set_phase(Phase::SearchInputs, -1, arm, ceiling);
        self.chased_point_progress = 0.0;
    }

    /// `ArmBlock.useItemOn`: the held item back to a player.
    pub fn take_held(&mut self, arm: Pos, ceiling: bool) -> Option<S> {
        let held = self.held.take()?;
        self.set_phase(Phase::SearchInputs, self.chased_point_index, arm, ceiling);
        Some(held)
    }

    /// `redstoneUpdate` with the block's neighbour signal.
    pub fn redstone_update(
        &mut self,
        powered: bool,
        arm: Pos,
        ceiling: bool,
        world: &mut dyn ArmWorld<S>,
    ) {
        if powered == self.redstone_locked {
            return;
        }
        self.redstone_locked = powered;
        self.dirty = true;
        if !self.redstone_locked {
            self.search_for_item(arm, ceiling, world);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Clone, Debug, PartialEq)]
    struct Item(&'static str, u32);

    impl ArmStack for Item {
        fn count(&self) -> u32 {
            self.1
        }
        fn same_item(&self, other: &Self) -> bool {
            self.0 == other.0
        }
    }

    /// Depots: one slot each, holding any amount of one item.
    #[derive(Default)]
    struct Depots(HashMap<Pos, Option<Item>>);

    impl ArmWorld<Item> for Depots {
        fn is_valid(&self, point: &Point) -> bool {
            self.0.contains_key(&point.pos)
        }
        fn slot_count(&mut self, _: &Point) -> usize {
            1
        }
        fn extract(
            &mut self,
            point: &Point,
            _: usize,
            amount: u32,
            simulate: bool,
        ) -> Option<Item> {
            let slot = self.0.get_mut(&point.pos)?;
            let held = slot.clone()?;
            let n = held.1.min(amount);
            if !simulate {
                *slot = (held.1 > n).then(|| Item(held.0, held.1 - n));
            }
            Some(Item(held.0, n))
        }
        fn insert(&mut self, point: &Point, stack: Item, simulate: bool) -> Option<Item> {
            let Some(slot) = self.0.get_mut(&point.pos) else {
                return Some(stack);
            };
            if slot.is_some() {
                return Some(stack);
            }
            if !simulate {
                *slot = Some(stack);
            }
            None
        }
        fn keep_alive(&mut self, _: &Point) {}
    }

    const ARM: Pos = [0, 64, 0];

    fn arm_between(world: &mut Depots) -> Arm<Item> {
        world.0.insert([2, 64, 0], Some(Item("iron", 3)));
        world.0.insert([-2, 64, 0], None);
        let mut arm = Arm::new();
        let mut input = Point::new(PointType::Depot, [2, 64, 0]);
        input.cycle_mode();
        arm.set_points(vec![input, Point::new(PointType::Depot, [-2, 64, 0])]);
        arm
    }

    /// Ticks a new arm until it sets off for its input: it starts at
    /// progress 0 and searches only once its first move is done.
    fn set_off(arm: &mut Arm<Item>, world: &mut Depots) -> u32 {
        let mut ticks = 0;
        while arm.phase != Phase::MoveToInput {
            arm.tick(64.0, ARM, false, world);
            ticks += 1;
        }
        ticks
    }

    #[test]
    fn the_arm_carries_a_stack_from_input_to_output() {
        let mut world = Depots::default();
        let mut arm = arm_between(&mut world);
        let mut phases = vec![arm.phase];
        for _ in 0..200 {
            arm.tick(64.0, ARM, false, &mut world);
            if phases.last() != Some(&arm.phase) {
                phases.push(arm.phase);
            }
        }
        assert_eq!(world.0[&[-2, 64, 0]], Some(Item("iron", 3)));
        assert_eq!(world.0[&[2, 64, 0]], None);
        assert_eq!(
            phases,
            vec![
                Phase::SearchInputs,
                Phase::MoveToInput,
                Phase::SearchOutputs,
                Phase::MoveToOutput,
                Phase::SearchInputs
            ]
        );
    }

    #[test]
    fn movement_takes_1024_over_the_speed_ticks() {
        let mut world = Depots::default();
        let mut arm = arm_between(&mut world);
        assert_eq!(set_off(&mut arm, &mut world), 16);
        let mut ticks = 0;
        while arm.phase == Phase::MoveToInput {
            arm.tick(64.0, ARM, false, &mut world);
            ticks += 1;
        }
        // 64/1024 a tick: 16 ticks to the point, collected on the 16th.
        assert_eq!(ticks, 16);
        assert_eq!(arm.held, Some(Item("iron", 3)));
    }

    #[test]
    fn no_output_with_room_means_nothing_is_taken() {
        let mut world = Depots::default();
        let mut arm = arm_between(&mut world);
        world.0.insert([-2, 64, 0], Some(Item("gold", 1)));
        for _ in 0..100 {
            arm.tick(64.0, ARM, false, &mut world);
        }
        assert_eq!(arm.phase, Phase::SearchInputs);
        assert_eq!(arm.held, None);
    }

    #[test]
    fn a_stopped_arm_never_moves() {
        let mut world = Depots::default();
        let mut arm = arm_between(&mut world);
        set_off(&mut arm, &mut world);
        for _ in 0..100 {
            arm.tick(0.0, ARM, false, &mut world);
        }
        assert_eq!(arm.phase, Phase::MoveToInput);
        assert_eq!(arm.chased_point_progress, 0.0);
        // A new one that never turned never even searches.
        let mut idle = arm_between(&mut world);
        for _ in 0..100 {
            idle.tick(0.0, ARM, false, &mut world);
        }
        assert_eq!(idle.phase, Phase::SearchInputs);
    }

    #[test]
    fn redstone_locks_the_search() {
        let mut world = Depots::default();
        let mut arm = arm_between(&mut world);
        arm.redstone_update(true, ARM, false, &mut world);
        for _ in 0..50 {
            arm.tick(64.0, ARM, false, &mut world);
        }
        assert_eq!(arm.phase, Phase::SearchInputs);
        arm.redstone_update(false, ARM, false, &mut world);
        assert_eq!(arm.phase, Phase::MoveToInput);
    }

    #[test]
    fn funnels_are_deposit_only() {
        let mut p = Point::new(PointType::Funnel(Dir::North), [0, 0, 0]);
        p.cycle_mode();
        assert_eq!(p.mode, Mode::Deposit);
        let mut d = Point::new(PointType::Depot, [0, 0, 0]);
        d.cycle_mode();
        assert_eq!(d.mode, Mode::Take);
    }

    #[test]
    fn range_is_strictly_under_five() {
        assert!(in_range([4, 64, 0], ARM));
        assert!(!in_range([5, 64, 0], ARM));
        assert!(in_range([3, 67, 0], ARM) == (9 + 9 < 25));
    }

    #[test]
    fn angles_point_the_base_at_the_target() {
        // A depot straight south: atan2(0, 2) + 180 = 180.
        let a = Point::new(PointType::Depot, [0, 64, 2]).target_angles(ARM, false);
        assert!((a.base - 180.0).abs() < 1e-3);
        // Straight east: atan2(2, 0) = 90, + 180.
        let e = Point::new(PointType::Depot, [2, 64, 0]).target_angles(ARM, false);
        assert!((e.base - 270.0).abs() < 1e-3);
        // Within reach: both joints bend (no NaN).
        assert!(a.lower.is_finite() && a.upper > 0.0 && a.head.is_finite());
    }

    #[test]
    fn the_arm_backs_up_to_rest_then_reaches_out() {
        let mut world = Depots::default();
        let mut arm = arm_between(&mut world);
        set_off(&mut arm, &mut world);
        // Halfway there the arm is at rest.
        for _ in 0..8 {
            arm.tick(64.0, ARM, false, &mut world);
        }
        assert!((arm.chased_point_progress - 0.5).abs() < 1e-6);
        assert!((arm.angles.lower - NO_TARGET.lower).abs() < 1e-3);
        let target = Point::new(PointType::Depot, [2, 64, 0]).target_angles(ARM, false);
        for _ in 0..7 {
            arm.tick(64.0, ARM, false, &mut world);
        }
        arm.animate(ARM, false);
        assert!((arm.chased_point_progress - 15.0 / 16.0).abs() < 1e-6);
        assert!(arm.angles.lower != NO_TARGET.lower);
        // The base turns all the way over the whole move.
        let mut a = arm.clone();
        a.chased_point_progress = 1.0;
        a.animate(ARM, false);
        assert!((a.angles.lower - target.lower).abs() < 1e-3);
        // The short way round: -90 is the target's 270.
        assert!(((a.angles.base - target.base).rem_euclid(360.0)).abs() < 1e-3);
    }

    #[test]
    fn points_round_trip_relative_to_the_arm() {
        let p = Point {
            kind: PointType::Funnel(Dir::East),
            pos: [3, 65, -1],
            mode: Mode::Deposit,
        };
        let s = p.serialize(ARM);
        assert_eq!(s, "create:funnel 3 1 -1 DEPOSIT east");
        let back = Point::deserialize(&s, ARM, |_| Some(PointType::Funnel(Dir::East)));
        assert_eq!(back, Some(p));
        assert_eq!(
            Point::deserialize(&s, ARM, |_| Some(PointType::Depot)),
            None
        );
    }

    #[test]
    fn angle_lerp_takes_the_short_way() {
        assert!((angle_lerp(0.5, 350.0, 10.0) - 360.0).abs() < 1e-3);
        assert!((angle_lerp(0.5, 10.0, 350.0) - 0.0).abs() < 1e-3);
    }
}
