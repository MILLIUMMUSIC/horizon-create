// Create's sequenced assembly, ported from Create 6.0.10 (MIT, The Create Team):
// `SequencedAssemblyRecipe`, `SequencedAssemblyItem`. A machine asks for an item's next
// step; if it is of the machine's kind, the step's result is the item advanced. No
// Horizon types: `sequenced_glue.rs` keeps the recipes and the items' component.

use super::create_fluids::FluidStack;
use super::create_processing::{FluidIngredient, Output, ingredient_choices, parse_basin_recipe};
use serde_json::Value;

/// The kinds of machine a step can ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    Pressing,
    Cutting,
    Deploying,
    Filling,
}

/// One `SequencedRecipe`.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Pressing,
    Cutting {
        processing_time: i32,
    },
    Deploying {
        /// The held item's choices (ids and `#tags`).
        held: Vec<String>,
        keep_held: bool,
    },
    Filling {
        fluid: FluidIngredient,
    },
}

impl Step {
    pub fn kind(&self) -> StepKind {
        match self {
            Step::Pressing => StepKind::Pressing,
            Step::Cutting { .. } => StepKind::Cutting,
            Step::Deploying { .. } => StepKind::Deploying,
            Step::Filling { .. } => StepKind::Filling,
        }
    }
    /// A fluid the spout has is one this step takes (`matchItemAndFluid`).
    pub fn takes_fluid(&self, fluid: &FluidStack) -> bool {
        match self {
            Step::Filling { fluid: f } => !fluid.is_empty() && f.fluids.contains(&fluid.fluid),
            _ => false,
        }
    }
}

/// `SequencedAssemblyRecipe.SequencedAssembly`, an item's data component.
#[derive(Clone, Debug, PartialEq)]
pub struct Assembly {
    pub id: String,
    pub step: i32,
    pub progress: f32,
}

/// A `SequencedAssemblyRecipe`.
#[derive(Clone, Debug, PartialEq)]
pub struct SequencedRecipe {
    pub id: String,
    /// The first item's choices.
    pub ingredient: Vec<String>,
    pub transitional: String,
    pub loops: i32,
    pub steps: Vec<Step>,
    /// `resultPool`: `chance` here is a weight.
    pub results: Vec<Output>,
}

