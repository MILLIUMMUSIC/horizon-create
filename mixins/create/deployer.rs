// Create's deployer, ported from Create 6.0.10 (MIT, The Create Team):
// DeployerBlockEntity's cycle (waiting, expanding, retracting), its timer and
// hand animation, DeployerItemHandler, BeltDeployerCallbacks and the
// `create:deploying` / `create:item_application` recipes. No Horizon types:
// what the deployer does in front of it (DeployerHandler with its fake
// player) is the glue's, through `DeployerWorld`.

use super::create_belt::ProcessingResult;
use super::create_kinetics::{Axis, Dir};
use super::create_processing::{Output, Recipe, ingredient_choices};
use serde_json::Value;

/// `DeployerBlockEntity.State`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Waiting,
    Expanding,
    Retracting,
    Dumping,
}

impl State {
    pub fn code(self) -> u8 {
        self as u8
    }
    pub fn from_code(c: u8) -> State {
        match c {
            1 => State::Expanding,
            2 => State::Retracting,
            3 => State::Dumping,
            _ => State::Waiting,
        }
    }
}

/// `DeployerBlockEntity.Mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Punch,
    Use,
}

/// What the deployer does through its fake player (`DeployerHandler`).
pub trait DeployerWorld<S> {
    /// `DeployerHandler.shouldActivate` for the held stack.
    fn should_activate(&self, held: Option<&S>) -> bool;
    /// `start`'s ray: how far the first block in front is from the ray's
    /// origin (1.5 blocks out), at most 1.
    fn ray_distance(&self) -> f32;
    /// `activate`: the hand used in front, its held stack changed in place;
    /// what the fake player picked up goes into `inventory`.
    fn activate(&mut self, mode: Mode, held: &mut Option<S>, inventory: &mut Vec<S>);
}

/// `DeployerBlockEntity`'s fields.
#[derive(Clone, Debug, PartialEq)]
pub struct Deployer<S> {
    pub state: State,
    pub mode: Mode,
    pub timer: i32,
    pub reach: f32,
    /// The fake player's main hand (`heldItem`).
    pub held: Option<S>,
    /// The fake player's other items, moved to `overflow` while waiting.
    pub inventory: Vec<S>,
    pub overflow: Vec<S>,
    pub redstone_locked: bool,
    /// `filtering`: the item id the held item must be (none: any).
    pub filter: Option<String>,
    /// Something changed for saving and clients (`sendData`).
    pub dirty: bool,
}

impl<S: Clone> Default for Deployer<S> {
    fn default() -> Self {
        Self::new()
    }
}

/// `getTimerSpeed`.
pub fn timer_speed(speed: f32) -> i32 {
    if speed == 0.0 {
        0
    } else {
        (speed * 2.0).abs().clamp(8.0, 512.0) as i32
    }
}

impl<S: Clone> Deployer<S> {
    pub fn new() -> Self {
        Deployer {
            state: State::Waiting,
            mode: Mode::Use,
            timer: 0,
            reach: 0.0,
            held: None,
            inventory: Vec::new(),
            overflow: Vec::new(),
            redstone_locked: false,
            filter: None,
            dirty: false,
        }
    }

    /// `DeployerBlockEntity.tick` on the server.
    pub fn tick(&mut self, speed: f32, world: &mut dyn DeployerWorld<S>) {
        if speed == 0.0 {
            return;
        }
        let ts = timer_speed(speed);
        if self.timer > 0 {
            self.timer -= ts;
            return;
        }
        match self.state {
            State::Waiting => {
                if !self.overflow.is_empty() {
                    self.timer = ts * 10;
                    return;
                }
                // The fake player's other items go to the overflow (no
                // filter here: `filtering.test` passes everything).
                let mut changed = false;
                while !self.inventory.is_empty() {
                    if self.overflow.len() > 10 {
                        break;
                    }
                    self.overflow.push(self.inventory.remove(0));
                    changed = true;
                }
                if changed {
                    self.dirty = true;
                    self.timer = ts * 10;
                    return;
                }
                if self.mode == Mode::Use && !world.should_activate(self.held.as_ref()) {
                    self.timer = ts * 10;
                    return;
                }
                // Fist bumps (two punching deployers face to face) aren't ported.
                if self.redstone_locked {
                    return;
                }
                self.start(world);
            }
            State::Expanding => {
                world.activate(self.mode, &mut self.held, &mut self.inventory);
                self.state = State::Retracting;
                self.timer = 1000;
                self.dirty = true;
            }
            State::Retracting => {
                self.state = State::Waiting;
                self.timer = 500;
                self.dirty = true;
            }
            State::Dumping => {}
        }
    }

