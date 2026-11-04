// The rules of the smart observer and the threshold switch
// (`DirectedDirectionalBlock`, `FilteredDetectorFilterSlot`,
// `SmartObserverBlockEntity`, `ThresholdSwitchBlockEntity`,
// `ThresholdSwitchScreen`), with no Horizon types.

use super::create_kinetics::{Axis, Dir, rotate_centered};

/// `DirectedDirectionalBlock.TARGET`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Floor,
    Wall,
    Ceiling,
}

impl Target {
    pub fn parse(s: &str) -> Option<Target> {
        match s {
            "floor" => Some(Target::Floor),
            "wall" => Some(Target::Wall),
            "ceiling" => Some(Target::Ceiling),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Target::Floor => "floor",
            Target::Wall => "wall",
            Target::Ceiling => "ceiling",
        }
    }
}

/// `getTargetDirection`.
pub fn target_direction(target: Target, facing: Dir) -> Dir {
    match target {
        Target::Ceiling => Dir::Up,
        Target::Floor => Dir::Down,
        Target::Wall => facing,
    }
}

/// `getStateForPlacement` of both: the first of `looks` with something to
/// detect, else the look's opposite (sneaking: the look itself).
pub fn placement(
    looks: &[Dir],
    can_detect: impl Fn(Dir) -> bool,
    look: Dir,
    sneaking: bool,
    horizontal: Dir,
) -> (Target, Dir) {
    let preferred = looks
        .iter()
        .copied()
        .find(|d| can_detect(*d))
        .unwrap_or(if sneaking { look } else { look.opposite() });
    match preferred {
        Dir::Up => (Target::Ceiling, horizontal),
        Dir::Down => (Target::Floor, horizontal),
        d => (Target::Wall, d),
    }
}

/// `getRotatedBlockState`, with `face` the side the wrench clicked.
pub fn wrenched(target: Target, facing: Dir, face: Dir) -> (Target, Dir) {
    if face.axis() == Axis::Y {
        return (target, facing.clockwise(Axis::Y));
    }
    let mut next = target_direction(target, facing).clockwise(face.axis());
    if matches!(face, Dir::North | Dir::West) {
        next = next.opposite();
    }
    match next {
        Dir::Up => (Target::Ceiling, facing),
        Dir::Down => (Target::Floor, facing),
        d => (Target::Wall, d),
    }
}

/// `FilteredDetectorFilterSlot.isSideActive`.
pub fn side_active(target: Target, facing: Dir, side: Dir, slot_at_bottom: bool) -> bool {
    let to = target_direction(target, facing);
    if side == to {
        return false;
    }
    if to.opposite() == side {
        return true;
    }
    if to.axis() != Axis::Y {
        return side == Dir::Up || (side == Dir::Down && slot_at_bottom);
    }
    let side = if to == Dir::Up { side.opposite() } else { side };
    if !slot_at_bottom {
        return side == facing;
    }
    side.axis() == facing.clockwise(Axis::Y).axis()
}

/// `getLocalOffset` of the slot on `side`.
pub fn slot_centre(side: Dir) -> [f32; 3] {
    let south = [0.5, 0.5, 15.5 / 16.0];
    let v = rotate_centered(south, side.horizontal_angle(), Axis::Y);
    rotate_centered(v, side.vertical_angle(), Axis::X)
}

/// `rotate`: degrees about Y, then X, then Z.
pub fn slot_turns(side: Dir, facing: Dir) -> [f32; 3] {
    let y = side.horizontal_angle() + 180.0;
    let x = match side {
        Dir::Up => 90.0,
        Dir::Down => 270.0,
        _ => 0.0,
    };
    let z = if side == Dir::Up && facing.axis() != Axis::Y {
        180.0 - facing.horizontal_angle()
    } else {
        0.0
    };
    [y, x, z]
}

// ---- The smart observer ---------------------------------------------------------

/// `DEFAULT_DELAY`.
pub const DEFAULT_DELAY: i32 = 6;
/// A block of the filter's item at the target.
pub const BLOCK_DELAY: i32 = 3;
/// `onFunnelTransfer`.
pub const FUNNEL_DELAY: i32 = 4;

