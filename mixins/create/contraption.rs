// Create's contraptions as milestone 5 part 1 needs them, injected as
// `crate::game::create_contraption`: `Contraption.searchMovedStructure` and
// `moveBlock` with `BlockMovementChecks` (asked of the world as `Facts`),
// `PistonContraption` (its poles, head and the blocks in front),
// `TranslatingContraption`'s colliders and `ContraptionCollider`'s world
// check, and `LinearActuatorBlockEntity` / `MechanicalPistonBlockEntity`'s
// motion. Contraptions are no entities here: the glue takes their blocks out
// of the world, draws them moving and puts them back. No Horizon types.

use super::create_kinetics::{Axis, Dir, Pos};
use std::collections::{HashSet, VecDeque};

/// `CKinetics` defaults.
pub const MAX_BLOCKS_MOVED: usize = 2048;
pub const MAX_CHASSIS_RANGE: i32 = 16;
pub const MAX_PISTON_POLES: i32 = 64;
pub const MIN_WINDMILL_SAILS: usize = 8;
pub const WINDMILL_SAILS_PER_RPM: usize = 8;

/// `MechanicalPistonBlock.STATE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PistonState {
    Retracted,
    Moving,
    Extended,
}

impl PistonState {
    #[allow(dead_code)]
    pub const ALL: [PistonState; 3] = [
        PistonState::Retracted,
        PistonState::Moving,
        PistonState::Extended,
    ];
    #[allow(dead_code)]
    pub fn name(self) -> &'static str {
        match self {
            PistonState::Retracted => "retracted",
            PistonState::Moving => "moving",
            PistonState::Extended => "extended",
        }
    }
}

/// The blocks of the piston's parts (prepare_content.py's `piston_blocks`).
pub const POLE: &str = "create:piston_extension_pole";
pub const HEAD: &str = "create:mechanical_piston_head";
pub const STICKY_HEAD: &str = "create:hz_mechanical_piston_head_sticky";
/// (sticky, axis_along_first, state) for each piston block.
pub const PISTON_BLOCKS: [(&str, bool, bool, PistonState); 12] = [
    (
        "create:mechanical_piston",
        false,
        false,
        PistonState::Retracted,
    ),
    (
        "create:hz_mechanical_piston_across_moving",
        false,
        false,
        PistonState::Moving,
    ),
    (
        "create:hz_mechanical_piston_across_extended",
        false,
        false,
        PistonState::Extended,
    ),
    (
        "create:hz_mechanical_piston_along_retracted",
        false,
        true,
        PistonState::Retracted,
    ),
    (
        "create:hz_mechanical_piston_along_moving",
        false,
        true,
        PistonState::Moving,
    ),
    (
        "create:hz_mechanical_piston_along_extended",
        false,
        true,
        PistonState::Extended,
    ),
    (
        "create:sticky_mechanical_piston",
        true,
        false,
        PistonState::Retracted,
    ),
    (
        "create:hz_sticky_mechanical_piston_across_moving",
        true,
        false,
        PistonState::Moving,
    ),
    (
        "create:hz_sticky_mechanical_piston_across_extended",
        true,
        false,
        PistonState::Extended,
    ),
    (
        "create:hz_sticky_mechanical_piston_along_retracted",
        true,
        true,
        PistonState::Retracted,
    ),
    (
        "create:hz_sticky_mechanical_piston_along_moving",
        true,
        true,
        PistonState::Moving,
    ),
    (
        "create:hz_sticky_mechanical_piston_along_extended",
        true,
        true,
        PistonState::Extended,
    ),
];

/// A piston block's (sticky, axis_along_first, state).
pub fn piston_of(name: &str) -> Option<(bool, bool, PistonState)> {
    PISTON_BLOCKS
        .iter()
        .find(|(n, ..)| *n == name)
        .map(|&(_, sticky, along, state)| (sticky, along, state))
}

/// The block of a piston in `state`.
pub fn piston_block(sticky: bool, along: bool, state: PistonState) -> &'static str {
    PISTON_BLOCKS
        .iter()
        .find(|&&(_, s, a, st)| s == sticky && a == along && st == state)
        .map_or(PISTON_BLOCKS[0].0, |(n, ..)| n)
}

/// `AbstractChassisBlock`'s kinds: `LinearChassisBlock.sameKind` tells the
/// two linear ones apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChassisKind {
    Linear,
    SecondaryLinear,
    Radial,
}

impl ChassisKind {
    fn base(self) -> &'static str {
        match self {
            ChassisKind::Linear => "linear_chassis",
            ChassisKind::SecondaryLinear => "secondary_linear_chassis",
            ChassisKind::Radial => "radial_chassis",
        }
    }
    /// The sticky properties, in the order of their bits.
    fn props(self) -> &'static [&'static str] {
        match self {
            ChassisKind::Radial => &["north", "east", "south", "west"],
            _ => &["top", "bottom"],
        }
    }
}

/// A chassis block's name: `create:<kind>` with no sticky side, else a
/// hidden `create:hz_<kind>_<sides>` (Horizon's log template has `axis`
/// only). `sticky` holds a bit per property of `ChassisKind::props`.
pub fn chassis_block(kind: ChassisKind, sticky: u8) -> String {
    if sticky == 0 {
        return format!("create:{}", kind.base());
    }
    let sides: Vec<&str> = (kind.props().iter().enumerate())
        .filter(|(i, _)| sticky & (1 << i) != 0)
        .map(|(_, p)| *p)
        .collect();
    format!("create:hz_{}_{}", kind.base(), sides.join("_"))
}

/// The kind and sticky bits of a chassis block's name.
pub fn chassis_of(name: &str) -> Option<(ChassisKind, u8)> {
    let path = name.strip_prefix("create:")?;
    for kind in [
        ChassisKind::Linear,
        ChassisKind::SecondaryLinear,
        ChassisKind::Radial,
    ] {
        if path == kind.base() {
            return Some((kind, 0));
        }
        let Some(sides) = path
            .strip_prefix("hz_")
            .and_then(|p| p.strip_prefix(kind.base()))
        else {
            continue;
        };
        let Some(sides) = sides.strip_prefix('_') else {
            continue;
        };
        let mut bits = 0u8;
        for side in sides.split('_') {
            let i = kind.props().iter().position(|p| *p == side)?;
            bits |= 1 << i;
        }
        if chassis_block(kind, bits) == name {
            return Some((kind, bits));
        }
    }
    None
}

/// `getGlueableSide`: the bit of the sticky property on `face`, none when
/// that face takes no slime.
pub fn glueable_side(kind: ChassisKind, axis: Axis, face: Dir) -> Option<u8> {
    use Dir::*;
    const N: u8 = 1;
    const E: u8 = 2;
    const S: u8 = 4;
    const W: u8 = 8;
    if kind != ChassisKind::Radial {
        if face.axis() != axis {
            return None;
        }
        return Some(if face.positive() { 1 } else { 2 });
    }
    match (axis, face) {
        (Axis::X, North) => Some(W),
        (Axis::X, South) => Some(E),
        (Axis::X, Up) => Some(N),
        (Axis::X, Down) => Some(S),
        (Axis::Y, North) => Some(N),
        (Axis::Y, South) => Some(S),
        (Axis::Y, East) => Some(E),
        (Axis::Y, West) => Some(W),
        (Axis::Z, Up) => Some(N),
        (Axis::Z, Down) => Some(S),
        (Axis::Z, East) => Some(E),
        (Axis::Z, West) => Some(W),
        _ => None,
    }
}

/// `StickerBlock.neighborChanged`: `POWERED`, and a rising edge flips `EXTENDED`.
pub fn sticker_state(was_powered: bool, extended: bool, signal: bool) -> (bool, bool) {
    if signal == was_powered {
        return (was_powered, extended);
    }
    (signal, if signal { !extended } else { extended })
}

/// A chassis, as the structure search sees it: its block, axis, sticky
/// bits and `ChassisBlockEntity.range`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chassis {
    pub kind: ChassisKind,
    pub axis: Axis,
    pub sticky: u8,
    pub range: i32,
}

impl Chassis {
    /// Whether `face` is a sticky side (`state.getValue(getGlueableSide)`).
    pub fn sticky_on(&self, face: Dir) -> bool {
        glueable_side(self.kind, self.axis, face).is_some_and(|b| self.sticky & b != 0)
    }

    /// `glueAllowedOnSide`: a linear chassis takes no slime towards one of
    /// its kind on its axis.
    pub fn glue_allowed(&self, other: Option<&Chassis>) -> bool {
        if self.kind == ChassisKind::Radial {
            return true;
        }
        !other.is_some_and(|o| o.kind == self.kind && o.axis == self.axis)
    }

    /// `AbstractChassisBlock.useItemOn` with slime (`slime`) or an empty
    /// hand while sneaking: the new sticky bits, none when nothing changes.
    /// `neighbour` answers the chassis on each side.
    pub fn use_item(
        &self,
        face: Dir,
        slime: bool,
        neighbour: impl Fn(Dir) -> Option<Chassis>,
    ) -> Option<u8> {
        let side = glueable_side(self.kind, self.axis, face)?;
        if slime && self.sticky & side != 0 {
            // Slime on a sticky side makes every allowed side sticky.
            let mut bits = self.sticky;
            for d in Dir::ALL {
                if let Some(b) = glueable_side(self.kind, self.axis, d)
                    && bits & b == 0
                    && self.glue_allowed(neighbour(d).as_ref())
                {
                    bits |= b;
                }
            }
            return Some(bits);
        }
        if (self.sticky & side != 0) == slime {
            return None;
        }
        if !self.glue_allowed(neighbour(face).as_ref()) {
            return None;
        }
        Some(if slime {
            self.sticky | side
        } else {
            self.sticky & !side
        })
    }

    /// `LinearChassisBlock.updateShape`: a side facing one of its kind on
    /// its axis loses its slime.
    pub fn update_shape(&self, side: Dir, other: Option<&Chassis>) -> u8 {
        if self.kind == ChassisKind::Radial {
            return self.sticky;
        }
        match glueable_side(self.kind, self.axis, side) {
            Some(b) if other.is_some_and(|o| o.kind == self.kind && o.axis == self.axis) => {
                self.sticky & !b
            }
            _ => self.sticky,
        }
    }
}

/// The piston's own blocks, as the structure search sees them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PistonPart {
    Piston {
        facing: Dir,
        sticky: bool,
        state: PistonState,
    },
    Pole {
        facing: Dir,
    },
    Head {
        facing: Dir,
        sticky: bool,
    },
}

/// `isStickyBlock`: slime and honey.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sticky {
    Slime,
    Honey,
}

/// What `BlockMovementChecks` and `Contraption.moveBlock` ask of a block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// `isMovementNecessary` (air and replaceable blocks without collision
    /// are left behind).
    pub necessary: bool,
    /// `isMovementAllowed` (unbreakable blocks, `PushReaction.BLOCK`...);
    /// a moving piston is refused here, by `allowed`.
    pub allowed: bool,
    /// `isBrittle`: torches, ladders, rails, carpets...
    pub brittle: bool,
    /// `WoolCarpetBlock`, which a piston pushes although it is brittle.
    pub carpet: bool,
    /// `PushReaction.PUSH_ONLY`.
    pub push_only: bool,
    pub sticky: Option<Sticky>,
    /// `isBlockAttachedTowards`, by `Dir` index.
    pub attached: [bool; 6],
    /// `isNotSupportive`, by `Dir` index (the piston's own parts are worked
    /// out here).
    pub not_supportive: [bool; 6],
    /// A collision shape (`getCollisionShape` not empty).
    pub solid: bool,
    /// `canBeReplaced`.
    pub replaceable: bool,
    pub part: Option<PistonPart>,
    pub chassis: Option<Chassis>,
    /// `AllBlockTags.WINDMILL_SAILS`.
    pub sail: bool,
    /// `AllBlocks.PULLEY_MAGNET`.
    pub magnet: bool,
    /// An extended sticker: the way it holds on to the block before it.
    pub sticker: Option<Dir>,
}

impl Facts {
    fn allowed(&self) -> bool {
        // isMovementAllowedFallback: pistons only when they aren't moving.
        if let Some(PistonPart::Piston { state, .. }) = self.part {
            return state != PistonState::Moving;
        }
        self.allowed
    }
    fn not_supportive(&self, d: Dir) -> bool {
        match self.part {
            Some(PistonPart::Pole { facing } | PistonPart::Head { facing, .. }) => {
                d.axis() != facing.axis()
            }
            _ => self.not_supportive[d as usize],
        }
    }
    /// `ContraptionCollider.isCollidingWithWorld` for a block in the way.
    pub fn blocks_contraption(&self) -> bool {
        self.solid && !self.replaceable
    }
}

/// NeoForge's `canStickTo`: either is sticky, but not slime against honey.
fn can_stick(a: Option<Sticky>, b: Option<Sticky>) -> bool {
    match (a, b) {
        (Some(Sticky::Slime), Some(Sticky::Honey)) | (Some(Sticky::Honey), Some(Sticky::Slime)) => {
            false
        }
        (a, b) => a.is_some() || b.is_some(),
    }
}