    /// `start`.
    pub fn start(&mut self, world: &dyn DeployerWorld<S>) {
        self.state = State::Expanding;
        self.reach = 0.5 + world.ray_distance().min(0.75);
        self.timer = 1000;
        self.dirty = true;
    }

    /// `changeMode` (the wrench on its front).
    pub fn change_mode(&mut self) {
        self.mode = match self.mode {
            Mode::Punch => Mode::Use,
            Mode::Use => Mode::Punch,
        };
        self.dirty = true;
    }

    /// `discardPlayer`: everything it holds, to drop.
    pub fn discard(&mut self) -> Vec<S> {
        let mut out: Vec<S> = self.held.take().into_iter().collect();
        out.append(&mut self.inventory);
        out.append(&mut self.overflow);
        out
    }
}

/// The hand partial (`getHandPose`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandPose {
    Pointing,
    Punching,
    Holding,
}

pub fn hand_pose(mode: Mode, holding: bool) -> HandPose {
    if mode == Mode::Punch {
        HandPose::Punching
    } else if holding {
        HandPose::Holding
    } else {
        HandPose::Pointing
    }
}

/// `getHandOffset(partialTicks)`: how far the pole and hand are out.
pub fn hand_offset(
    state: State,
    timer: i32,
    timer_speed: i32,
    partial: f32,
    pose: HandPose,
    reach: f32,
) -> f32 {
    let mut progress = 0.0;
    if state == State::Expanding {
        progress = 1.0 - (timer as f32 - partial * timer_speed as f32) / 1000.0;
    }
    if state == State::Retracting {
        progress = (timer as f32 - partial * timer_speed as f32) / 1000.0;
    }
    let hand_length = match pose {
        HandPose::Pointing => 0.0,
        HandPose::Holding => 4.0 / 16.0,
        HandPose::Punching => 3.0 / 16.0,
    };
    (progress.clamp(0.0, 1.0) * (reach + hand_length)).min(21.0 / 16.0)
}

/// `DirectionalAxisKineticBlock.getRotationAxis`.
pub fn rotation_axis(facing: Dir, along_first: bool) -> Axis {
    match facing.axis() {
        Axis::X => {
            if along_first {
                Axis::Y
            } else {
                Axis::Z
            }
        }
        Axis::Y => {
            if along_first {
                Axis::X
            } else {
                Axis::Z
            }
        }
        Axis::Z => {
            if along_first {
                Axis::X
            } else {
                Axis::Y
            }
        }
    }
}

/// `DirectionalAxisKineticBlock.getStateForPlacement`'s `alongFirst`:
/// `shaft_towards(side)` says whether the block on that side has a shaft
/// facing the deployer; `horizontal_x` whether the player faces along X.
pub fn placement_along_first(
    facing: Dir,
    horizontal_x: bool,
    shaft_towards: impl Fn(Dir) -> bool,
) -> bool {
    let axis = facing.axis();
    if axis != Axis::Y {
        let mut along = axis == Axis::Z;
        if shaft_towards(Dir::Up) || shaft_towards(Dir::Down) {
            along = axis == Axis::X;
        }
        return along;
    }
    let mut along = horizontal_x;
    let mut preferred: Option<Dir> = None;
    for side in [Dir::North, Dir::South, Dir::West, Dir::East] {
        // prefersConnectionTo(side.getClockWise(), false) is always false.
        if !shaft_towards(side) {
            continue;
        }
        if preferred.is_some_and(|p| p.axis() != side.axis()) {
            preferred = None;
            break;
        }
        preferred = Some(side);
    }
    if let Some(p) = preferred {
        along = p.axis() == Axis::X;
    }
    along
}

