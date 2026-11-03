// Horizon's side of Create's mechanical crafter, injected as
// `crate::game::create_crafter_glue`: each crafter's block entity
// (`MechanicalCrafterBlockEntity`: its one-item slot, cover, phase, countdown
// and grouped items, kept here and in its data), the chain's work
// (`checkCompletedRecipe`, passing items on, crafting with vanilla's crafting
// recipes and `create:mechanical_crafting`, putting the result out), clicks
// on its front (`MechanicalCrafterBlock.useItemOn`), and what its block
// entity draws (`MechanicalCrafterRenderer`). The rules are in crafter.rs.

use super::Game;
use super::create_crafter::{ConnectedInput, CrafterWorld, Grouped, InputWorld, MechanicalRecipe, Phase, Pointing};
use super::create_kinetics::{Dir, Pos};
use horizon_client::containers::{BlockEntity, ModBlockEntity};
use horizon_client::crafting::Ingredient;
use horizon_client::items::ItemStack;
use horizon_world::nbt::{JavaString, Tag};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

pub(super) const CRAFTER: &str = "create:mechanical_crafter";
const SLOT_COVER: &str = "create:crafter_slot_cover";

/// One crafter's block entity, as `MechanicalCrafterBlockEntity` keeps it.
#[derive(Clone, Debug)]
struct Crafter {
    phase: Phase,
    count_down: i32,
    covered: bool,
    grid: Grouped<Option<ItemStack>>,
    /// Not saved: true at first, as Create's (`wasPoweredBefore`).
    was_powered: bool,
    /// `lazyTick` every 20 ticks.
    lazy: i32,
    /// The slot was filled: `whenContentsChanged` checks the recipe.
    check: bool,
    input: ConnectedInput,
}

impl Default for Crafter {
    fn default() -> Self {
        Crafter {
            phase: Phase::Idle,
            count_down: 0,
            covered: false,
            grid: Grouped::default(),
            was_powered: true,
            lazy: 20,
            check: false,
            input: ConnectedInput::default(),
        }
    }
}

static CRAFTERS: Mutex<Option<HashMap<(String, Pos), Crafter>>> = Mutex::new(None);

fn crafters() -> MutexGuard<'static, Option<HashMap<(String, Pos), Crafter>>> {
    CRAFTERS.lock().unwrap_or_else(|e| e.into_inner())
}

/// A world opened or closed.
pub(super) fn reset() {
    *crafters() = None;
    *RECIPES.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

// ---- Its data -------------------------------------------------------------------

fn text(be: &ModBlockEntity, key: &str) -> String {
    match be.nbt.get(&JavaString::from(key)) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    }
}

fn int(be: &ModBlockEntity, key: &str) -> i32 {
    match be.nbt.get(&JavaString::from(key)) {
        Some(Tag::Int(n)) => *n,
        Some(Tag::Byte(n)) => i32::from(*n),
        _ => 0,
    }
}

/// `read`: the phase, countdown, cover and grid (each cell's place, count
/// and item; other components aren't kept).
fn load(be: &ModBlockEntity) -> Crafter {
    let mut grid = Grouped::default();
    for cell in text(be, "Grid").split(';').filter(|c| !c.is_empty()) {
        let mut parts = cell.splitn(4, ' ');
        let (Some(x), Some(y), Some(n), id) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let (Ok(x), Ok(y), Ok(n)) = (x.parse(), y.parse(), n.parse::<u32>()) else {
            continue;
        };
        let stack = id.filter(|i| !i.is_empty() && n > 0).map(|i| ItemStack::new(i, n));
        grid.grid.insert((x, y), stack);
    }
    Crafter {
        phase: Phase::parse(&text(be, "Phase")),
        count_down: int(be, "CountDown"),
        covered: int(be, "Cover") != 0,
        grid,
        input: ConnectedInput::load(&text(be, "Input")),
        ..Crafter::default()
    }
}

