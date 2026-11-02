// Create's andesite funnel, injected as `crate::game::create_funnel`: its
// states (`FunnelBlock`'s facing and extracting), its modes
// (`FunnelBlockEntity.determineCurrentMode`), the belt funnel it becomes on
// a belt (`BeltFunnelBlock.getShapeForPosition`), its extraction timer and
// where extracted items go (`activateExtractor`), and which items it
// collects (`FunnelBlock.entityInside`); the brass funnel's filter
// (`FilteringBehaviour`: one item, no lists or attributes), its extraction
// amount (`getAmountToExtract`) and where its filter is drawn
// (`FunnelFilterSlotPositioning`). No Horizon types.

use super::create_belt::{BeltFunnel, FunnelShape};
use super::create_kinetics::{Axis, Dir, Pos};

/// `defaultExtractionTimer`'s default, in ticks.
pub const EXTRACTION_TIMER: i32 = 8;

/// The funnel's blocks here: a lightning rod's template (all six facings)
/// for each value of `EXTRACTING`, which it has no property for. The item
/// places the extracting one (as Create, unless sneaking).
pub const EXTRACTING_FUNNEL: &str = "create:andesite_funnel";
pub const COLLECTING_FUNNEL: &str = "create:hz_andesite_funnel_collecting";
/// The brass funnel's, the same way.
pub const BRASS_EXTRACTING_FUNNEL: &str = "create:brass_funnel";
pub const BRASS_COLLECTING_FUNNEL: &str = "create:hz_brass_funnel_collecting";

/// Whether `name` is one of the funnels' blocks.
pub fn is_funnel(name: &str) -> bool {
    matches!(
        name,
        EXTRACTING_FUNNEL | COLLECTING_FUNNEL | BRASS_EXTRACTING_FUNNEL | BRASS_COLLECTING_FUNNEL
    )
}

/// A funnel's block state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FunnelState {
    /// `FACING`: where its opening faces; the inventory is behind it.
    pub facing: Dir,
    pub extracting: bool,
    /// A brass funnel (`supportsFiltering`).
    pub brass: bool,
}

impl FunnelState {
    pub fn from_block<'a>(
        name: &str,
        prop: impl Fn(&str) -> Option<&'a str>,
    ) -> Option<FunnelState> {
        let (extracting, brass) = match name {
            EXTRACTING_FUNNEL => (true, false),
            COLLECTING_FUNNEL => (false, false),
            BRASS_EXTRACTING_FUNNEL => (true, true),
            BRASS_COLLECTING_FUNNEL => (false, true),
            _ => return None,
        };
        Some(FunnelState {
            facing: Dir::parse(prop("facing")?)?,
            extracting,
            brass,
        })
    }
    pub fn block(self) -> &'static str {
        match (self.brass, self.extracting) {
            (false, true) => EXTRACTING_FUNNEL,
            (false, false) => COLLECTING_FUNNEL,
            (true, true) => BRASS_EXTRACTING_FUNNEL,
            (true, false) => BRASS_COLLECTING_FUNNEL,
        }
    }
    /// The item it is (and drops).
    pub fn item(self) -> &'static str {
        if self.brass {
            BRASS_EXTRACTING_FUNNEL
        } else {
            EXTRACTING_FUNNEL
        }
    }
    /// `FunnelFilterSlotPositioning.isSideActive` for the funnel block (a
    /// funnel acting as a belt funnel here keeps its own model, and its
    /// filter where that model has it): a vertical funnel's filter is on
    /// each of its sides, a horizontal one's on its front.
    pub fn filter_sides(self) -> Vec<Dir> {
        if !self.brass {
            return Vec::new();
        }
        if self.facing.axis() == Axis::Y {
            vec![Dir::North, Dir::South, Dir::West, Dir::East]
        } else {
            vec![self.facing]
        }
    }
    /// Where the inventory it works on is (`BlockFace(pos, facing.getOpposite())`).
    pub fn behind(self, pos: Pos) -> Pos {
        let n = self.facing.opposite().normal();
        [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]]
    }
    /// `canInsertIntoFunnel` (when not powered: see `mode_powered`).
    pub fn collects(self) -> bool {
        !self.extracting
    }
    /// `supportsDirectBeltInput`: a collecting funnel facing up takes what a
    /// belt's end hands it.
    pub fn takes_from_belt_end(self) -> bool {
        !self.extracting && self.facing == Dir::Up
    }
    /// `onWrenched`: `EXTRACTING` cycles.
    pub fn wrenched(self) -> FunnelState {
        FunnelState {
            extracting: !self.extracting,
            ..self
        }
    }
}