/// `IWrenchable.getRotatedBlockState` of a deployer (facing, along first)
/// clicked on `face` away from its front: along its facing's axis the shaft
/// turns (`AXIS_ALONG_FIRST_COORDINATE` cycles), else the facing turns about
/// the face's axis.
pub fn wrenched(facing: Dir, along_first: bool, face: Dir) -> (Dir, bool) {
    if facing.axis() == face.axis() {
        return (facing, !along_first);
    }
    let mut f = facing;
    loop {
        f = f.clockwise(face.axis());
        if f.axis() != face.axis() {
            return (f, along_first);
        }
    }
}

/// `BeltDeployerCallbacks.onItemReceived`: the result, and whether the
/// deployer starts (`start`).
pub fn on_item_received(
    speed: f32,
    mode: Mode,
    facing_down: bool,
    state: State,
    redstone_locked: bool,
    holding: bool,
    has_recipe: bool,
) -> (ProcessingResult, bool) {
    if speed == 0.0 || mode == Mode::Punch || !facing_down {
        return (ProcessingResult::Pass, false);
    }
    if state != State::Waiting {
        return (ProcessingResult::Hold, false);
    }
    if redstone_locked {
        return (ProcessingResult::Pass, false);
    }
    if !holding {
        return (ProcessingResult::Hold, false);
    }
    if !has_recipe {
        return (ProcessingResult::Pass, false);
    }
    (ProcessingResult::Hold, true)
}

/// What `whenItemHeld` does besides its result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldStep {
    Nothing,
    Start,
    /// `activate`: apply the recipe to one item.
    Apply,
}

/// `BeltDeployerCallbacks.whenItemHeld`.
#[allow(clippy::too_many_arguments)]
pub fn when_item_held(
    speed: f32,
    facing_down: bool,
    state: State,
    timer: i32,
    redstone_locked: bool,
    holding: bool,
    has_recipe: bool,
) -> (ProcessingResult, HeldStep) {
    if speed == 0.0 || !facing_down {
        return (ProcessingResult::Pass, HeldStep::Nothing);
    }
    if !holding {
        return (ProcessingResult::Hold, HeldStep::Nothing);
    }
    if !has_recipe {
        return (ProcessingResult::Pass, HeldStep::Nothing);
    }
    if state == State::Retracting && timer == 1000 {
        return (ProcessingResult::Hold, HeldStep::Apply);
    }
    if state == State::Waiting {
        if redstone_locked {
            return (ProcessingResult::Pass, HeldStep::Nothing);
        }
        return (ProcessingResult::Hold, HeldStep::Start);
    }
    (ProcessingResult::Hold, HeldStep::Nothing)
}

/// A `create:deploying` or `create:item_application` recipe: the item
/// worked on (`recipe.ingredient`), the held one, and its results.
#[derive(Clone, Debug, PartialEq)]
pub struct DeployingRecipe {
    pub recipe: Recipe,
    pub held: Vec<String>,
    /// `ItemApplicationRecipe.shouldKeepHeldItem`.
    pub keep_held: bool,
}

