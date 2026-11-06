// Create's item processing, ported from Create 6.0.10 (MIT, The Create Team):
// PressingBehaviour's cycle, ProcessingRecipe and ProcessingOutput, the
// millstone's speed and the water wheel's flow score. Injected as
// `crate::game::create_processing`; no Horizon types, tested with `cargo test`.

use super::create_fluids::{FluidStack, LAVA, WATER, has_block_state};
use serde_json::Value;

/// `PressingBehaviour.CYCLE`: ticks of one press, down and up.
pub const PRESS_CYCLE: i32 = 240;
/// `PressingBehaviour.ENTITY_SCAN`.
const ENTITY_SCAN: i32 = 10;

/// `PressingBehaviour.Mode`, with how far down the head reaches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PressMode {
    /// Items on the ground below.
    #[default]
    World,
    /// An item on a depot (or belt) two blocks below.
    Belt,
    /// A basin two blocks below (`MechanicalPressBlockEntity`'s compacting).
    Basin,
}

impl PressMode {
    pub fn head_offset(self) -> f32 {
        match self {
            PressMode::World => 1.0,
            PressMode::Belt => 19.0 / 16.0,
            PressMode::Basin => 22.0 / 16.0,
        }
    }
    /// As saved and sent to clients.
    pub fn code(self) -> u8 {
        match self {
            PressMode::World => 0,
            PressMode::Belt => 1,
            PressMode::Basin => 2,
        }
    }
    pub fn from_code(code: u8) -> PressMode {
        match code {
            1 => PressMode::Belt,
            2 => PressMode::Basin,
            _ => PressMode::World,
        }
    }
}

/// What a tick of the press asks of the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PressStep {
    Idle,
    /// Not running: look for items on the ground below to press.
    ScanWorld,
    /// The head is down (`runningTicks == CYCLE / 2`): press now.
    Press,
    /// The cycle ended (`onPressingCompleted`).
    Completed,
    Moving,
    /// `basinChecker` (`updateBasin`): look for a recipe in the basin below.
    /// Asked by `basin_check`, not by `tick`.
    CheckBasin,
}

/// `PressingBehaviour`'s state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pressing {
    pub running: bool,
    pub finished: bool,
    pub mode: PressMode,
    pub running_ticks: i32,
    pub prev_running_ticks: i32,
    pub entity_scan_cooldown: i32,
    /// `basinChecker.scheduleUpdate()`.
    pub needs_update: bool,
    /// The speed `onSpeedChanged` last saw.
    pub last_speed: f32,
}

impl Pressing {
    /// `BasinOperatingBlockEntity`'s `onSpeedChanged` and `DeferralBehaviour.tick`:
    /// whether `updateBasin` should look in the basin now. It always answers
    /// true, so the flag clears even when it is turning too little or running.
    pub fn basin_check(&mut self, speed: f32) -> bool {
        if speed != self.last_speed {
            self.last_speed = speed;
            self.needs_update = true;
        }
        let check = self.needs_update && speed != 0.0 && !self.running;
        self.needs_update = false;
        check
    }
    /// `MechanicalPressBlockEntity.startProcessingBasin`.
    pub fn start_basin(&mut self) {
        if self.running && self.running_ticks <= PRESS_CYCLE / 2 {
            return;
        }
        self.start(PressMode::Basin);
    }
    /// `MechanicalPressBlockEntity.onBasinRemoved`.
    pub fn basin_removed(&mut self) {
        self.running = false;
        self.running_ticks = 0;
    }
    /// `getRunningTickSpeed`: 1 to 60 ticks of the cycle per game tick.
    pub fn running_tick_speed(speed: f32) -> i32 {
        if speed == 0.0 {
            return 0;
        }
        let t = (speed.abs() / 512.0).clamp(0.0, 1.0);
        (1.0 + t * (60.0 - 1.0)) as i32
    }
    /// `start`.
    pub fn start(&mut self, mode: PressMode) {
        self.mode = mode;
        self.running = true;
        self.prev_running_ticks = 0;
        self.running_ticks = 0;
    }
    /// `PressingBehaviour.tick` on the server, at network speed `speed`.
    /// The caller does what the step asks: `ScanWorld` looks for items below
    /// (and calls `start(World)` on one it can press), `Press` presses.
    pub fn tick(&mut self, speed: f32) -> PressStep {
        if !self.running {
            if speed == 0.0 {
                return PressStep::Idle;
            }
            if self.entity_scan_cooldown > 0 {
                self.entity_scan_cooldown -= 1;
            }
            if self.entity_scan_cooldown <= 0 {
                self.entity_scan_cooldown = ENTITY_SCAN;
                return PressStep::ScanWorld;
            }
            return PressStep::Idle;
        }
        let mut step = PressStep::Moving;
        if self.running_ticks == PRESS_CYCLE / 2 && speed != 0.0 {
            step = PressStep::Press;
        }
        if self.running_ticks > PRESS_CYCLE {
            self.finished = true;
            self.running = false;
            return PressStep::Completed;
        }
        self.prev_running_ticks = self.running_ticks;
        self.running_ticks += Self::running_tick_speed(speed);
        if self.prev_running_ticks < PRESS_CYCLE / 2 && self.running_ticks >= PRESS_CYCLE / 2 {
            self.running_ticks = PRESS_CYCLE / 2;
        }
        step
    }
    /// `applyInWorld` found an item: scan again at once next time.
    pub fn rescan(&mut self) {
        self.entity_scan_cooldown = 0;
    }
}