/// `FunnelBlockEntity.Mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Powered.
    Paused,
    Collect,
    PushingToBelt,
    TakingFromBelt,
    Extract,
}

/// The belt segment under a funnel: its `HORIZONTAL_FACING` and its
/// `getMovementFacing`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeltBelow {
    pub facing: Dir,
    pub movement: Dir,
}

/// `BeltFunnelBlock.getShapeForPosition`: a horizontal funnel on a belt
/// becomes a belt funnel (`FunnelBlock.updateShape`) of this shape.
pub fn belt_funnel_shape(facing: Dir, extracting: bool, belt: BeltBelow) -> FunnelShape {
    let perpendicular = if extracting {
        FunnelShape::Pushing
    } else {
        FunnelShape::Pulling
    };
    if belt.facing.axis() != facing.axis() {
        perpendicular
    } else {
        FunnelShape::Retracted
    }
}

/// The belt funnel a funnel at a belt is, if it is one.
pub fn as_belt_funnel(state: FunnelState, belt: Option<BeltBelow>) -> Option<BeltFunnel> {
    if state.facing.axis() == Axis::Y {
        return None;
    }
    let belt = belt?;
    Some(BeltFunnel {
        facing: state.facing,
        shape: belt_funnel_shape(state.facing, state.extracting, belt),
        powered: false,
    })
}

/// `determineCurrentMode`.
pub fn mode(state: FunnelState, belt: Option<BeltBelow>) -> Mode {
    if let Some(f) = as_belt_funnel(state, belt) {
        return match f.shape {
            FunnelShape::Pulling => Mode::TakingFromBelt,
            FunnelShape::Pushing => Mode::PushingToBelt,
            _ => {
                if belt.is_some_and(|b| b.movement == state.facing) {
                    Mode::PushingToBelt
                } else {
                    Mode::TakingFromBelt
                }
            }
        };
    }
    if state.extracting {
        Mode::Extract
    } else {
        Mode::Collect
    }
}

/// `determineCurrentMode` with the block's `POWERED`: a powered funnel
/// pauses.
pub fn mode_powered(state: FunnelState, belt: Option<BeltBelow>, powered: bool) -> Mode {
    if powered {
        Mode::Paused
    } else {
        mode(state, belt)
    }
}

/// `FunnelBlockEntity.tick`'s timer: whether it acts this tick.
pub fn tick_cooldown(cooldown: &mut i32, mode: Mode) -> bool {
    if mode == Mode::Paused {
        *cooldown = 0;
    }
    if matches!(mode, Mode::TakingFromBelt | Mode::Collect | Mode::Paused) {
        return false;
    }
    if *cooldown > 0 {
        *cooldown -= 1;
        return false;
    }
    true
}

/// `getAmountToExtract`: an extractor (or a belt funnel) takes one at a
/// time without a filter (an andesite funnel's isn't active), a brass one
/// its filter's amount (a full stack: the amount's value box isn't ported);
/// others whole stacks.
pub fn amount_to_extract(mode: Mode, filter: Option<&Filter>) -> u32 {
    match mode {
        Mode::Extract | Mode::PushingToBelt | Mode::TakingFromBelt => filter.map_or(1, |f| f.count),
        _ => 64,
    }
}

/// A brass funnel's `FilteringBehaviour`: one item (no filter items, lists
/// or attributes) and the amount to extract (`count`, up to that many).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    /// The item's id; none: anything passes.
    pub item: Option<String>,
    pub count: u32,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            item: None,
            count: 64,
        }
    }
}

impl Filter {
    /// `test`: an empty filter lets everything through, otherwise the same
    /// item (components aren't compared).
    pub fn test(&self, item: &str) -> bool {
        self.item.as_deref().is_none_or(|f| f == item)
    }
    /// `setFilter` with what the player holds (`onShortInteract`); an
    /// empty hand clears it.
    pub fn set(&mut self, item: Option<&str>) {
        self.item = item.filter(|i| !i.is_empty()).map(str::to_owned);
    }
}