fn parse_step(json: &Value) -> Option<Step> {
    match json.get("type")?.as_str()? {
        "create:pressing" => Some(Step::Pressing),
        "create:cutting" => Some(Step::Cutting {
            processing_time: json
                .get("processing_time")
                .and_then(Value::as_i64)
                .unwrap_or(0) as i32,
        }),
        "create:deploying" | "create:item_application" => {
            let held = ingredient_choices(json.get("ingredients")?.get(1)?);
            (!held.is_empty()).then_some(Step::Deploying {
                held,
                keep_held: json
                    .get("keep_held_item")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        }
        "create:filling" => {
            let r = parse_basin_recipe("", json)?;
            (!r.unsupported)
                .then(|| r.fluid_ingredients.into_iter().next())?
                .map(|fluid| Step::Filling { fluid })
        }
        _ => None,
    }
}

impl SequencedRecipe {
    /// A recipe file, or none when one of its steps can't run here (a fluid
    /// Horizon doesn't have).
    pub fn parse(id: &str, json: &Value) -> Option<SequencedRecipe> {
        let ingredient = ingredient_choices(json.get("ingredient")?);
        let transitional = json
            .get("transitional_item")?
            .get("id")?
            .as_str()?
            .to_owned();
        let steps = json
            .get("sequence")?
            .as_array()?
            .iter()
            .map(parse_step)
            .collect::<Option<Vec<_>>>()?;
        let results: Vec<Output> = json
            .get("results")?
            .as_array()?
            .iter()
            .filter_map(|r| {
                Some(Output {
                    item: r.get("id")?.as_str()?.to_owned(),
                    count: r.get("count").and_then(Value::as_u64).unwrap_or(1) as u32,
                    chance: r.get("chance").and_then(Value::as_f64).unwrap_or(1.0) as f32,
                })
            })
            .collect();
        if ingredient.is_empty() || steps.is_empty() || results.is_empty() {
            return None;
        }
        Some(SequencedRecipe {
            id: id.to_owned(),
            ingredient,
            transitional,
            // The codec's default.
            loops: json.get("loops").and_then(Value::as_i64).unwrap_or(1) as i32,
            steps,
            results,
        })
    }
    /// `getNextRecipe`: the step an item at `step` waits for.
    pub fn next_step(&self, step: i32) -> &Step {
        &self.steps[step.max(0) as usize % self.steps.len()]
    }
    /// All the steps from start to finish.
    pub fn total(&self) -> i32 {
        self.steps.len() as i32 * self.loops
    }
    /// `advance`: the assembly after this step, or none when the item is finished
    /// (`rollResult`).
    pub fn advance(&self, step: i32) -> Option<Assembly> {
        let next = step + 1;
        if next / self.steps.len() as i32 >= self.loops {
            return None;
        }
        Some(Assembly {
            id: self.id.clone(),
            step: next,
            progress: next as f32 / self.total() as f32,
        })
    }
    /// `rollResult`: `random` from 0 to 1 picks one by weight.
    pub fn roll(&self, random: f32) -> Option<&Output> {
        let total: f32 = self.results.iter().map(|o| o.chance).sum();
        if total == 0.0 {
            return None;
        }
        let mut number = random * total;
        for o in &self.results {
            number -= o.chance;
            if number < 0.0 {
                return Some(o);
            }
        }
        None
    }
    /// `addToTooltip`'s list: the next steps, at most one pass, none past the end.
    pub fn upcoming(&self, step: i32) -> Vec<&Step> {
        let remaining = self.total() - step;
        (0..self.steps.len() as i32)
            .take_while(|i| *i < remaining)
            .map(|i| self.next_step(i + step))
            .collect()
    }
}

/// `SequencedAssemblyItem`'s bar: its width, and its colour from `0xFFC074` to
/// `0x46FFE0` (`Color.mixColors`).
pub fn bar(progress: f32) -> (i32, u32) {
    let mix = |a: u32, b: u32| {
        (a as f32 + (b as f32 - a as f32) * progress.clamp(0.0, 1.0)).round() as u32
    };
    let rgb = mix(0xFF, 0x46) << 16 | mix(0xC0, 0xFF) << 8 | mix(0x74, 0xE0);
    ((progress * 13.0).round() as i32, rgb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn track() -> SequencedRecipe {
        SequencedRecipe::parse(
            "create:sequenced_assembly.track",
            &json!({"type": "create:sequenced_assembly", "ingredient": {"tag": "create:sleepers"},
                "results": [{"id": "create:track"}],
                "sequence": [
                    {"type": "create:deploying", "ingredients": [{"item": "create:incomplete_track"},
                        [{"tag": "c:nuggets/iron"}, {"tag": "c:nuggets/zinc"}]],
                        "results": [{"id": "create:incomplete_track"}]},
                    {"type": "create:pressing", "ingredients": [{"item": "create:incomplete_track"}],
                        "results": [{"id": "create:incomplete_track"}]}],
                "transitional_item": {"id": "create:incomplete_track"}}),
        )
        .unwrap()
    }

    #[test]
    fn a_recipe_file_reads_its_steps() {
        let r = track();
        assert_eq!(r.loops, 1);
        assert_eq!(r.ingredient, vec!["#create:sleepers"]);
        assert_eq!(r.next_step(0).kind(), StepKind::Deploying);
        assert_eq!(r.next_step(1), &Step::Pressing);
        match r.next_step(0) {
            Step::Deploying { held, keep_held } => {
                assert_eq!(held, &vec!["#c:nuggets/iron", "#c:nuggets/zinc"]);
                assert!(!keep_held);
            }
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn filling_steps_keep_their_fluid_and_unknown_fluids_leave_the_recipe_out() {
        let file = |fluid: &str| {
            json!({"ingredient": {"item": "create:powdered_obsidian"}, "results": [{"id": "create:sturdy_sheet"}],
                "sequence": [{"type": "create:filling", "ingredients": [{"item": "create:unprocessed_obsidian_sheet"},
                    {"type": "neoforge:single", "amount": 500, "fluid": fluid}],
                    "results": [{"id": "create:unprocessed_obsidian_sheet"}]}],
                "transitional_item": {"id": "create:unprocessed_obsidian_sheet"}})
        };
        let r = SequencedRecipe::parse("s", &file("minecraft:lava")).unwrap();
        match r.next_step(0) {
            Step::Filling { fluid } => assert_eq!(fluid.amount, 500),
            s => panic!("{s:?}"),
        }
        assert!(
            r.next_step(0)
                .takes_fluid(&FluidStack::new("minecraft:lava", 1000))
        );
        assert!(
            !r.next_step(0)
                .takes_fluid(&FluidStack::new("minecraft:water", 1000))
        );
        assert_eq!(SequencedRecipe::parse("s", &file("create:chocolate")), None);
    }

    #[test]
    fn a_step_of_another_kind_leaves_the_recipe_out() {
        let file = json!({"ingredient": {"item": "a:b"}, "results": [{"id": "a:c"}],
            "sequence": [{"type": "somemod:welding"}], "transitional_item": {"id": "a:d"}});
        assert_eq!(SequencedRecipe::parse("s", &file), None);
    }

    #[test]
    fn loops_repeat_the_sequence_and_then_it_finishes() {
        let mut r = track();
        r.loops = 2;
        assert_eq!(r.total(), 4);
        let a = r.advance(0).unwrap();
        assert_eq!((a.step, a.progress), (1, 0.25));
        assert_eq!(r.next_step(a.step), &Step::Pressing);
        assert_eq!(r.next_step(2).kind(), StepKind::Deploying);
        assert_eq!(r.advance(2).unwrap().progress, 0.75);
        assert_eq!(r.advance(3), None);
        r.loops = 1;
        assert_eq!(r.advance(1), None);
    }

    #[test]
    fn results_are_picked_by_weight() {
        let mut r = track();
        r.results = vec![
            Output {
                item: "good".into(),
                count: 1,
                chance: 120.0,
            },
            Output {
                item: "junk".into(),
                count: 1,
                chance: 30.0,
            },
        ];
        assert_eq!(r.roll(0.0).unwrap().item, "good");
        assert_eq!(r.roll(0.79).unwrap().item, "good");
        assert_eq!(r.roll(0.81).unwrap().item, "junk");
        r.results.clear();
        assert_eq!(r.roll(0.5), None);
    }

    #[test]
    fn the_tooltip_lists_a_pass_and_stops_at_the_end() {
        let mut r = track();
        r.loops = 2;
        assert_eq!(r.upcoming(0).len(), 2);
        assert_eq!(r.upcoming(1)[0], &Step::Pressing);
        assert_eq!(r.upcoming(3).len(), 1);
        assert!(r.upcoming(4).is_empty());
    }

    #[test]
    fn the_bar_fills_from_orange_to_teal() {
        assert_eq!(bar(0.0), (0, 0xFFC074));
        assert_eq!(bar(1.0), (13, 0x46FFE0));
        assert_eq!(bar(0.5).0, 7);
    }
}