fn save(be: &mut ModBlockEntity, c: &Crafter) {
    let grid: String = c
        .grid
        .grid
        .iter()
        .map(|((x, y), s)| match s {
            Some(s) => format!("{x} {y} {} {};", s.count, s.item),
            None => format!("{x} {y} 0 ;"),
        })
        .collect();
    be.nbt.insert(JavaString::from("Phase"), Tag::String(JavaString::from(c.phase.name())));
    be.nbt.insert(JavaString::from("CountDown"), Tag::Int(c.count_down));
    be.nbt.insert(JavaString::from("Cover"), Tag::Byte(i8::from(c.covered)));
    be.nbt.insert(JavaString::from("Grid"), Tag::String(JavaString::from(grid.as_str())));
    be.nbt.insert(JavaString::from("Input"), Tag::String(JavaString::from(c.input.save().as_str())));
}

// ---- The world, as the chain sees it ------------------------------------------------

/// The crafter at `pos`: its facing and pointing.
pub(super) fn crafter_at(game: &Game<'_>, pos: Pos) -> Option<(Dir, Pointing)> {
    let s = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2]))?;
    if s.name != CRAFTER {
        return None;
    }
    let facing = s.properties.get("facing").and_then(|f| Dir::parse(f))?;
    let pointing = s.properties.get("pointing").and_then(|p| Pointing::parse(p)).unwrap_or(Pointing::Up);
    Some((facing, pointing))
}

struct World<'a, 'g>(&'a Game<'g>);

impl CrafterWorld for World<'_, '_> {
    fn crafter(&self, pos: Pos) -> Option<(Dir, Pointing)> {
        crafter_at(self.0, pos)
    }
}

fn speed_at(game: &Game<'_>, pos: Pos) -> f32 {
    super::create_glue::with_kinetics(game, |k, level| k.get(pos).map_or(0.0, |_| k.speed_of(level, pos)))
}

/// The crafter `pos`'s slot: the ticking one's (`own`, taken out of the
/// world while it ticks) or another's in the world.
fn slot(game: &Game<'_>, pos: Pos, own: (Pos, &ModBlockEntity)) -> Option<ItemStack> {
    if pos == own.0 {
        return own.1.items.first().cloned().flatten();
    }
    match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(BlockEntity::Mod(m)) => m.items.first().cloned().flatten(),
        _ => None,
    }
}

fn take_slot(game: &mut Game<'_>, pos: Pos, own: (Pos, &mut ModBlockEntity)) -> Option<ItemStack> {
    if pos == own.0 {
        return own.1.items.first_mut().and_then(Option::take);
    }
    game.native_with_mod_block_entity(pos, |be| be.items.first_mut().and_then(Option::take)).flatten()
}

/// Every crafter's state, the ticking one's in `own`: `f` on the state of
/// `pos`, made from its data when this world hasn't seen it.
fn with_crafter<R>(game: &Game<'_>, pos: Pos, f: impl FnOnce(&mut Crafter) -> R) -> R {
    let dim = game.dim.id().to_owned();
    let loaded = match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(BlockEntity::Mod(m)) => Some(load(m)),
        _ => None,
    };
    let mut guard = crafters();
    let map = guard.get_or_insert_with(HashMap::new);
    let c = map.entry((dim, pos)).or_insert_with(|| loaded.unwrap_or_default());
    f(c)
}

// ---- Recipes ----------------------------------------------------------------------

/// `create:mechanical_crafting` recipes, each cell's ingredient read.
type Mechanical = Vec<(MechanicalRecipe, HashMap<Vec<String>, Ingredient>)>;

static RECIPES: Mutex<Option<Arc<Mechanical>>> = Mutex::new(None);

fn mechanical_recipes(game: &Game<'_>) -> Arc<Mechanical> {
    let mut guard = RECIPES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = guard.as_ref() {
        return r.clone();
    }
    let list: Mechanical = game
        .recipes()
        .custom
        .get("create:mechanical_crafting")
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|r| super::create_crafter::parse_mechanical(&r.id, &r.json))
        .filter_map(|r| {
            let mut read = HashMap::new();
            for choices in r.cells.iter().flatten() {
                if read.contains_key(choices) {
                    continue;
                }
                let json = serde_json::Value::Array(choices.iter().cloned().map(serde_json::Value::String).collect());
                read.insert(choices.clone(), super::create_machines::read_ingredient(game, &json)?);
            }
            Some((r, read))
        })
        .collect();
    let list = Arc::new(list);
    *guard = Some(list.clone());
    list
}