/// The level, as a contraption's assembly sees it.
pub trait StructureWorld {
    type State: Clone;
    fn loaded(&self, pos: Pos) -> bool;
    fn outside_height(&self, pos: Pos) -> bool;
    fn facts(&self, pos: Pos) -> Facts;
    fn state(&self, pos: Pos) -> Self::State;
    /// `MECHANICAL_PISTON_HEAD` facing `facing`.
    fn head(&self, facing: Dir, sticky: bool) -> Self::State;
    /// `PISTON_EXTENSION_POLE` facing `facing`.
    fn pole(&self, facing: Dir) -> Self::State;
    /// `SuperGlueEntity.isGlued`: the glue holding `pos` to its neighbour
    /// towards `dir`, by an index of the world's choosing.
    fn glue(&self, _pos: Pos, _dir: Dir) -> Option<usize> {
        None
    }
}

/// `ChassisBlockEntity.addAttachedChasses`: the chassis this one joins
/// (a radial one along its axis, a linear one on any side, of its kind and
/// axis).
pub fn attached_chassis<W: StructureWorld>(world: &W, pos: Pos, c: &Chassis) -> Vec<Pos> {
    let mut out = Vec::new();
    for d in Dir::ALL {
        if c.kind == ChassisKind::Radial && d.axis() != c.axis {
            continue;
        }
        let next = step(pos, d, 1);
        if !world.loaded(next) {
            break;
        }
        if world
            .facts(next)
            .chassis
            .is_some_and(|o| o.kind == c.kind && o.axis == c.axis)
        {
            out.push(next);
        }
    }
    out
}

/// `ChassisBlockEntity.getIncludedBlockPositions` for a contraption moving
/// towards `forced`.
pub fn chassis_positions<W: StructureWorld>(
    world: &W,
    pos: Pos,
    c: &Chassis,
    forced: Option<Dir>,
) -> Vec<Pos> {
    let mut out = Vec::new();
    if c.kind != ChassisKind::Radial {
        // getIncludedBlockPositionsLinear: along the axis, both ways.
        for facing in [
            Dir::positive_of(c.axis),
            Dir::positive_of(c.axis).opposite(),
        ] {
            let sticky = c.sticky_on(facing);
            for i in 1..=c.range {
                let current = step(pos, facing, i);
                if forced != Some(facing) && !sticky {
                    break;
                }
                let f = world.facts(current);
                if !f.necessary || f.brittle {
                    break;
                }
                out.push(current);
                if f.not_supportive(facing) {
                    break;
                }
            }
        }
        return out;
    }
    // getIncludedBlockPositionsRadial: a flood in the plane from each
    // sticky side, within the radius.
    let reach = (c.range as f64 + 0.5).powi(2);
    for facing in Dir::ALL {
        if facing.axis() == c.axis || !c.sticky_on(facing) {
            continue;
        }
        let mut frontier = VecDeque::from([step(pos, facing, 1)]);
        let mut visited: HashSet<Pos> = HashSet::new();
        while let Some(search) = frontier.pop_front() {
            if visited.contains(&search) {
                continue;
            }
            let d2: i32 = (0..3).map(|k| (search[k] - pos[k]).pow(2)).sum();
            if f64::from(d2) >= reach {
                continue;
            }
            let f = world.facts(search);
            if !f.necessary || f.brittle {
                continue;
            }
            visited.insert(search);
            if search != pos {
                out.push(search);
            }
            for offset in Dir::ALL {
                if offset.axis() == c.axis {
                    continue;
                }
                if search == pos && offset != facing {
                    continue;
                }
                if f.not_supportive(offset) {
                    continue;
                }
                frontier.push_back(step(search, offset, 1));
            }
        }
    }
    out
}

/// `SuperGlueEntity.isValidFace`: glue holds on `dir` of this block.
pub fn glue_valid_face(f: &Facts, dir: Dir) -> bool {
    f.attached[dir as usize] || (f.necessary && !f.not_supportive(dir))
}

/// `SuperGlueEntity.isSideSticky`, for the blocks ported: a sticky piston's
/// front, slime and honey, a chassis' sticky sides.
pub fn side_sticky(f: &Facts, dir: Dir) -> bool {
    match f.part {
        Some(PistonPart::Piston {
            facing,
            sticky: true,
            ..
        }) => return facing == dir,
        Some(PistonPart::Piston { .. }) => return false,
        _ => {}
    }
    f.sticky.is_some() || f.chassis.is_some_and(|c| c.sticky_on(dir))
}

/// The blocks inside `SuperGlueEntity.span(a, b)` (both corners included).
pub fn in_span(a: Pos, b: Pos, p: Pos) -> bool {
    (0..3).all(|k| a[k].min(b[k]) <= p[k] && p[k] <= a[k].max(b[k]))
}

/// `SuperGlueSelectionHelper.searchGlueGroup` as the server asks it
/// (`includeOther` false): the blocks glue from `start` to `end` would
/// join, none when it would hold nothing.
pub fn glue_group<W: StructureWorld>(world: &W, start: Pos, end: Pos) -> Option<HashSet<Pos>> {
    let mut frontier = VecDeque::from([start]);
    let mut visited = HashSet::from([start]);
    let mut attached = HashSet::new();
    while let Some(current) = frontier.pop_front() {
        attached.insert(current);
        let cf = world.facts(current);
        for d in Dir::ALL {
            let offset = step(current, d, 1);
            let of = world.facts(offset);
            let already_sticky = side_sticky(&of, d.opposite());
            if !already_sticky && !in_span(start, end, offset) {
                continue;
            }
            if !of.necessary {
                continue;
            }
            if !glue_valid_face(&cf, d) || !glue_valid_face(&of, d.opposite()) {
                continue;
            }
            if visited.insert(offset) {
                frontier.push_back(offset);
            }
        }
    }
    if attached.len() < 2 && attached.contains(&end) {
        return None;
    }
    Some(attached)
}

/// `AssemblyException`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssemblyError {
    TooManyPoles,
    NoPoles,
    Unmovable(Pos),
    Unloaded(Pos),
    TooLarge,
    /// `notEnoughSails`: a windmill with fewer than `MIN_WINDMILL_SAILS`.
    NotEnoughSails(usize),
}

/// A moved block: its place relative to the contraption's anchor, its
/// state, and what its placement and collisions need of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Moved<S> {
    pub local: Pos,
    pub state: S,
    pub solid: bool,
    pub brittle: bool,
    /// Whether it is one of the piston's poles (`isExtensionPole`).
    pub pole: bool,
}

fn add(a: Pos, b: Pos) -> Pos {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: Pos, b: Pos) -> Pos {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn step(p: Pos, d: Dir, n: i32) -> Pos {
    let v = d.normal();
    [p[0] + v[0] * n, p[1] + v[1] * n, p[2] + v[2] * n]
}

/// `Contraption.searchMovedStructure`'s loop of `moveBlock` from `frontier`
/// (`forced` none for a bearing): the blocks taken, in order, with their
/// facts; the glue found holding them added to `glue`. `already` blocks
/// count towards `maxBlocksMoved`.
pub fn search_structure<W: StructureWorld>(
    world: &W,
    mut frontier: VecDeque<Pos>,
    anchoring: &HashSet<Pos>,
    forced: Option<Dir>,
    glue: &mut Vec<usize>,
    already: usize,
) -> Result<Vec<(Pos, Facts)>, AssemblyError> {
    let mut out = Vec::new();
    let mut visited: HashSet<Pos> = HashSet::new();
    while let Some(pos) = frontier.pop_front() {
        visited.insert(pos);
        if world.outside_height(pos) || anchoring.contains(&pos) {
            continue;
        }
        if !world.loaded(pos) {
            return Err(AssemblyError::Unloaded(pos));
        }
        let f = world.facts(pos);
        if !f.necessary {
            continue;
        }
        if !f.allowed() {
            return Err(AssemblyError::Unmovable(pos));
        }
        // moveChassis: its group, and the blocks it holds.
        if let Some(c) = f.chassis {
            let mut held = attached_chassis(world, pos, &c);
            held.extend(chassis_positions(world, pos, &c, forced));
            for p in held {
                if !visited.contains(&p) && !frontier.contains(&p) {
                    frontier.push_back(p);
                }
            }
        }
        // Slime blocks and super glue drag adjacent blocks.
        for dir in Dir::ALL {
            let next = step(pos, dir, 1);
            if anchoring.contains(&next) {
                continue;
            }
            let nf = world.facts(next);
            if !nf.allowed() {
                if Some(dir) == forced {
                    return Err(AssemblyError::Unmovable(pos));
                }
                continue;
            }
            let attached = nf.attached[dir.opposite() as usize];
            let mut stick =
                !nf.brittle && can_stick(f.sticky, nf.sticky) && can_stick(nf.sticky, f.sticky);
            if stick
                && (f.push_only
                    || nf.push_only
                    || f.not_supportive(dir)
                    || nf.not_supportive(dir.opposite()))
            {
                stick = false;
            }
            let pushed = Some(dir) == forced && !f.not_supportive(dir);
            let glued = world.glue(pos, dir);
            if let Some(g) = glued
                && !glue.contains(&g)
            {
                glue.push(g);
            }
            // Contraption.moveBlock: an extended sticker brings what it holds.
            let held = f.sticker == Some(dir) && !nf.not_supportive(dir.opposite());
            if !visited.contains(&next)
                && !frontier.contains(&next)
                && (stick || attached || pushed || held || glued.is_some())
            {
                frontier.push_back(next);
            }
        }
        out.push((pos, f));
        if already + out.len() > MAX_BLOCKS_MOVED {
            return Err(AssemblyError::TooLarge);
        }
    }
    Ok(out)
}

/// `PistonContraption`: a mechanical piston's poles and head, and what they
/// push or pull.
#[derive(Clone, Debug, PartialEq)]
pub struct PistonContraption<S> {
    pub orientation: Dir,
    pub sticky: bool,
    /// The block in front of the piston when it was assembled
    /// (`pos.relative(direction)`).
    pub anchor: Pos,
    pub blocks: Vec<Moved<S>>,
    pub extension_length: i32,
    pub initial_extension_progress: i32,
    /// The glue found holding the moved blocks (`glueToRemove`), by the
    /// world's indices.
    pub glue: Vec<usize>,
}

impl<S: Clone> PistonContraption<S> {
    /// `PistonContraption.assemble` for the piston at `pos` facing
    /// `orientation`, retracting when `retract`; none when `pos` is no piston.
    pub fn assemble<W: StructureWorld<State = S>>(
        world: &W,
        pos: Pos,
        orientation: Dir,
        retract: bool,
    ) -> Result<Option<Self>, AssemblyError> {
        let Some(PistonPart::Piston { sticky, state, .. }) = world.facts(pos).part else {
            return Ok(None);
        };
        let mut c = PistonContraption {
            orientation,
            sticky,
            anchor: step(pos, orientation, 1),
            blocks: Vec::new(),
            extension_length: 0,
            initial_extension_progress: 0,
            glue: Vec::new(),
        };
        let anchoring = c.collect_extensions(world, pos, state)?;
        let forced = if retract {
            orientation.opposite()
        } else {
            orientation
        };
        c.search(world, &anchoring, forced)?;
        Ok(Some(c))
    }

    /// `collectExtensions`: the poles in front (when extended) and behind,
    /// the head (or a pole) at the piston's own place. Answers the places
    /// they take in the world (`isAnchoringBlockAt`).
    fn collect_extensions<W: StructureWorld<State = S>>(
        &mut self,
        world: &W,
        pos: Pos,
        state: PistonState,
    ) -> Result<HashSet<Pos>, AssemblyError> {
        let d = self.orientation;
        let mut poles: Vec<(Pos, S, bool)> = Vec::new();
        let mut front = 0;
        let mut at = pos;
        if state == PistonState::Extended {
            loop {
                let next = step(at, d, 1);
                match world.facts(next).part {
                    Some(PistonPart::Pole { facing }) if facing.axis() == d.axis() => {
                        at = next;
                        poles.push((at, world.pole(d), true));
                        front += 1;
                    }
                    Some(PistonPart::Head { facing, sticky }) if facing == d => {
                        at = next;
                        poles.push((at, world.head(d, sticky), false));
                        front += 1;
                        break;
                    }
                    _ => break,
                }
                if front > MAX_PISTON_POLES {
                    return Err(AssemblyError::TooManyPoles);
                }
            }
        }
        if front == 0 {
            poles.push((pos, world.head(d, self.sticky), false));
        } else {
            poles.push((pos, world.pole(d), true));
        }
        let mut back = 0;
        let mut end = pos;
        loop {
            let next = step(end, d.opposite(), 1);
            match world.facts(next).part {
                Some(PistonPart::Pole { facing }) if facing.axis() == d.axis() => {
                    end = next;
                    poles.push((end, world.pole(d), true));
                    back += 1;
                }
                _ => break,
            }
            if front + back > MAX_PISTON_POLES {
                return Err(AssemblyError::TooManyPoles);
            }
        }
        self.extension_length = front + back;
        self.initial_extension_progress = front;
        if self.extension_length == 0 {
            return Err(AssemblyError::NoPoles);
        }
        let mut anchoring = HashSet::new();
        for (p, s, pole) in poles {
            anchoring.insert(p);
            self.blocks.push(Moved {
                local: sub(step(p, d, -front), self.anchor),
                state: s,
                solid: true,
                brittle: false,
                pole,
            });
        }
        Ok(anchoring)
    }

    /// `searchMovedStructure` from `addToInitialFrontier`, then `moveBlock`
    /// until the frontier is empty.
    fn search<W: StructureWorld<State = S>>(
        &mut self,
        world: &W,
        anchoring: &HashSet<Pos>,
        forced: Dir,
    ) -> Result<(), AssemblyError> {
        let mut frontier: VecDeque<Pos> = VecDeque::new();
        self.initial_frontier(world, forced, &mut frontier)?;
        let found = search_structure(
            world,
            frontier,
            anchoring,
            Some(forced),
            &mut self.glue,
            self.blocks.len(),
        )?;
        for (pos, f) in found {
            // addBlock, shifted back by the poles in front.
            self.blocks.push(Moved {
                local: sub(
                    step(pos, self.orientation, -self.initial_extension_progress),
                    self.anchor,
                ),
                state: world.state(pos),
                solid: f.solid,
                brittle: f.brittle,
                pole: false,
            });
        }
        Ok(())
    }

    /// `addToInitialFrontier`: the line of blocks in front of the head (only
    /// the first, and only for a sticky piston, when retracting).
    fn initial_frontier<W: StructureWorld<State = S>>(
        &self,
        world: &W,
        direction: Dir,
        frontier: &mut VecDeque<Pos>,
    ) -> Result<(), AssemblyError> {
        let retracting = direction != self.orientation;
        if retracting && !self.sticky {
            return Ok(());
        }
        for offset in 0..=MAX_CHASSIS_RANGE {
            if offset == 1 && retracting {
                return Ok(());
            }
            let pos = step(
                self.anchor,
                self.orientation,
                offset + self.initial_extension_progress,
            );
            if retracting && world.outside_height(pos) {
                return Ok(());
            }
            if !world.loaded(pos) {
                return Err(AssemblyError::Unloaded(pos));
            }
            let f = world.facts(pos);
            if !f.necessary {
                return Ok(());
            }
            if f.brittle && !f.carpet {
                return Ok(());
            }
            if matches!(f.part, Some(PistonPart::Head { facing, .. }) if facing == direction.opposite())
            {
                return Ok(());
            }
            if !f.allowed() {
                if retracting {
                    return Ok(());
                }
                return Err(AssemblyError::Unmovable(pos));
            }
            if retracting && f.push_only {
                return Ok(());
            }
            frontier.push_back(pos);
            if f.not_supportive(self.orientation) {
                return Ok(());
            }
        }
        Ok(())
    }

    /// Where the piston itself stands.
    pub fn piston_pos(&self) -> Pos {
        step(self.anchor, self.orientation, -1)
    }

    /// The world place of a block at `offset` blocks out (`toPosition`).
    pub fn world_pos(&self, local: Pos, offset: i32) -> Pos {
        step(add(local, self.anchor), self.orientation, offset)
    }

    /// `isCollidingWithWorld` with the colliders placed at `grid`.
    pub fn colliding_with_world<W: StructureWorld<State = S>>(
        &self,
        world: &W,
        grid: Pos,
        dir: Dir,
    ) -> bool {
        colliding_with_world(&self.blocks, world, grid, dir)
    }

    /// Where the blocks go at `grid_offset` (`addBlocksToWorld`): non-brittle
    /// blocks first, without the one landing in the piston
    /// (`customBlockPlacement`, which retracts the piston for a head).
    /// Answers the places with the blocks' indices, and whether the piston
    /// is retracted.
    pub fn placements(&self, grid_offset: i32) -> (Vec<(Pos, usize)>, bool) {
        let piston = self.piston_pos();
        let mut out = Vec::new();
        let mut retracted = false;
        for brittle in [false, true] {
            for (i, b) in self
                .blocks
                .iter()
                .enumerate()
                .filter(|(_, b)| b.brittle == brittle)
            {
                let at = self.world_pos(b.local, grid_offset);
                if at == piston {
                    retracted |= !b.pole;
                    continue;
                }
                out.push((at, i));
            }
        }
        (out, retracted)
    }

    /// The world places the blocks are taken from (`removeBlocksFromWorld`
    /// at the initial extension), the piston's own one excepted
    /// (`customBlockRemoval` makes it MOVING instead).
    pub fn removals(&self) -> Vec<Pos> {
        let piston = self.piston_pos();
        self.blocks
            .iter()
            .map(|b| self.world_pos(b.local, self.initial_extension_progress))
            .filter(|p| *p != piston)
            .collect()
    }
}

/// `TranslatingContraption.createColliders`: blocks with collision whose
/// next place along `dir` isn't another such block of the structure.
pub fn colliders<S>(blocks: &[Moved<S>], dir: Dir) -> Vec<Pos> {
    let solid: HashSet<Pos> = blocks.iter().filter(|b| b.solid).map(|b| b.local).collect();
    solid
        .iter()
        .filter(|p| !solid.contains(&step(**p, dir, 1)))
        .copied()
        .collect()
}

/// `ContraptionCollider.isCollidingWithWorld` with the structure's
/// anchor at `grid`, moving towards `dir`. A pulley's magnet just above
/// the anchor doesn't stop it going up.
pub fn colliding_with_world<S, W: StructureWorld>(
    blocks: &[Moved<S>],
    world: &W,
    grid: Pos,
    dir: Dir,
) -> bool {
    colliders(blocks, dir).into_iter().any(|p| {
        let at = add(p, grid);
        if !world.loaded(at) {
            return true;
        }
        let f = world.facts(at);
        if f.magnet && p == [0, 0, 0] && dir == Dir::Up {
            return false;
        }
        f.blocks_contraption()
    })
}

/// Blocks in the order `addBlocksToWorld` places them: the others first,
/// brittle ones (torches...) once what holds them is back.
pub fn placing_order<S>(blocks: &[Moved<S>]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..blocks.len()).filter(|&i| !blocks[i].brittle).collect();
    order.extend((0..blocks.len()).filter(|&i| blocks[i].brittle));
    order
}