/// `getRenderedHeadOffset` (times the mode's reach): how far the head is
/// down, 0 at rest, at `ticks` running ticks.
pub fn press_head_offset(running: bool, mode: PressMode, ticks: f32) -> f32 {
    if !running {
        return 0.0;
    }
    let t = ticks.abs();
    let offset = if t < (PRESS_CYCLE * 2 / 3) as f32 {
        ((t / PRESS_CYCLE as f32 * 2.0).powi(3)).clamp(0.0, 1.0)
    } else {
        ((PRESS_CYCLE as f32 - t) / PRESS_CYCLE as f32 * 3.0).clamp(0.0, 1.0)
    };
    offset * mode.head_offset()
}

/// One `ProcessingOutput`.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub item: String,
    pub count: u32,
    pub chance: f32,
}

/// A `ProcessingRecipe` (pressing, milling...): its one ingredient's
/// choices (an item, or `#tag`), its outputs and its duration.
#[derive(Clone, Debug, PartialEq)]
pub struct Recipe {
    pub id: String,
    pub ingredient: Vec<String>,
    pub results: Vec<Output>,
    pub processing_time: i32,
}

/// An ingredient as 1.21.1 writes it (`{"item": ...}`, `{"tag": ...}`, or
/// a list of them), in the form Horizon's ingredient reader takes: an id,
/// `#tag`, or a list.
pub fn ingredient_choices(v: &Value) -> Vec<String> {
    match v {
        Value::Array(list) => list.iter().flat_map(ingredient_choices).collect(),
        Value::String(s) => vec![s.clone()],
        Value::Object(o) => {
            if let Some(item) = o.get("item").and_then(Value::as_str) {
                vec![item.to_owned()]
            } else if let Some(tag) = o.get("tag").and_then(Value::as_str) {
                vec![format!("#{tag}")]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

/// A recipe file of Create's processing types.
pub fn parse_recipe(id: &str, json: &Value) -> Option<Recipe> {
    let ingredient = ingredient_choices(json.get("ingredients")?.get(0)?);
    if ingredient.is_empty() {
        return None;
    }
    let results = json
        .get("results")?
        .as_array()?
        .iter()
        .filter_map(|r| {
            let item = r.get("id").or_else(|| r.get("item"))?.as_str()?.to_owned();
            Some(Output {
                item,
                count: r.get("count").and_then(Value::as_u64).unwrap_or(1) as u32,
                chance: r.get("chance").and_then(Value::as_f64).unwrap_or(1.0) as f32,
            })
        })
        .collect();
    Some(Recipe {
        id: id.to_owned(),
        ingredient,
        results,
        processing_time: json
            .get("processing_time")
            .or_else(|| json.get("processingTime"))
            .and_then(Value::as_i64)
            .unwrap_or(100) as i32,
    })
}

/// The ingredient's JSON for Horizon's ingredient reader.
pub fn ingredient_json(r: &Recipe) -> Value {
    Value::Array(r.ingredient.iter().cloned().map(Value::String).collect())
}

/// `ProcessingRecipe.rollResults`: each output, its chance rolled once per
/// item (`ProcessingOutput.rollOutput`); `next_float` is the level's random.
pub fn roll_results(r: &Recipe, mut next_float: impl FnMut() -> f32) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    for o in &r.results {
        let count = if o.chance < 1.0 {
            let mut count = o.count;
            for _ in 0..o.count {
                if next_float() > o.chance {
                    count -= 1;
                }
            }
            count
        } else {
            o.count
        };
        if count > 0 {
            out.push((o.item.clone(), count));
        }
    }
    out
}

/// `MillstoneBlockEntity.getProcessingSpeed`: timer ticks per game tick.
pub fn millstone_speed(speed: f32) -> i32 {
    ((speed / 16.0).abs() as i32).clamp(1, 512)
}

/// `WaterWheelBlockEntity.determineAndApplyFlowScore` for a small wheel
/// turning about `axis` (0 x, 1 y, 2 z): `flows` holds the fluid's flow
/// vector at each of the four neighbours off the axis (`SMALL_OFFSETS`),
/// by their offset.
pub fn water_wheel_flow_score(axis: usize, flows: &[([i32; 3], [f64; 3])]) -> i32 {
    let mut score = 0;
    for (offset, flow) in flows {
        // Only the flow in the wheel's plane.
        let mut f = *flow;
        f[axis] = 0.0;
        let len = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt();
        if len == 0.0 {
            continue;
        }
        let f = [f[0] / len, f[1] / len, f[2] / len];
        let n = offset.map(f64::from);
        let nl = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let n = [n[0] / nl, n[1] / nl, n[2] / nl];
        // VecHelper.rotate(normal, 90, axis): a quarter turn about the axis.
        let m = rotate_90(n, axis);
        let dot = f[0] * m[0] + f[1] * m[1] + f[2] * m[2];
        if dot.abs() > 0.5 {
            score += dot.signum() as i32;
        }
    }
    score
}

/// `VecHelper.rotate(v, 90, axis)`: cos 90 is taken as 0 and sin 90 as 1,
/// rotating x toward... as Mojang's `Vec3.xRot/yRot/zRot` do.
fn rotate_90(v: [f64; 3], axis: usize) -> [f64; 3] {
    let [x, y, z] = v;
    match axis {
        // xRot: y' = y cos + z sin, z' = z cos - y sin
        0 => [x, z, -y],
        // yRot: x' = x cos + z sin, z' = z cos - x sin
        1 => [z, y, -x],
        // zRot: x' = x cos + y sin, y' = y cos - x sin
        _ => [y, -x, z],
    }
}

/// `WaterWheelBlockEntity.getGeneratedSpeed` for a small wheel.
pub fn water_wheel_speed(flow_score: i32) -> f32 {
    water_wheel_speed_sized(flow_score, 1)
}

/// `getGeneratedSpeed` of a wheel of `getSize` `size` (2: the large one).
pub fn water_wheel_speed_sized(flow_score: i32, size: i32) -> f32 {
    (flow_score.clamp(-1, 1) * 8 / size) as f32
}

/// `WaterWheelBlockEntity.SMALL_OFFSETS` / `LARGE_OFFSETS`: where a wheel
/// about `axis` (0 x, 1 y, 2 z) feels the flow, from its centre.
pub fn wheel_offsets(axis: usize, large: bool) -> Vec<[i32; 3]> {
    let dirs: Vec<[i32; 3]> = (0..3)
        .filter(|k| *k != axis)
        .flat_map(|k| {
            let mut a = [0; 3];
            let mut b = [0; 3];
            a[k] = 1;
            b[k] = -1;
            [a, b]
        })
        .collect();
    if !large {
        return dirs;
    }
    let mut out = Vec::new();
    for d in &dirs {
        let centre = d.map(|c| c * 2);
        out.push(centre);
        for d2 in &dirs {
            if (0..3).any(|k| d[k] != 0 && d2[k] != 0) {
                continue;
            }
            out.push([0, 1, 2].map(|k| centre[k] + d2[k]));
        }
    }
    out
}

// ---- RecipeApplier ---------------------------------------------------------

/// `RecipeApplier.applyRecipeOn` with a processing recipe, on a stack of
/// `count`: each item rolls its results (`roll`), merged onto the stacks
/// rolled before up to their `max_stack`. Crafting remainders
/// (`returnProcessingRemainder`) are added by the glue (`with_remainder`).
pub fn apply_processing_on(
    count: u32,
    mut roll: impl FnMut() -> Vec<(String, u32)>,
    max_stack: impl Fn(&str) -> u32,
) -> Vec<(String, u32)> {
    let mut stacks: Vec<(String, u32)> = Vec::new();
    for _ in 0..count {
        for (item, mut n) in roll() {
            for previous in stacks.iter_mut() {
                if n == 0 || previous.0 != item {
                    continue;
                }
                let amount = max_stack(&previous.0).saturating_sub(previous.1).min(n);
                previous.1 += amount;
                n -= amount;
            }
            if n > 0 {
                stacks.push((item, n));
            }
        }
    }
    stacks
}

/// `ItemHelper.multipliedOutput`: a cooking recipe's result for a whole
/// stack, split into full stacks.
pub fn multiplied_output(
    count: u32,
    out: &str,
    out_count: u32,
    max_stack: u32,
) -> Vec<(String, u32)> {
    let mut total = count * out_count;
    let mut stacks = Vec::new();
    let max = max_stack.max(1);
    while total > max {
        stacks.push((out.to_owned(), max));
        total -= max;
    }
    stacks.push((out.to_owned(), total));
    stacks
}

// ---- The mechanical mixer and the basin -------------------------------------

/// `IRotate.SpeedLevel.MEDIUM` (`mediumSpeed` default): the mixer's
/// `getMinimumRequiredSpeedLevel`.
pub const MIXER_MIN_SPEED: f32 = 30.0;
/// `BasinBlockEntity`: input and output inventories of 9 slots each.
pub const BASIN_SLOTS: usize = 9;

/// What a tick of the mixer asks of the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixStep {
    Idle,
    /// `BasinOperatingBlockEntity.updateBasin`: look for a recipe in the
    /// basin two below and `start` it.
    CheckBasin,
    /// The head is down and the time is up: `applyBasinRecipe`.
    Apply,
}

/// `MechanicalMixerBlockEntity`'s state, with its `basinChecker`
/// (`DeferralBehaviour`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mixing {
    pub running: bool,
    pub running_ticks: i32,
    pub processing_ticks: i32,
    /// `DeferralBehaviour.needsUpdate`.
    pub needs_update: bool,
    /// The current recipe's `getProcessingDuration` (0: none given).
    pub duration: i32,
    /// The speed of the last tick, for `onSpeedChanged`.
    pub last_speed: f32,
}

impl Default for Mixing {
    fn default() -> Self {
        Self::new()
    }
}

impl Mixing {
    pub fn new() -> Self {
        Mixing {
            running: false,
            running_ticks: 0,
            processing_ticks: 0,
            needs_update: true,
            duration: 0,
            last_speed: 0.0,
        }
    }
    /// `isSpeedRequirementFulfilled`.
    pub fn fast_enough(speed: f32) -> bool {
        speed.abs() >= MIXER_MIN_SPEED
    }
    /// `MechanicalMixerBlockEntity.tick` on the server at network speed
    /// `speed` (zero while overstressed). The caller does what the step asks.
    pub fn tick(&mut self, speed: f32) -> MixStep {
        // BasinOperatingBlockEntity.onSpeedChanged: check the basin again.
        if speed != self.last_speed {
            self.last_speed = speed;
            self.needs_update = true;
        }
        let mut step = MixStep::Idle;
        // DeferralBehaviour.tick (inside super.tick, before the mixer's own):
        // updateBasin always answers true, so the flag clears.
        if self.needs_update {
            self.needs_update = false;
            if Self::fast_enough(speed) && speed != 0.0 && !self.running {
                step = MixStep::CheckBasin;
            }
        }
        if self.running_ticks >= 40 {
            self.running = false;
            self.running_ticks = 0;
            self.needs_update = true;
            return step;
        }
        if self.running {
            if speed == 0.0 || !Self::fast_enough(speed) {
                if self.running_ticks < 20 {
                    self.running_ticks = 40 - self.running_ticks;
                } else if self.running_ticks == 20 {
                    self.running_ticks += 1;
                }
            }
            if self.running_ticks == 20 {
                if self.processing_ticks < 0 {
                    let recipe_speed = if self.duration != 0 {
                        self.duration as f32 / 100.0
                    } else {
                        1.0
                    };
                    // Mth.log2 of an int: its floor.
                    let ratio = (512.0 / speed.abs()) as i32;
                    let log2 = if ratio > 0 {
                        31 - ratio.leading_zeros() as i32
                    } else {
                        0
                    };
                    self.processing_ticks = (log2 * (recipe_speed * 15.0).ceil() as i32 + 1).max(1);
                } else {
                    self.processing_ticks -= 1;
                    if self.processing_ticks == 0 {
                        self.running_ticks += 1;
                        self.processing_ticks = -1;
                        step = MixStep::Apply;
                    }
                }
            }
            if self.running_ticks != 20 {
                self.running_ticks += 1;
            }
        }
        step
    }
    /// `startProcessingBasin` for a recipe of `duration`, from `CheckBasin`.
    /// The deferral ran before the mixer's own tick code in Create, so this
    /// tick's step down is taken here.
    pub fn start(&mut self, duration: i32) {
        if self.running && self.running_ticks <= 20 {
            return;
        }
        self.duration = duration;
        self.running = true;
        self.running_ticks = 1;
    }
    /// `continueWithPreviousRecipe`, after `Apply`: mix again at once.
    pub fn continue_mixing(&mut self) {
        self.running_ticks = 20;
    }
    /// `onBasinRemoved`.
    pub fn basin_removed(&mut self) {
        if !self.running {
            return;
        }
        self.running_ticks = 40;
        self.running = false;
    }
}

/// `getRenderedHeadOffset`: how far down the head is, in blocks, at
/// `ticks` running ticks (`partial` of a tick on).
pub fn mixer_head_offset(running: bool, ticks: i32, partial: f32) -> f32 {
    let wave = |t: f32| (2.0 - (t * std::f32::consts::PI).cos()) / 2.0;
    let offset = if !running {
        0.0
    } else if ticks < 20 {
        wave((ticks as f32 + partial) / 20.0) - 0.5
    } else if ticks <= 20 {
        1.0
    } else {
        wave(((40 - ticks) as f32 - partial) / 20.0) - 0.5
    };
    offset + 7.0 / 16.0
}

/// `getRenderedHeadRotationSpeed`.
pub fn mixer_head_speed(speed: f32, running: bool, ticks: i32) -> f32 {
    if running {
        if ticks < 15 {
            return speed;
        }
        if ticks <= 20 {
            return speed * 2.0;
        }
        return speed;
    }
    speed / 2.0
}

/// `HeatCondition` of a basin recipe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heat {
    None,
    Heated,
    Superheated,
}