/// `RecipeGridHandler.tryToApplyRecipe`: a crafting recipe for the grid
/// (`allowRegularCraftingInCrafter`, not `_manual_only` ones), else a
/// mechanical crafting one.
fn try_to_apply(game: &Game<'_>, grid: &Grouped<Option<ItemStack>>) -> Option<ItemStack> {
    let (w, h, cells) = grid.input(|s| s.is_none())?;
    let cells: Vec<Option<ItemStack>> = cells.into_iter().map(Option::flatten).collect();
    let size = w.max(h);
    let mut square = vec![None; size * size];
    for y in 0..h {
        for x in 0..w {
            square[y * size + x] = cells[y * w + x].clone();
        }
    }
    if let Some((name, result)) = game.recipes().find(&square, size)
        && !name.ends_with("_manual_only")
    {
        return Some(result);
    }
    for (r, read) in mechanical_recipes(game).iter() {
        let test = |choices: &[String], s: &ItemStack| read.get(choices).is_some_and(|i| i.test(s));
        if r.matches(w, h, &cells, test) {
            return Some(ItemStack::new(&r.result, r.count));
        }
    }
    None
}

// ---- MechanicalCrafterBlockEntity -----------------------------------------------------

fn sound(game: &mut Game<'_>, pos: Pos, name: &str, volume: f32, pitch: f32) {
    super::create_glue::play_create_sound(game, name, pos.map(|v| f64::from(v) + 0.5), volume, pitch);
}

/// `native_block_entity_tick` for a crafter: `tick` and `lazyTick`.
pub(super) fn tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    if entity.items.is_empty() {
        entity.items.push(None);
    }
    let speed = speed_at(game, pos);
    // Its saved state, the first time this world sees it.
    {
        let key = (game.dim.id().to_owned(), pos);
        let mut guard = crafters();
        let map = guard.get_or_insert_with(HashMap::new);
        // A new block entity (never saved) starts afresh, whatever stood here.
        if entity.nbt.get(&JavaString::from("Phase")).is_none() {
            map.insert(key, Crafter::default());
        } else {
            map.entry(key).or_insert_with(|| load(entity));
        }
    }
    let mut c = with_crafter(game, pos, |c| c.clone());
    let powered = super::create_glue::signal(game, pos);
    let mut start: Option<bool> = None;
    if std::mem::take(&mut c.check) && c.phase == Phase::Idle {
        start = Some(false);
    }
    c.lazy -= 1;
    let lazy = c.lazy <= 0;
    if lazy {
        c.lazy = 20;
    }
    let mut actions: Vec<Action> = Vec::new();
    if c.phase != Phase::Accepting {
        if c.was_powered != powered {
            c.was_powered = powered;
            if powered {
                start = Some(true);
            }
        }
        let cds = super::create_crafter::count_down_speed(speed);
        match c.phase {
            Phase::Assembling => {
                c.count_down -= cds;
                if c.count_down < 0 {
                    c.count_down = 0;
                    if super::create_crafter::targeting(&World(game), pos).is_some() {
                        c.phase = Phase::Exporting;
                        c.count_down = if c.grid.grid.values().all(Option::is_none) { 0 } else { 1000 };
                    } else if let Some(result) = try_to_apply(game, &c.grid) {
                        // The crafting remainders (buckets) beside the result.
                        let containers: Vec<ItemStack> = c
                            .grid
                            .grid
                            .values()
                            .flatten()
                            .filter_map(|s| game.items().get(&s.item).and_then(|i| i.crafting_remainder.clone()))
                            .map(|r| ItemStack::new(&r, 1))
                            .collect();
                        c.grid = Grouped::of(Some(result));
                        for (i, stack) in containers.into_iter().enumerate() {
                            let mut g = Grouped::default();
                            g.grid.insert((i as i32, 0), Some(stack));
                            g.merge_onto(&mut c.grid, Pointing::Left);
                        }
                        c.phase = Phase::Crafting;
                        c.count_down = 2000;
                    } else {
                        actions.push(Action::EjectChain);
                    }
                }
            }
            Phase::Exporting => {
                c.count_down -= cds;
                if c.count_down < 0 {
                    c.count_down = 0;
                    actions.push(Action::Export);
                }
            }
            Phase::Crafting => {
                let prev = c.count_down;
                c.count_down -= cds;
                if c.count_down < 1000 && prev >= 1000 {
                    sound(game, pos, "crafter_click", 1.0, 2.0);
                    sound(game, pos, "crafter_craft", 1.0, 1.0);
                }
                if c.count_down < 0 {
                    c.count_down = 0;
                    actions.push(Action::Insert);
                }
            }
            Phase::Inserting if target_takes_from_belt(game, pos) => actions.push(Action::Insert),
            _ => {}
        }
    }
    if lazy {
        if c.phase == Phase::Idle && entity.items[0].is_some() {
            start = Some(start.unwrap_or(false));
        }
        if c.phase == Phase::Inserting {
            actions.push(Action::Insert);
        }
    }
    with_crafter(game, pos, |s| *s = c);
    if let Some(powered_start) = start {
        check_completed_recipe(game, pos, entity, powered_start, speed);
    }
    for a in actions {
        match a {
            Action::EjectChain => eject_whole_grid(game, pos, entity),
            Action::Export => export(game, pos, entity),
            Action::Insert => try_insert(game, pos, entity),
        }
    }
    let c = with_crafter(game, pos, |c| c.clone());
    save(entity, &c);
    note_client(game, pos, entity, &c);
}