/// `KineticBlockEntity.convertToLinear`.
pub fn convert_to_linear(rpm: f32) -> f32 {
    rpm / 512.0
}

/// What a linear actuator's tick asks of the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ActuatorStep {
    Idle,
    /// `assemble`: build the contraption and `start` it.
    Assemble,
    /// It moved to `offset` (draw it there).
    Moved,
    /// `disassemble` at `grid_offset`.
    Disassemble {
        grid_offset: i32,
    },
}

/// `LinearActuatorBlockEntity` with `MechanicalPistonBlockEntity`'s speed,
/// in the default movement mode (`MOVE_PLACE`: placed whenever it stops).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Actuator {
    pub running: bool,
    pub offset: f32,
    pub extension_length: i32,
    pub assemble_next_tick: bool,
    pub waiting: bool,
    pub last_rpm: f32,
    /// `MovementMode`: 0 always place when stopped, 1 only in the starting
    /// position, 2 only when the anchor is destroyed.
    pub mode: u8,
    /// `getInitialOffset`: where the structure started.
    pub initial: i32,
    /// `sequencedOffsetLimit`: how far a sequenced gearshift lets it go, or -1.
    pub limit: f64,
}

impl Default for Actuator {
    fn default() -> Self {
        Actuator {
            running: false,
            offset: 0.0,
            extension_length: 0,
            assemble_next_tick: true,
            limit: -1.0,
            waiting: false,
            last_rpm: 0.0,
            mode: 0,
            initial: 0,
        }
    }
}

impl Actuator {
    /// `MechanicalPistonBlockEntity.getMovementSpeed` on the server.
    pub fn movement_speed(&self, rpm: f32, facing: Dir) -> f32 {
        let speed = convert_to_linear(rpm).clamp(-0.49, 0.49);
        let modifier = if facing.positive() { 1.0 } else { -1.0 }
            * if facing.axis() == Axis::Z { -1.0 } else { 1.0 };
        let speed =
            (speed * -modifier).clamp(-self.offset, self.extension_length as f32 - self.offset);
        if self.limit >= 0.0 {
            speed.clamp(-self.limit as f32, self.limit as f32)
        } else {
            speed
        }
    }

    /// `getGridOffset`.
    pub fn grid_offset(&self, offset: f32) -> i32 {
        ((offset + 0.5) as i32).clamp(0, self.extension_length)
    }

    /// `onSpeedChanged`: called with the network's speed every tick; acts
    /// when it changes.
    pub fn speed(&mut self, rpm: f32) {
        let prev = self.last_rpm;
        if prev == rpm {
            return;
        }
        self.last_rpm = rpm;
        self.assemble_next_tick = true;
        self.waiting = false;
        if self.running && prev != 0.0 && prev.signum() != rpm.signum() {
            // `Math.round(offset * 16) / 16`: an int division.
            self.offset = ((self.offset * 16.0).round() as i32 / 16) as f32;
        }
    }

    /// The direction a speed `rpm` moves the structure (`assemble`).
    pub fn movement_direction(rpm: f32, facing: Dir) -> Dir {
        let positive = Dir::positive_of(facing.axis());
        if (rpm > 0.0) ^ (facing.axis() != Axis::Z) {
            positive
        } else {
            positive.opposite()
        }
    }

    /// The end of `assemble` once the contraption is built: false when it is
    /// in the way of the world (`colliding`, asked with the colliders' place
    /// and direction) or already at the end it moves to; else it runs.
    pub fn start<S: Clone>(
        &mut self,
        c: &PistonContraption<S>,
        rpm: f32,
        facing: Dir,
        colliding: impl Fn(Pos, Dir) -> bool,
    ) -> bool {
        let dir = Self::movement_direction(rpm, facing);
        let anchor = step(c.anchor, c.orientation, c.initial_extension_progress);
        if colliding(step(anchor, dir, 1), dir) {
            return false;
        }
        self.extension_length = c.extension_length;
        let speed = self.movement_speed(rpm, facing);
        let resulting = c.initial_extension_progress as f32 + speed.signum() * 0.5;
        if resulting <= 0.0 || resulting >= self.extension_length as f32 {
            return false;
        }
        self.running = true;
        self.offset = c.initial_extension_progress as f32;
        self.initial = c.initial_extension_progress;
        true
    }

    /// `tick` on the server. `colliding` answers whether the contraption is
    /// in the way of the world with its colliders at a grid place moving a
    /// direction (`collideBlocks`); `anchor` and `orientation` place it.
    /// A `stalled` structure (an actor breaking a block) stays where it is.
    pub fn tick(
        &mut self,
        rpm: f32,
        facing: Dir,
        anchor: Pos,
        stalled: bool,
        colliding: impl Fn(Pos, Dir) -> bool,
    ) -> ActuatorStep {
        if self.waiting {
            return ActuatorStep::Idle;
        }
        if self.assemble_next_tick {
            self.assemble_next_tick = false;
            if self.running {
                if rpm == 0.0 {
                    return ActuatorStep::Disassemble {
                        grid_offset: self.grid_offset(self.offset),
                    };
                }
                return ActuatorStep::Idle;
            }
            return if rpm != 0.0 {
                ActuatorStep::Assemble
            } else {
                ActuatorStep::Idle
            };
        }
        if !self.running || stalled {
            return ActuatorStep::Idle;
        }
        let speed = self.movement_speed(rpm, facing);
        if self.limit > 0.0 {
            self.limit = (self.limit - f64::from(speed.abs())).max(0.0);
        }
        let new = self.offset + speed;
        // moveAndCollideContraption: moved, then checked where it is.
        if speed != 0.0 {
            let dir = if speed > 0.0 {
                facing
            } else {
                facing.opposite()
            };
            let n = facing.normal();
            let mut grid = [0, 1, 2].map(|i| (anchor[i] as f32 + n[i] as f32 * new).floor() as i32);
            if dir.positive() {
                grid = step(grid, dir, 1);
            }
            if colliding(grid, dir) {
                // collided(): back on the grid, then placed.
                let at = self.grid_offset(self.offset);
                return self.try_disassemble(self.grid_offset(at as f32 - speed));
            }
        }
        self.offset = new;
        if self.offset <= 0.0 || self.offset >= self.extension_length as f32 {
            self.offset = if self.offset <= 0.0 {
                0.0
            } else {
                self.extension_length as f32
            };
            return self.try_disassemble(self.offset as i32);
        }
        ActuatorStep::Moved
    }

    /// `LinearActuatorBlockEntity.tryDisassemble`: stopped, its structure placed
    /// unless the movement mode keeps it (then it waits for a speed change).
    fn try_disassemble(&mut self, grid_offset: i32) -> ActuatorStep {
        if self.mode == 2 || (self.mode == 1 && (self.offset + 0.5) as i32 != self.initial) {
            self.waiting = true;
            return ActuatorStep::Idle;
        }
        ActuatorStep::Disassemble { grid_offset }
    }