/// Where a brass funnel's filter is drawn on side `side`
/// (`FunnelFilterSlotPositioning`, for the funnel block): the slot's
/// centre in the block, its turns (degrees about Y, then X, as
/// `ValueBoxTransform.Sided.rotate` and the funnel's tilt) and the scale
/// (`ValueBoxTransform`'s half).
pub fn filter_slot(state: FunnelState, side: Dir) -> ([f32; 3], f32, f32) {
    let angle = side.horizontal_angle();
    let local = if state.facing.axis() == Axis::Y {
        let y = if state.facing == Dir::Down { 14.0 } else { 2.0 };
        [8.0, y, 15.5]
    } else {
        [8.0, 12.2, 8.55]
    };
    let at = rotate_centered_y([local[0] / 16.0, local[1] / 16.0, local[2] / 16.0], angle);
    let y_rot = angle + 180.0;
    let x_rot = if state.facing.axis() == Axis::Y {
        0.0
    } else {
        -22.5
    };
    (at, y_rot, x_rot)
}

/// `VecHelper.rotateCentered` about Y.
pub fn rotate_centered_y(v: [f32; 3], degrees: f32) -> [f32; 3] {
    let (sin, cos) = degrees.to_radians().sin_cos();
    let (x, z) = (v[0] - 0.5, v[2] - 0.5);
    [x * cos + z * sin + 0.5, v[1], z * cos - x * sin + 0.5]
}

/// `activateExtractor`'s item entity: where it appears and how it moves.
pub fn extracted_item(pos: Pos, facing: Dir) -> ([f64; 3], [f64; 3]) {
    let n = facing.normal().map(f64::from);
    let vertical = facing.axis() == Axis::Y;
    let up = facing == Dir::Up;
    let scale = if vertical {
        if up { 0.15 } else { 0.5 }
    } else {
        0.25
    };
    let mut p = [0, 1, 2].map(|k| f64::from(pos[k]) + 0.5 + n[k] * scale);
    if !vertical {
        p[1] -= 0.45;
    }
    let motion = if up { [0.0, 4.0 / 16.0, 0.0] } else { [0.0; 3] };
    (p, motion)
}

/// `getEntityOverflowScanningArea`: the block, and the one below it unless
/// the funnel faces up (min and max corners).
pub fn overflow_area(pos: Pos, facing: Dir) -> ([f64; 3], [f64; 3]) {
    let lo = pos.map(f64::from);
    let mut min = lo;
    if facing != Dir::Up {
        min[1] -= 1.0;
    }
    (min, [lo[0] + 1.0, lo[1] + 1.0, lo[2] + 1.0])
}