enum Action {
    EjectChain,
    Export,
    Insert,
}

/// `checkCompletedRecipe`: with the shaft turning, the whole chain begins
/// when every crafter has an item or a cover (a powered start: when any has
/// an item).
fn check_completed_recipe(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity, powered_start: bool, speed: f32) {
    if speed == 0.0 {
        return;
    }
    let chain = {
        let own = (pos, &*entity);
        super::create_crafter::chain_if(
            &World(game),
            pos,
            |p| {
                let item = slot(game, p, own).is_some();
                if powered_start { item } else { item || with_crafter(game, p, |c| c.covered) }
            },
            powered_start,
        )
    };
    let Some(chain) = chain else {
        return;
    };
    for p in chain {
        // begin
        let stack = take_slot(game, p, (pos, &mut *entity));
        let first = super::create_crafter::preceding(&World(game), p).is_empty();
        with_crafter(game, p, |c| {
            c.phase = Phase::Accepting;
            c.grid = Grouped::of(stack);
            if first {
                c.phase = Phase::Assembling;
                c.count_down = 1;
            }
        });
        if p != pos {
            let c = with_crafter(game, p, |c| c.clone());
            game.native_with_mod_block_entity(p, |be| save(be, &c));
        }
    }
}

/// The exporting crafter's grid onto the one it points at, which goes on
/// when everything before it waits (`continueIfAllPrecedingFinished`).
fn export(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let Some(target) = super::create_crafter::targeting(&World(game), pos) else {
        eject_whole_grid(game, pos, entity);
        return;
    };
    let pointing = crafter_at(game, pos).map_or(Pointing::Up, |c| c.1);
    let grid = with_crafter(game, pos, |c| {
        c.phase = Phase::Waiting;
        c.count_down = 0;
        std::mem::take(&mut c.grid)
    });
    let empty = grid.grid.values().all(Option::is_none);
    let size = with_crafter(game, target, |t| {
        grid.merge_onto(&mut t.grid, pointing);
        t.grid.grid.len()
    });
    if !empty {
        sound(game, pos, "crafter_click", 1.0, size as f32 / 16.0 + 0.5);
    }
    let preceding = super::create_crafter::preceding(&World(game), target);
    let all_waiting = preceding.iter().all(|p| with_crafter(game, *p, |c| c.phase == Phase::Waiting));
    if all_waiting {
        with_crafter(game, target, |t| {
            t.phase = Phase::Assembling;
            t.count_down = 1;
        });
    }
    let t = with_crafter(game, target, |c| c.clone());
    game.native_with_mod_block_entity(target, |be| save(be, &t));
}