    /// `disassemble`: stopped, its contraption placed.
    pub fn stopped(&mut self, grid_offset: i32) {
        self.running = false;
        self.offset = grid_offset as f32;
    }
}

// ---- Rope pulleys ---------------------------------------------------------------

/// `CKinetics.maxRopeLength`.
pub const MAX_ROPE_LENGTH: i32 = 384;

/// `PulleyContraption`: what hangs from a rope pulley's rope, below the
/// rope's end.
#[derive(Clone, Debug, PartialEq)]
pub struct PulleyContraption<S> {
    /// `pos.below(offset + 1)` when it was assembled.
    pub anchor: Pos,
    /// The rope's length then.
    pub initial_offset: i32,
    pub blocks: Vec<Moved<S>>,
    pub glue: Vec<usize>,
}

impl<S: Clone> PulleyContraption<S> {
    /// `PulleyContraption.assemble` for the pulley at `pos` with `ropes`
    /// rope blocks (and a magnet) below it: none when nothing hangs there.
    pub fn assemble<W: StructureWorld<State = S>>(
        world: &W,
        pos: Pos,
        ropes: i32,
    ) -> Result<Option<Self>, AssemblyError> {
        let anchor = step(pos, Dir::Down, ropes + 1);
        let mut frontier = VecDeque::new();
        if !world.facts(anchor).brittle {
            frontier.push_back(anchor);
        }
        // isAnchoringBlockAt: the rope and the pulley above the anchor.
        let anchoring: HashSet<Pos> = (1..=ropes + 1).map(|i| step(anchor, Dir::Up, i)).collect();
        let mut glue = Vec::new();
        let found = search_structure(world, frontier, &anchoring, None, &mut glue, 0)?;
        if found.is_empty() {
            return Ok(None);
        }
        let blocks = found
            .into_iter()
            .map(|(p, f)| Moved {
                local: sub(p, anchor),
                state: world.state(p),
                solid: f.solid,
                brittle: f.brittle,
                pole: false,
            })
            .collect();
        Ok(Some(PulleyContraption {
            anchor,
            initial_offset: ropes,
            blocks,
            glue,
        }))
    }

    /// The anchor's place with the rope `offset` long (`toPosition`).
    pub fn anchor_at(&self, offset: i32) -> Pos {
        step(self.anchor, Dir::Down, offset - self.initial_offset)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PulleyStep {
    Idle,
    /// `assemble`: count the rope, build what hangs on it, `start`.
    Assemble,
    Moved,
    /// `disassemble` at `grid_offset` (rope and magnet down to there); with
    /// `reassemble`, the rope reached a block (`visitNewPosition`) and takes
    /// it next tick.
    Disassemble {
        grid_offset: i32,
        reassemble: bool,
    },
}

/// `PulleyBlockEntity` (a `LinearActuatorBlockEntity`) in `MOVE_PLACE`:
/// positive speeds let the rope down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pulley {
    pub running: bool,
    /// The rope's length.
    pub offset: f32,
    /// `getExtensionRange`.
    pub range: i32,
    pub assemble_next_tick: bool,
    pub last_rpm: f32,
    /// Something hangs on the rope (`needsContraption`).
    pub carrying: bool,
    /// `MovementMode` (as the piston's) and the rope's length it started at.
    pub mode: u8,
    pub initial: i32,
    /// Waiting for a speed change (`waitingForSpeedChange`).
    pub waiting: bool,
}

impl Default for Pulley {
    fn default() -> Self {
        Pulley {
            running: false,
            offset: 0.0,
            range: 0,
            assemble_next_tick: true,
            last_rpm: 0.0,
            carrying: false,
            mode: 0,
            initial: 0,
            waiting: false,
        }
    }
}

impl Pulley {
    /// `getExtensionRange` for a pulley at height `y` over a world from
    /// `min_y`.
    pub fn range_at(y: i32, min_y: i32) -> i32 {
        MAX_ROPE_LENGTH.min(y - 1 - min_y).max(0)
    }

    /// `LinearActuatorBlockEntity.getMovementSpeed`.
    pub fn movement_speed(rpm: f32) -> f32 {
        convert_to_linear(rpm).clamp(-0.49, 0.49)
    }

    pub fn grid_offset(&self, offset: f32) -> i32 {
        ((offset + 0.5) as i32).clamp(0, self.range)
    }

    /// `onSpeedChanged`, as the piston's.
    pub fn speed(&mut self, rpm: f32) {
        let prev = self.last_rpm;
        if prev == rpm {
            return;
        }
        self.last_rpm = rpm;
        self.assemble_next_tick = true;
        self.waiting = false;
        if self.running && prev != 0.0 && prev.signum() != rpm.signum() {
            self.offset = ((self.offset * 16.0).round() as i32 / 16) as f32;
        }
    }

    /// `tryDisassemble`: the movement mode may keep the structure.
    fn try_stop(&mut self, grid_offset: i32) -> PulleyStep {
        if self.mode == 2 || (self.mode == 1 && (self.offset + 0.5) as i32 != self.initial) {
            self.waiting = true;
            return PulleyStep::Idle;
        }
        PulleyStep::Disassemble {
            grid_offset,
            reassemble: false,
        }
    }

    /// `assemble` with `ropes` counted below the pulley: false when it has
    /// nowhere to go (at the bottom going down, at the top going up, or
    /// unable to take what hangs there going down: `built` none when it
    /// couldn't, `Some(false)` when nothing hangs there).
    pub fn start(&mut self, rpm: f32, ropes: i32, built: Option<bool>) -> bool {
        self.offset = ropes as f32;
        if (ropes >= self.range && rpm > 0.0) || (ropes <= 0 && rpm < 0.0) {
            return false;
        }
        if built.is_none() && rpm > 0.0 {
            return false;
        }
        self.carrying = built == Some(true);
        self.running = true;
        self.initial = ropes;
        true
    }

    /// `tick` on the server. `stops_rope(depth)`: the block `depth` below
    /// the pulley stops a bare rope (movement necessary, not brittle);
    /// `colliding(grid, dir)`: what hangs, its anchor at `grid` moving
    /// `dir`, is in the way; `anchor_at(offset)` places that anchor.
    pub fn tick(
        &mut self,
        rpm: f32,
        stalled: bool,
        stops_rope: impl Fn(i32) -> bool,
        colliding: impl Fn(Pos, Dir) -> bool,
        anchor_at: impl Fn(f32) -> Option<[f32; 3]>,
    ) -> PulleyStep {
        if self.waiting {
            return PulleyStep::Idle;
        }
        if self.assemble_next_tick {
            self.assemble_next_tick = false;
            if self.running {
                if rpm == 0.0 {
                    return PulleyStep::Disassemble {
                        grid_offset: self.grid_offset(self.offset),
                        reassemble: false,
                    };
                }
                return PulleyStep::Idle;
            }
            return if rpm != 0.0 {
                PulleyStep::Assemble
            } else {
                PulleyStep::Idle
            };
        }
        if !self.running || stalled {
            return PulleyStep::Idle;
        }
        let speed = Self::movement_speed(rpm);
        let new = self.offset + speed;
        // visitNewPosition: a bare rope going down stops on a block.
        if new as i32 != self.offset as i32
            && !self.carrying
            && speed > 0.0
            && stops_rope(new as i32 + 1)
        {
            return PulleyStep::Disassemble {
                grid_offset: self.grid_offset(self.offset),
                reassemble: true,
            };
        }
        if self.carrying
            && speed != 0.0
            && let Some(at) = anchor_at(new)
        {
            let dir = if speed > 0.0 { Dir::Down } else { Dir::Up };
            let mut grid = at.map(|c| c.floor() as i32);
            if dir == Dir::Up {
                grid = step(grid, dir, 1);
            }
            if colliding(grid, dir) {
                let at = self.grid_offset(self.offset);
                return self.try_stop(self.grid_offset(at as f32 - speed));
            }
        }
        self.offset = new;
        if self.offset <= 0.0 || self.offset >= self.range as f32 {
            self.offset = self.offset.clamp(0.0, self.range as f32);
            return self.try_stop(self.offset as i32);
        }
        PulleyStep::Moved
    }

    /// `disassemble`: stopped with the rope `grid_offset` long.
    pub fn stopped(&mut self, grid_offset: i32, reassemble: bool) {
        self.running = false;
        self.carrying = false;
        self.offset = grid_offset as f32;
        self.assemble_next_tick = reassemble;
    }
}

// ---- Gantries --------------------------------------------------------------------

/// `GantryShaftBlock.Part`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GantryPart {
    Start,
    Middle,
    End,
    Single,
}

impl GantryPart {
    pub const ALL: [GantryPart; 4] = [
        GantryPart::Start,
        GantryPart::Middle,
        GantryPart::End,
        GantryPart::Single,
    ];
    pub fn name(self) -> &'static str {
        match self {
            GantryPart::Start => "start",
            GantryPart::Middle => "middle",
            GantryPart::End => "end",
            GantryPart::Single => "single",
        }
    }

    /// `updateShape` along the shaft: `front` and `back` tell whether a
    /// shaft of the same facing joins it towards its facing and away.
    pub fn of(front: bool, back: bool) -> GantryPart {
        match (front, back) {
            (true, true) => GantryPart::Middle,
            (true, false) => GantryPart::Start,
            (false, true) => GantryPart::End,
            (false, false) => GantryPart::Single,
        }
    }

    /// `GantryShaftBlockEntity.canAssembleOn` (unpowered) with the pinion
    /// moving at `speed`.
    pub fn can_assemble_on(self, speed: f32) -> bool {
        match self {
            GantryPart::End => speed < 0.0,
            GantryPart::Middle => speed != 0.0,
            GantryPart::Start => speed > 0.0,
            GantryPart::Single => false,
        }
    }
}

/// A gantry shaft's block name: `create:gantry_shaft` alone, a hidden
/// `create:hz_gantry_shaft_<part>` in a line (the template keeps `facing`).
pub fn gantry_shaft_block(part: GantryPart) -> String {
    match part {
        GantryPart::Single => "create:gantry_shaft".to_owned(),
        p => format!("create:hz_gantry_shaft_{}", p.name()),
    }
}

pub fn gantry_shaft_of(name: &str) -> Option<GantryPart> {
    GantryPart::ALL
        .into_iter()
        .find(|p| gantry_shaft_block(*p) == name)
}

/// `GantryShaftBlockEntity.getPinionMovementSpeed`: blocks a tick along
/// the shaft's facing.
pub fn pinion_speed(shaft_rpm: f32) -> f32 {
    convert_to_linear(-shaft_rpm).clamp(-0.49, 0.49)
}

/// `GantryContraption`: the carriage and what it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct GantryContraption<S> {
    /// The carriage's facing (away from its shaft).
    pub facing: Dir,
    /// The carriage's place when it was assembled.
    pub anchor: Pos,
    pub blocks: Vec<Moved<S>>,
    pub glue: Vec<usize>,
}

impl<S: Clone> GantryContraption<S> {
    /// `GantryContraption.assemble` from the carriage at `pos`: the shaft
    /// behind it anchors.
    pub fn assemble<W: StructureWorld<State = S>>(
        world: &W,
        pos: Pos,
        facing: Dir,
    ) -> Result<Option<Self>, AssemblyError> {
        let mut frontier = VecDeque::new();
        if !world.facts(pos).brittle {
            frontier.push_back(pos);
        }
        let anchoring = HashSet::from([step(pos, facing, -1)]);
        let mut glue = Vec::new();
        let found = search_structure(world, frontier, &anchoring, None, &mut glue, 0)?;
        if found.is_empty() {
            return Ok(None);
        }
        let blocks = found
            .into_iter()
            .map(|(p, f)| Moved {
                local: sub(p, pos),
                state: world.state(p),
                solid: f.solid,
                brittle: f.brittle,
                pole: false,
            })
            .collect();
        Ok(Some(GantryContraption {
            facing,
            anchor: pos,
            blocks,
            glue,
        }))
    }
}

/// What a moving gantry does this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GantryStep {
    /// Moved by this much.
    Moved([f64; 3]),
    /// Put back with the anchor at this place.
    Disassemble(Pos),
}

/// The shaft under a moving carriage, as `checkPinionShaft` reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShaftUnder {
    pub facing: Dir,
    pub part: GantryPart,
    pub rpm: f32,
}