/// `SmartObserverBlockEntity`'s countdown, and the block tick it schedules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Observer {
    pub turn_off: i32,
    pub off_due: bool,
}

impl Observer {
    /// The start of a tick: true when the scheduled block tick turns it off.
    pub fn tick(&mut self) -> bool {
        let off = std::mem::take(&mut self.off_due);
        if self.turn_off > 0 {
            self.turn_off -= 1;
            if self.turn_off == 0 {
                self.off_due = true;
            }
        }
        off
    }
    pub fn activate(&mut self, ticks: i32) {
        self.turn_off = ticks;
    }
}

// ---- The threshold switch -------------------------------------------------------

/// `ThresholdType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetType {
    Unsupported,
    Item,
    Fluid,
    Custom,
}

impl TargetType {
    pub fn index(self) -> u32 {
        match self {
            TargetType::Unsupported => 0,
            TargetType::Item => 1,
            TargetType::Fluid => 2,
            TargetType::Custom => 3,
        }
    }
    pub fn from_index(i: u32) -> TargetType {
        match i {
            1 => TargetType::Item,
            2 => TargetType::Fluid,
            3 => TargetType::Custom,
            _ => TargetType::Unsupported,
        }
    }
}

/// `ThresholdSwitchBlockEntity`'s fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Switch {
    pub on_above: i32,
    pub off_below: i32,
    pub min: i32,
    pub level: i32,
    pub max: i32,
    pub in_stacks: bool,
    pub redstone: bool,
    pub inverted: bool,
    pub powered_after_delay: bool,
}

impl Default for Switch {
    fn default() -> Self {
        Switch {
            on_above: 128,
            off_below: 64,
            min: 0,
            level: -1,
            max: 0,
            in_stacks: false,
            redstone: false,
            inverted: false,
            powered_after_delay: false,
        }
    }
}

/// What `updateCurrentLevel` leaves for the block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelUpdate {
    /// The block's `level`; none: leave it.
    pub display: Option<i32>,
    /// `scheduleBlockTick`.
    pub schedule: bool,
}

impl Switch {
    /// `updateCurrentLevel`, given what the target holds as (min, level, max);
    /// none when nothing there can be read.
    pub fn update(&mut self, observed: Option<(i32, i32, i32)>) -> LevelUpdate {
        let Some((min, level, max)) = observed else {
            self.min = -1;
            self.max = -1;
            if self.level == -1 {
                return LevelUpdate {
                    display: None,
                    schedule: false,
                };
            }
            self.level = -1;
            self.redstone = false;
            return LevelUpdate {
                display: Some(0),
                schedule: true,
            };
        };
        self.min = min;
        self.max = max;
        self.level = level.max(min).min(max);
        let was = self.redstone;
        if self.redstone && self.level <= self.off_below {
            self.redstone = false;
        } else if !self.redstone && self.level >= self.on_above {
            self.redstone = true;
        }
        let normed = (self.level - min) as f32 / (max - min) as f32;
        let display = if self.level > 0 {
            (1.0 + normed * 4.0) as i32
        } else {
            0
        };
        LevelUpdate {
            display: Some(display),
            schedule: was != self.redstone,
        }
    }
    /// `shouldBePowered`.
    pub fn should_be_powered(&self) -> bool {
        self.inverted != self.redstone
    }
}

/// `getValueStep`.
pub fn value_step(kind: TargetType, stacks: bool) -> i32 {
    if kind == TargetType::Fluid {
        1000
    } else if stacks {
        64
    } else {
        1
    }
}

/// A `ScrollInput`'s value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Input {
    pub state: i32,
    pub min: i32,
    pub max: i32,
}