/// Whether the crafter puts out onto something that takes items from belts.
fn target_takes_from_belt(game: &Game<'_>, pos: Pos) -> bool {
    let Some((facing, pointing)) = crafter_at(game, pos) else {
        return false;
    };
    let d = super::create_crafter::target_direction(facing, pointing);
    let n = d.normal();
    let at = [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]];
    super::create_logistics::has_belt_input(game, at) && super::create_logistics::can_insert_side(game, at, d)
}

/// `tryInsert`: the result into the inventory or belt it points at; with
/// nowhere to go the chain ejects its grid.
fn try_insert(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let Some((facing, pointing)) = crafter_at(game, pos) else {
        return;
    };
    let d = super::create_crafter::target_direction(facing, pointing);
    let n = d.normal();
    let at = [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]];
    let belt = target_takes_from_belt(game, pos);
    if !belt && !super::create_logistics::has_inventory_at(game, at) {
        eject_whole_grid(game, pos, entity);
        return;
    }
    let grid = with_crafter(game, pos, |c| c.grid.clone());
    let mut left = Grouped::default();
    for (key, stack) in grid.grid {
        let Some(stack) = stack else {
            continue;
        };
        let remainder = if belt {
            super::create_logistics::insert_side(game, at, d, stack)
        } else {
            super::create_logistics::inventory_insert(game, at, d.opposite(), stack)
        };
        if let Some(r) = remainder {
            left.grid.insert(key, Some(r));
        }
    }
    if left.grid.is_empty() {
        with_crafter(game, pos, |c| c.grid = Grouped::default());
        eject_whole_grid(game, pos, entity);
    } else {
        with_crafter(game, pos, |c| {
            c.grid = left;
            c.phase = Phase::Inserting;
        });
    }
}

/// `ejectWholeGrid`: every crafter of the chain drops what it holds before
/// its face and goes idle.
fn eject_whole_grid(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let chain = super::create_crafter::chain_if(&World(game), pos, |_| true, false);
    let Some(chain) = chain else {
        return;
    };
    for p in chain {
        let facing = crafter_at(game, p).map_or([0, 0, 0], |c| c.0.normal());
        let at = [
            f64::from(p[0]) + 0.5 + f64::from(facing[0]) * 0.75,
            f64::from(p[1]) + 0.5,
            f64::from(p[2]) + 0.5 + f64::from(facing[2]) * 0.75,
        ];
        let grid = with_crafter(game, p, |c| {
            c.phase = Phase::Idle;
            std::mem::take(&mut c.grid)
        });
        let mut drops: Vec<ItemStack> = grid.grid.into_values().flatten().collect();
        drops.extend(take_slot(game, p, (pos, &mut *entity)));
        for s in drops {
            game.spawn_item(s, at, [0.0, 0.0, 0.0], 10);
        }
        if p != pos {
            let c = with_crafter(game, p, |c| c.clone());
            game.native_with_mod_block_entity(p, |be| save(be, &c));
        }
    }
}