/// `GantryContraptionEntity.tickContraption` on the server: `at` the
/// anchor's corner, `facing` the carriage's; `shaft(pos)` reads a gantry
/// shaft, `colliding(grid, dir)` the world in the way. The structure stays
/// put its first two ticks (`tickCount > 2`).
pub fn gantry_tick(
    at: [f64; 3],
    facing: Dir,
    ticks: u32,
    stalled: bool,
    shaft: impl Fn(Pos) -> Option<ShaftUnder>,
    colliding: impl Fn(Pos, Dir) -> bool,
) -> GantryStep {
    let rounded = at.map(|c| (c + 0.5).floor() as i32);
    let centre = at.map(|c| c + 0.5);
    let shaft_pos = step(centre.map(|c| c.floor() as i32), facing, -1);
    // checkPinionShaft.
    let Some(under) = shaft(shaft_pos) else {
        return GantryStep::Disassemble(rounded);
    };
    let speed = pinion_speed(under.rpm);
    if speed == 0.0 {
        return GantryStep::Disassemble(rounded);
    }
    let n = under.facing.normal();
    let motion = [0, 1, 2].map(|k| f64::from(n[k]) * f64::from(speed));
    let next = [0, 1, 2].map(|k| centre[k] + motion[k]);
    let axis = under.facing.axis();
    let current_coord = f64::from(axis.choose([0, 1, 2].map(|k| centre[k].floor() as i32)));
    let next_coord = match axis {
        Axis::X => next[0],
        Axis::Y => next[1],
        Axis::Z => next[2],
    };
    let backwards = speed * if under.facing.positive() { 1.0 } else { -1.0 } < 0.0;
    if (current_coord + 0.5 < next_coord) != backwards && !under.part.can_assemble_on(speed) {
        return GantryStep::Disassemble(rounded);
    }
    // collideBlocks: in the way where it moves to.
    let dir = if speed > 0.0 {
        under.facing
    } else {
        under.facing.opposite()
    };
    let mut grid = [0, 1, 2].map(|k| (at[k] + motion[k]).floor() as i32);
    if dir.positive() {
        grid = step(grid, dir, 1);
    }
    if colliding(grid, dir) {
        return GantryStep::Disassemble(rounded);
    }
    if ticks <= 2 || stalled {
        return GantryStep::Moved([0.0; 3]);
    }
    GantryStep::Moved(motion)
}

// ---- Elevator pulley -------------------------------------------------------------

/// `ElevatorColumn.ColumnCoords`: the column of elevator contacts at `x`,
/// `z`, facing `side`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Column {
    pub x: i32,
    pub z: i32,
    pub side: Dir,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElevatorError {
    NoContacts,
    TooManyContacts,
    ColumnConflict,
}

/// `ElevatorContraption.capture`'s redstone contacts: of the contacts in the
/// structure (their place from the anchor and facing), the one facing
/// sideways gives the column (the block it faces, turned back) and the
/// contact's height in the cabin (`contactYOffset`).
pub fn elevator_column(
    contacts: &[(Pos, Dir)],
    anchor: Pos,
) -> Result<(Column, i32), ElevatorError> {
    let sideways: Vec<&(Pos, Dir)> = contacts
        .iter()
        .filter(|(_, f)| f.axis() != Axis::Y)
        .collect();
    match sideways.as_slice() {
        [] => Err(ElevatorError::NoContacts),
        [(local, facing)] => {
            let at = step(*local, *facing, 1);
            Ok((
                Column {
                    x: at[0] + anchor[0],
                    z: at[2] + anchor[2],
                    side: facing.opposite(),
                },
                at[1],
            ))
        }
        _ => Err(ElevatorError::TooManyContacts),
    }
}

/// `ElevatorColumn`: its contacts' heights, the floor called and whether an
/// elevator runs on it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ElevatorColumn {
    pub contacts: Vec<i32>,
    pub target: Option<i32>,
    pub active: bool,
    /// The contact calling now (`CALLING`).
    pub calling: Option<i32>,
}

impl ElevatorColumn {
    /// `add`: kept in order.
    pub fn add(&mut self, y: i32) {
        if let Err(i) = self.contacts.binary_search(&y) {
            self.contacts.insert(i, y);
        }
    }

    #[allow(dead_code)]
    pub fn remove(&mut self, y: i32) {
        self.contacts.retain(|c| *c != y);
    }

    /// `callToContactAndUpdate`: the contact at `y` calls (the others stop).
    pub fn call(&mut self, y: i32) {
        self.calling = Some(y);
        self.target = Some(y);
    }
}

/// `ElevatorPulleyBlockEntity`'s motion: the cabin's distance below the
/// pulley (`offset`) and the speed it had (`prevSpeed`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Elevator {
    pub offset: f32,
    pub prev_speed: f32,
    /// The cabin's contact below its anchor (`contactYOffset`).
    pub contact_y_offset: i32,
    pub max_contact_y: i32,
    pub min_contact_y: i32,
}

impl Elevator {
    /// After `assemble`: the floors it can reach from the pulley at `pulley_y`.
    pub fn new(pulley_y: i32, offset: i32, contact_y_offset: i32) -> Self {
        let max = pulley_y + contact_y_offset - 1;
        Elevator {
            offset: offset as f32,
            prev_speed: 0.0,
            contact_y_offset,
            max_contact_y: max,
            min_contact_y: max - MAX_ROPE_LENGTH,
        }
    }

    /// The contact level the cabin stands at with `offset`.
    pub fn level_at(&self, pulley_y: i32, offset: i32) -> i32 {
        pulley_y + self.contact_y_offset - 1 - offset
    }

    /// `getTargetOffset` (with `getCurrentTargetY`): the offset of the floor
    /// called, or where it is when none can be reached.
    pub fn target_offset(&self, pulley_y: i32, target_y: Option<i32>) -> i32 {
        match target_y {
            Some(y) if y >= self.min_contact_y && y <= self.max_contact_y => {
                pulley_y - y + self.contact_y_offset - 1
            }
            _ => self.offset as i32,
        }
    }

    /// `getMovementSpeed` on the server: toward `target`, as fast as the
    /// speed allows (`convertToLinear(speed * 2)`), speeding up and slowing
    /// down smoothly; and whether it has arrived.
    pub fn movement_speed(&mut self, target: i32, rpm: f32) -> (f32, bool) {
        let diff = target as f32 - self.offset;
        let movement = convert_to_linear(rpm * 2.0).clamp(-1.99, 1.99);
        let limit = movement.abs();
        let acc = 0.0075 + (0.0175 - 0.0075) * limit;
        let deceleration = (2.0 * diff.abs() * acc).sqrt();
        let speed = diff
            .clamp(-limit, limit)
            .clamp(self.prev_speed - acc, self.prev_speed + acc)
            .clamp(-deceleration, deceleration);
        self.prev_speed = speed;
        (speed, diff.abs() < 0.5)
    }
}

// ---- Bearings ------------------------------------------------------------------

/// `BearingContraption`: what a mechanical or windmill bearing turns, the
/// structure in front of it.
#[derive(Clone, Debug, PartialEq)]
pub struct BearingContraption<S> {
    pub facing: Dir,
    /// `pos.relative(facing)`, the block the structure turns about.
    pub anchor: Pos,
    pub blocks: Vec<Moved<S>>,
    /// `sailBlocks`.
    pub sails: usize,
    pub glue: Vec<usize>,
}

impl<S: Clone> BearingContraption<S> {
    /// `BearingContraption.assemble` for the bearing at `pos`: none when
    /// nothing stands in front of it.
    pub fn assemble<W: StructureWorld<State = S>>(
        world: &W,
        pos: Pos,
        facing: Dir,
        windmill: bool,
    ) -> Result<Option<Self>, AssemblyError> {
        let anchor = step(pos, facing, 1);
        let mut frontier = VecDeque::new();
        if !world.facts(anchor).brittle {
            frontier.push_back(anchor);
        }
        // isAnchoringBlockAt: the bearing itself.
        let anchoring = HashSet::from([pos]);
        let mut glue = Vec::new();
        let found = search_structure(world, frontier, &anchoring, None, &mut glue, 0)?;
        let sails = found.iter().filter(|(_, f)| f.sail).count();
        if windmill && sails < MIN_WINDMILL_SAILS {
            return Err(AssemblyError::NotEnoughSails(sails));
        }
        if found.is_empty() {
            return Ok(None);
        }
        let blocks = found
            .into_iter()
            .map(|(p, f)| Moved {
                local: sub(p, anchor),
                state: world.state(p),
                solid: f.solid,
                brittle: f.brittle,
                pole: false,
            })
            .collect();
        Ok(Some(BearingContraption {
            facing,
            anchor,
            blocks,
            sails,
            glue,
        }))
    }

    /// Where a block lands when the structure is put back turned by
    /// `angle` (`StructureTransform.apply`, the angle in quarter turns).
    pub fn placed_at(&self, local: Pos, angle: f32) -> Pos {
        add(
            self.anchor,
            turn_pos(local, rounded_angle(angle), self.facing.axis()),
        )
    }
}

/// `WindmillBearingBlockEntity.getGeneratedSpeed`, turning clockwise.
pub fn windmill_speed(sails: usize) -> i32 {
    (sails / WINDMILL_SAILS_PER_RPM).clamp(1, 16) as i32
}

/// `KineticBlockEntity.convertToAngular`: degrees a tick.
pub fn convert_to_angular(rpm: f32) -> f32 {
    rpm * 3.0 / 10.0
}

/// `StructureTransform(offset, x, y, z)`'s angle: rounded to a quarter
/// turn, in -90..=270.
pub fn rounded_angle(angle: f32) -> i32 {
    let mut a = ((angle / 90.0).round() as i32 * 90) % 360;
    if a < -90 {
        a += 360;
    }
    a
}

/// The `Rotation` of a rounded angle, as its ordinal (clockwise quarter
/// turns `rotateFacing` makes).
pub fn rotation_steps(angle: i32) -> usize {
    match angle {
        -90 | 270 => 1,
        90 => 3,
        180 => 2,
        _ => 0,
    }
}

/// `VecHelper.rotate` of a block offset by a rounded angle.
pub fn turn_pos(local: Pos, angle: i32, axis: Axis) -> Pos {
    let v = super::create_kinetics::rotate_vec(local.map(|c| c as f32), angle as f32, axis);
    v.map(|c| c.round() as i32)
}

/// Which of `StructureTransform.apply`'s cases a block state takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnShape {
    Plain,
    /// `StairBlock`.
    Stairs,
    /// `FaceAttachedHorizontalDirectionalBlock` (buttons, levers...).
    FaceAttached,
}

fn get<'a>(props: &'a [(String, String)], key: &str) -> Option<&'a str> {
    props
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn put(props: &mut [(String, String)], key: &str, value: &str) {
    if let Some(p) = props.iter_mut().find(|(k, _)| k == key) {
        p.1 = value.to_owned();
    }
}

/// `BlockState.rotate(rotation)` as vanilla's blocks answer it, from their
/// properties: facing, axis, sign rotation and the sides of fences and
/// walls (rails' and stairs' shapes are left).
fn vanilla_rotate(props: &mut [(String, String)], steps: usize) {
    let turn = |d: Dir| (0..steps).fold(d, |d, _| d.clockwise(Axis::Y));
    if let Some(f) = get(props, "facing").and_then(Dir::parse) {
        put(props, "facing", turn(f).name());
    }
    if steps % 2 == 1 {
        match get(props, "axis") {
            Some("x") => put(props, "axis", "z"),
            Some("z") => put(props, "axis", "x"),
            _ => {}
        }
    }
    if let Some(r) = get(props, "rotation").and_then(|r| r.parse::<usize>().ok()) {
        put(props, "rotation", &((r + 4 * steps) % 16).to_string());
    }
    let sides: Vec<(Dir, String)> = [Dir::North, Dir::East, Dir::South, Dir::West]
        .into_iter()
        .filter_map(|d| get(props, d.name()).map(|v| (d, v.to_owned())))
        .collect();
    if sides.len() == 4 {
        for (d, v) in sides {
            put(props, turn(d).name(), &v);
        }
    }
}

/// `StructureTransform.apply(BlockState)` for a turn of `angle` (rounded)
/// about `axis`: the new properties. `six_facing`: the block's `facing`
/// takes up and down (`BlockStateProperties.FACING`, not
/// `HORIZONTAL_FACING`).
pub fn turn_props(
    props: &[(String, String)],
    shape: TurnShape,
    six_facing: bool,
    axis: Axis,
    angle: i32,
) -> Vec<(String, String)> {
    let mut props = props.to_vec();
    let steps = rotation_steps(angle);
    if steps == 0 {
        return props;
    }
    if axis == Axis::Y {
        vanilla_rotate(&mut props, steps);
        return props;
    }
    let rotate_facing = |d: Dir| (0..steps).fold(d, |d, _| d.clockwise(axis));
    match shape {
        TurnShape::FaceAttached => {
            let z = axis == Axis::Z;
            let forced = if z { Dir::West } else { Dir::South };
            let (Some(facing), Some(face)) = (
                get(&props, "facing").and_then(Dir::parse),
                get(&props, "face"),
            ) else {
                return props;
            };
            if facing.axis() == axis && face == "wall" {
                return props;
            }
            for _ in 0..steps {
                let face = get(&props, "face").unwrap_or("wall").to_owned();
                let facing = get(&props, "facing")
                    .and_then(Dir::parse)
                    .unwrap_or(Dir::North);
                put(
                    &mut props,
                    "facing",
                    if face == "ceiling" {
                        forced
                    } else {
                        forced.opposite()
                    }
                    .name(),
                );
                if face != "wall" {
                    put(&mut props, "face", "wall");
                    continue;
                }
                let floor = facing.positive() != z;
                put(&mut props, "face", if floor { "floor" } else { "ceiling" });
            }
            props
        }
        TurnShape::Stairs => {
            let Some(facing) = get(&props, "facing").and_then(Dir::parse) else {
                return props;
            };
            let flip_half = |props: &mut Vec<(String, String)>| {
                let half = if get(props, "half") == Some("top") {
                    "bottom"
                } else {
                    "top"
                };
                put(props, "half", half);
            };
            if facing.axis() != axis {
                for _ in 0..steps {
                    let d = get(&props, "facing").and_then(Dir::parse).unwrap_or(facing);
                    let bottom = get(&props, "half") == Some("bottom");
                    if d.positive() ^ bottom ^ (d.axis() == Axis::Z) {
                        flip_half(&mut props);
                    } else {
                        put(&mut props, "facing", d.opposite().name());
                    }
                }
            } else if steps == 2 {
                flip_half(&mut props);
            }
            props
        }
        TurnShape::Plain => {
            if let (true, Some(f)) = (six_facing, get(&props, "facing").and_then(Dir::parse)) {
                put(&mut props, "facing", rotate_facing(f).name());
            } else if let Some(a) = get(&props, "axis").and_then(Axis::parse) {
                put(
                    &mut props,
                    "axis",
                    rotate_facing(Dir::positive_of(a)).axis().name(),
                );
            } else if steps == 2 {
                if get(&props, "facing")
                    .and_then(Dir::parse)
                    .is_some_and(|f| f.axis() == axis)
                {
                    return props;
                }
                vanilla_rotate(&mut props, 2);
                match get(&props, "type") {
                    Some("bottom") => put(&mut props, "type", "top"),
                    Some("top") => put(&mut props, "type", "bottom"),
                    _ => {}
                }
            }
            props
        }
    }
}