/// Reads one recipe (`ItemApplicationRecipe`'s two ingredients).
pub fn parse_deploying(id: &str, json: &Value) -> Option<DeployingRecipe> {
    let ingredients = json.get("ingredients")?.as_array()?;
    let target = ingredient_choices(ingredients.first()?);
    let held = ingredient_choices(ingredients.get(1)?);
    if target.is_empty() || held.is_empty() {
        return None;
    }
    let results = json
        .get("results")?
        .as_array()?
        .iter()
        .filter_map(|r| {
            Some(Output {
                item: r.get("id").or_else(|| r.get("item"))?.as_str()?.to_owned(),
                count: r.get("count").and_then(Value::as_u64).unwrap_or(1) as u32,
                chance: r.get("chance").and_then(Value::as_f64).unwrap_or(1.0) as f32,
            })
        })
        .collect();
    Some(DeployingRecipe {
        recipe: Recipe {
            id: id.to_owned(),
            ingredient: target,
            results,
            processing_time: 0,
        },
        held,
        keep_held: json
            .get("keep_held_item")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// What one application does to the held stack (`activate`'s end):
/// unchanged when kept, one damage on a damageable item (gone when it
/// breaks), else one used up, leaving its crafting remainder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldUse {
    Keep,
    Damage,
    Shrink,
}

pub fn held_use(keep_held: bool, max_damage: u32) -> HeldUse {
    if keep_held {
        HeldUse::Keep
    } else if max_damage > 0 {
        HeldUse::Damage
    } else {
        HeldUse::Shrink
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Front {
        activations: u32,
        should: bool,
    }

    impl DeployerWorld<u32> for Front {
        fn should_activate(&self, _: Option<&u32>) -> bool {
            self.should
        }
        fn ray_distance(&self) -> f32 {
            1.0
        }
        fn activate(&mut self, _: Mode, held: &mut Option<u32>, inventory: &mut Vec<u32>) {
            self.activations += 1;
            if let Some(h) = held.as_mut() {
                *h -= 1;
                if *h == 0 {
                    *held = None;
                }
            }
            inventory.push(99);
        }
    }

    #[test]
    fn a_cycle_expands_activates_and_retracts() {
        let mut d = Deployer::new();
        d.held = Some(2);
        let mut f = Front {
            activations: 0,
            should: true,
        };
        // 16 RPM: a timer speed of 32.
        assert_eq!(timer_speed(16.0), 32);
        d.tick(16.0, &mut f);
        assert_eq!(d.state, State::Expanding);
        assert!((d.reach - 1.25).abs() < 1e-6);
        let mut ticks = 1;
        while f.activations == 0 {
            d.tick(16.0, &mut f);
            ticks += 1;
        }
        // 1000 / 32 rounds up to 32 ticks out, then the activation.
        assert_eq!(ticks, 34);
        assert_eq!(d.state, State::Retracting);
        assert_eq!(d.held, Some(1));
        // Back in: 1000 more, then waiting with a 500 timer.
        while d.state != State::Waiting {
            d.tick(16.0, &mut f);
        }
        assert_eq!(d.timer, 500);
        // What the fake player picked up goes to the overflow first, and
        // then the deployer waits for it to be taken.
        while d.overflow.is_empty() {
            d.tick(16.0, &mut f);
        }
        assert_eq!(d.overflow, vec![99]);
        for _ in 0..200 {
            d.tick(16.0, &mut f);
        }
        assert_eq!(f.activations, 1);
    }

    #[test]
    fn nothing_to_do_waits_ten_steps() {
        let mut d: Deployer<u32> = Deployer::new();
        let mut f = Front {
            activations: 0,
            should: false,
        };
        d.tick(16.0, &mut f);
        assert_eq!(d.state, State::Waiting);
        assert_eq!(d.timer, 320);
        d.redstone_locked = true;
        f.should = true;
        d.timer = 0;
        d.tick(16.0, &mut f);
        assert_eq!(d.state, State::Waiting);
    }

    #[test]
    fn the_hand_goes_out_by_its_reach_and_length() {
        let pose = hand_pose(Mode::Use, true);
        assert_eq!(pose, HandPose::Holding);
        assert_eq!(hand_offset(State::Waiting, 0, 32, 0.0, pose, 1.25), 0.0);
        let out = hand_offset(State::Retracting, 1000, 32, 0.0, pose, 1.0);
        assert!((out - 1.25).abs() < 1e-6);
        // Capped at 21/16.
        assert!(
            (hand_offset(State::Retracting, 1000, 32, 0.0, pose, 1.25) - 21.0 / 16.0).abs() < 1e-6
        );
        let half = hand_offset(State::Expanding, 500, 32, 0.0, HandPose::Pointing, 1.0);
        assert!((half - 0.5).abs() < 1e-6);
    }

    #[test]
    fn rotation_axis_follows_axis_along_first() {
        assert_eq!(rotation_axis(Dir::Down, false), Axis::Z);
        assert_eq!(rotation_axis(Dir::Down, true), Axis::X);
        assert_eq!(rotation_axis(Dir::North, false), Axis::Y);
        assert_eq!(rotation_axis(Dir::East, true), Axis::Y);
    }

    #[test]
    fn placement_prefers_a_shaft_beside_it() {
        assert!(placement_along_first(Dir::North, false, |_| false));
        assert!(!placement_along_first(Dir::North, false, |d| d == Dir::Up));
        assert!(!placement_along_first(Dir::East, false, |_| false));
        assert!(placement_along_first(Dir::Down, true, |_| false));
        assert!(!placement_along_first(Dir::Down, true, |d| d == Dir::North));
        // Shafts on two axes: no preference.
        assert!(placement_along_first(Dir::Down, true, |d| d == Dir::North || d == Dir::East));
    }

    #[test]
    fn wrench_turns_the_shaft_from_behind_and_the_facing_from_the_side() {
        assert_eq!(wrenched(Dir::Down, false, Dir::Up), (Dir::Down, true));
        let (f, a) = wrenched(Dir::North, false, Dir::Up);
        assert_eq!(f.axis(), Axis::X);
        assert!(!a);
    }

    #[test]
    fn belt_callbacks_hold_start_and_apply() {
        let r = on_item_received(16.0, Mode::Use, true, State::Waiting, false, true, true);
        assert_eq!(r, (ProcessingResult::Hold, true));
        assert_eq!(
            on_item_received(16.0, Mode::Punch, true, State::Waiting, false, true, true),
            (ProcessingResult::Pass, false)
        );
        assert_eq!(
            on_item_received(16.0, Mode::Use, true, State::Waiting, false, false, true),
            (ProcessingResult::Hold, false)
        );
        assert_eq!(
            on_item_received(16.0, Mode::Use, true, State::Waiting, false, true, false),
            (ProcessingResult::Pass, false)
        );
        assert_eq!(
            when_item_held(16.0, true, State::Retracting, 1000, false, true, true),
            (ProcessingResult::Hold, HeldStep::Apply)
        );
        assert_eq!(
            when_item_held(16.0, true, State::Retracting, 968, false, true, true),
            (ProcessingResult::Hold, HeldStep::Nothing)
        );
        assert_eq!(
            when_item_held(16.0, true, State::Waiting, 0, false, true, true),
            (ProcessingResult::Hold, HeldStep::Start)
        );
    }

    #[test]
    fn recipes_read_both_ingredients_and_keep_held() {
        let json: Value = serde_json::from_str(
            r#"{"type":"create:deploying","ingredients":[{"item":"minecraft:exposed_chiseled_copper"},
            {"tag":"minecraft:axes"}],"keep_held_item":true,"results":[{"id":"minecraft:chiseled_copper"}]}"#,
        )
        .unwrap();
        let r = parse_deploying("create:deploying.x", &json).unwrap();
        assert_eq!(
            r.recipe.ingredient,
            vec!["minecraft:exposed_chiseled_copper"]
        );
        assert_eq!(r.held, vec!["#minecraft:axes"]);
        assert!(r.keep_held);
        assert_eq!(r.recipe.results[0].item, "minecraft:chiseled_copper");
        assert_eq!(held_use(true, 250), HeldUse::Keep);
        assert_eq!(held_use(false, 250), HeldUse::Damage);
        assert_eq!(held_use(false, 0), HeldUse::Shrink);
    }
}
