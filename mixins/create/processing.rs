// Create's item processing, ported from Create 6.0.10 (MIT, The Create Team):
// PressingBehaviour's cycle, ProcessingRecipe and ProcessingOutput, the
// millstone's speed and the water wheel's flow score. Injected as
// `crate::game::create_processing`; no Horizon types, tested with `cargo test`.

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
}

impl PressMode {
    pub fn head_offset(self) -> f32 {
        match self {
            PressMode::World => 1.0,
            PressMode::Belt => 19.0 / 16.0,
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
}

impl Pressing {
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
    (flow_score.clamp(-1, 1) * 8) as f32
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
}