impl Input {
    fn clamp(&mut self) {
        if self.state >= self.max {
            self.state = self.max - 1;
        }
        if self.state < self.min {
            self.state = self.min;
        }
    }
    fn set(&mut self, state: i32) {
        self.state = state;
        self.clamp();
    }
    fn range(&mut self, min: i32, max: i32) {
        self.min = min;
        self.max = max;
    }
    /// `mouseScrolled`: true when it changed; `SHIFT_STEP` is `ScrollInput`'s.
    fn scroll(&mut self, forward: bool, shift: bool, step: i32) -> bool {
        let prior = self.state;
        self.state += if forward { step } else { -step };
        if shift {
            self.state -= self.state % SHIFT_STEP;
        }
        self.clamp();
        prior != self.state
    }
    /// The scroll sound's pitch.
    pub fn pitch(&self) -> f32 {
        1.5 + 0.1 * (self.state - self.min) as f32 / (self.max - self.min) as f32
    }
}

const SHIFT_STEP: i32 = 5;

/// `ThresholdSwitchScreen`'s inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inputs {
    pub on: Input,
    pub off: Input,
    pub stacks: bool,
    /// `lastModification`: ticks since a change, -1 for none.
    pub modified: i32,
}

impl Inputs {
    /// `init`.
    pub fn new(s: &Switch, kind: TargetType) -> Inputs {
        let step = value_step(kind, s.in_stacks);
        let mut off = Input {
            state: 0,
            min: s.min,
            max: s.max + 1 - step,
        };
        off.set(s.off_below);
        let mut on = Input {
            state: 0,
            min: s.min + step,
            max: s.max + 1,
        };
        on.set(s.on_above);
        let mut i = Inputs {
            on,
            off,
            stacks: s.in_stacks,
            modified: -1,
        };
        i.on_changed(kind, 0);
        i.off_changed(kind, 0);
        i.update_boxes(kind, s.min, s.max);
        i
    }
    fn step(&self, kind: TargetType) -> i32 {
        value_step(kind, self.stacks)
    }
    /// `onAbove`'s `calling`.
    fn on_changed(&mut self, kind: TargetType, depth: u32) {
        self.modified = 0;
        let step = self.step(kind);
        if depth > 8 || (self.off.state / step == 0 && self.on.state / step == 0) {
            return;
        }
        if self.off.state / step >= self.on.state / step {
            self.off.set((self.on.state - step) / step * step);
            self.off_changed(kind, depth + 1);
        }
    }
    /// `offBelow`'s `calling`.
    fn off_changed(&mut self, kind: TargetType, depth: u32) {
        self.modified = 0;
        let step = self.step(kind);
        if depth > 8 || (self.on.state / step == 0 && self.off.state / step == 0) {
            return;
        }
        if self.on.state / step <= self.off.state / step {
            self.on.set((self.off.state + step) / step * step);
            self.on_changed(kind, depth + 1);
        }
    }
    /// The wheel on `onAbove`: the sound's pitch if it changed.
    pub fn scroll_on(&mut self, kind: TargetType, forward: bool, shift: bool) -> Option<f32> {
        let step = self.step(kind) * if shift { 10 } else { 1 };
        if !self.on.scroll(forward, shift, step) {
            return None;
        }
        let pitch = self.on.pitch();
        self.on_changed(kind, 0);
        Some(pitch)
    }
    /// The wheel on `offBelow`.
    pub fn scroll_off(&mut self, kind: TargetType, forward: bool, shift: bool) -> Option<f32> {
        let step = self.step(kind) * if shift { 10 } else { 1 };
        if !self.off.scroll(forward, shift, step) {
            return None;
        }
        let pitch = self.off.pitch();
        self.off_changed(kind, 0);
        Some(pitch)
    }
    /// The wheel on the measure, a `SelectionScrollInput` (so inverted): up
    /// goes to items. The rounding comes with the next `update_boxes`.
    pub fn scroll_stacks(&mut self, forward: bool) -> Option<f32> {
        let mut input = Input {
            state: i32::from(self.stacks),
            min: 0,
            max: 2,
        };
        if !input.scroll(!forward, false, 1) {
            return None;
        }
        self.stacks = input.state == 1;
        Some(input.pitch())
    }
    /// `updateInputBoxes`.
    pub fn update_boxes(&mut self, kind: TargetType, level_min: i32, level_max: i32) {
        let step = self.step(kind);
        let (min, max) = (level_min + step, level_max);
        self.on.range(min, max + 1);
        let rounded = (self.on.state / step * step).max(min).min(max);
        if rounded != self.on.state {
            self.on.set(rounded);
            self.on_changed(kind, 0);
        }
        let (min, max) = (level_min, level_max - step);
        self.off.range(min, max + 1);
        let rounded = (self.off.state / step * step).max(min).min(max);
        if rounded != self.off.state {
            self.off.set(rounded);
            self.off_changed(kind, 0);
        }
    }
    /// `tick`, once the box update: true when its settings are due to be sent
    /// (20 ticks after the last change).
    pub fn tick(&mut self) -> bool {
        if self.modified >= 0 {
            self.modified += 1;
        }
        if self.modified >= 20 {
            self.modified = -1;
            return true;
        }
        false
    }
}