/// `ConnectedInput.getItemHandler`'s insertion (a funnel, chute or click on
/// the crafter): `ItemHandlerHelper.insertItem` over the group's
/// inventories in order, each taking one item when idle, uncovered and
/// empty (`Inventory.insertItem`). What is left, none when it took all.
pub(super) fn insert(game: &mut Game<'_>, pos: Pos, mut stack: ItemStack) -> Option<ItemStack> {
    let input = with_crafter(game, pos, |c| c.input.clone());
    let controller = if input.controller { pos } else { input.controller_of(pos) };
    let members = with_crafter(game, controller, |c| c.input.clone());
    // A member whose controller isn't one has no inventories.
    if !members.controller || crafter_at(game, controller).is_none() {
        return Some(stack);
    }
    let facing = crafter_at(game, controller).map_or(Dir::South, |c| c.0);
    for member in super::create_crafter::inventory_order(controller, facing, &members.data) {
        if stack.count == 0 {
            break;
        }
        if crafter_at(game, member).is_none() {
            continue;
        }
        let (phase, covered) = with_crafter(game, member, |c| (c.phase, c.covered));
        if phase != Phase::Idle || covered {
            continue;
        }
        let mut one = stack.clone();
        one.count = 1;
        let put = game
            .native_with_mod_block_entity(member, |be| {
                if be.items.is_empty() {
                    be.items.push(None);
                }
                if be.items[0].is_some() {
                    return false;
                }
                be.items[0] = Some(one);
                true
            })
            .unwrap_or(false);
        if put {
            stack.count -= 1;
            with_crafter(game, member, |c| c.check = true);
            let at = member.map(|v| f64::from(v) + 0.5);
            game.with_level(|_, level, _| {
                level.native_play_sound("minecraft:entity.item_frame.add_item", at, super::create_sounds::BLOCKS, 0.25, 0.5);
            });
        }
    }
    (stack.count > 0).then_some(stack)
}

/// The crafters' inputs, as `toggleConnection` changes them (each crafter's
/// data is written on its next tick).
struct Inputs<'a, 'g>(&'a Game<'g>);

impl InputWorld for Inputs<'_, '_> {
    fn input(&self, pos: Pos) -> Option<ConnectedInput> {
        crafter_at(self.0, pos)?;
        Some(with_crafter(self.0, pos, |c| c.input.clone()))
    }
    fn set_input(&mut self, pos: Pos, input: ConnectedInput) {
        with_crafter(self.0, pos, |c| c.input = input);
    }
}

/// `EdgeInteractionHandler.onBlockActivated` with the wrench (not
/// sneaking): a hit near an edge of a side face toward a crafter facing
/// the same way connects or splits their inputs. Not yet: the group drawn
/// as one casing (`CrafterCTBehaviour`), splitting it when a crafter of it
/// is broken.
fn edge_wrench(game: &mut Game<'_>, pos: Pos, face: Dir, facing: Dir) -> bool {
    let solid = |game: &Game<'_>, p: Pos, side: Dir| {
        let state = game.world.block(p[0], p[1], p[2]);
        game.res.facts().get(state).sturdy_full & (1 << side as u8) != 0
    };
    let out = face.normal();
    let ahead = [pos[0] + out[0], pos[1] + out[1], pos[2] + out[2]];
    if solid(game, ahead, face.opposite()) {
        return false;
    }
    let hit = super::create_logistics::click_point(game, pos, face);
    let hit = [0, 1, 2].map(|i| hit[i] + 0.5);
    for d in Dir::ALL {
        if d.axis() == face.axis() {
            continue;
        }
        let n = d.normal();
        let neighbour = [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]];
        let neighbour_ahead = [neighbour[0] + out[0], neighbour[1] + out[1], neighbour[2] + out[2]];
        if solid(game, neighbour_ahead, face.opposite()) {
            continue;
        }
        let other = crafter_at(game, neighbour).map(|c| c.0);
        if !super::create_crafter::should_connect(facing, face, d, other) {
            continue;
        }
        let b = super::create_crafter::edge_box(d);
        if (0..3).all(|i| hit[i] >= b[i] && hit[i] <= b[i + 3]) {
            super::create_crafter::toggle_connection(&mut Inputs(game), pos, neighbour);
            let at = pos.map(|v| f64::from(v) + 0.5);
            game.with_level(|_, level, _| {
                level.native_play_sound("minecraft:entity.item_frame.add_item", at, super::create_sounds::BLOCKS, 0.25, 0.1);
            });
            return true;
        }
    }
    false
}