impl Heat {
    /// The requirement as `create_blaze::heat_met` takes it.
    pub fn requirement(self) -> u8 {
        match self {
            Heat::None => 0,
            Heat::Heated => 1,
            Heat::Superheated => 2,
        }
    }
}

/// A `create:mixing` recipe (a `BasinRecipe`).
#[derive(Clone, Debug, PartialEq)]
pub struct BasinRecipe {
    pub id: String,
    /// Each item ingredient's choices (ids and `#tags`).
    pub ingredients: Vec<Vec<String>>,
    pub results: Vec<Output>,
    /// `processing_time` (0: none given).
    pub processing_time: i32,
    pub heat: Heat,
    /// `getFluidIngredients`: `SizedFluidIngredient`s.
    pub fluid_ingredients: Vec<FluidIngredient>,
    /// `getFluidResults`.
    pub fluid_results: Vec<FluidStack>,
    /// Fluid results or ingredients there is no fluid for here (milk, a
    /// fluid tag other than water's and lava's), or an item ingredient of a
    /// type not read (`neoforge:block_tag`).
    pub unsupported: bool,
}

/// A `SizedFluidIngredient`: any of these fluids, this much.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FluidIngredient {
    pub fluids: Vec<String>,
    pub amount: i32,
}

impl FluidIngredient {
    /// `SizedFluidIngredient.test`: the fluid, at least this much.
    pub fn test(&self, stack: &FluidStack) -> bool {
        !stack.is_empty() && stack.amount >= self.amount && self.fluids.contains(&stack.fluid)
    }
}