/// `FunnelBlock.entityInside`: whether an item entity at `at`, touching the
/// funnel at `pos`, is on its open side.
pub fn collects_entity_at(pos: Pos, facing: Dir, at: [f64; 3]) -> bool {
    let n = facing.normal().map(f64::from);
    let open = [0, 1, 2].map(|k| f64::from(pos[k]) + 0.5 - n[k] * 0.25);
    let diff = [at[0] - open[0], at[1] - open[1], at[2] - open[2]];
    let projected = match facing.axis() {
        Axis::X => diff[0],
        Axis::Y => diff[1],
        Axis::Z => diff[2],
    };
    (projected < 0.0) != facing.positive()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn funnel(facing: Dir, extracting: bool) -> FunnelState {
        FunnelState {
            facing,
            extracting,
            brass: false,
        }
    }

    #[test]
    fn blocks_and_modes() {
        let f = FunnelState::from_block(EXTRACTING_FUNNEL, |k| (k == "facing").then_some("north"))
            .unwrap();
        assert_eq!(f, funnel(Dir::North, true));
        assert_eq!(f.behind([0, 0, 0]), [0, 0, 1]);
        assert_eq!(f.wrenched().block(), COLLECTING_FUNNEL);
        assert_eq!(mode(f, None), Mode::Extract);
        assert_eq!(mode(f.wrenched(), None), Mode::Collect);
        // On a belt along it: retracted, by the belt's movement.
        let along = BeltBelow {
            facing: Dir::South,
            movement: Dir::North,
        };
        assert_eq!(
            belt_funnel_shape(Dir::North, true, along),
            FunnelShape::Retracted
        );
        assert_eq!(mode(f, Some(along)), Mode::PushingToBelt);
        let away = BeltBelow {
            movement: Dir::South,
            ..along
        };
        assert_eq!(mode(f, Some(away)), Mode::TakingFromBelt);
        // Across it: pushing or pulling.
        let across = BeltBelow {
            facing: Dir::East,
            movement: Dir::East,
        };
        assert_eq!(mode(f, Some(across)), Mode::PushingToBelt);
        assert_eq!(mode(f.wrenched(), Some(across)), Mode::TakingFromBelt);
        // A funnel facing up isn't a belt funnel.
        assert_eq!(mode(funnel(Dir::Up, false), Some(across)), Mode::Collect);
        assert_eq!(mode_powered(f, Some(across), true), Mode::Paused);
        assert_eq!(mode_powered(f, None, false), Mode::Extract);
        assert!(funnel(Dir::Up, false).takes_from_belt_end());
    }

    #[test]
    fn timer() {
        let mut c = 0;
        assert!(tick_cooldown(&mut c, Mode::Extract));
        c = EXTRACTION_TIMER;
        let waits = (0..20)
            .take_while(|_| !tick_cooldown(&mut c, Mode::Extract))
            .count();
        assert_eq!(waits, 8);
        assert!(!tick_cooldown(&mut 0, Mode::Collect));
        assert_eq!(amount_to_extract(Mode::Extract, None), 1);
        assert_eq!(amount_to_extract(Mode::Collect, None), 64);
    }

    #[test]
    fn brass_funnel_and_filter() {
        let f = FunnelState::from_block(BRASS_COLLECTING_FUNNEL, |k| {
            (k == "facing").then_some("east")
        })
        .unwrap();
        assert!(f.brass && !f.extracting);
        assert_eq!(f.wrenched().block(), BRASS_EXTRACTING_FUNNEL);
        assert_eq!(f.item(), BRASS_EXTRACTING_FUNNEL);
        assert!(is_funnel(BRASS_COLLECTING_FUNNEL) && !is_funnel("create:brass_tunnel"));
        assert_eq!(f.filter_sides(), vec![Dir::East]);
        assert_eq!(funnel(Dir::Up, true).filter_sides(), Vec::<Dir>::new());
        let up = FunnelState {
            brass: true,
            ..funnel(Dir::Up, false)
        };
        assert_eq!(up.filter_sides().len(), 4);
        // A brass extractor takes its filter's amount, a full stack by
        // default, whether or not an item is set.
        let mut filter = Filter::default();
        assert_eq!(amount_to_extract(Mode::Extract, Some(&filter)), 64);
        assert_eq!(amount_to_extract(Mode::Collect, Some(&filter)), 64);
        assert!(filter.test("minecraft:iron_ingot"));
        filter.set(Some("minecraft:gold_ingot"));
        assert!(filter.test("minecraft:gold_ingot") && !filter.test("minecraft:iron_ingot"));
        filter.set(None);
        assert!(filter.test("minecraft:iron_ingot"));
        // Its slot: facing north, near the middle of the block, tilted.
        let north = FunnelState {
            brass: true,
            ..funnel(Dir::North, true)
        };
        let (at, y, x) = filter_slot(north, Dir::North);
        assert!(
            (at[0] - 0.5).abs() < 1e-5 && (at[1] - 12.2 / 16.0).abs() < 1e-5,
            "{at:?}"
        );
        assert!((at[2] - (16.0 - 8.55) / 16.0).abs() < 1e-5, "{at:?}");
        assert_eq!((y, x), (360.0, -22.5));
        // Facing up: at the bottom of each side, the south one at the
        // south edge.
        let (at, _, x) = filter_slot(up, Dir::South);
        assert!((at[2] - 15.5 / 16.0).abs() < 1e-5 && (at[1] - 2.0 / 16.0).abs() < 1e-5);
        assert_eq!(x, 0.0);
    }

    #[test]
    fn where_items_go_and_come_from() {
        let (p, m) = extracted_item([0, 0, 0], Dir::North);
        assert!(
            (p[1] - 0.05).abs() < 1e-9 && p[0] == 0.5 && p[2] == 0.25,
            "{p:?}"
        );
        assert_eq!(m, [0.0; 3]);
        let (p, m) = extracted_item([0, 0, 0], Dir::Up);
        assert_eq!(p, [0.5, 0.65, 0.5]);
        assert_eq!(m[1], 0.25);
        assert_eq!(overflow_area([0, 5, 0], Dir::North).0, [0.0, 4.0, 0.0]);
        // A funnel facing up (a chest below) collects from above its middle.
        assert!(collects_entity_at([0, 0, 0], Dir::Up, [0.5, 0.6, 0.5]));
        assert!(!collects_entity_at([0, 0, 0], Dir::Up, [0.5, 0.1, 0.5]));
        // Facing north: in front of it, at lower z.
        assert!(collects_entity_at([0, 0, 0], Dir::North, [0.5, 0.5, 0.2]));
        assert!(!collects_entity_at([0, 0, 0], Dir::North, [0.5, 0.5, 0.9]));
    }
}