/// `ConfigureThresholdSwitchPacket` as `create:values` rows: the flag says it is
/// one, two bits which field.
pub const CONFIG_FLAG: i32 = 1 << 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Config {
    OffBelow(i32),
    OnAbove(i32),
    Flags { inverted: bool, stacks: bool },
}

impl Config {
    pub fn encode(self) -> i32 {
        let (kind, n) = match self {
            Config::OffBelow(n) => (0, n),
            Config::OnAbove(n) => (1, n),
            Config::Flags { inverted, stacks } => (2, i32::from(inverted) | i32::from(stacks) << 1),
        };
        CONFIG_FLAG | kind | (n & 0x0FFF_FFFF) << 2
    }
    pub fn decode(value: i32) -> Option<Config> {
        if value & CONFIG_FLAG == 0 {
            return None;
        }
        let n = (value & !CONFIG_FLAG) >> 2;
        match value & 3 {
            0 => Some(Config::OffBelow(n)),
            1 => Some(Config::OnAbove(n)),
            2 => Some(Config::Flags {
                inverted: n & 1 != 0,
                stacks: n & 2 != 0,
            }),
            _ => None,
        }
    }
}

/// `format`.
pub fn format_value(kind: TargetType, stacks: bool, value: i32) -> String {
    match kind {
        TargetType::Custom => format!("{value}y"),
        TargetType::Item if stacks => format!("{value} Stacks"),
        TargetType::Item => format!("{value} Items"),
        _ => format!("{value} Buckets"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_is_up_down_or_the_facing() {
        assert_eq!(target_direction(Target::Ceiling, Dir::North), Dir::Up);
        assert_eq!(target_direction(Target::Floor, Dir::North), Dir::Down);
        assert_eq!(target_direction(Target::Wall, Dir::East), Dir::East);
    }

    #[test]
    fn placement_prefers_what_it_can_detect() {
        let looks = [
            Dir::North,
            Dir::Down,
            Dir::East,
            Dir::West,
            Dir::Up,
            Dir::South,
        ];
        let n = Dir::North;
        assert_eq!(
            placement(&looks, |d| d == Dir::East, n, false, n),
            (Target::Wall, Dir::East)
        );
        assert_eq!(
            placement(&looks, |d| d == Dir::Down, n, false, Dir::West),
            (Target::Floor, Dir::West)
        );
        // Nothing: the look's opposite, or sneaking the look.
        assert_eq!(
            placement(&looks, |_| false, n, false, n),
            (Target::Wall, Dir::South)
        );
        assert_eq!(
            placement(&looks, |_| false, n, true, n),
            (Target::Wall, Dir::North)
        );
        assert_eq!(
            placement(&looks, |_| false, Dir::Up, false, n),
            (Target::Floor, n)
        );
    }

    #[test]
    fn wrench_on_top_turns_the_facing() {
        assert_eq!(
            wrenched(Target::Wall, Dir::North, Dir::Up),
            (Target::Wall, Dir::East)
        );
        assert_eq!(
            wrenched(Target::Floor, Dir::North, Dir::Down),
            (Target::Floor, Dir::East)
        );
    }

    #[test]
    fn wrench_on_a_side_turns_the_target() {
        // getClockWise(X) of north is up; on the east face (positive) it stays.
        let (t, f) = wrenched(Target::Wall, Dir::North, Dir::East);
        assert_eq!(target_direction(t, f), Dir::North.clockwise(Axis::X));
        let (t, f) = wrenched(Target::Wall, Dir::North, Dir::West);
        assert_eq!(
            target_direction(t, f),
            Dir::North.clockwise(Axis::X).opposite()
        );
    }

    #[test]
    fn filter_slots_by_target() {
        // On a wall: its back and the top; the switch's the bottom too.
        assert!(!side_active(Target::Wall, Dir::North, Dir::North, true));
        assert!(side_active(Target::Wall, Dir::North, Dir::South, false));
        assert!(side_active(Target::Wall, Dir::North, Dir::Up, false));
        assert!(!side_active(Target::Wall, Dir::North, Dir::Down, false));
        assert!(side_active(Target::Wall, Dir::North, Dir::Down, true));
        assert!(!side_active(Target::Wall, Dir::North, Dir::East, true));
        // Looking down: the observer's slot on its facing, the switch's across.
        assert!(side_active(Target::Floor, Dir::North, Dir::Up, false));
        assert!(!side_active(Target::Floor, Dir::North, Dir::Down, false));
        assert!(side_active(Target::Floor, Dir::North, Dir::North, false));
        assert!(!side_active(Target::Floor, Dir::North, Dir::South, false));
        assert!(side_active(Target::Floor, Dir::North, Dir::East, true));
        assert!(side_active(Target::Floor, Dir::North, Dir::West, true));
        assert!(!side_active(Target::Floor, Dir::North, Dir::North, true));
        // Looking up, the observer's is opposite its facing.
        assert!(side_active(Target::Ceiling, Dir::North, Dir::South, false));
        assert!(!side_active(Target::Ceiling, Dir::North, Dir::North, false));
    }

    #[test]
    fn slot_lies_on_its_side() {
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-3);
        assert!(close(slot_centre(Dir::South), [0.5, 0.5, 15.5 / 16.0]));
        assert!(close(slot_centre(Dir::North), [0.5, 0.5, 0.5 / 16.0]));
        assert!(close(slot_centre(Dir::East), [15.5 / 16.0, 0.5, 0.5]));
        assert!(close(slot_centre(Dir::Up), [0.5, 15.5 / 16.0, 0.5]));
        assert!(close(slot_centre(Dir::Down), [0.5, 0.5 / 16.0, 0.5]));
    }

    #[test]
    fn observer_turns_off_a_tick_after_the_countdown() {
        let mut o = Observer::default();
        o.activate(2);
        assert!(!o.tick());
        assert_eq!(o.turn_off, 1);
        assert!(!o.tick());
        assert_eq!(o.turn_off, 0);
        assert!(o.tick());
        assert!(!o.tick());
    }

    #[test]
    fn switch_has_hysteresis() {
        let mut s = Switch {
            on_above: 10,
            off_below: 4,
            ..Switch::default()
        };
        let u = s.update(Some((0, 5, 100)));
        assert!(!s.redstone);
        assert!(!u.schedule);
        assert_eq!(u.display, Some(1));
        let u = s.update(Some((0, 10, 100)));
        assert!(s.redstone);
        assert!(u.schedule);
        // Between the two: stays.
        let u = s.update(Some((0, 5, 100)));
        assert!(s.redstone);
        assert!(!u.schedule);
        let u = s.update(Some((0, 4, 100)));
        assert!(!s.redstone);
        assert!(u.schedule);
    }

    #[test]
    fn switch_level_shows_how_full() {
        let mut s = Switch::default();
        assert_eq!(s.update(Some((0, 0, 100))).display, Some(0));
        assert_eq!(s.update(Some((0, 1, 100))).display, Some(1));
        assert_eq!(s.update(Some((0, 50, 100))).display, Some(3));
        assert_eq!(s.update(Some((0, 100, 100))).display, Some(5));
        assert_eq!(s.update(Some((0, 200, 100))).display, Some(5));
        assert_eq!(s.level, 100);
    }

    #[test]
    fn switch_losing_its_target_goes_dark_once() {
        let mut s = Switch {
            on_above: 1,
            off_below: 0,
            ..Switch::default()
        };
        assert_eq!(
            s.update(None),
            LevelUpdate {
                display: None,
                schedule: false
            }
        );
        s.update(Some((0, 5, 10)));
        assert!(s.redstone);
        assert_eq!(
            s.update(None),
            LevelUpdate {
                display: Some(0),
                schedule: true
            }
        );
        assert!(!s.redstone);
        assert_eq!(s.level, -1);
        assert_eq!((s.min, s.max), (-1, -1));
        assert_eq!(
            s.update(None),
            LevelUpdate {
                display: None,
                schedule: false
            }
        );
    }

    #[test]
    fn inverted_switch_is_powered_when_off() {
        let s = Switch {
            inverted: true,
            ..Switch::default()
        };
        assert!(s.should_be_powered());
        let s = Switch {
            inverted: true,
            redstone: true,
            ..Switch::default()
        };
        assert!(!s.should_be_powered());
    }

    fn sw(on: i32, off: i32, max: i32) -> Switch {
        Switch {
            on_above: on,
            off_below: off,
            min: 0,
            level: 0,
            max,
            ..Switch::default()
        }
    }

    #[test]
    fn screen_keeps_on_above_off() {
        let mut i = Inputs::new(&sw(128, 64, 1728), TargetType::Item);
        assert_eq!((i.on.state, i.off.state), (128, 64));
        for _ in 0..63 {
            i.scroll_on(TargetType::Item, false, false);
        }
        assert_eq!((i.on.state, i.off.state), (65, 64));
        i.scroll_on(TargetType::Item, false, false);
        assert_eq!((i.on.state, i.off.state), (64, 63));
        assert_eq!(i.modified, 0);
        i.scroll_off(TargetType::Item, true, false);
        assert_eq!((i.on.state, i.off.state), (65, 64));
    }

    #[test]
    fn screen_clamps_to_the_inventory() {
        let i = Inputs::new(&sw(128, 64, 27), TargetType::Item);
        assert_eq!((i.on.state, i.off.state), (27, 26));
    }

    #[test]
    fn screen_rounds_to_stacks_and_buckets() {
        let mut i = Inputs::new(&sw(128, 70, 1728), TargetType::Item);
        assert_eq!(i.scroll_stacks(true), None);
        assert!(i.scroll_stacks(false).is_some());
        assert!(i.stacks);
        i.update_boxes(TargetType::Item, 0, 1728);
        assert_eq!((i.on.state, i.off.state), (128, 64));
        i.scroll_on(TargetType::Item, true, false);
        assert_eq!(i.on.state, 192);
        let i = Inputs::new(&sw(2500, 1500, 8000), TargetType::Fluid);
        assert_eq!((i.on.state, i.off.state), (2000, 1000));
    }

    #[test]
    fn shift_steps_ten_and_rounds_to_five() {
        let mut i = Inputs::new(&sw(128, 64, 1728), TargetType::Item);
        i.scroll_on(TargetType::Item, true, true);
        assert_eq!(i.on.state, 135);
    }

    #[test]
    fn screen_sends_twenty_ticks_after_a_change() {
        // Opening counts as one: `init` calls the inputs' `onChanged`.
        let mut i = Inputs::new(&sw(128, 64, 1728), TargetType::Item);
        assert_eq!(i.modified, 0);
        assert!(!i.tick());
        i.scroll_on(TargetType::Item, true, false);
        for _ in 0..19 {
            assert!(!i.tick());
        }
        assert!(i.tick());
        assert!(!i.tick());
    }

    #[test]
    fn config_round_trips() {
        for c in [
            Config::OffBelow(0),
            Config::OnAbove(1_728_000),
            Config::Flags {
                inverted: true,
                stacks: false,
            },
            Config::Flags {
                inverted: false,
                stacks: true,
            },
        ] {
            assert_eq!(Config::decode(c.encode()), Some(c));
        }
        assert_eq!(Config::decode(256), None);
    }

    #[test]
    fn formats_by_kind() {
        assert_eq!(format_value(TargetType::Item, false, 3), "3 Items");
        assert_eq!(format_value(TargetType::Item, true, 3), "3 Stacks");
        assert_eq!(format_value(TargetType::Fluid, false, 3), "3 Buckets");
        assert_eq!(format_value(TargetType::Custom, false, 60), "60y");
    }
}