/// The fluids a fluid tag holds here: vanilla's two tags (Create's milk,
/// `c:milk`, has no fluid in Horizon).
fn fluid_tag(tag: &str) -> Option<Vec<String>> {
    match tag {
        "minecraft:water" | "c:water" => Some(vec![WATER.to_owned()]),
        "minecraft:lava" | "c:lava" => Some(vec![LAVA.to_owned()]),
        _ => None,
    }
}

/// `MechanicalPressBlockEntity.canCompress`: a crafting recipe of 4 or 9 cells that
/// all take the same (`ItemHelper.matchAllIngredients`); `ingredients` holds each
/// cell's items, none for a blank one.
pub fn can_compress(ingredients: &[Vec<String>]) -> bool {
    matches!(ingredients.len(), 4 | 9) && ingredients.iter().all(|i| *i == ingredients[0])
}

impl BasinRecipe {
    /// A crafting recipe as the press runs it in a basin, `ingredients` as
    /// `can_compress` takes them.
    pub fn compressed(
        id: &str,
        ingredients: Vec<Vec<String>>,
        item: &str,
        count: u32,
    ) -> BasinRecipe {
        BasinRecipe {
            id: id.to_owned(),
            ingredients,
            results: vec![Output {
                item: item.to_owned(),
                count,
                chance: 1.0,
            }],
            processing_time: 0,
            heat: Heat::None,
            fluid_ingredients: Vec::new(),
            fluid_results: Vec::new(),
            unsupported: false,
        }
    }