/// `MechanicalCrafterBlock.useItemOn` on the front, and `onWrenched` there
/// (the wrench cycles `pointing`). True when it took the click.
pub(super) fn right_click(game: &mut Game<'_>, pos: Pos, face: [i32; 3]) -> bool {
    let Some((facing, pointing)) = crafter_at(game, pos) else {
        return false;
    };
    let held = game.held().cloned();
    let wrench = held.as_ref().is_some_and(|h| &*h.item == "create:wrench");
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    if wrench && !sneaking && edge_wrench(game, pos, Dir::nearest(face), facing) {
        return true;
    }
    if Dir::nearest(face) != facing {
        return false;
    }
    if wrench {
        if sneaking {
            return false;
        }
        let next = pointing.next();
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let dim = game.dim.id().to_owned();
        if let Some(st) = game.with_level(|_, level, _| level.native_state_with_property(state, "pointing", next.name()))
        {
            game.native_set_block(&dim, pos, st, super::native_hooks::flags::UPDATE_ALL);
        }
        return true;
    }
    if held.as_ref().is_some_and(|h| &*h.item == super::create_arms::ARM) {
        return false;
    }
    let (phase, covered) = with_crafter(game, pos, |c| (c.phase, c.covered));
    if phase != Phase::Idle {
        let mut be = take_entity(game, pos);
        if let Some(be) = be.as_mut() {
            eject_whole_grid(game, pos, be);
        }
        put_entity(game, pos, be);
        return true;
    }
    let creative = game.native_acting_player().map_or(game.mode_id(), |p| p.game_mode) == 1;
    let in_slot = match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(BlockEntity::Mod(m)) => m.items.first().cloned().flatten(),
        _ => None,
    };
    if let Some(h) = &held {
        if &*h.item == SLOT_COVER {
            if covered || in_slot.is_some() {
                return false;
            }
            with_crafter(game, pos, |c| c.covered = true);
            if !creative {
                shrink_hand(game);
            }
            return true;
        }
        let left = insert(game, pos, h.clone());
        if left.as_ref().map(|l| l.count) != Some(h.count) {
            *game.held_mut() = left;
        }
        return true;
    }
    let Some(stack) = in_slot else {
        if covered {
            with_crafter(game, pos, |c| c.covered = false);
            if !creative {
                game.give_or_drop(ItemStack::new(SLOT_COVER, 1));
            }
            return true;
        }
        return false;
    };
    game.native_with_mod_block_entity(pos, |be| {
        if let Some(s) = be.items.first_mut() {
            *s = None;
        }
    });
    game.give_or_drop(stack);
    true
}

fn shrink_hand(game: &mut Game<'_>) {
    let hand = game.held_mut();
    if let Some(h) = hand.as_mut() {
        h.count -= 1;
        if h.count == 0 {
            *hand = None;
        }
    }
}

/// The crafter's block entity, out of the world while a click works on it.
fn take_entity(game: &mut Game<'_>, pos: Pos) -> Option<Box<ModBlockEntity>> {
    match game.world.block_entities.remove(&(pos[0], pos[1], pos[2])) {
        Some(BlockEntity::Mod(m)) => Some(m),
        Some(other) => {
            game.world.block_entities.insert((pos[0], pos[1], pos[2]), other);
            None
        }
        None => None,
    }
}

fn put_entity(game: &mut Game<'_>, pos: Pos, be: Option<Box<ModBlockEntity>>) {
    if let Some(mut be) = be {
        let c = with_crafter(game, pos, |c| c.clone());
        save(&mut be, &c);
        game.world.block_entities.insert((pos[0], pos[1], pos[2]), BlockEntity::Mod(be));
        game.world.modified.insert((pos[0] >> 4, pos[2] >> 4));
    }
}

// ---- Clients ------------------------------------------------------------------------

/// What clients draw (over `create:depots`): the phase and cover, then the
/// slot's item (idle) or each grid cell as `x,y,id`.
fn note_client(game: &Game<'_>, pos: Pos, entity: &ModBlockEntity, c: &Crafter) {
    let mut view = vec![(String::new(), Phase::ALL.iter().position(|p| *p == c.phase).unwrap_or(0) as u32 + 8 * u32::from(c.covered))];
    if c.phase == Phase::Idle {
        if let Some(s) = entity.items.first().cloned().flatten() {
            view.push((format!("0,0,{}", s.item), 1));
        }
    } else {
        for ((x, y), s) in &c.grid.grid {
            if let Some(s) = s {
                view.push((format!("{x},{y},{}", s.item), 1));
            }
        }
    }
    super::create_machines::note_view(game, pos, view);
}

