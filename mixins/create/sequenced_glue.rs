// Horizon's side of sequenced assembly (rules in sequenced.rs): the recipes with their
// ingredients read, the `create:sequenced_assembly` item component, what machines ask
// (`SequencedAssemblyRecipe.getRecipe`), and the item's bar and tooltip.

use super::Game;
use super::create_deployer::DeployingRecipe;
use super::create_fluids::FluidStack;
use super::create_processing::{Output, Recipe};
use super::create_sequenced::{Assembly, SequencedRecipe, Step, StepKind, bar};
use horizon_client::crafting::Ingredient;
use horizon_client::items::ItemStack;
use horizon_client::text::{Style, Text};
use horizon_world::nbt::{Compound, JavaString, Tag};
use std::sync::{Arc, Mutex};

const COMPONENT: &str = "create:sequenced_assembly";

struct Read {
    recipe: SequencedRecipe,
    ingredient: Option<Ingredient>,
    /// Each deploying step's held ingredient, by step.
    held: Vec<Option<Ingredient>>,
}

static RECIPES: Mutex<Option<Arc<Vec<Read>>>> = Mutex::new(None);

fn choices(game: &Game<'_>, list: &[String]) -> Option<Ingredient> {
    let json = serde_json::Value::Array(list.iter().cloned().map(serde_json::Value::String).collect());
    super::create_machines::read_ingredient(game, &json)
}

fn recipes(game: &Game<'_>) -> Arc<Vec<Read>> {
    let mut guard = RECIPES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = guard.as_ref() {
        return r.clone();
    }
    let list: Vec<Read> = game
        .recipes()
        .custom
        .get("create:sequenced_assembly")
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|r| SequencedRecipe::parse(&r.id, &r.json))
        .map(|recipe| {
            let ingredient = choices(game, &recipe.ingredient);
            let held = recipe
                .steps
                .iter()
                .map(|s| match s {
                    Step::Deploying { held, .. } => choices(game, held),
                    _ => None,
                })
                .collect();
            Read { recipe, ingredient, held }
        })
        .collect();
    let list = Arc::new(list);
    *guard = Some(list.clone());
    list
}