/// `MechanicalBearingBlockEntity`'s motion (`ROTATE_PLACE`; a windmill's
/// with `windmill`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bearing {
    pub running: bool,
    pub angle: f32,
    pub assemble_next_tick: bool,
    pub last_rpm: f32,
    /// `RotationMode`: 0 always place when stopped, 1 only near the initial
    /// angle, 2 only when the anchor is destroyed.
    pub mode: u8,
    /// `sequencedAngleLimit`: how far a sequenced gearshift lets it turn.
    pub limit: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BearingStep {
    Idle,
    /// Assembled this tick (and turned).
    Assembled,
    Turned,
    /// Put the structure back at `angle` (`disassemble`).
    Disassemble {
        angle: f32,
    },
}

impl Bearing {
    /// `onSpeedChanged`: a windmill keeps its queued assembly; a stop or a
    /// turn round snaps the angle to a degree.
    pub fn speed(&mut self, rpm: f32, windmill: bool) {
        if rpm == self.last_rpm {
            return;
        }
        let prev = self.last_rpm;
        self.last_rpm = rpm;
        if !windmill {
            self.assemble_next_tick = true;
        }
        if self.running && prev != 0.0 && rpm.signum() != prev.signum()
            || self.running && rpm == 0.0 && prev != 0.0
        {
            self.angle = self.angle.round();
        }
    }

    /// `MechanicalBearingBlock.useItemOn` with an empty hand: true when it
    /// must put its structure back now.
    pub fn clicked(&mut self) -> bool {
        if self.running {
            return true;
        }
        self.assemble_next_tick = true;
        false
    }

    /// `tick`: `assemble` takes the structure (true when it did).
    pub fn tick(
        &mut self,
        rpm: f32,
        windmill: bool,
        stalled: bool,
        assemble: impl FnOnce() -> bool,
    ) -> BearingStep {
        let mut assembled = false;
        if self.assemble_next_tick {
            self.assemble_next_tick = false;
            if self.running {
                if rpm == 0.0 {
                    // `canDisassemble`: ROTATE_PLACE, or ROTATE_PLACE_RETURNED near the
                    // initial angle (a windmill has no mode).
                    let near = self.angle.abs() < 22.5 || self.angle.abs() > 360.0 - 22.5;
                    if !windmill && !(self.mode == 0 || (self.mode == 1 && near)) {
                        return BearingStep::Idle;
                    }
                    // A windmill goes back as it was built.
                    let angle = if windmill { 0.0 } else { self.angle };
                    return BearingStep::Disassemble { angle };
                }
            } else {
                if rpm == 0.0 && !windmill {
                    return BearingStep::Idle;
                }
                if assemble() {
                    self.running = true;
                    self.angle = 0.0;
                    assembled = true;
                }
            }
        }
        if !self.running || stalled && !assembled {
            return BearingStep::Idle;
        }
        let mut angular = convert_to_angular(rpm);
        if let Some(limit) = self.limit {
            angular = angular.clamp(-limit as f32, limit as f32);
            self.limit = Some((limit - f64::from(angular.abs())).max(0.0));
        }
        self.angle = (self.angle + angular) % 360.0;
        if assembled {
            BearingStep::Assembled
        } else {
            BearingStep::Turned
        }
    }