    /// What the basin can do without blaze burners: items and the fluids
    /// Horizon has.
    pub fn supported(&self) -> bool {
        !self.unsupported && (!self.ingredients.is_empty() || !self.fluid_ingredients.is_empty())
    }

    /// As a single-output recipe, for `roll_results`.
    pub fn as_recipe(&self) -> Recipe {
        Recipe {
            id: self.id.clone(),
            ingredient: Vec::new(),
            results: self.results.clone(),
            processing_time: self.processing_time,
        }
    }
}

/// A `create:mixing` (or compacting) recipe file.
pub fn parse_basin_recipe(id: &str, json: &Value) -> Option<BasinRecipe> {
    let mut unsupported = false;
    let mut ingredients = Vec::new();
    let mut fluid_ingredients = Vec::new();
    let mut fluid_results = Vec::new();
    for i in json.get("ingredients")?.as_array()? {
        let kind = i.get("type").and_then(Value::as_str);
        if i.get("fluid").is_some() || i.get("amount").is_some() {
            // neoforge:single (`fluid`) or neoforge:tag (`tag`), sized.
            let amount = i.get("amount").and_then(Value::as_i64).unwrap_or(1000) as i32;
            let fluids = match (
                i.get("fluid").and_then(Value::as_str),
                i.get("tag").and_then(Value::as_str),
            ) {
                (Some(f), _) if has_block_state(f) => Some(vec![f.to_owned()]),
                (_, Some(tag)) => fluid_tag(tag),
                _ => None,
            };
            match fluids {
                Some(fluids) => fluid_ingredients.push(FluidIngredient { fluids, amount }),
                None => unsupported = true,
            }
            continue;
        }
        if kind.is_some_and(|t| t != "neoforge:compound") {
            unsupported = true;
            continue;
        }
        let choices = ingredient_choices(i);
        if choices.is_empty() {
            return None;
        }
        ingredients.push(choices);
    }
    let mut results = Vec::new();
    for r in json.get("results")?.as_array()? {
        // A fluid result has an amount.
        if let Some(amount) = r.get("amount").and_then(Value::as_i64) {
            let fluid = r
                .get("id")
                .or_else(|| r.get("fluid"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if has_block_state(fluid) {
                fluid_results.push(FluidStack::new(fluid, amount as i32));
            } else {
                unsupported = true;
            }
            continue;
        }
        results.push(Output {
            item: r.get("id").or_else(|| r.get("item"))?.as_str()?.to_owned(),
            count: r.get("count").and_then(Value::as_u64).unwrap_or(1) as u32,
            chance: r.get("chance").and_then(Value::as_f64).unwrap_or(1.0) as f32,
        });
    }
    let heat = match json.get("heat_requirement").and_then(Value::as_str) {
        Some("heated") => Heat::Heated,
        Some("superheated") => Heat::Superheated,
        _ => Heat::None,
    };
    Some(BasinRecipe {
        id: id.to_owned(),
        ingredients,
        results,
        processing_time: json
            .get("processing_time")
            .or_else(|| json.get("processingTime"))
            .and_then(Value::as_i64)
            .unwrap_or(0) as i32,
        heat,
        fluid_ingredients,
        fluid_results,
        unsupported,
    })
}

/// `BasinRecipe.apply`'s fluid pass over the basin's tanks (as its fluid
/// capability lists them: outputs, then inputs): for each ingredient, its
/// amount from the first tank that has all of it still. With `simulate`,
/// only whether every ingredient is there; otherwise the tanks shrink.
pub fn basin_take_fluids(
    tanks: &mut [FluidStack],
    ingredients: &[FluidIngredient],
    simulate: bool,
) -> bool {
    let mut extracted = vec![0; tanks.len()];
    'ingredients: for ingredient in ingredients {
        let mut required = ingredient.amount;
        for (t, stack) in tanks.iter_mut().enumerate() {
            if simulate && stack.amount <= extracted[t] {
                continue;
            }
            // SizedFluidIngredient.test: the fluid and at least the amount.
            if !ingredient.test(stack) {
                continue;
            }
            let drained = required.min(stack.amount);
            if !simulate {
                stack.amount -= drained;
                if stack.amount <= 0 {
                    *stack = FluidStack::empty();
                }
            }
            required -= drained;
            if required != 0 {
                continue;
            }
            extracted[t] += drained;
            continue 'ingredients;
        }
        return false;
    }
    true
}

/// `BasinRecipe.apply`'s item pass (simulated): one item for each of
/// `ingredients` ingredients, from the first slot that still has one and
/// passes `test(ingredient, slot)`. How many it takes from each slot, or
/// none when an ingredient isn't there.
pub fn basin_take(
    ingredients: usize,
    counts: &[u32],
    test: impl Fn(usize, usize) -> bool,
) -> Option<Vec<u32>> {
    let mut taken = vec![0u32; counts.len()];
    'ingredients: for i in 0..ingredients {
        for slot in 0..counts.len() {
            if counts[slot] <= taken[slot] {
                continue;
            }
            if !test(i, slot) {
                continue;
            }
            taken[slot] += 1;
            continue 'ingredients;
        }
        return None;
    }
    Some(taken)
}