/// `MechanicalCrafterRenderer` through `native_block_entity_submit`: the
/// items before its face, the lid (covered or working), and the belt to the
/// crafter it passes to or the arrow. Its cog turns with the kinetics.
/// Not yet: the items moving while they pass on and spinning as they're
/// crafted, the belt's scrolling texture, the connected casing.
pub(super) fn submit(
    game: &Game<'_>,
    pos: Pos,
    entity: &ModBlockEntity,
    draw: &mut super::native_block_entity_draw::BlockEntityDraw,
) {
    if entity.id != CRAFTER {
        return;
    }
    let Some((facing, pointing)) = crafter_at(game, pos) else {
        return;
    };
    // The stone template keeps no light of its own.
    draw.light = horizon_client::block_entity_render::vertex_light(super::create_logistics::neighbour_light(game, pos));
    let view = super::create_machines::client_slots(pos).unwrap_or_default();
    let code = view.first().map_or(0, |v| v.1);
    let phase = Phase::ALL.get((code % 8) as usize).copied().unwrap_or(Phase::Idle);
    let covered = code >= 8;
    let y_rot = match facing {
        Dir::South => 0.0,
        Dir::West => -90.0,
        Dir::North => 180.0,
        Dir::East => -270.0,
        _ => 0.0,
    };
    // renderAndTransform: about Y by yRot + 90, then about east by xRot.
    let part = |draw: &mut super::native_block_entity_draw::BlockEntityDraw, name: &str| {
        draw.push();
        draw.rotate_about([0.0, 1.0, 0.0], y_rot + 90.0, [0.5; 3]);
        draw.rotate_about([1.0, 0.0, 0.0], pointing.x_rotation(), [0.5; 3]);
        game.native_draw_block(draw, name, &[]);
        draw.pop();
    };
    if (covered || phase != Phase::Idle) && phase != Phase::Crafting && phase != Phase::Inserting {
        part(draw, "create:hz_part_crafter_lid");
    }
    let d = super::create_crafter::target_direction(facing, pointing);
    let n = d.normal();
    let target = crafter_at(game, [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]]);
    if super::create_crafter::is_valid_target((facing, pointing), target) {
        part(draw, "create:hz_part_crafter_belt");
        part(draw, "create:hz_part_crafter_belt_frame");
    } else {
        part(draw, "create:hz_part_crafter_arrow");
    }
    // renderItems: before the face, half size, turned with the crafter.
    let f = facing.normal();
    draw.push();
    draw.translate([0.5 + f[0] as f32 * 0.58, 0.5, 0.5 + f[2] as f32 * 0.58]);
    draw.scale([0.5; 3]);
    draw.rotate_about([0.0, 1.0, 0.0], y_rot, [0.0; 3]);
    let only_first = phase == Phase::Inserting || phase == Phase::Crafting;
    for (key, _) in view.iter().skip(1) {
        let mut parts = key.splitn(3, ',');
        let (Some(x), Some(y), Some(id)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let (Ok(x), Ok(y)) = (x.parse::<i32>(), y.parse::<i32>()) else {
            continue;
        };
        if only_first && (x != 0 || y != 0) {
            continue;
        }
        let spacing = if phase == Phase::Idle { 0.0 } else { 0.5 };
        draw.push();
        draw.translate([x as f32 * spacing, y as f32 * spacing, 0.0]);
        draw.rotate_about([0.0, 1.0, 0.0], 180.0, [0.0; 3]);
        draw.translate([0.0, 0.0, (x + y * 3) as f32 / 1024.0 - 1.0 / 256.0]);
        game.native_draw_item(draw, &ItemStack::new(id, 1), horizon_assets::item_model::DisplayContext::Fixed, None, 0);
        draw.pop();
    }
    draw.pop();
}