    /// After `disassemble`.
    pub fn stopped(&mut self) {
        self.running = false;
        self.angle = 0.0;
        self.assemble_next_tick = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum B {
        Stone,
        Slime,
        Torch(Dir),
        Bedrock,
        Part(PistonPart),
        Chassis(Chassis),
        Sail,
    }

    /// Blocks, and glue as pairs of its corners.
    #[derive(Default)]
    struct World(HashMap<Pos, B>, Vec<(Pos, Pos)>);

    impl StructureWorld for World {
        type State = String;
        fn loaded(&self, _: Pos) -> bool {
            true
        }
        fn outside_height(&self, p: Pos) -> bool {
            p[1] < -64 || p[1] >= 320
        }
        fn facts(&self, p: Pos) -> Facts {
            let solid = Facts {
                necessary: true,
                allowed: true,
                solid: true,
                ..Facts::default()
            };
            match self.0.get(&p) {
                None => Facts {
                    allowed: true,
                    replaceable: true,
                    ..Facts::default()
                },
                Some(B::Stone) => solid,
                Some(B::Slime) => Facts {
                    sticky: Some(Sticky::Slime),
                    ..solid
                },
                Some(B::Bedrock) => Facts {
                    allowed: false,
                    ..solid
                },
                Some(B::Torch(on)) => {
                    let mut attached = [false; 6];
                    attached[*on as usize] = true;
                    Facts {
                        necessary: true,
                        allowed: true,
                        brittle: true,
                        attached,
                        not_supportive: [true; 6],
                        ..Facts::default()
                    }
                }
                Some(B::Part(part)) => Facts {
                    part: Some(*part),
                    ..solid
                },
                Some(B::Chassis(c)) => Facts {
                    chassis: Some(*c),
                    ..solid
                },
                Some(B::Sail) => Facts {
                    sail: true,
                    sticky: Some(Sticky::Slime),
                    ..solid
                },
            }
        }
        fn glue(&self, p: Pos, d: Dir) -> Option<usize> {
            let q = step(p, d, 1);
            let inside = |a: Pos, b: Pos, x: Pos| {
                (0..3).all(|k| a[k].min(b[k]) <= x[k] && x[k] <= a[k].max(b[k]))
            };
            self.1
                .iter()
                .position(|(a, b)| inside(*a, *b, p) && inside(*a, *b, q))
        }
        fn state(&self, p: Pos) -> String {
            format!("{:?}", self.0.get(&p))
        }
        fn head(&self, facing: Dir, sticky: bool) -> String {
            format!("head {facing:?} {sticky}")
        }
        fn pole(&self, facing: Dir) -> String {
            format!("pole {facing:?}")
        }
    }

    /// A piston at the origin facing east with `poles` behind it.
    fn rig(poles: i32, sticky: bool) -> World {
        let mut w = World::default();
        w.0.insert(
            [0, 0, 0],
            B::Part(PistonPart::Piston {
                facing: Dir::East,
                sticky,
                state: PistonState::Retracted,
            }),
        );
        for i in 1..=poles {
            w.0.insert([-i, 0, 0], B::Part(PistonPart::Pole { facing: Dir::East }));
        }
        w
    }

    #[test]
    fn a_pulley_waits_at_the_end_in_a_keeping_mode() {
        let mut p = Pulley {
            running: true,
            offset: 5.995,
            range: 6,
            assemble_next_tick: false,
            carrying: true,
            mode: 2,
            ..Pulley::default()
        };
        let step = p.tick(16.0, false, |_| false, |_, _| false, |_| None);
        assert_eq!(step, PulleyStep::Idle);
        assert!(p.waiting);
        // A speed change lets it go on (and place, in the default mode).
        p.speed(-16.0);
        assert!(!p.waiting);
    }

    #[test]
    fn a_bearing_in_a_keeping_mode_stays_assembled_when_stopped() {
        for (mode, angle, placed) in [
            (0u8, 100.0, true),
            (1, 100.0, false),
            (1, 10.0, true),
            (2, 10.0, false),
        ] {
            let mut b = Bearing {
                limit: None,
                running: true,
                angle,
                assemble_next_tick: true,
                last_rpm: 16.0,
                mode,
            };
            let step = b.tick(0.0, false, false, || false);
            assert_eq!(
                matches!(step, BearingStep::Disassemble { .. }),
                placed,
                "mode {mode} at {angle}"
            );
        }
    }

    #[test]
    fn a_movement_mode_keeps_the_structure_from_being_placed() {
        let mut a = Actuator {
            running: true,
            offset: 3.995,
            extension_length: 4,
            assemble_next_tick: false,
            initial: 0,
            mode: 2,
            ..Actuator::default()
        };
        // Never place: it waits at the end for a speed change.
        let step = a.tick(-16.0, Dir::East, [0, 0, 0], false, |_, _| false);
        assert_eq!(step, ActuatorStep::Idle);
        assert!(a.waiting);
        // Place only in the starting position: away from it, it waits too.
        let mut b = Actuator {
            mode: 1,
            waiting: false,
            offset: 3.995,
            ..a
        };
        assert_eq!(
            b.tick(-16.0, Dir::East, [0, 0, 0], false, |_, _| false),
            ActuatorStep::Idle
        );
        assert!(b.waiting);
        // The default places it.
        let mut c = Actuator {
            mode: 0,
            waiting: false,
            offset: 3.995,
            ..a
        };
        assert!(matches!(
            c.tick(-16.0, Dir::East, [0, 0, 0], false, |_, _| false),
            ActuatorStep::Disassemble { .. }
        ));
    }

    #[test]
    fn a_retracted_piston_pushes_the_line_in_front() {
        let mut w = rig(2, false);
        w.0.insert([1, 0, 0], B::Stone);
        w.0.insert([2, 0, 0], B::Stone);
        w.0.insert([1, 1, 0], B::Stone); // beside, not attached: stays
        w.0.insert([2, 0, 1], B::Torch(Dir::West)); // brittle, attached to nothing here
        let c = PistonContraption::assemble(&w, [0, 0, 0], Dir::East, false)
            .unwrap()
            .unwrap();
        assert_eq!(c.anchor, [1, 0, 0]);
        assert_eq!(c.extension_length, 2);
        assert_eq!(c.initial_extension_progress, 0);
        // Head inside the piston, two poles behind, two stones.
        let mut at: Vec<Pos> = c.blocks.iter().map(|b| c.world_pos(b.local, 0)).collect();
        at.sort();
        assert_eq!(
            at,
            vec![[-2, 0, 0], [-1, 0, 0], [0, 0, 0], [1, 0, 0], [2, 0, 0]]
        );
        assert_eq!(c.blocks[0].state, "head East false");
        // Out two blocks: the head and stones moved on, nothing in the piston.
        let (placed, retracted) = c.placements(2);
        assert!(!retracted);
        let mut at: Vec<Pos> = placed.iter().map(|(p, _)| *p).collect();
        at.sort();
        assert_eq!(at, vec![[1, 0, 0], [2, 0, 0], [3, 0, 0], [4, 0, 0]]);
        assert_eq!(c.removals().len(), 4);
    }

    #[test]
    fn torches_on_the_moved_blocks_come_along() {
        let mut w = rig(1, false);
        w.0.insert([1, 0, 0], B::Stone);
        // A torch on the stone's north side: attached towards the south.
        w.0.insert([1, 0, -1], B::Torch(Dir::South));
        let c = PistonContraption::assemble(&w, [0, 0, 0], Dir::East, false)
            .unwrap()
            .unwrap();
        assert!(
            c.blocks
                .iter()
                .any(|b| c.world_pos(b.local, 0) == [1, 0, -1] && b.brittle)
        );
        // Placed after the solid blocks.
        let (placed, _) = c.placements(1);
        assert!(c.blocks[placed.last().unwrap().1].brittle);
    }

    #[test]
    fn slime_drags_its_neighbours_and_bedrock_stops_it() {
        let mut w = rig(1, false);
        w.0.insert([1, 0, 0], B::Slime);
        w.0.insert([1, 1, 0], B::Stone);
        let c = PistonContraption::assemble(&w, [0, 0, 0], Dir::East, false)
            .unwrap()
            .unwrap();
        assert!(
            c.blocks
                .iter()
                .any(|b| c.world_pos(b.local, 0) == [1, 1, 0])
        );
        w.0.insert([2, 0, 0], B::Bedrock);
        assert_eq!(
            PistonContraption::assemble(&w, [0, 0, 0], Dir::East, false),
            Err(AssemblyError::Unmovable([2, 0, 0]))
        );
    }

    #[test]
    fn an_extended_piston_takes_its_poles_and_a_sticky_one_pulls() {
        let mut w = World::default();
        let piston = |sticky| {
            B::Part(PistonPart::Piston {
                facing: Dir::Up,
                sticky,
                state: PistonState::Extended,
            })
        };
        w.0.insert([0, 0, 0], piston(false));
        w.0.insert([0, 1, 0], B::Part(PistonPart::Pole { facing: Dir::Up }));
        w.0.insert(
            [0, 2, 0],
            B::Part(PistonPart::Head {
                facing: Dir::Up,
                sticky: false,
            }),
        );
        w.0.insert([0, 3, 0], B::Stone);
        let c = PistonContraption::assemble(&w, [0, 0, 0], Dir::Up, true)
            .unwrap()
            .unwrap();
        assert_eq!((c.extension_length, c.initial_extension_progress), (2, 2));
        // Not sticky: the stone stays.
        assert_eq!(c.blocks.len(), 3);
        // Back in: the head lands in the piston (retracting it), the poles
        // below it... the pole at the piston's place goes behind.
        let (placed, retracted) = c.placements(0);
        assert!(retracted);
        let mut at: Vec<Pos> = placed.iter().map(|(p, _)| *p).collect();
        at.sort();
        assert_eq!(at, vec![[0, -2, 0], [0, -1, 0]]);
        w.0.insert([0, 0, 0], piston(true));
        w.0.insert(
            [0, 2, 0],
            B::Part(PistonPart::Head {
                facing: Dir::Up,
                sticky: true,
            }),
        );
        let c = PistonContraption::assemble(&w, [0, 0, 0], Dir::Up, true)
            .unwrap()
            .unwrap();
        assert!(
            c.blocks
                .iter()
                .any(|b| c.world_pos(b.local, 2) == [0, 3, 0])
        );
        // No poles at all.
        let w = rig(0, false);
        assert_eq!(
            PistonContraption::assemble(&w, [0, 0, 0], Dir::East, false),
            Err(AssemblyError::NoPoles)
        );
    }

    #[test]
    fn colliders_face_the_motion() {
        let mut w = rig(1, false);
        w.0.insert([1, 0, 0], B::Stone);
        let c = PistonContraption::assemble(&w, [0, 0, 0], Dir::East, false)
            .unwrap()
            .unwrap();
        let mut east = colliders(&c.blocks, Dir::East);
        east.sort();
        assert_eq!(east, vec![[0, 0, 0]]); // the stone, at the anchor
        // A wall two blocks out stops it once it gets there.
        w.0.insert([3, 0, 0], B::Stone);
        assert!(!c.colliding_with_world(&w, [2, 0, 0], Dir::East));
        assert!(c.colliding_with_world(&w, [3, 0, 0], Dir::East));
    }

    #[test]
    fn the_actuator_extends_and_places() {
        let mut w = rig(2, false);
        w.0.insert([1, 0, 0], B::Stone);
        let c = PistonContraption::assemble(&w, [0, 0, 0], Dir::East, false)
            .unwrap()
            .unwrap();
        let mut a = Actuator::default();
        // 64 RPM east: a positive axis not Z, speed negated by the modifier...
        let rpm = -64.0;
        a.speed(rpm);
        assert!(a.movement_speed(rpm, Dir::East) == 0.0); // no range yet
        assert_eq!(
            a.tick(rpm, Dir::East, c.anchor, false, |_, _| false),
            ActuatorStep::Assemble
        );
        assert_eq!(Actuator::movement_direction(rpm, Dir::East), Dir::East);
        assert!(a.start(&c, rpm, Dir::East, |_, _| false));
        assert_eq!(a.movement_speed(rpm, Dir::East), 0.125);
        let mut steps = 0;
        loop {
            match a.tick(rpm, Dir::East, c.anchor, false, |g, d| {
                c.colliding_with_world(&w, g, d)
            }) {
                ActuatorStep::Moved => steps += 1,
                ActuatorStep::Disassemble { grid_offset } => {
                    assert_eq!(grid_offset, 2);
                    break;
                }
                s => panic!("{s:?}"),
            }
        }
        assert_eq!(steps, 15);
        a.stopped(2);
        assert!(!a.running);
        // The other way at the start: nothing to do.
        let mut b = Actuator::default();
        b.speed(-rpm);
        assert_eq!(
            b.tick(-rpm, Dir::East, c.anchor, false, |_, _| false),
            ActuatorStep::Assemble
        );
        assert!(!b.start(&c, -rpm, Dir::East, |_, _| false));
    }

    #[test]
    fn a_wall_stops_it_on_the_grid() {
        let mut w = rig(3, false);
        for x in 1..=3 {
            w.0.insert([x, 0, 0], B::Stone);
        }
        w.0.insert([4, 0, 0], B::Bedrock);
        let c = PistonContraption::assemble(&w, [0, 0, 0], Dir::East, false);
        // Bedrock behind the stones in the line: unmovable.
        assert_eq!(c, Err(AssemblyError::Unmovable([4, 0, 0])));
        let mut w = rig(3, false);
        w.0.insert([1, 0, 0], B::Stone);
        // Not in the line (a gap), so the wall at x 3 is in the way.
        w.0.insert([3, 0, 0], B::Bedrock);
        let c = PistonContraption::assemble(&w, [0, 0, 0], Dir::East, false)
            .unwrap()
            .unwrap();
        let mut a = Actuator::default();
        a.speed(-64.0);
        a.tick(-64.0, Dir::East, c.anchor, false, |_, _| false);
        assert!(a.start(&c, -64.0, Dir::East, |g, d| {
            c.colliding_with_world(&w, g, d)
        }));
        let end = loop {
            if let ActuatorStep::Disassemble { grid_offset } =
                a.tick(-64.0, Dir::East, c.anchor, false, |g, d| {
                    c.colliding_with_world(&w, g, d)
                })
            {
                break grid_offset;
            }
        };
        assert_eq!(end, 1);
        // Stopping the network places it where it is.
        let mut a = Actuator::default();
        a.speed(-64.0);
        a.tick(-64.0, Dir::East, c.anchor, false, |_, _| false);
        a.start(&c, -64.0, Dir::East, |_, _| false);
        for _ in 0..3 {
            a.tick(-64.0, Dir::East, c.anchor, false, |_, _| false);
        }
        a.speed(0.0);
        assert_eq!(
            a.tick(0.0, Dir::East, c.anchor, false, |_, _| false),
            ActuatorStep::Disassemble { grid_offset: 0 }
        );
    }

    fn moved(c: &PistonContraption<String>) -> Vec<Pos> {
        c.blocks.iter().map(|b| c.world_pos(b.local, 0)).collect()
    }

    #[test]
    fn chassis_names_round_trip() {
        for kind in [
            ChassisKind::Linear,
            ChassisKind::SecondaryLinear,
            ChassisKind::Radial,
        ] {
            let n = if kind == ChassisKind::Radial { 16 } else { 4 };
            for bits in 0..n {
                assert_eq!(chassis_of(&chassis_block(kind, bits)), Some((kind, bits)));
            }
        }
        assert_eq!(
            chassis_block(ChassisKind::Radial, 0b0101),
            "create:hz_radial_chassis_north_south"
        );
        assert_eq!(chassis_of("create:hz_linear_chassis_bottom_top"), None);
    }

    #[test]
    fn slime_on_a_chassis_side() {
        let c = Chassis {
            kind: ChassisKind::Linear,
            axis: Axis::Y,
            sticky: 0,
            range: 8,
        };
        assert_eq!(c.use_item(Dir::Up, true, |_| None), Some(1));
        assert_eq!(c.use_item(Dir::North, true, |_| None), None);
        // Slime on a sticky side covers the others; one towards a chassis
        // of its kind stays bare.
        let top = Chassis { sticky: 1, ..c };
        assert_eq!(top.use_item(Dir::Up, true, |_| None), Some(3));
        assert_eq!(
            top.use_item(Dir::Up, true, |d| (d == Dir::Down).then_some(c)),
            Some(1)
        );
        // An empty hand while sneaking takes it off.
        assert_eq!(top.use_item(Dir::Up, false, |_| None), Some(0));
        assert_eq!(top.update_shape(Dir::Up, Some(&c)), 0);
        let radial = Chassis {
            kind: ChassisKind::Radial,
            axis: Axis::X,
            sticky: 0,
            range: 2,
        };
        assert_eq!(radial.use_item(Dir::Up, true, |_| None), Some(1));
        assert_eq!(radial.use_item(Dir::East, true, |_| None), None);
    }

    #[test]
    fn a_linear_chassis_takes_its_sticky_side() {
        let mut w = rig(1, false);
        let c = Chassis {
            kind: ChassisKind::Linear,
            axis: Axis::Y,
            sticky: 1,
            range: 2,
        };
        w.0.insert([1, 0, 0], B::Chassis(c));
        for y in 1..=3 {
            w.0.insert([1, y, 0], B::Stone);
        }
        w.0.insert([1, -1, 0], B::Stone);
        let m = moved(
            &PistonContraption::assemble(&w, [0, 0, 0], Dir::East, false)
                .unwrap()
                .unwrap(),
        );
        assert!(m.contains(&[1, 1, 0]) && m.contains(&[1, 2, 0]));
        assert!(!m.contains(&[1, 3, 0]) && !m.contains(&[1, -1, 0]));
        // Its neighbour of the same kind and axis joins it.
        w.0.insert([1, 0, 1], B::Chassis(Chassis { sticky: 0, ..c }));
        w.0.insert([1, 0, 2], B::Chassis(Chassis { axis: Axis::X, ..c }));
        let m = moved(
            &PistonContraption::assemble(&w, [0, 0, 0], Dir::East, false)
                .unwrap()
                .unwrap(),
        );
        assert!(m.contains(&[1, 0, 1]) && !m.contains(&[1, 0, 2]));
    }

    #[test]
    fn a_radial_chassis_takes_a_disc() {
        let mut w = rig(1, false);
        let c = Chassis {
            kind: ChassisKind::Radial,
            axis: Axis::X,
            sticky: 1,
            range: 2,
        };
        w.0.insert([1, 0, 0], B::Chassis(c));
        for p in [[1, 1, 0], [1, 2, 0], [1, 1, 1], [1, 3, 0], [1, -1, 0]] {
            w.0.insert(p, B::Stone);
        }
        let m = moved(
            &PistonContraption::assemble(&w, [0, 0, 0], Dir::East, false)
                .unwrap()
                .unwrap(),
        );
        assert!(m.contains(&[1, 1, 0]) && m.contains(&[1, 2, 0]) && m.contains(&[1, 1, 1]));
        assert!(!m.contains(&[1, 3, 0]) && !m.contains(&[1, -1, 0]));
    }

    #[test]
    fn super_glue_holds_its_blocks() {
        let mut w = rig(1, false);
        w.0.insert([1, 0, 0], B::Stone);
        w.0.insert([1, 1, 0], B::Stone);
        w.0.insert([1, 2, 0], B::Stone);
        w.1.push(([1, 0, 0], [1, 1, 0]));
        let c = PistonContraption::assemble(&w, [0, 0, 0], Dir::East, false)
            .unwrap()
            .unwrap();
        let m = moved(&c);
        assert!(m.contains(&[1, 1, 0]) && !m.contains(&[1, 2, 0]));
        assert_eq!(c.glue, vec![0]);
    }

    #[test]
    fn glue_joins_the_blocks_in_its_span() {
        let mut w = World::default();
        for p in [[0, 0, 0], [1, 0, 0], [2, 0, 0], [0, 1, 0]] {
            w.0.insert(p, B::Stone);
        }
        let g = glue_group(&w, [0, 0, 0], [1, 0, 0]).unwrap();
        assert_eq!(g.len(), 2);
        // Slime beside it joins whatever the span says.
        w.0.insert([0, 0, 1], B::Slime);
        assert!(
            glue_group(&w, [0, 0, 0], [1, 0, 0])
                .unwrap()
                .contains(&[0, 0, 1])
        );
        // A lone block holds nothing.
        assert!(glue_group(&w, [9, 9, 9], [9, 9, 9]).is_none());
    }

    #[test]
    fn a_bearing_takes_what_holds_to_its_front() {
        let mut w = World::default();
        w.0.insert([0, 1, 0], B::Slime);
        w.0.insert([1, 1, 0], B::Stone);
        w.0.insert([0, 0, 1], B::Stone);
        let c = BearingContraption::assemble(&w, [0, 0, 0], Dir::Up, false)
            .unwrap()
            .unwrap();
        assert_eq!(c.anchor, [0, 1, 0]);
        let locals: Vec<Pos> = c.blocks.iter().map(|b| b.local).collect();
        assert_eq!(locals, vec![[0, 0, 0], [1, 0, 0]]);
        // Nothing in front: nothing to turn.
        assert_eq!(
            BearingContraption::<String>::assemble(&w, [5, 0, 0], Dir::Up, false),
            Ok(None)
        );
        // Put back a quarter turn about Y.
        assert_eq!(c.placed_at([1, 0, 0], 90.0), [0, 1, -1]);
        assert_eq!(c.placed_at([1, 0, 0], 44.0), [1, 1, 0]);
    }

    #[test]
    fn a_windmill_needs_eight_sails() {
        let mut w = World::default();
        for i in 1..=7 {
            w.0.insert([0, i, 0], B::Sail);
        }
        assert_eq!(
            BearingContraption::assemble(&w, [0, 0, 0], Dir::Up, true),
            Err(AssemblyError::NotEnoughSails(7))
        );
        w.0.insert([0, 8, 0], B::Sail);
        let c = BearingContraption::assemble(&w, [0, 0, 0], Dir::Up, true)
            .unwrap()
            .unwrap();
        assert_eq!((c.sails, windmill_speed(c.sails)), (8, 1));
        assert_eq!(windmill_speed(200), 16);
    }

    #[test]
    fn the_bearing_turns_and_puts_back_on_a_quarter() {
        let mut b = Bearing::default();
        b.speed(32.0, false);
        assert_eq!(b.tick(32.0, false, false, || true), BearingStep::Assembled);
        assert!((b.angle - 9.6).abs() < 1e-4);
        for _ in 0..9 {
            b.tick(32.0, false, false, || true);
        }
        b.speed(0.0, false);
        assert_eq!(b.angle, 96.0);
        assert_eq!(
            b.tick(0.0, false, false, || true),
            BearingStep::Disassemble { angle: 96.0 }
        );
        assert_eq!(rounded_angle(96.0), 90);
        assert_eq!(rounded_angle(-100.0), -90);
        assert_eq!(rotation_steps(-90), rotation_steps(270));
        b.stopped();
        // A stopped bearing doesn't assemble; a windmill does.
        b.assemble_next_tick = true;
        assert_eq!(b.tick(0.0, false, false, || true), BearingStep::Idle);
        b.assemble_next_tick = true;
        assert_eq!(b.tick(0.0, true, false, || true), BearingStep::Assembled);
    }

    #[test]
    fn a_stalled_structure_stays_put() {
        // Actors breaking a block stall the bearing and the gantry.
        let mut b = Bearing::default();
        b.speed(32.0, false);
        b.tick(32.0, false, false, || true);
        let angle = b.angle;
        assert_eq!(b.tick(32.0, false, true, || true), BearingStep::Idle);
        assert_eq!(b.angle, angle);
        let shaft = |_: Pos| {
            Some(ShaftUnder {
                facing: Dir::East,
                part: GantryPart::Middle,
                rpm: 64.0,
            })
        };
        assert_eq!(
            gantry_tick([0.0; 3], Dir::Up, 9, true, shaft, |_, _| false),
            GantryStep::Moved([0.0; 3])
        );
    }

    #[test]
    fn elevators_find_their_column_and_floors() {
        // A contact facing east one block up in the cabin anchored at (10, 60, 5).
        let (column, y) = elevator_column(
            &[([1, 1, 0], Dir::East), ([0, 0, 0], Dir::Down)],
            [10, 60, 5],
        )
        .unwrap();
        assert_eq!(
            column,
            Column {
                x: 12,
                z: 5,
                side: Dir::West
            }
        );
        assert_eq!(y, 1);
        assert_eq!(
            elevator_column(&[([0, 0, 0], Dir::Up)], [0; 3]),
            Err(ElevatorError::NoContacts)
        );
        assert_eq!(
            elevator_column(&[([0, 0, 0], Dir::East), ([0, 1, 0], Dir::North)], [0; 3]),
            Err(ElevatorError::TooManyContacts)
        );
        let mut col = ElevatorColumn::default();
        for y in [70, 61, 65, 61] {
            col.add(y);
        }
        assert_eq!(col.contacts, vec![61, 65, 70]);
        col.call(70);
        assert_eq!((col.target, col.calling), (Some(70), Some(70)));

        // Pulley at y 80, cabin 19 below (its contact 1 up): level 61.
        let mut e = Elevator::new(80, 19, 1);
        assert_eq!(e.level_at(80, 19), 61);
        assert_eq!(e.target_offset(80, Some(70)), 10);
        assert_eq!(e.target_offset(80, Some(90)), 19);
        // At 64 rpm it speeds up 0.0175 a tick toward 0.5, then slows down.
        let (v, arrived) = e.movement_speed(10, 64.0);
        assert!((v + 0.01).abs() < 1e-6 && !arrived);
        let mut ticks = 0;
        loop {
            let (v, arrived) = e.movement_speed(10, 64.0);
            if arrived {
                break;
            }
            e.offset += v;
            ticks += 1;
            assert!(ticks < 400);
        }
        assert!((e.offset - 10.0).abs() < 0.5);
    }

    #[test]
    fn states_turn_with_the_structure() {
        let p = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        // A quarter turn about Y: facing, axis and fence sides.
        assert_eq!(
            turn_props(
                &p(&[("facing", "north")]),
                TurnShape::Plain,
                false,
                Axis::Y,
                270
            ),
            p(&[("facing", "east")])
        );
        assert_eq!(
            turn_props(&p(&[("axis", "x")]), TurnShape::Plain, false, Axis::Y, 90),
            p(&[("axis", "z")])
        );
        let fence = p(&[
            ("east", "false"),
            ("north", "true"),
            ("south", "false"),
            ("west", "false"),
        ]);
        assert_eq!(
            turn_props(&fence, TurnShape::Plain, false, Axis::Y, 270),
            p(&[
                ("east", "true"),
                ("north", "false"),
                ("south", "false"),
                ("west", "false")
            ])
        );
        // About X: a six-way facing turns, a log's axis too, a horizontal
        // facing only on a half turn.
        assert_eq!(
            turn_props(
                &p(&[("facing", "up")]),
                TurnShape::Plain,
                true,
                Axis::X,
                270
            ),
            p(&[("facing", "north")])
        );
        assert_eq!(
            turn_props(&p(&[("axis", "y")]), TurnShape::Plain, false, Axis::X, 90),
            p(&[("axis", "z")])
        );
        assert_eq!(
            turn_props(
                &p(&[("facing", "east")]),
                TurnShape::Plain,
                false,
                Axis::X,
                90
            ),
            p(&[("facing", "east")])
        );
        assert_eq!(
            turn_props(
                &p(&[("facing", "north"), ("type", "bottom")]),
                TurnShape::Plain,
                false,
                Axis::X,
                180
            ),
            p(&[("facing", "south"), ("type", "top")])
        );
        // Stairs facing north, upright, a quarter about X.
        let stairs = p(&[("facing", "north"), ("half", "bottom")]);
        let once = turn_props(&stairs, TurnShape::Stairs, false, Axis::X, 270);
        assert_eq!(once, p(&[("facing", "south"), ("half", "bottom")]));
        // A lever on the floor turned about X stands on a wall.
        let lever = p(&[("face", "floor"), ("facing", "east")]);
        assert_eq!(
            turn_props(&lever, TurnShape::FaceAttached, false, Axis::X, 270),
            p(&[("face", "wall"), ("facing", "north")])
        );
    }

    #[test]
    fn a_pulley_takes_what_hangs_below_its_rope() {
        let mut w = World::default();
        // Pulley at y 10, two ropes (counted by the glue), a slime block
        // with a stone beside it below.
        w.0.insert([0, 7, 0], B::Slime);
        w.0.insert([1, 7, 0], B::Stone);
        w.0.insert([0, 6, 0], B::Stone);
        w.0.insert([0, 5, 0], B::Stone);
        let c = PulleyContraption::assemble(&w, [0, 10, 0], 2)
            .unwrap()
            .unwrap();
        assert_eq!(c.anchor, [0, 7, 0]);
        let locals: Vec<Pos> = c.blocks.iter().map(|b| b.local).collect();
        assert!(
            locals.contains(&[1, 0, 0])
                && locals.contains(&[0, -1, 0])
                && !locals.contains(&[0, -2, 0])
        );
        assert_eq!(c.anchor_at(5), [0, 4, 0]);
        assert_eq!(
            PulleyContraption::<String>::assemble(&w, [9, 10, 9], 0),
            Ok(None)
        );
    }

    #[test]
    fn a_bare_rope_goes_down_to_a_block_then_takes_it() {
        let mut p = Pulley {
            range: 20,
            ..Pulley::default()
        };
        p.speed(64.0);
        assert_eq!(
            p.tick(64.0, false, |_| false, |_, _| false, |_| None),
            PulleyStep::Assemble
        );
        // Nothing hangs there yet: a bare rope runs.
        assert!(p.start(64.0, 0, Some(false)));
        let mut steps = 0;
        let stop = loop {
            steps += 1;
            match p.tick(64.0, false, |d| d >= 5, |_, _| false, |_| None) {
                PulleyStep::Moved => continue,
                s => break s,
            }
        };
        // At 64 RPM an eighth of a block a tick: the rope's end reaches the
        // block five below after 31 moves.
        assert_eq!(
            stop,
            PulleyStep::Disassemble {
                grid_offset: 4,
                reassemble: true
            }
        );
        assert_eq!(steps, 32);
        p.stopped(4, true);
        assert!(p.assemble_next_tick);
        // Going up from the top of the rope can't start.
        assert!(!p.start(-64.0, 0, Some(true)));
        // Going down unable to take the block can't either.
        assert!(!p.start(64.0, 3, None));
    }

    #[test]
    fn gantry_parts_and_assembly() {
        assert_eq!(GantryPart::of(true, false), GantryPart::Start);
        assert_eq!(GantryPart::of(true, true), GantryPart::Middle);
        for p in GantryPart::ALL {
            assert_eq!(gantry_shaft_of(&gantry_shaft_block(p)), Some(p));
        }
        assert!(GantryPart::Start.can_assemble_on(0.1) && !GantryPart::Start.can_assemble_on(-0.1));
        assert!(!GantryPart::Single.can_assemble_on(0.1));
        // A positive shaft speed drives the pinion backwards.
        assert_eq!(pinion_speed(64.0), -0.125);
        let mut w = World::default();
        w.0.insert([0, 1, 0], B::Slime);
        w.0.insert([1, 1, 0], B::Stone);
        w.0.insert([0, 0, 0], B::Stone);
        let c = GantryContraption::assemble(&w, [0, 1, 0], Dir::Up)
            .unwrap()
            .unwrap();
        let locals: Vec<Pos> = c.blocks.iter().map(|b| b.local).collect();
        // The shaft below (anchoring) stays.
        assert_eq!(locals, vec![[0, 0, 0], [1, 0, 0]]);
    }

    #[test]
    fn a_sequenced_gearshift_limits_a_bearing() {
        let mut b = Bearing {
            running: true,
            assemble_next_tick: false,
            limit: Some(10.0),
            ..Bearing::default()
        };
        for _ in 0..3 {
            b.tick(16.0, false, false, || false);
        }
        assert_eq!(b.angle, 10.0);
        assert_eq!(b.limit, Some(0.0));
        let mut a = Actuator {
            running: true,
            assemble_next_tick: false,
            extension_length: 8,
            limit: 1.0,
            ..Actuator::default()
        };
        assert_eq!(a.movement_speed(-256.0, Dir::East), 0.49);
        a.limit = 0.1;
        assert!((a.movement_speed(-256.0, Dir::East) - 0.1).abs() < 1e-6);
    }

    #[test]
    fn a_sticker_flips_on_a_rising_edge() {
        assert_eq!(sticker_state(false, false, true), (true, true));
        assert_eq!(sticker_state(true, true, false), (false, true));
        assert_eq!(sticker_state(false, true, true), (true, false));
        assert_eq!(sticker_state(true, true, true), (true, true));
    }

    #[test]
    fn a_carriage_runs_to_the_end_of_its_shaft() {
        // Shafts facing east along x 0..=3 under the carriage (y 0), the
        // carriage at y 1 facing up; -64 RPM: an eighth of a block a tick east.
        let shaft = |p: Pos| {
            (p[1] == 0 && p[2] == 0 && (0..=3).contains(&p[0])).then(|| ShaftUnder {
                facing: Dir::East,
                part: match p[0] {
                    0 => GantryPart::Start,
                    3 => GantryPart::End,
                    _ => GantryPart::Middle,
                },
                rpm: -64.0,
            })
        };
        let mut at = [0.0, 1.0, 0.0];
        let mut ticks = 0;
        let end = loop {
            ticks += 1;
            match gantry_tick(at, Dir::Up, ticks, false, shaft, |_, _| false) {
                GantryStep::Moved(m) => at = [at[0] + m[0], at[1] + m[1], at[2] + m[2]],
                GantryStep::Disassemble(p) => break p,
            }
            assert!(ticks < 100);
        };
        assert_eq!(end, [3, 1, 0]);
        // A wall in the way stops it on the grid.
        let end = gantry_tick([1.9, 1.0, 0.0], Dir::Up, 9, false, shaft, |g, _| g[0] >= 3);
        assert_eq!(end, GantryStep::Disassemble([2, 1, 0]));
    }
}