/// `BasinInventory.insertItem` through `ItemHandlerHelper.insertItem`: the
/// slot a stack goes to (`same`: it holds the same item, `empty`). An item
/// already in the basin only stacks onto its slot; a new one takes the
/// first empty slot.
pub fn basin_insert_slot(
    slots: usize,
    same: impl Fn(usize) -> bool,
    empty: impl Fn(usize) -> bool,
) -> Option<usize> {
    if let Some(i) = (0..slots).find(|&i| same(i)) {
        return Some(i);
    }
    (0..slots).find(|&i| empty(i))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_press_cycle_presses_once_at_the_bottom() {
        let mut p = Pressing::default();
        p.start(PressMode::Belt);
        let speed = 64.0;
        let mut presses = 0;
        let mut ticks = 0;
        loop {
            ticks += 1;
            match p.tick(speed) {
                PressStep::Press => presses += 1,
                PressStep::Completed => break,
                _ => {}
            }
            assert!(ticks < 1000);
        }
        assert_eq!(presses, 1);
        assert!(!p.running && p.finished);
        // 64 rpm: lerp(1/8, 1, 60) = 8 cycle ticks a tick; 120 / 8 = 15
        // ticks down, one at the bottom, 15 up, and the tick that ends it.
        assert_eq!(Pressing::running_tick_speed(64.0), 8);
        assert_eq!(ticks, 32);
    }

    #[test]
    fn a_press_looks_in_its_basin_when_told_or_when_its_speed_changes() {
        let mut p = Pressing::default();
        // Not turning: nothing to do, and nothing left to do later.
        assert!(!p.basin_check(0.0));
        p.needs_update = true;
        assert!(!p.basin_check(0.0));
        assert!(!p.needs_update);
        // It starts turning.
        assert!(p.basin_check(16.0));
        assert!(!p.basin_check(16.0));
        p.needs_update = true;
        assert!(p.basin_check(16.0));
        // Running, it doesn't look, and the request is dropped.
        p.start_basin();
        p.needs_update = true;
        assert!(!p.basin_check(16.0));
        assert!(!p.needs_update);
    }

    #[test]
    fn a_press_over_a_basin_reaches_further_and_stops_when_it_goes() {
        let mut p = Pressing::default();
        p.start_basin();
        assert!(p.running && p.mode == PressMode::Basin);
        let mut presses = 0;
        while p.running {
            if p.tick(64.0) == PressStep::Press {
                presses += 1;
            }
        }
        assert_eq!(presses, 1);
        assert!((press_head_offset(true, PressMode::Basin, 120.0) - 1.375).abs() < 1e-6);
        // Going down it isn't restarted; coming back up it is.
        p.start_basin();
        p.running_ticks = 60;
        p.start_basin();
        assert_eq!(p.running_ticks, 60);
        p.running_ticks = 180;
        p.start_basin();
        assert_eq!(p.running_ticks, 0);
        p.running_ticks = 60;
        p.basin_removed();
        assert!(!p.running && p.running_ticks == 0);
    }

    #[test]
    fn press_modes_keep_their_codes() {
        for m in [PressMode::World, PressMode::Belt, PressMode::Basin] {
            assert_eq!(PressMode::from_code(m.code()), m);
        }
    }

    #[test]
    fn only_full_squares_of_one_ingredient_compress() {
        let iron = || vec!["minecraft:iron_ingot".to_owned()];
        assert!(can_compress(&vec![iron(); 9]));
        assert!(can_compress(&vec![iron(); 4]));
        assert!(!can_compress(&vec![iron(); 8]));
        assert!(!can_compress(&[iron()]));
        let mut mixed = vec![iron(); 9];
        mixed[4] = vec!["minecraft:gold_ingot".to_owned()];
        assert!(!can_compress(&mixed));
        // A hole in the middle (a furnace, a chest) is a blank cell.
        let mut ring = vec![iron(); 9];
        ring[4] = Vec::new();
        assert!(!can_compress(&ring));
        let r = BasinRecipe::compressed("iron_block", vec![iron(); 9], "minecraft:iron_block", 1);
        assert!(r.supported());
        assert_eq!(r.as_recipe().results[0].item, "minecraft:iron_block");
    }

    #[test]
    fn compacting_recipes_read_as_basin_recipes() {
        let r = parse_basin_recipe(
            "create:compacting.andesite_from_flint",
            &json!({"type": "create:compacting", "ingredients": [
                {"item": "minecraft:flint"}, {"item": "minecraft:flint"}, {"item": "minecraft:gravel"},
                {"type": "neoforge:single", "amount": 100, "fluid": "minecraft:lava"}],
                "results": [{"id": "minecraft:andesite"}]}),
        )
        .unwrap();
        assert!(r.supported());
        assert_eq!(r.ingredients.len(), 3);
        assert_eq!(r.fluid_ingredients[0].amount, 100);
        // Create's chocolate has no fluid here yet.
        let c = parse_basin_recipe(
            "create:compacting.chocolate",
            &json!({"type": "create:compacting", "ingredients": [
                {"type": "neoforge:single", "amount": 250, "fluid": "create:chocolate"}],
                "results": [{"id": "create:bar_of_chocolate"}]}),
        )
        .unwrap();
        assert!(!c.supported());
    }

    #[test]
    fn a_stopped_press_waits_and_scans_every_ten_ticks() {
        let mut p = Pressing::default();
        assert_eq!(p.tick(0.0), PressStep::Idle);
        let scans = (0..30)
            .filter(|_| p.tick(16.0) == PressStep::ScanWorld)
            .count();
        assert_eq!(scans, 3);
    }

    #[test]
    fn the_head_goes_down_and_back() {
        assert_eq!(press_head_offset(false, PressMode::World, 50.0), 0.0);
        assert_eq!(press_head_offset(true, PressMode::World, 120.0), 1.0);
        assert!((press_head_offset(true, PressMode::Belt, 120.0) - 19.0 / 16.0).abs() < 1e-6);
        assert_eq!(press_head_offset(true, PressMode::World, 240.0), 0.0);
        assert!(press_head_offset(true, PressMode::World, 30.0) < 0.05);
    }

    #[test]
    fn recipes_read_create_json() {
        let r = parse_recipe(
            "create:milling/allium",
            &json!({"type": "create:milling", "ingredients": [{"item": "minecraft:allium"}],
                "processing_time": 50, "results": [
                    {"count": 2, "id": "minecraft:magenta_dye"},
                    {"chance": 0.1, "count": 2, "id": "minecraft:purple_dye"}]}),
        )
        .unwrap();
        assert_eq!(r.ingredient, vec!["minecraft:allium"]);
        assert_eq!(r.processing_time, 50);
        assert_eq!(r.results[1].chance, 0.1);
        let p = parse_recipe(
            "create:pressing/iron_ingot",
            &json!({"ingredients": [{"tag": "c:ingots/iron"}], "results": [{"id": "create:iron_sheet"}]}),
        )
        .unwrap();
        assert_eq!(p.ingredient, vec!["#c:ingots/iron"]);
        assert_eq!(ingredient_json(&p), json!(["#c:ingots/iron"]));
    }

    #[test]
    fn chances_roll_per_item() {
        let r = Recipe {
            id: String::new(),
            ingredient: vec![],
            results: vec![
                Output {
                    item: "a".into(),
                    count: 2,
                    chance: 1.0,
                },
                Output {
                    item: "b".into(),
                    count: 2,
                    chance: 0.5,
                },
                Output {
                    item: "c".into(),
                    count: 1,
                    chance: 0.1,
                },
            ],
            processing_time: 100,
        };
        // Rolls: b keeps one (0.4 <= 0.5, 0.9 > 0.5), c loses its one.
        let mut rolls = [0.4f32, 0.9, 0.3].into_iter();
        let out = roll_results(&r, || rolls.next().unwrap());
        assert_eq!(out, vec![("a".into(), 2), ("b".into(), 1)]);
    }

    #[test]
    fn millstone_speed_is_rpm_over_sixteen() {
        assert_eq!(millstone_speed(8.0), 1);
        assert_eq!(millstone_speed(-64.0), 4);
        assert_eq!(millstone_speed(256.0), 16);
    }

    #[test]
    fn water_flowing_past_one_side_turns_the_wheel() {
        // A wheel about Z with water running east (+x) along its top.
        let flows = [([0, 1, 0], [1.0, 0.0, 0.0]), ([0, -1, 0], [0.0, 0.0, 0.0])];
        let top = water_wheel_flow_score(2, &flows);
        assert_eq!(top.abs(), 1);
        // The same flow under it turns it the other way.
        let under = water_wheel_flow_score(2, &[([0, -1, 0], [1.0, 0.0, 0.0])]);
        assert_eq!(under, -top);
        // Along the axis it does nothing.
        assert_eq!(
            water_wheel_flow_score(2, &[([0, 1, 0], [0.0, 0.0, 1.0])]),
            0
        );
        assert_eq!(water_wheel_speed(3), 8.0);
        assert_eq!(water_wheel_speed(-1), -8.0);
    }

    #[test]
    fn a_mixing_cycle_goes_down_mixes_and_comes_back() {
        let mut m = Mixing::new();
        // Too slow: nothing is checked.
        assert_eq!(m.tick(16.0), MixStep::Idle);
        assert_eq!(m.tick(32.0), MixStep::CheckBasin);
        m.start(0);
        let mut ticks = 0;
        let mut applied = 0;
        while m.running {
            ticks += 1;
            if m.tick(32.0) == MixStep::Apply {
                applied += 1;
            }
            assert!(ticks < 1000);
        }
        assert_eq!(applied, 1);
        // 32 rpm: log2(16) * 15 + 1 = 61 ticks of mixing at the bottom, 20
        // down (one taken by `start`) and 20 up, and the tick that ends it.
        assert_eq!(ticks, 19 + 1 + 61 + 19 + 1);
        // It looks again as it ends.
        assert!(m.needs_update);
    }

    #[test]
    fn a_mixer_that_slows_down_lifts_its_head() {
        let mut m = Mixing::new();
        m.tick(64.0);
        m.start(0);
        for _ in 0..5 {
            m.tick(64.0);
        }
        assert_eq!(m.running_ticks, 6);
        m.tick(8.0);
        assert_eq!(m.running_ticks, 35);
        // Continuing after a batch skips the way down.
        m.continue_mixing();
        assert_eq!(m.running_ticks, 20);
        m.basin_removed();
        assert!(!m.running && m.running_ticks == 40);
    }

    #[test]
    fn the_mixer_head_moves() {
        assert_eq!(mixer_head_offset(false, 0, 0.0), 7.0 / 16.0);
        assert_eq!(mixer_head_offset(true, 20, 0.0), 1.0 + 7.0 / 16.0);
        assert!((mixer_head_offset(true, 0, 0.0) - 7.0 / 16.0).abs() < 1e-6);
        assert!((mixer_head_offset(true, 40, 0.0) - 7.0 / 16.0).abs() < 1e-6);
        assert_eq!(mixer_head_speed(32.0, true, 18), 64.0);
        assert_eq!(mixer_head_speed(32.0, false, 0), 16.0);
    }

    #[test]
    fn basin_recipes_read_items_and_note_the_rest() {
        let alloy = parse_basin_recipe(
            "create:mixing.andesite_alloy",
            &json!({"type": "create:mixing", "ingredients": [{"item": "minecraft:andesite"},
                {"tag": "c:nuggets/iron"}], "results": [{"id": "create:andesite_alloy"}]}),
        )
        .unwrap();
        assert!(alloy.supported());
        assert_eq!(
            alloy.ingredients,
            vec![
                vec!["minecraft:andesite".to_owned()],
                vec!["#c:nuggets/iron".to_owned()]
            ]
        );
        assert_eq!(alloy.processing_time, 0);
        let brass = parse_basin_recipe(
            "b",
            &json!({"heat_requirement": "heated", "ingredients": [{"tag": "c:ingots/copper"}],
                "results": [{"count": 2, "id": "create:brass_ingot"}]}),
        )
        .unwrap();
        assert_eq!(brass.heat, Heat::Heated);
        assert!(brass.supported());
        let dough = parse_basin_recipe(
            "d",
            &json!({"ingredients": [{"tag": "c:flours/wheat"},
                {"type": "neoforge:single", "amount": 1000, "fluid": "minecraft:water"}],
                "results": [{"id": "create:dough"}]}),
        )
        .unwrap();
        assert!(dough.supported());
        assert_eq!(
            dough.fluid_ingredients,
            vec![FluidIngredient {
                fluids: vec![WATER.to_owned()],
                amount: 1000
            }]
        );
        let honey = parse_basin_recipe(
            "h",
            &json!({"ingredients": [{"item": "minecraft:honey_block"}], "results": [{"amount": 1000, "id": "create:honey"}]}),
        )
        .unwrap();
        // Create's honey has no fluid here.
        assert!(honey.unsupported && !honey.supported());
        let tea = parse_basin_recipe(
            "t",
            &json!({"ingredients": [{"tag": "minecraft:leaves"},
                {"type": "neoforge:tag", "amount": 250, "tag": "c:milk"}],
                "results": [{"amount": 500, "id": "create:tea"}]}),
        )
        .unwrap();
        assert!(tea.unsupported);
        let lava = parse_basin_recipe(
            "l",
            &json!({"heat_requirement": "superheated", "ingredients": [{"tag": "c:cobblestones"}],
                "results": [{"amount": 50, "id": "minecraft:lava"}]}),
        )
        .unwrap();
        assert_eq!(lava.fluid_results, vec![FluidStack::new(LAVA, 50)]);
        assert!(lava.supported());
        assert_eq!(lava.heat, Heat::Superheated);
        let mud = parse_basin_recipe(
            "m",
            &json!({"ingredients": [{"type": "neoforge:block_tag", "tag": "minecraft:convertable_to_mud"},
                {"type": "neoforge:single", "amount": 250, "fluid": "minecraft:water"}],
                "results": [{"id": "minecraft:mud"}]}),
        )
        .unwrap();
        assert!(!mud.supported());
    }

    #[test]
    fn the_basin_takes_each_fluid_from_one_tank() {
        let water = |n| FluidStack::new(WATER, n);
        let need = |n| {
            vec![FluidIngredient {
                fluids: vec![WATER.to_owned()],
                amount: n,
            }]
        };
        // Outputs first, then inputs: 600 + 600 in two input tanks can't
        // make 1000 (each ingredient comes from one tank).
        let mut tanks = vec![
            FluidStack::empty(),
            FluidStack::empty(),
            water(600),
            water(600),
        ];
        assert!(!basin_take_fluids(&mut tanks, &need(1000), true));
        let mut tanks = vec![
            FluidStack::empty(),
            FluidStack::empty(),
            water(1000),
            FluidStack::empty(),
        ];
        assert!(basin_take_fluids(&mut tanks, &need(250), true));
        assert!(basin_take_fluids(&mut tanks, &need(250), false));
        assert_eq!(tanks[2], water(750));
        assert!(basin_take_fluids(&mut tanks, &need(750), false));
        assert!(tanks[2].is_empty());
        assert!(!basin_take_fluids(&mut tanks, &need(1), true));
    }

    #[test]
    fn the_basin_takes_one_item_per_ingredient() {
        // Slots: andesite x1, iron nuggets x3. Ingredient 0 andesite, 1 nugget.
        let test = |i: usize, slot: usize| i == slot;
        assert_eq!(basin_take(2, &[1, 3], test), Some(vec![1, 1]));
        // Two of the same ingredient come from one slot while it lasts.
        let any = |_: usize, _: usize| true;
        assert_eq!(basin_take(3, &[2, 5], any), Some(vec![2, 1]));
        assert_eq!(basin_take(3, &[1, 1], any), None);
        assert_eq!(basin_take(1, &[0, 0], any), None);
        // Insertion: onto its own slot, else the first empty one.
        let items = [Some("a"), None, Some("b"), None];
        let slot =
            |item: &str| basin_insert_slot(4, |i| items[i] == Some(item), |i| items[i].is_none());
        assert_eq!(slot("b"), Some(2));
        assert_eq!(slot("c"), Some(1));
    }

    #[test]
    fn recipes_apply_on_whole_stacks() {
        // Three gravel splashed: each rolls a flint, merged into one stack.
        let out = apply_processing_on(3, || vec![("minecraft:flint".into(), 1)], |_| 64);
        assert_eq!(out, vec![("minecraft:flint".into(), 3)]);
        let out = apply_processing_on(3, || vec![("a".into(), 1)], |_| 2);
        assert_eq!(out, vec![("a".into(), 2), ("a".into(), 1)]);
        // A cooking result times the stack, split into full stacks.
        assert_eq!(
            multiplied_output(40, "minecraft:glass", 2, 64),
            vec![
                ("minecraft:glass".into(), 64),
                ("minecraft:glass".into(), 16)
            ]
        );
    }

    #[test]
    fn large_water_wheels() {
        assert_eq!(wheel_offsets(2, false).len(), 4);
        let large = wheel_offsets(2, true);
        assert_eq!(large.len(), 12);
        assert!(
            large.contains(&[2, 0, 0])
                && large.contains(&[2, 1, 0])
                && large.contains(&[-1, -2, 0])
        );
        assert!(large.iter().all(|o| o[2] == 0));
        assert_eq!(water_wheel_speed_sized(3, 2), 4.0);
        assert_eq!(water_wheel_speed_sized(-1, 2), -4.0);
    }
}