/// A world opened or closed: the next one may have other recipes.
pub(super) fn world_phase() {
    *RECIPES.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// `native_component_types`.
pub(super) fn component_types(types: &mut Vec<String>) {
    types.push(COMPONENT.to_owned());
}

/// The component on a stack.
pub(super) fn assembly_of(stack: &ItemStack) -> Option<Assembly> {
    let Tag::Compound(c) = stack.data()?.other(COMPONENT)? else {
        return None;
    };
    let Tag::String(id) = c.get(&JavaString::from("id"))? else {
        return None;
    };
    let Tag::Int(step) = c.get(&JavaString::from("step"))? else {
        return None;
    };
    let Tag::Float(progress) = c.get(&JavaString::from("progress"))? else {
        return None;
    };
    Some(Assembly { id: id.to_string_lossy(), step: *step, progress: f32::from_bits(*progress) })
}

fn with_assembly(item: &str, a: &Assembly) -> ItemStack {
    let mut c = Compound::new();
    c.insert(JavaString::from("id"), Tag::String(JavaString::from(a.id.as_str())));
    c.insert(JavaString::from("step"), Tag::Int(a.step));
    c.insert(JavaString::from("progress"), Tag::Float(a.progress.to_bits()));
    let mut stack = ItemStack::new(item, 1);
    stack.data_mut().set_other(COMPONENT, Some(Tag::Compound(c)));
    stack
}

fn step_of(stack: &ItemStack) -> i32 {
    assembly_of(stack).map_or(0, |a| a.step)
}

/// `appliesTo`: the first item, or an item already on this recipe.
fn applies(r: &Read, stack: &ItemStack) -> bool {
    r.ingredient.as_ref().is_some_and(|i| i.test(stack)) || assembly_of(stack).is_some_and(|a| a.id == r.recipe.id)
}

/// `getRecipe`: the first recipe whose next step for `stack` is of `kind` and passes `test`.
fn find(game: &Game<'_>, stack: &ItemStack, kind: StepKind, test: impl Fn(&Read, usize) -> bool) -> Option<(Arc<Vec<Read>>, usize, usize)> {
    let list = recipes(game);
    let step = step_of(stack);
    let found = list.iter().enumerate().find_map(|(at, r)| {
        if !applies(r, stack) || r.recipe.next_step(step).kind() != kind {
            return None;
        }
        let index = step.max(0) as usize % r.recipe.steps.len();
        test(r, index).then_some((at, index))
    })?;
    Some((list, found.0, found.1))
}

/// `advance` on one item, as the step's results.
fn advanced(r: &SequencedRecipe, stack: &ItemStack) -> Vec<ItemStack> {
    match r.advance(step_of(stack)) {
        Some(a) => vec![with_assembly(&r.transitional, &a)],
        None => r
            .roll(super::create_machines::random_float())
            .map(|o| ItemStack::new(&o.item, o.count))
            .into_iter()
            .collect(),
    }
}

/// Whether `stack` waits for a step of `kind`.
pub(super) fn is_step(game: &Game<'_>, stack: &ItemStack, kind: StepKind) -> bool {
    find(game, stack, kind, |_, _| true).is_some()
}

/// What one item of `stack` becomes in a step of `kind`, if it waits for one.
pub(super) fn advance(game: &Game<'_>, stack: &ItemStack, kind: StepKind) -> Option<Vec<ItemStack>> {
    let (list, at, _) = find(game, stack, kind, |_, _| true)?;
    Some(advanced(&list[at].recipe, stack))
}

fn held_fits(r: &Read, index: usize, held: &ItemStack) -> bool {
    r.held[index].as_ref().is_some_and(|i| i.test(held))
}

/// `DeployerBlockEntity.getRecipe`'s assembly: the step as a deploying recipe.
pub(super) fn deploying(game: &Game<'_>, target: &ItemStack, held: &ItemStack) -> Option<DeployingRecipe> {
    let (list, at, index) = find(game, target, StepKind::Deploying, |r, i| held_fits(r, i, held))?;
    let r = &list[at].recipe;
    let Step::Deploying { held: choices, keep_held } = &r.steps[index] else {
        return None;
    };
    Some(DeployingRecipe {
        recipe: Recipe {
            id: r.id.clone(),
            ingredient: Vec::new(),
            results: vec![Output { item: r.transitional.clone(), count: 1, chance: 1.0 }],
            processing_time: 0,
        },
        held: choices.clone(),
        keep_held: *keep_held,
    })
}

/// What `target` becomes under a deployer holding `held`, if that is its next step.
pub(super) fn deployed(game: &Game<'_>, target: &ItemStack, held: &ItemStack) -> Option<Vec<ItemStack>> {
    let (list, at, _) = find(game, target, StepKind::Deploying, |r, i| held_fits(r, i, held))?;
    Some(advanced(&list[at].recipe, target))
}

/// `SawBlockEntity.getRecipes`' assembly: the step as a cutting recipe.
pub(super) fn cutting(game: &Game<'_>, input: &ItemStack) -> Option<Recipe> {
    let (list, at, index) = find(game, input, StepKind::Cutting, |_, _| true)?;
    let r = &list[at].recipe;
    let Step::Cutting { processing_time } = r.steps[index] else {
        return None;
    };
    Some(Recipe {
        id: r.id.clone(),
        ingredient: Vec::new(),
        results: vec![Output { item: r.transitional.clone(), count: 1, chance: 1.0 }],
        processing_time,
    })
}

/// `FillingBySpout.getRequiredAmountForItem`'s assembly.
pub(super) fn filling_amount(game: &Game<'_>, stack: &ItemStack, fluid: &FluidStack) -> Option<i32> {
    let (list, at, index) = find(game, stack, StepKind::Filling, |r, i| r.recipe.steps[i].takes_fluid(fluid))?;
    match &list[at].recipe.steps[index] {
        Step::Filling { fluid: f } => Some(f.amount),
        _ => None,
    }
}

/// `FillingBySpout.fillItem`'s assembly: what one item becomes with `fluid`.
pub(super) fn filled(game: &Game<'_>, stack: &ItemStack, fluid: &FluidStack) -> Option<Option<ItemStack>> {
    let (list, at, _) = find(game, stack, StepKind::Filling, |r, i| match &r.recipe.steps[i] {
        Step::Filling { fluid: f } => f.test(fluid),
        _ => false,
    })?;
    Some(advanced(&list[at].recipe, stack).into_iter().next())
}

/// `native_item_bar`: `SequencedAssemblyItem`'s bar.
pub(super) fn item_bar(stack: &ItemStack, shown: &mut Option<(i32, u32)>) {
    if let Some(a) = assembly_of(stack) {
        *shown = Some(bar(a.progress));
    }
}

fn describe(game: &Game<'_>, r: &Read, index: usize) -> Text {
    match &r.recipe.steps[index] {
        Step::Pressing => Text::translate("create.recipe.assembly.pressing", &[]),
        Step::Cutting { .. } => Text::translate("create.recipe.assembly.cutting", &[]),
        Step::Deploying { .. } => {
            let name = r.held[index]
                .as_ref()
                .and_then(|i| i.order.first())
                .map_or_else(
                    || Text::literal(""),
                    |item| {
                        let (ns, path) = item.split_once(':').unwrap_or(("minecraft", item));
                        let kind = if game.pack.assets.registry.default_state(item).is_some() { "block" } else { "item" };
                        Text::translate(&format!("{kind}.{ns}.{path}"), &[])
                    },
                );
            Text::translate("create.recipe.assembly.deploying_item", &[name])
        }
        Step::Filling { fluid } => {
            let name = fluid.fluids.first().map_or_else(
                || Text::literal(""),
                |f| {
                    let (ns, path) = f.split_once(':').unwrap_or(("minecraft", f));
                    Text::translate(&format!("block.{ns}.{path}"), &[])
                },
            );
            Text::translate("create.recipe.assembly.spout_filling_fluid", &[name])
        }
    }
}

/// `native_tooltip`: `SequencedAssemblyRecipe.addToTooltip`.
pub(super) fn tooltip(game: &Game<'_>, stack: &ItemStack, lines: &mut Vec<(Text, u32)>) {
    let Some(a) = assembly_of(stack) else {
        return;
    };
    let list = recipes(game);
    let Some(r) = list.iter().find(|r| r.recipe.id == a.id) else {
        return;
    };
    const GRAY: u32 = 0xAA_AA_AA;
    const DARK_GRAY: u32 = 0x55_55_55;
    const AQUA: u32 = 0x55_FF_FF;
    const DARK_AQUA: u32 = 0x00_AA_AA;
    let line = |t: Text, color: u32| (t.with_style(&Style::color(color)), u32::MAX);
    lines.push((Text::literal(""), u32::MAX));
    lines.push(line(Text::translate("create.recipe.sequenced_assembly", &[]), GRAY));
    let (step, total) = (Text::literal(a.step.to_string()), Text::literal(r.recipe.total().to_string()));
    lines.push(line(Text::translate("create.recipe.assembly.progress", &[step, total]), DARK_GRAY));
    let length = r.recipe.steps.len();
    for i in 0..r.recipe.upcoming(a.step).len() {
        let text = describe(game, r, (i + a.step.max(0) as usize) % length);
        lines.push(if i == 0 {
            line(Text::translate("create.recipe.assembly.next", &[text]), AQUA)
        } else {
            line(Text::literal("-> ").append(text), DARK_AQUA)
        });
    }
}
