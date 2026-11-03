// Horizon's side of Create's item vault, mechanical arm and deployer,
// injected as `crate::game::create_arms`: the vault and the arm as mod block
// entities (the vault's 20 slots per block as Horizon container slots), the
// one item handler every arm point, funnel and chute goes through
// (`ItemHandler`, standing in for NeoForge's `Capabilities.ItemHandler`),
// the arm item's point selection, the deployer (no block entity: its state
// is kept here and saved beside the world) and what it does in front of it in
// place of `DeployerFakePlayer`, deploying recipes on belts and depots, and
// the arm's and deployer's moving parts for clients (`create:arms`).
// The rules are in vault.rs, arm.rs and deployer.rs.

use super::Game;
use super::create_arm::{
    Arm, ArmStack, ArmWorld, Mode as PointMode, Phase, Point, PointType, in_range,
};
use super::create_belt::{ProcessingResult, Stack as _};
use super::create_deployer::{
    Deployer, DeployerWorld, DeployingRecipe, HandPose, HeldStep, HeldUse, Mode, State,
    hand_offset, hand_pose, held_use, on_item_received, parse_deploying, placement_along_first,
    timer_speed, when_item_held,
};
use super::create_kinetics::{Axis, DEPLOYER, DEPLOYER_ALONG, Dir, Kind, Pos};
use super::create_vault::{VAULT_CAPACITY, VaultLevel, VaultPart, members};
use horizon_client::containers::{ModBlockEntity, ModContainer};
use horizon_client::crafting::Ingredient;
use horizon_client::items::ItemStack;
use horizon_world::nbt::{Compound, JavaString, Tag};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

pub(super) const VAULT: &str = "create:item_vault";
pub(super) const ARM: &str = "create:mechanical_arm";
/// The arm's and deployer's moving parts for clients.
pub(super) const ARMS: &str = "create:arms";
const WRENCH: &str = "create:wrench";
/// Ticks between full updates, and between saves of the deployers.
const SNAPSHOT_EVERY: u64 = 100;
const SAVE_EVERY: u64 = 1200;

/// A placement heard in `native_place_block` (which can't change the
/// world), finished at the level's tick: the block, the player's look and
/// sneaking, and an arm's selected points.
struct Placed {
    dim: String,
    pos: Pos,
    yaw: f32,
    pitch: f32,
    sneaking: bool,
    points: Vec<Point>,
}

#[derive(Default)]
struct Arms {
    /// `ArmInteractionPointHandler.currentSelection`, by player.
    selection: HashMap<u128, Vec<Point>>,
    /// The selecting players' names, for the window to find its own
    /// selection (its client knows its name, not its server UUID).
    selection_names: HashMap<u128, String>,
    arms: HashMap<(String, Pos), Arm<ItemStack>>,
    deployers: HashMap<(String, Pos), Deployer<ItemStack>>,
    /// The fake player's `blockBreakingProgress`, by deployer.
    breaking: HashMap<(String, Pos), (Pos, f32)>,
    placed: Vec<Placed>,
    /// Dimensions whose deployers were read from their save.
    loaded: HashSet<String>,
    root: Option<PathBuf>,
    ticks: u64,
    /// Arm and deployer updates for the level's tick end, by dimension.
    pending: Vec<(String, Pos, Option<Vec<u8>>)>,
    /// What clients were last told of each.
    shown: HashMap<(String, Pos), Vec<u8>>,
    recipes: Option<Arc<Recipes>>,
    random: u64,
}

type Recipes = Vec<(DeployingRecipe, Option<Ingredient>, Option<Ingredient>)>;

static STATE: Mutex<Option<Arms>> = Mutex::new(None);

fn state() -> MutexGuard<'static, Option<Arms>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn with_state<R>(f: impl FnOnce(&mut Arms) -> R) -> R {
    let mut guard = state();
    f(guard.get_or_insert_with(Arms::default))
}

impl ArmStack for ItemStack {
    fn count(&self) -> u32 {
        self.count
    }
    fn same_item(&self, other: &Self) -> bool {
        self.item == other.item
    }
}

fn block_name(game: &Game<'_>, pos: Pos) -> Option<String> {
    let w = &game.world;
    if !w.loaded(pos[0], pos[2]) || pos[1] < w.shape.min_y || pos[1] >= w.shape.max_y() {
        return None;
    }
    game.pack
        .assets
        .registry
        .state(w.block(pos[0], pos[1], pos[2]))
        .map(|s| s.name.clone())
}

fn prop(game: &Game<'_>, pos: Pos, key: &str) -> Option<String> {
    let state = game
        .pack
        .assets
        .registry
        .state(game.world.block(pos[0], pos[1], pos[2]))?;
    state.properties.get(key).cloned()
}

fn add(a: Pos, b: Pos) -> Pos {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn max_stack(game: &Game<'_>, stack: &ItemStack) -> u32 {
    game.items()
        .get(&stack.item)
        .map_or(64, |i| i.max_stack)
        .max(1)
}

fn with_count(stack: &ItemStack, count: u32) -> ItemStack {
    let mut s = stack.clone();
    s.count = count;
    s
}

/// Blocks of ours this module answers for.
const ROPE_PULLEY: &str = "create:rope_pulley";
const GANTRY_CARRIAGE: &str = "create:gantry_carriage";

pub(super) fn is_ours(name: &str) -> bool {
    name == VAULT || name == ARM || name == DEPLOYER || name == DEPLOYER_ALONG
}

fn deployer_at(game: &Game<'_>, pos: Pos) -> Option<(Dir, bool)> {
    let name = block_name(game, pos)?;
    let along = match name.as_str() {
        DEPLOYER => false,
        DEPLOYER_ALONG => true,
        _ => return None,
    };
    Some((Dir::parse(&prop(game, pos, "facing")?)?, along))
}

fn speed_at(game: &Game<'_>, pos: Pos) -> f32 {
    super::create_glue::with_kinetics(game, |k, level| {
        k.get(pos).map_or(0.0, |_| k.speed_of(level, pos))
    })
}

// ---- Block entities -------------------------------------------------------

/// `native_block_entity` for the vault and the arm; true when it answered.
pub(super) fn block_entity(block: &str, entity: &mut Option<(String, Compound)>) -> bool {
    if block != VAULT && block != ARM {
        return false;
    }
    if entity.is_none() {
        let mut nbt = Compound::new();
        if block == VAULT {
            // ItemVaultBlock.onPlace: updateConnectivity, however the block
            // was placed (a player, a command).
            put(&mut nbt, "Uninitialized", Tag::Byte(1));
        }
        *entity = Some((block.to_owned(), nbt));
    }
    true
}

pub(super) fn block_entity_ticks(block: &str) -> bool {
    block == VAULT || block == ARM
}

/// `native_block_entity_container`: the vault's own 20 slots (hoppers reach
/// one block's; funnels, chutes and arms the whole vault's), and the arm's
/// held item, which nothing outside reaches.
pub(super) fn block_entity_container(block: &str) -> Option<ModContainer> {
    match block {
        VAULT => {
            let all: Vec<usize> = (0..VAULT_CAPACITY).collect();
            Some(ModContainer {
                slots: VAULT_CAPACITY,
                faces: std::array::from_fn(|_| all.clone()),
                inputs: all.clone(),
                outputs: all,
                menu: None,
            })
        }
        ARM => Some(ModContainer {
            slots: 1,
            faces: std::array::from_fn(|_| Vec::new()),
            inputs: Vec::new(),
            outputs: Vec::new(),
            menu: None,
        }),
        _ => None,
    }
}

/// `native_block_entity_tick`; true when it was one of ours.
pub(super) fn block_entity_tick(
    game: &mut Game<'_>,
    pos: Pos,
    entity: &mut ModBlockEntity,
) -> bool {
    match entity.id.as_str() {
        VAULT => vault_tick(game, pos, entity),
        ARM => arm_tick(game, pos, entity),
        _ => return false,
    }
    true
}

fn get_int(nbt: &Compound, key: &str) -> Option<i32> {
    match nbt.get(&JavaString::from(key)) {
        Some(Tag::Int(v)) => Some(*v),
        _ => None,
    }
}

fn get_float(nbt: &Compound, key: &str) -> f32 {
    match nbt.get(&JavaString::from(key)) {
        Some(Tag::Float(v)) => f32::from_bits(*v),
        _ => 0.0,
    }
}

fn get_string(nbt: &Compound, key: &str) -> Option<String> {
    match nbt.get(&JavaString::from(key)) {
        Some(Tag::String(s)) => Some(s.to_string_lossy()),
        _ => None,
    }
}

fn put(nbt: &mut Compound, key: &str, tag: Tag) {
    nbt.insert(JavaString::from(key), tag);
}

// ---- The item vault -------------------------------------------------------

fn vault_axis(game: &Game<'_>, pos: Pos) -> Axis {
    match prop(game, pos, "axis").as_deref() {
        Some("z") => Axis::Z,
        _ => Axis::X,
    }
}

/// A vault block's part, from its block entity (`ItemVaultBlockEntity.read`).
fn read_part(axis: Axis, nbt: &Compound) -> VaultPart {
    let mut part = VaultPart::new(axis);
    if let Some(Tag::IntArray(c)) = nbt.get(&JavaString::from("Controller"))
        && c.len() == 3
    {
        part.controller = Some([c[0], c[1], c[2]]);
    }
    part.width = get_int(nbt, "Size").unwrap_or(1).max(1);
    part.length = get_int(nbt, "Length").unwrap_or(1).max(1);
    part.update_connectivity =
        matches!(nbt.get(&JavaString::from("Uninitialized")), Some(Tag::Byte(b)) if *b != 0);
    part
}

fn write_part(part: &VaultPart, nbt: &mut Compound) {
    match part.controller {
        Some(c) => put(nbt, "Controller", Tag::IntArray(c.to_vec())),
        None => {
            nbt.remove(&JavaString::from("Controller"));
        }
    }
    put(nbt, "Size", Tag::Int(part.width));
    put(nbt, "Length", Tag::Int(part.length));
    put(
        nbt,
        "Uninitialized",
        Tag::Byte(i8::from(part.update_connectivity)),
    );
}

/// The vaults as one vault block's tick sees them: its own block entity
/// is out of the world while it ticks.
struct Vaults<'a, 'g> {
    game: &'a mut Game<'g>,
    here: Pos,
    entity: &'a mut ModBlockEntity,
}

impl VaultLevel for Vaults<'_, '_> {
    fn part(&self, pos: Pos) -> Option<VaultPart> {
        if pos == self.here {
            return Some(read_part(vault_axis(self.game, pos), &self.entity.nbt));
        }
        if block_name(self.game, pos).as_deref() != Some(VAULT) {
            return None;
        }
        let key = (pos[0], pos[1], pos[2]);
        match self.game.world.block_entities.get(&key) {
            Some(horizon_client::containers::BlockEntity::Mod(m)) if m.id == VAULT => {
                Some(read_part(vault_axis(self.game, pos), &m.nbt))
            }
            _ => None,
        }
    }
    fn set_part(&mut self, pos: Pos, part: VaultPart) {
        if pos == self.here {
            write_part(&part, &mut self.entity.nbt);
            return;
        }
        self.game
            .native_with_mod_block_entity(pos, |be| write_part(&part, &mut be.nbt));
    }
}

fn vault_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    if entity.items.len() < VAULT_CAPACITY {
        entity.items.resize(VAULT_CAPACITY, None);
    }
    let mut level = Vaults {
        game,
        here: pos,
        entity,
    };
    super::create_vault::tick(&mut level, pos);
    // What clients draw of it: its axis, controller and width (the
    // connected textures, `ItemVaultCTBehaviour`).
    let part = read_part(vault_axis(game, pos), &entity.nbt);
    let mut view = vec![3u8, u8::from(part.axis == Axis::X)];
    for v in part.controller_of(pos) {
        view.extend_from_slice(&v.to_le_bytes());
    }
    view.push(part.width.clamp(0, 255) as u8);
    let dim = game.dim.id().to_owned();
    with_state(|s| note(s, &dim, pos, view));
}

/// `ItemVaultBlock.getAnalogOutputSignal`: the whole vault's slots
/// (`ItemHelper.calcRedstoneFromBlockEntity` of its combined inventory).
pub(super) fn vault_analog(level: &super::GameLevel<'_>, pos: Pos, max_stack: &dyn Fn(&str) -> u32) -> i32 {
    let axis_of = |p: Pos| {
        let facing = level
            .registry
            .state(level.world.block(p[0], p[1], p[2]))
            .and_then(|s| s.properties.get("axis").cloned());
        match facing.as_deref() {
            Some("z") => Axis::Z,
            _ => Axis::X,
        }
    };
    let part_at = |p: Pos| match level.world.block_entities.get(&(p[0], p[1], p[2])) {
        Some(horizon_client::containers::BlockEntity::Mod(m)) if m.id == VAULT => Some((read_part(axis_of(p), &m.nbt), m)),
        _ => None,
    };
    let Some((part, _)) = part_at(pos) else {
        return 0;
    };
    let controller = part.controller_of(pos);
    let members = match part_at(controller) {
        Some((c, _)) => members(controller, &c),
        None => vec![pos],
    };
    let mut slots = Vec::new();
    for p in members {
        if let Some((_, m)) = part_at(p) {
            for i in 0..VAULT_CAPACITY {
                slots.push(m.items.get(i).and_then(Option::as_ref));
            }
        }
    }
    super::create_glue::redstone_from_inventory(slots.into_iter(), 64, max_stack)
}

/// The vault blocks whose slots make the vault at `pos`, in slot order.
fn vault_members(game: &Game<'_>, pos: Pos) -> Vec<Pos> {
    let part_at = |p: Pos| -> Option<VaultPart> {
        match game.world.block_entities.get(&(p[0], p[1], p[2])) {
            Some(horizon_client::containers::BlockEntity::Mod(m)) if m.id == VAULT => {
                Some(read_part(vault_axis(game, p), &m.nbt))
            }
            _ => None,
        }
    };
    let Some(part) = part_at(pos) else {
        return vec![pos];
    };
    let controller = part.controller_of(pos);
    match part_at(controller) {
        Some(c) => members(controller, &c),
        None => vec![pos],
    }
}

// ---- Inventories ------------------------------------------------------------

/// A block's `IItemHandler` (`Capabilities.ItemHandler.BLOCK`): the one
/// place the arm, funnels and chutes reach another block's items through,
/// to move onto Horizon's `block_capability` hook once there is one
/// (docs/PLAN.md).
pub(super) trait ItemHandler {
    fn slots(&self, game: &Game<'_>) -> usize;
    /// `insertItem(slot, stack, simulate)`: what is left.
    fn insert(
        &self,
        game: &mut Game<'_>,
        slot: usize,
        stack: ItemStack,
        simulate: bool,
    ) -> Option<ItemStack>;
    /// `extractItem(slot, amount, simulate)`.
    fn extract(
        &self,
        game: &mut Game<'_>,
        slot: usize,
        amount: u32,
        simulate: bool,
    ) -> Option<ItemStack>;
}

/// The inventories the port has.
#[derive(Clone, Debug)]
pub(super) enum Inv {
    /// A vault's blocks, 20 slots each (`SameSizeCombinedInvWrapper`).
    Vault(Vec<Pos>),
    /// `DepotItemHandler`.
    Depot(Pos),
    /// `BasinInventory` (inputs) and the output inventory.
    Basin(Pos),
    /// `ChuteItemHandler`.
    Chute(Pos),
    /// Another mod block entity's container (the millstone's
    /// `MillstoneInventoryHandler`).
    Mod(Pos, ModContainer),
    /// `DeployerItemHandler`.
    Deployer(Pos),
    /// `ItemHandlerBeltSegment`.
    Belt(Pos),
    /// A chest or barrel (`InvWrapper`).
    Chest(Pos),
    /// A portable storage interface's `InterfaceItemHandler`: the storage of
    /// the structure it is connected to.
    Psi(Pos),
    /// A crafter's `ConnectedInput` handler: its group's slots in order.
    Crafter(Vec<Pos>),
    /// `BlazeBurnerPoint.insert`: fuel, one at a time.
    Burner(Pos),
    /// A crushing wheel controller's `ProcessingInventory`.
    Crusher(Pos),
    /// The arm's `CampfirePoint`, `ComposterPoint` and `RespawnAnchorPoint`.
    Campfire(Pos),
    Composter(Pos),
    Anchor(Pos),
}

/// A vanilla block's state property.
fn vanilla_prop(game: &Game<'_>, pos: Pos, key: &str) -> Option<String> {
    let id = game.world.block(pos[0], pos[1], pos[2]);
    game.pack.assets.registry.state(id)?.properties.get(key).cloned()
}

/// `CampfirePoint.insert`: one cookable item into a free slot.
fn campfire_insert(game: &mut Game<'_>, pos: Pos, stack: ItemStack, simulate: bool) -> Option<ItemStack> {
    use horizon_client::containers::BlockEntity as B;
    let Some(time) = game.recipes().cook("minecraft:campfire_cooking", &stack).map(|r| r.time as i32) else {
        return Some(stack);
    };
    let key = (pos[0], pos[1], pos[2]);
    let Some(B::Campfire(c)) = game.world.block_entities.get_mut(&key) else {
        return Some(stack);
    };
    let mut rest = stack.clone();
    if simulate {
        if !c.items.iter().any(Option::is_none) {
            return Some(stack);
        }
        rest.count -= 1;
    } else if !c.place_food(&mut rest, time) {
        return Some(stack);
    } else {
        game.mark_block_entity_updated(key);
    }
    (rest.count > 0).then_some(rest)
}

/// `ComposterPoint.insert` (the composter's container from above): one
/// compostable while it isn't full.
fn composter_insert(game: &mut Game<'_>, pos: Pos, stack: ItemStack, simulate: bool) -> Option<ItemStack> {
    let level: i32 = vanilla_prop(game, pos, "level").and_then(|l| l.parse().ok()).unwrap_or(8);
    let compostable = game.items().get(&stack.item).is_some_and(|i| i.compostable.is_some());
    if level >= 7 || !compostable {
        return Some(stack);
    }
    if simulate {
        return (stack.count > 1).then(|| with_count(&stack, stack.count - 1));
    }
    let one = with_count(&stack, 1);
    let left = game.native_insert_item(pos, horizon_core::geometry::Direction::Up, one);
    let used = u32::from(left.is_none());
    (stack.count > used).then(|| with_count(&stack, stack.count - used))
}

/// `ComposterPoint.extract` (from below): its bone meal when full.
fn composter_extract(game: &mut Game<'_>, pos: Pos, simulate: bool) -> Option<ItemStack> {
    let level: i32 = vanilla_prop(game, pos, "level").and_then(|l| l.parse().ok()).unwrap_or(0);
    if level != 8 {
        return None;
    }
    if !simulate {
        let at = horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]);
        game.with_level(|b, level, ticks| b.composter_empty(level, ticks, at));
    }
    Some(ItemStack::new("minecraft:bone_meal", 1))
}

/// `RespawnAnchorPoint.insert`: glowstone charges it while it isn't full.
fn anchor_insert(game: &mut Game<'_>, pos: Pos, stack: ItemStack, simulate: bool) -> Option<ItemStack> {
    let charge: i32 = vanilla_prop(game, pos, "charges").and_then(|l| l.parse().ok()).unwrap_or(4);
    if &*stack.item != "minecraft:glowstone" || charge >= 4 {
        return Some(stack);
    }
    if !simulate {
        let id = game.world.block(pos[0], pos[1], pos[2]);
        if let Some(next) = game.with_props(id, &[("charges", &(charge + 1).to_string())]) {
            let dim = game.dim.id().to_owned();
            game.native_set_block(&dim, pos, next, super::native_hooks::flags::UPDATE_ALL);
            let c = pos.map(|v| f64::from(v) + 0.5);
            game.with_level(|_, level, _| {
                level.native_play_sound("minecraft:block.respawn_anchor.charge", c, 4, 1.0, 1.0)
            });
        }
    }
    (stack.count > 1).then(|| with_count(&stack, stack.count - 1))
}

/// `Capabilities.ItemHandler.BLOCK` at `pos`.
pub(super) fn handler_at(game: &Game<'_>, pos: Pos) -> Option<Inv> {
    use horizon_client::containers::BlockEntity as B;
    if deployer_at(game, pos).is_some() {
        return Some(Inv::Deployer(pos));
    }
    if block_name(game, pos).as_deref() == Some(super::create_actors::PSI) {
        return Some(Inv::Psi(pos));
    }
    if super::create_logistics::is_belt(block_name(game, pos)?.as_str()) {
        return Some(Inv::Belt(pos));
    }
    if block_name(game, pos).as_deref() == Some(super::create_crafter_glue::CRAFTER) {
        return Some(Inv::Crafter(super::create_crafter_glue::group(game, pos)));
    }
    match game.world.block_entities.get(&(pos[0], pos[1], pos[2]))? {
        B::Mod(m) => Some(match m.id.as_str() {
            VAULT => Inv::Vault(vault_members(game, pos)),
            super::create_machines::DEPOT => Inv::Depot(pos),
            super::create_machines::BASIN => Inv::Basin(pos),
            super::create_logistics::CHUTE => Inv::Chute(pos),
            ARM => return None,
            id => {
                let mut c = None;
                super::create_machines::block_entity_container(id, &mut c);
                Inv::Mod(pos, c?)
            }
        }),
        B::Chest(_) => Some(Inv::Chest(pos)),
        _ => None,
    }
}

fn mod_items(game: &Game<'_>, pos: Pos) -> Option<Vec<Option<ItemStack>>> {
    match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(horizon_client::containers::BlockEntity::Mod(m)) => Some(m.items.clone()),
        _ => None,
    }
}

fn set_mod_items(game: &mut Game<'_>, pos: Pos, items: Vec<Option<ItemStack>>) {
    game.native_with_mod_block_entity(pos, |be| be.items = items);
}

/// `ItemStackHandler.insertItem` into one slot holding up to `limit`.
fn slot_insert(slot: &mut Option<ItemStack>, stack: ItemStack, limit: u32) -> Option<ItemStack> {
    let mut limit = limit.min(64);
    if let Some(existing) = slot.as_ref() {
        if !existing.same_item_same_components(&stack) {
            return Some(stack);
        }
        limit = limit.saturating_sub(existing.count);
    }
    if limit == 0 {
        return Some(stack);
    }
    let moved = stack.count.min(limit);
    match slot {
        Some(existing) => existing.count += moved,
        None => *slot = Some(with_count(&stack, moved)),
    }
    (stack.count > moved).then(|| with_count(&stack, stack.count - moved))
}

/// `ItemStackHandler.extractItem` from one slot.
fn slot_extract(slot: &mut Option<ItemStack>, amount: u32, max: u32) -> Option<ItemStack> {
    let existing = slot.as_mut()?;
    let n = amount.min(existing.count).min(max);
    if n == 0 {
        return None;
    }
    let out = with_count(existing, n);
    existing.count -= n;
    if existing.count == 0 {
        *slot = None;
    }
    Some(out)
}

fn chest_items(game: &Game<'_>, pos: Pos) -> Option<Vec<Option<ItemStack>>> {
    match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(horizon_client::containers::BlockEntity::Chest(items)) => Some(items.clone()),
        _ => None,
    }
}

fn set_chest_items(game: &mut Game<'_>, pos: Pos, new: Vec<Option<ItemStack>>) {
    if let Some(horizon_client::containers::BlockEntity::Chest(items)) =
        game.world.block_entities.get_mut(&(pos[0], pos[1], pos[2]))
    {
        *items = new;
        game.world.modified.insert((pos[0] >> 4, pos[2] >> 4));
    }
}

/// Runs `f` on a copy of the slots at `pos` (a mod block entity's or a
/// chest's) and keeps the change unless simulating.
fn on_slots<R>(
    game: &mut Game<'_>,
    pos: Pos,
    chest: bool,
    simulate: bool,
    f: impl FnOnce(&mut Vec<Option<ItemStack>>) -> R,
) -> Option<R> {
    let mut items = if chest {
        chest_items(game, pos)?
    } else {
        mod_items(game, pos)?
    };
    let before = items.clone();
    let r = f(&mut items);
    if !simulate && items != before {
        if chest {
            set_chest_items(game, pos, items);
        } else {
            set_mod_items(game, pos, items);
        }
    }
    Some(r)
}

impl ItemHandler for Inv {
    fn slots(&self, game: &Game<'_>) -> usize {
        match self {
            Inv::Vault(blocks) => blocks.len() * VAULT_CAPACITY,
            Inv::Depot(_) => 9,
            Inv::Basin(_) => 2 * super::create_processing::BASIN_SLOTS,
            Inv::Chute(_) | Inv::Belt(_) => 1,
            Inv::Mod(_, c) => c.slots,
            Inv::Deployer(pos) => {
                let dim = game.dim.id().to_owned();
                1 + with_state(|s| {
                    s.deployers
                        .get(&(dim, *pos))
                        .map_or(0, |d| d.overflow.len())
                })
            }
            Inv::Chest(pos) => chest_items(game, *pos).map_or(0, |i| i.len()),
            Inv::Psi(pos) => super::create_actor_glue::psi_slots(game, *pos),
            Inv::Crafter(members) => members.len(),
            Inv::Burner(_) | Inv::Crusher(_) => 1,
            Inv::Campfire(_) | Inv::Composter(_) | Inv::Anchor(_) => 1,
        }
    }

    fn insert(
        &self,
        game: &mut Game<'_>,
        slot: usize,
        stack: ItemStack,
        simulate: bool,
    ) -> Option<ItemStack> {
        let max = max_stack(game, &stack);
        match self {
            Inv::Vault(blocks) => {
                let Some(&block) = blocks.get(slot / VAULT_CAPACITY) else {
                    return Some(stack);
                };
                let i = slot % VAULT_CAPACITY;
                on_slots(game, block, false, simulate, |items| {
                    if items.len() < VAULT_CAPACITY {
                        items.resize(VAULT_CAPACITY, None);
                    }
                    slot_insert(&mut items[i], stack.clone(), max)
                })
                .unwrap_or(Some(stack))
            }
            Inv::Depot(pos) => {
                // Only into an empty depot (`canMergeItems` is false).
                if slot != 0 {
                    return Some(stack);
                }
                on_slots(game, *pos, false, simulate, |items| {
                    if items.iter().any(Option::is_some) {
                        return Some(stack.clone());
                    }
                    if items.is_empty() {
                        items.push(None);
                    }
                    let held = stack.count.min(max);
                    items[0] = Some(with_count(&stack, held));
                    (stack.count > held).then(|| with_count(&stack, stack.count - held))
                })
                .unwrap_or(Some(stack))
            }
            Inv::Basin(pos) => {
                let inputs = super::create_processing::BASIN_SLOTS;
                if slot >= inputs {
                    return Some(stack);
                }
                on_slots(game, *pos, false, simulate, |items| {
                    if items.len() < 2 * inputs {
                        items.resize(2 * inputs, None);
                    }
                    // BasinInventory: one slot per item, the first free one.
                    let mut first_free = None;
                    for i in 0..inputs {
                        if i != slot
                            && items[i]
                                .as_ref()
                                .is_some_and(|s| s.same_item_same_components(&stack))
                        {
                            return Some(stack.clone());
                        }
                        if items[i].is_none() && first_free.is_none() {
                            first_free = Some(i);
                        }
                    }
                    if items[slot].is_none() && first_free != Some(slot) {
                        return Some(stack.clone());
                    }
                    slot_insert(&mut items[slot], stack.clone(), max)
                })
                .unwrap_or(Some(stack))
            }
            Inv::Chute(pos) => {
                // ChuteItemHandler: the whole stack into an empty chute.
                let empty =
                    mod_items(game, *pos).is_some_and(|i| i.first().is_none_or(Option::is_none));
                if !empty {
                    return Some(stack);
                }
                if !simulate {
                    game.native_with_mod_block_entity(*pos, |be| {
                        super::create_logistics::set_chute_item(be, Some(stack), 1.0);
                    });
                }
                None
            }
            Inv::Mod(pos, c) => {
                if !c.inputs.contains(&slot) {
                    return Some(stack);
                }
                let n = c.slots;
                on_slots(game, *pos, false, simulate, |items| {
                    if items.len() < n {
                        items.resize(n, None);
                    }
                    slot_insert(&mut items[slot], stack.clone(), max)
                })
                .unwrap_or(Some(stack))
            }
            Inv::Deployer(pos) => {
                let dim = game.dim.id().to_owned();
                with_state(|s| {
                    let Some(d) = s.deployers.get_mut(&(dim, *pos)) else {
                        return Some(stack);
                    };
                    if slot < d.overflow.len() {
                        return Some(stack);
                    }
                    // DeployerItemHandler.isItemValid: the filter's item.
                    if d.filter.as_deref().is_some_and(|f| f != &*stack.item) {
                        return Some(stack);
                    }
                    match &mut d.held {
                        None => {
                            let n = stack.count.min(max);
                            if !simulate {
                                d.held = Some(with_count(&stack, n));
                                d.dirty = true;
                            }
                            (stack.count > n).then(|| with_count(&stack, stack.count - n))
                        }
                        Some(held) => {
                            if !held.same_item_same_components(&stack) {
                                return Some(stack);
                            }
                            let space = max.saturating_sub(held.count);
                            if space == 0 {
                                return Some(stack);
                            }
                            let n = space.min(stack.count);
                            if !simulate {
                                held.count += n;
                                d.dirty = true;
                            }
                            (stack.count > n).then(|| with_count(&stack, stack.count - n))
                        }
                    }
                })
            }
            Inv::Belt(pos) => {
                let angle = with_state(|s| {
                    s.random = s
                        .random
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    (s.random >> 33) as i32 % 360
                });
                super::create_logistics::with_belt_segment(game, *pos, |inv, belt, index| {
                    inv.insert_at(belt, index, stack.clone(), angle, simulate)
                })
                .unwrap_or(Some(stack))
            }
            Inv::Chest(pos) => on_slots(game, *pos, true, simulate, |items| {
                match items.get_mut(slot) {
                    Some(s) => slot_insert(s, stack.clone(), max),
                    None => Some(stack.clone()),
                }
            })
            .unwrap_or(Some(stack)),
            Inv::Psi(pos) => super::create_actor_glue::psi_items(game, *pos, simulate, |items| {
                match items.get_mut(slot) {
                    Some(s) => slot_insert(s, stack.clone(), max),
                    None => Some(stack.clone()),
                }
            })
            .unwrap_or(Some(stack)),
            Inv::Crafter(members) => match members.get(slot) {
                Some(m) => super::create_crafter_glue::slot_insert(game, *m, stack, simulate),
                None => Some(stack),
            },
            Inv::Crusher(pos) => super::create_crusher_glue::insert(game, *pos, stack, simulate),
            // BlazeBurnerPoint.insert: one taken, the rest given back (its
            // container dropped); a single item gives back its container.
            Inv::Burner(pos) => {
                let Some(container) = super::create_machines::burner_insert(game, *pos, &stack, simulate) else {
                    return Some(stack);
                };
                if stack.count == 1 {
                    return container;
                }
                if !simulate && let Some(c) = container {
                    game.spawn_item(c, pos.map(|v| f64::from(v) + 0.5), [0.0; 3], 10);
                }
                Some(with_count(&stack, stack.count - 1))
            }
            Inv::Campfire(pos) => campfire_insert(game, *pos, stack, simulate),
            Inv::Composter(pos) => composter_insert(game, *pos, stack, simulate),
            Inv::Anchor(pos) => anchor_insert(game, *pos, stack, simulate),
        }
    }

    fn extract(
        &self,
        game: &mut Game<'_>,
        slot: usize,
        amount: u32,
        simulate: bool,
    ) -> Option<ItemStack> {
        if amount == 0 {
            return None;
        }
        match self {
            Inv::Vault(blocks) => {
                let block = *blocks.get(slot / VAULT_CAPACITY)?;
                let i = slot % VAULT_CAPACITY;
                on_slots(game, block, false, simulate, |items| {
                    let max = items.get(i)?.as_ref().map_or(64, |s| s.max_stack());
                    slot_extract(items.get_mut(i)?, amount, max)
                })
                .flatten()
            }
            // DepotItemHandler: the held item (even while it is processed)
            // or the output buffer.
            Inv::Depot(pos) => on_slots(game, *pos, false, simulate, |items| {
                slot_extract(items.get_mut(slot)?, amount, u32::MAX)
            })
            .flatten(),
            Inv::Basin(pos) | Inv::Chute(pos) => on_slots(game, *pos, false, simulate, |items| {
                slot_extract(items.get_mut(slot)?, amount, 64)
            })
            .flatten(),
            Inv::Mod(pos, c) => {
                if !c.outputs.contains(&slot) {
                    return None;
                }
                on_slots(game, *pos, false, simulate, |items| {
                    slot_extract(items.get_mut(slot)?, amount, 64)
                })
                .flatten()
            }
            Inv::Deployer(pos) => {
                let dim = game.dim.id().to_owned();
                with_state(|s| {
                    let d = s.deployers.get_mut(&(dim, *pos))?;
                    if slot < d.overflow.len() {
                        let item = &mut d.overflow[slot];
                        let n = amount.min(item.count);
                        let out = with_count(item, n);
                        if !simulate {
                            item.count -= n;
                            if item.count == 0 {
                                d.overflow.remove(slot);
                            }
                            d.dirty = true;
                        }
                        return (n > 0).then_some(out);
                    }
                    // A held item the filter takes isn't taken out.
                    if d.filter.is_some() && d.filter.as_deref() == d.held.as_ref().map(|h| &*h.item) {
                        return None;
                    }
                    let held = d.held.as_mut()?;
                    let n = amount.min(held.count);
                    let out = with_count(held, n);
                    if !simulate {
                        held.count -= n;
                        if held.count == 0 {
                            d.held = None;
                        }
                        d.dirty = true;
                    }
                    Some(out)
                })
            }
            Inv::Belt(pos) => {
                super::create_logistics::with_belt_segment(game, *pos, |inv, _, index| {
                    inv.extract_at(index, amount, simulate)
                })
                .flatten()
            }
            Inv::Chest(pos) => on_slots(game, *pos, true, simulate, |items| {
                slot_extract(items.get_mut(slot)?, amount, 64)
            })
            .flatten(),
            Inv::Psi(pos) => super::create_actor_glue::psi_items(game, *pos, simulate, |items| {
                slot_extract(items.get_mut(slot)?, amount, 64)
            })
            .flatten(),
            Inv::Composter(pos) => composter_extract(game, *pos, simulate).filter(|_| amount > 0),
            Inv::Crafter(members) => super::create_crafter_glue::slot_extract(game, *members.get(slot)?, amount, simulate),
            Inv::Campfire(_) | Inv::Anchor(_) | Inv::Burner(_) | Inv::Crusher(_) => None,
        }
    }
}

/// `ItemHandlerHelper.insertItem`: slot by slot.
pub(super) fn insert_item(
    game: &mut Game<'_>,
    inv: &Inv,
    stack: ItemStack,
    simulate: bool,
) -> Option<ItemStack> {
    let mut stack = stack;
    for i in 0..inv.slots(game) {
        stack = inv.insert(game, i, stack, simulate)?;
    }
    Some(stack)
}

/// `ItemHandlerHelper.insertItemStacked`: onto matching stacks first (for
/// funnels' `InvManipulationBehaviour`).
fn insert_item_stacked(
    game: &mut Game<'_>,
    inv: &Inv,
    stack: ItemStack,
    simulate: bool,
) -> Option<ItemStack> {
    // Every slot takes only its own item or nothing: inserting slot by slot
    // with the matching ones first gives `insertItemStacked`'s order.
    let n = inv.slots(game);
    let mut stack = stack;
    let mut empty = Vec::new();
    for i in 0..n {
        let probe = inv.extract(game, i, 1, true);
        match probe {
            Some(p) if p.same_item_same_components(&with_count(&stack, p.count)) => {
                stack = inv.insert(game, i, stack, simulate)?;
            }
            Some(_) => {}
            None => empty.push(i),
        }
    }
    for i in empty {
        stack = inv.insert(game, i, stack, simulate)?;
    }
    Some(stack)
}

/// For logistics: a funnel's or chute's insertion into a vault (all of it)
/// or a deployer; none for other blocks.
pub(super) fn insert_into(
    game: &mut Game<'_>,
    pos: Pos,
    stack: &ItemStack,
) -> Option<Option<ItemStack>> {
    let inv = handler_at(game, pos)?;
    if !matches!(inv, Inv::Vault(_) | Inv::Deployer(_) | Inv::Psi(_)) {
        return None;
    }
    Some(insert_item_stacked(game, &inv, stack.clone(), false))
}

/// For logistics: `ItemHelper.extract` (`UPTO`) of up to `max` of one item
/// passing `test` from a vault or deployer, gathered from every slot
/// holding it, up to a stack; none for other blocks.
pub(super) fn extract_from(
    game: &mut Game<'_>,
    pos: Pos,
    max: u32,
    test: &dyn Fn(&ItemStack) -> bool,
) -> Option<Option<ItemStack>> {
    let inv = handler_at(game, pos)?;
    if !matches!(inv, Inv::Vault(_) | Inv::Deployer(_) | Inv::Psi(_)) {
        return None;
    }
    let mut out: Option<ItemStack> = None;
    let mut cap = max;
    for i in 0..inv.slots(game) {
        let want = cap - out.as_ref().map_or(0, |o| o.count);
        let Some(probe) = inv.extract(game, i, want, true) else {
            continue;
        };
        let same = match &out {
            None => test(&probe),
            Some(o) => {
                let mut p = probe.clone();
                p.count = o.count;
                p == *o
            }
        };
        if !same {
            continue;
        }
        let Some(taken) = inv.extract(game, i, want, false) else {
            continue;
        };
        match out.as_mut() {
            None => {
                cap = max.min(super::create_belt::Stack::max_stack(&taken)).max(1);
                out = Some(taken);
            }
            Some(o) => o.count += taken.count,
        }
        if out.as_ref().is_some_and(|o| o.count >= cap) {
            break;
        }
    }
    Some(out)
}

/// Whether a funnel or chute finds an inventory here that isn't a block
/// entity's (a deployer).
pub(super) fn has_inventory(game: &Game<'_>, pos: Pos) -> bool {
    deployer_at(game, pos).is_some() || block_name(game, pos).as_deref() == Some(super::create_actors::PSI)
}

// ---- The mechanical arm -----------------------------------------------------

/// `ArmInteractionPointType.getPrimaryType` at `pos`.
fn point_type_at(game: &Game<'_>, pos: Pos) -> Option<PointType> {
    if let Some((facing, _)) = deployer_at(game, pos) {
        return Some(PointType::Deployer(facing));
    }
    let name = block_name(game, pos)?;
    match name.as_str() {
        super::create_machines::BASIN => Some(PointType::Basin),
        super::create_machines::DEPOT => Some(PointType::Depot),
        name if super::create_logistics::is_chute_block(name) => Some(PointType::Chute),
        "create:millstone" => Some(PointType::Millstone),
        super::create_machines::BLAZE => Some(PointType::BlazeBurner),
        super::create_crusher_glue::CONTROLLER => Some(PointType::CrushingWheels),
        // SawType: facing up and turning.
        n if super::create_machines::is_saw(n) => {
            (prop(game, pos, "facing").as_deref() == Some("up") && speed_at(game, pos) != 0.0).then_some(PointType::Saw)
        }
        super::create_crafter_glue::CRAFTER => {
            Some(PointType::Crafter(super::create_crafter_glue::crafter_at(game, pos)?.0))
        }
        "minecraft:campfire" | "minecraft:soul_campfire" => Some(PointType::Campfire),
        "minecraft:composter" => Some(PointType::Composter),
        "minecraft:respawn_anchor" => Some(PointType::RespawnAnchor),
        // FunnelType: a funnel that isn't extracting.
        super::create_funnel::COLLECTING_FUNNEL | super::create_funnel::BRASS_COLLECTING_FUNNEL => {
            Some(PointType::Funnel(Dir::parse(&prop(game, pos, "facing")?)?))
        }
        n if super::create_logistics::is_belt(n) => {
            let state = game
                .pack
                .assets
                .registry
                .state(game.world.block(pos[0], pos[1], pos[2]))?;
            let belt = super::create_belt::BeltState::from_block(n, |k| {
                state.properties.get(k).map(String::as_str)
            })?;
            belt.can_transport_objects().then_some(PointType::Belt)
        }
        _ => None,
    }
}

/// The world as an arm's tick sees it.
struct ArmAt<'a, 'g> {
    game: &'a mut Game<'g>,
}

impl ArmAt<'_, '_> {
    /// The handler a point reaches (`getHandler`): a funnel's is the
    /// inventory behind it.
    fn handler(&self, point: &Point) -> Option<Inv> {
        match point.kind {
            PointType::Funnel(f) => None.or_else(|| {
                let behind = add(point.pos, f.opposite().normal());
                handler_at(self.game, behind)
            }),
            PointType::Campfire => Some(Inv::Campfire(point.pos)),
            PointType::Composter => Some(Inv::Composter(point.pos)),
            PointType::RespawnAnchor => Some(Inv::Anchor(point.pos)),
            PointType::BlazeBurner => Some(Inv::Burner(point.pos)),
            PointType::CrushingWheels => Some(Inv::Crusher(point.pos)),
            _ => handler_at(self.game, point.pos),
        }
    }
}

impl ArmWorld<ItemStack> for ArmAt<'_, '_> {
    fn is_valid(&self, point: &Point) -> bool {
        point_type_at(self.game, point.pos).is_some_and(|k| k.name() == point.kind.name())
    }
    fn slot_count(&mut self, point: &Point) -> usize {
        if point.kind.deposit_only() {
            return 0;
        }
        self.handler(point).map_or(0, |h| h.slots(self.game))
    }
    fn extract(
        &mut self,
        point: &Point,
        slot: usize,
        amount: u32,
        simulate: bool,
    ) -> Option<ItemStack> {
        if point.kind.deposit_only() {
            return None;
        }
        let h = self.handler(point)?;
        h.extract(self.game, slot, amount, simulate)
    }
    fn insert(&mut self, point: &Point, stack: ItemStack, simulate: bool) -> Option<ItemStack> {
        let Some(h) = self.handler(point) else {
            return Some(stack);
        };
        if let PointType::Funnel(_) = point.kind {
            // FunnelPoint: the funnel's filter, then its inserter
            // (`insertItemStacked`).
            if !super::create_logistics::funnel_accepts(self.game, point.pos, &stack) {
                return Some(stack);
            }
            return insert_item_stacked(self.game, &h, stack, simulate);
        }
        insert_item(self.game, &h, stack, simulate)
    }
    fn keep_alive(&mut self, point: &Point) {
        if point.kind != PointType::Belt {
            return;
        }
        // BeltPoint.keepAlive: the first item on the segment waits.
        super::create_logistics::with_belt_segment(self.game, point.pos, |inv, _, index| {
            let lo = index as f32;
            if let Some(t) = inv
                .items
                .iter_mut()
                .find(|t| t.belt_position >= lo && t.belt_position < lo + 1.0)
            {
                t.locked_externally = true;
            }
        });
    }
}

fn read_points(nbt: &Compound, arm: Pos, game: &Game<'_>) -> Vec<Point> {
    get_string(nbt, "InteractionPoints")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| Point::deserialize(l, arm, |p| point_type_at(game, p)))
        .collect()
}

/// The arm's selection mode value box scrolled one step (`ScrollOptionBehaviour`):
/// its new mode's name, for the action bar.
pub(super) fn step_arm_mode(game: &Game<'_>, pos: Pos, up: bool) -> Option<&'static str> {
    let dim = game.dim.id().to_owned();
    with_state(|s| {
        let arm = s.arms.get_mut(&(dim, pos))?;
        arm.selection_mode = arm.selection_mode.step(up);
        arm.dirty = true;
        Some(arm.selection_mode.label())
    })
}

fn write_arm(arm: &Arm<ItemStack>, pos: Pos, nbt: &mut Compound) {
    let points: Vec<String> = arm
        .inputs
        .iter()
        .chain(&arm.outputs)
        .map(|p| p.serialize(pos))
        .collect();
    put(
        nbt,
        "InteractionPoints",
        Tag::String(JavaString::from(points.join("\n").as_str())),
    );
    put(
        nbt,
        "Phase",
        Tag::String(JavaString::from(arm.phase.name())),
    );
    put(nbt, "TargetPointIndex", Tag::Int(arm.chased_point_index));
    put(
        nbt,
        "MovementProgress",
        Tag::Float(arm.chased_point_progress.to_bits()),
    );
    put(nbt, "Powered", Tag::Byte(i8::from(arm.redstone_locked)));
    put(nbt, "LastInput", Tag::Int(arm.last_input_index));
    put(nbt, "LastOutput", Tag::Int(arm.last_output_index));
    put(nbt, "ScrollValue", Tag::Int(arm.selection_mode.code()));
    put(nbt, "Goggles", Tag::Byte(i8::from(arm.goggles)));
}

fn read_arm(nbt: &Compound, pos: Pos, game: &Game<'_>) -> Arm<ItemStack> {
    let mut arm = Arm::new();
    arm.set_points(read_points(nbt, pos, game));
    if let Some(p) = get_string(nbt, "Phase") {
        arm.phase = Phase::parse(&p);
    }
    arm.chased_point_index = get_int(nbt, "TargetPointIndex").unwrap_or(0);
    arm.chased_point_progress = get_float(nbt, "MovementProgress");
    arm.redstone_locked =
        matches!(nbt.get(&JavaString::from("Powered")), Some(Tag::Byte(b)) if *b != 0);
    arm.last_input_index = get_int(nbt, "LastInput").unwrap_or(-1);
    arm.last_output_index = get_int(nbt, "LastOutput").unwrap_or(-1);
    arm.selection_mode = super::create_arm::SelectionMode::from_code(get_int(nbt, "ScrollValue").unwrap_or(0));
    arm.goggles = matches!(nbt.get(&JavaString::from("Goggles")), Some(Tag::Byte(b)) if *b != 0);
    arm.dirty = false;
    arm
}

fn arm_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    if entity.items.is_empty() {
        entity.items.push(None);
    }
    let dim = game.dim.id().to_owned();
    let key = (dim.clone(), pos);
    let known = with_state(|s| s.arms.remove(&key));
    // initInteractionPoints: once the arm's whole range is loaded.
    let reach = super::create_arm::RANGE + 1;
    let loaded = (((pos[0] - reach) >> 4)..=((pos[0] + reach) >> 4)).all(|cx| {
        (((pos[2] - reach) >> 4)..=((pos[2] + reach) >> 4))
            .all(|cz| game.world.loaded(cx << 4, cz << 4))
    });
    if known.is_none() && !loaded {
        return;
    }
    let mut arm = known.unwrap_or_else(|| read_arm(&entity.nbt, pos, game));
    arm.held = entity.items[0].clone();
    let speed = speed_at(game, pos);
    // ArmBlock.neighborChanged: a signal locks it.
    let powered = super::create_glue::signal(game, pos);
    // ArmBlock.CEILING: an arm hung from a ceiling reaches down.
    let ceiling = arm_on_ceiling(game, pos);
    arm.redstone_update(powered, pos, ceiling, &mut ArmAt { game });
    arm.tick(speed, pos, ceiling, &mut ArmAt { game });
    // collectItem: the pick-up sound.
    if arm.picked_up {
        let pitch = 0.5 + horizon_core::random::RandomSource::next_float(&mut game.world.random) * 0.25;
        let at = pos.map(|v| f64::from(v) + 0.5);
        game.with_level(|_, level, _| {
            level.native_play_sound("minecraft:entity.item.pickup", at, super::create_sounds::BLOCKS, 0.125, pitch);
        });
    }
    arm.picked_up = false;
    entity.items[0] = arm.held.clone();
    if arm.dirty {
        arm.dirty = false;
        write_arm(&arm, pos, &mut entity.nbt);
    }
    let view = encode_arm(&arm);
    with_state(|s| {
        note(s, &dim, pos, view);
        s.arms.insert(key, arm);
    });
}

fn note(s: &mut Arms, dim: &str, pos: Pos, view: Vec<u8>) {
    let key = (dim.to_owned(), pos);
    if s.shown.get(&key) != Some(&view) {
        s.shown.insert(key, view.clone());
        s.pending.push((dim.to_owned(), pos, Some(view)));
    }
}

/// `ArmBlock.CEILING` of the arm at `pos`.
fn arm_on_ceiling(game: &Game<'_>, pos: Pos) -> bool {
    prop(game, pos, "ceiling").as_deref() == Some("true")
}

/// `ArmInteractionPoint.Mode.getColor`.
fn mode_color(mode: super::create_arm::Mode) -> u32 {
    match mode {
        super::create_arm::Mode::Deposit => 0xDDC166,
        super::create_arm::Mode::Take => 0x7FCDE0,
    }
}

/// `ArmInteractionPointHandler.tick` and `checkForWrench`, on the window's
/// side: with the arm item in hand its selected points are outlined in
/// their mode's colour, and with the wrench the points of the arm looked at.
pub(super) fn client_tick(game: &mut Game<'_>) {
    if !game.native_window() {
        return;
    }
    let held = game.held().map(|s| s.item.to_string());
    let dim = game.dim.id().to_owned();
    let points: Vec<Point> = match held.as_deref() {
        Some(ARM) => {
            let me = game.options.player_name();
            with_state(|s| {
                s.selection_names
                    .iter()
                    .find(|(_, n)| **n == me)
                    .and_then(|(u, _)| s.selection.get(u))
                    .cloned()
                    .unwrap_or_default()
            })
        }
        Some(super::create_glue::WRENCH) => {
            let Some(hit) = game.pick() else {
                return;
            };
            with_state(|s| {
                s.arms
                    .get(&(dim.clone(), hit.pos))
                    .map(|a| a.inputs.iter().chain(a.outputs.iter()).cloned().collect())
                    .unwrap_or_default()
            })
        }
        _ => return,
    };
    for p in points {
        let Some(b) = game.block_collision_shape(p.pos).bounds() else {
            continue;
        };
        let (lo, hi) = (b.min_position(), b.max_position());
        let [x, y, z] = p.pos.map(f64::from);
        let bounds = [lo.x + x, lo.y + y, lo.z + z, hi.x + x, hi.y + y, hi.z + z];
        let params = super::create_outline::Params {
            rgb: mode_color(p.mode),
            width: 1.0 / 16.0,
        };
        super::create_glue::show_outline(&format!("arm point {:?}", p.pos), bounds, params);
    }
}

/// `ArmInteractionPointHandler.leftClickingBlocksDeselectsThem`: with the
/// arm item in hand, a left click on a selected point takes it out of the
/// selection instead of hitting the block. True when it took the click.
pub(super) fn left_click(game: &mut Game<'_>, pos: Pos) -> bool {
    if game.held().is_none_or(|s| &*s.item != ARM) {
        return false;
    }
    let Some(player) = game.native_acting_player() else {
        return false;
    };
    with_state(|s| {
        let Some(list) = s.selection.get_mut(&player.uuid) else {
            return false;
        };
        let before = list.len();
        list.retain(|p| p.pos != pos);
        list.len() != before
    })
}

/// `native_use_item_on` with the arm item (`ArmItem.useOn` and
/// `ArmInteractionPointHandler.rightClickingBlocksSelectsThem`): a click on
/// a point selects it or cycles its mode; true when it took the click.
pub(super) fn use_item_on(game: &mut Game<'_>, stack: &ItemStack, pos: Pos) -> bool {
    if &*stack.item != ARM {
        return false;
    }
    let Some(kind) = point_type_at(game, pos) else {
        return false;
    };
    let Some(player) = game.native_acting_player() else {
        return true;
    };
    if player.game_mode == 3 {
        return false;
    }
    let mode = with_state(|s| {
        s.selection_names.insert(player.uuid, player.name.clone());
        let list = s.selection.entry(player.uuid).or_default();
        let i = match list.iter().position(|p| p.pos == pos) {
            Some(i) => i,
            None => {
                list.push(Point::new(kind, pos));
                list.len() - 1
            }
        };
        list[i].cycle_mode();
        list[i].mode
    });
    // The block's own name (a belt's or funnel's parts by their base block).
    let name = block_name(game, pos).unwrap_or_default();
    let base = if super::create_logistics::is_belt(&name) {
        "create:belt"
    } else {
        super::create_logistics::item_of(&name)
    };
    let (ns, path) = base.split_once(':').unwrap_or(("minecraft", base));
    let shown = horizon_client::text::Text::translate(&format!("block.{ns}.{path}"), &[]);
    let line = match mode {
        PointMode::Take => "Extract from ",
        PointMode::Deposit => "Deposit to ",
    };
    game.native_action_bar(
        player.uuid,
        horizon_client::text::Text::literal(line).append(shown),
    );
    true
}

// ---- Placement ---------------------------------------------------------------

/// `native_place_block`: an arm takes its player's selection
/// (`ArmPlacementPacket`, `flushSettings`); vaults and deployers get their
/// state at the level's tick.
pub(super) fn placed(game: &Game<'_>, pos: Pos) {
    let Some(name) = block_name(game, pos) else {
        return;
    };
    if !is_ours(&name)
        && super::create_contraption::piston_of(&name).is_none()
        && super::create_contraption::chassis_of(&name).is_none()
        && name != ROPE_PULLEY
        && name != super::create_contraption::gantry_shaft_block(super::create_contraption::GantryPart::Single)
        && name != GANTRY_CARRIAGE
        && name != super::create_actors::HARVESTER
        && name != super::create_actors::PLOUGH
        && name != super::create_actors::SAW
        && name != super::create_actors::SAW_ALONG
        && name != super::create_actors::PSI
    {
        return;
    }
    let player = game.native_acting_player();
    let points = if name == ARM {
        player
            .as_ref()
            .and_then(|p| with_state(|s| s.selection.remove(&p.uuid)))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    let entry = Placed {
        dim: game.dim.id().to_owned(),
        pos,
        yaw: game.yaw,
        pitch: game.pitch,
        sneaking,
        points,
    };
    with_state(|s| s.placed.push(entry));
}

/// `Direction.orderedByNearest(entity)[0]`: the way a look points most.
fn nearest_looking(yaw: f32, pitch: f32) -> Dir {
    let (y, p) = (yaw.to_radians(), pitch.to_radians());
    let v = [-y.sin() * p.cos(), -p.sin(), y.cos() * p.cos()];
    let (ax, ay, az) = (v[0].abs(), v[1].abs(), v[2].abs());
    if ax > ay && ax > az {
        if v[0] > 0.0 { Dir::East } else { Dir::West }
    } else if ay > az {
        if v[1] > 0.0 { Dir::Up } else { Dir::Down }
    } else if v[2] > 0.0 {
        Dir::South
    } else {
        Dir::North
    }
}

/// `Direction.fromYRot`.
fn horizontal_facing(yaw: f32) -> Dir {
    match (((yaw / 90.0 + 0.5).floor() as i32) & 3) as u8 {
        0 => Dir::South,
        1 => Dir::West,
        2 => Dir::North,
        _ => Dir::East,
    }
}

fn set_block(game: &mut Game<'_>, pos: Pos, name: &str, props: &[(&str, &str)]) -> bool {
    let Some(base) = game.pack.assets.registry.default_state(name) else {
        return false;
    };
    let Some(id) = game.with_props(base, props) else {
        return false;
    };
    let dimension = game.dim.id().to_owned();
    game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL)
}

fn finish_placed(game: &mut Game<'_>, p: Placed) {
    let pos = p.pos;
    let Some(name) = block_name(game, pos) else {
        return;
    };
    match name.as_str() {
        ARM => {
            let total = p.points.len();
            let points: Vec<Point> = p
                .points
                .into_iter()
                .filter(|q| in_range(q.pos, pos) && point_type_at(game, q.pos).is_some())
                .collect();
            let removed = total - points.len();
            if let Some(player) = game.native_acting_player() {
                let line = if removed > 0 {
                    Some(format!(
                        "{removed} selected targets were outside the arm's range"
                    ))
                } else {
                    let inputs = points.iter().filter(|q| q.mode == PointMode::Take).count();
                    let outputs = points.len() - inputs;
                    (inputs + outputs > 0)
                        .then(|| format!("Arm: {inputs} input(s), {outputs} output(s)"))
                };
                if let Some(line) = line {
                    game.native_action_bar(player.uuid, horizon_client::text::Text::literal(line));
                }
            }
            let mut arm: Arm<ItemStack> = Arm::new();
            arm.set_points(points);
            game.native_with_mod_block_entity(pos, |be| write_arm(&arm, pos, &mut be.nbt));
            let dim = game.dim.id().to_owned();
            with_state(|s| s.arms.remove(&(dim, pos)));
        }
        VAULT => {
            // onPlace: updateConnectivity.
            game.native_with_mod_block_entity(pos, |be| {
                put(&mut be.nbt, "Uninitialized", Tag::Byte(1))
            });
        }
        n if n == DEPLOYER
            || n == DEPLOYER_ALONG
            || n == super::create_actors::SAW
            || n == super::create_actors::SAW_ALONG
            || super::create_contraption::piston_of(n).is_some() =>
        {
            // DirectionalAxisKineticBlock.getStateForPlacement (deployers,
            // saws and mechanical pistons).
            let mut facing = nearest_looking(p.yaw, p.pitch).opposite();
            if p.sneaking {
                facing = facing.opposite();
            }
            let shaft_towards = |side: Dir| {
                let n = add(pos, side.normal());
                let Some(nname) = block_name(game, n) else {
                    return false;
                };
                let state = game
                    .pack
                    .assets
                    .registry
                    .state(game.world.block(n[0], n[1], n[2]));
                let kind = state.and_then(|s| {
                    Kind::from_block(&nname, |k| s.properties.get(k).map(String::as_str))
                });
                kind.is_some_and(|k| k.has_shaft_towards(side.opposite()))
            };
            let along = placement_along_first(
                facing,
                horizontal_facing(p.yaw).axis() == Axis::X,
                shaft_towards,
            );
            if let Some((sticky, ..)) = super::create_contraption::piston_of(n) {
                let block = super::create_contraption::piston_block(
                    sticky,
                    along,
                    super::create_contraption::PistonState::Retracted,
                );
                set_block(game, pos, block, &[("facing", facing.name())]);
                return;
            }
            if n == super::create_actors::SAW || n == super::create_actors::SAW_ALONG {
                let block = if along {
                    super::create_actors::SAW_ALONG
                } else {
                    super::create_actors::SAW
                };
                set_block(game, pos, block, &[("facing", facing.name())]);
                return;
            }
            let block = if along { DEPLOYER_ALONG } else { DEPLOYER };
            set_block(game, pos, block, &[("facing", facing.name())]);
            let dim = game.dim.id().to_owned();
            with_state(|s| {
                s.deployers.entry((dim, pos)).or_default();
            });
        }
        n if super::create_contraption::gantry_shaft_of(n).is_some() => {
            super::create_contraptions::placed_gantry_shaft(game, pos, p.sneaking, nearest_looking(p.yaw, p.pitch));
        }
        GANTRY_CARRIAGE => {
            // DirectionalAxisKineticBlock with the clicked face (the
            // template's) and cycleAxisIfNecessary: its pinion's axis across
            // the shaft behind it.
            let Some(facing) = game
                .pack
                .assets
                .registry
                .state(game.world.block(pos[0], pos[1], pos[2]))
                .and_then(|s| s.properties.get("facing").and_then(|f| Dir::parse(f)))
            else {
                return;
            };
            let behind = add(pos, facing.opposite().normal());
            let shaft_axis = game
                .pack
                .assets
                .registry
                .state(game.world.block(behind[0], behind[1], behind[2]))
                .filter(|s| super::create_contraption::gantry_shaft_of(&s.name).is_some())
                .and_then(|s| s.properties.get("facing").and_then(|f| Dir::parse(f)))
                .map(|d| d.axis());
            let along = shaft_axis.is_some_and(|shaft| {
                let rotation = super::create_deployer::rotation_axis(facing, false);
                // getValidGantryShaftAxis: neither the pinion's nor the facing's.
                let valid = Axis::ALL.into_iter().find(|a| *a != rotation && *a != facing.axis());
                valid != Some(shaft)
            });
            if along {
                set_block(game, pos, super::create_kinetics::GANTRY_CARRIAGE_ALONG, &[("facing", facing.name())]);
            }
            super::create_contraptions::queue_carriage(game, pos);
        }
        _ => super::create_contraptions::placed_chassis(game, pos, p.sneaking, nearest_looking(p.yaw, p.pitch)),
    }
}

/// `DeployerBlockEntity.addToGoggleTooltip`: its mode and the item it
/// holds, above the kinetic lines.
pub(super) fn deployer_goggle_lines(game: &Game<'_>, pos: Pos) -> Option<Vec<(String, u32)>> {
    let guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    let (ClientView::Deployer { punch, held, held_count, .. }, _) = guard.as_ref()?.get(&pos)? else {
        return None;
    };
    let mut lines = vec![
        ("Deployer Information".to_owned(), 0xFFFFFF),
        (format!("Mode: {}", if *punch { "Attack" } else { "Use" }), 0xFFFF55),
    ];
    if !held.is_empty() && *held_count > 0 {
        lines.push((format!("Item: {} x{held_count}", game.pack.names.display_item(held)), 0x55FF55));
    }
    Some(lines)
}

// ---- Players ------------------------------------------------------------------

/// `native_right_click_block` on a vault, arm or deployer; true when it took
/// the click (their templates' own uses never run).
pub(super) fn right_click(game: &mut Game<'_>, pos: Pos, face: [i32; 3]) -> bool {
    let Some(name) = block_name(game, pos) else {
        return false;
    };
    // ArmInteractionPointHandler cancels the click on a point before the
    // block's own use (a depot would take the arm item).
    if let Some(stack) = game.held().filter(|s| &*s.item == ARM).cloned()
        && point_type_at(game, pos).is_some()
    {
        return use_item_on(game, &stack, pos);
    }
    let holding_wrench = game.held().is_some_and(|s| &*s.item == WRENCH);
    match name.as_str() {
        // ItemVaultBlock has no use of its own.
        VAULT => false,
        ARM => {
            if holding_wrench {
                return false;
            }
            // ArmBlock.useItemOn: goggles go on an arm without them (the
            // item stays in hand), once.
            if game.held().is_some_and(|s| &*s.item == "create:goggles") {
                let dim = game.dim.id().to_owned();
                let put_on = with_state(|st| match st.arms.get_mut(&(dim, pos)) {
                    Some(a) if !a.goggles => {
                        a.goggles = true;
                        a.dirty = true;
                        true
                    }
                    _ => false,
                });
                if put_on {
                    return true;
                }
            }
            // ArmBlock.useItemOn: the held item back to the player.
            let taken = game
                .native_with_mod_block_entity(pos, |be| be.items.first_mut().and_then(Option::take))
                .flatten();
            if let Some(s) = taken {
                let dim = game.dim.id().to_owned();
                with_state(|st| {
                    if let Some(a) = st.arms.get_mut(&(dim, pos)) {
                        a.held = None;
                        a.take_held(pos, false);
                    }
                });
                game.give_or_drop(s);
            }
            true
        }
        DEPLOYER | DEPLOYER_ALONG => {
            let Some((facing, _)) = deployer_at(game, pos) else {
                return false;
            };
            // The filter slot (`DeployerFilterSlot`, centred on each side that
            // is neither its front nor along its rotation axis; the hook has no
            // hit position: the clicked face stands for the slot).
            let clicked = Dir::nearest(face);
            let (_, along) = deployer_at(game, pos).unwrap_or((facing, false));
            if clicked.axis() != facing.axis()
                && clicked.axis() != super::create_deployer::rotation_axis(facing, along)
                && !holding_wrench
                && game.held().is_none_or(|s| &*s.item != ARM)
                && game.native_acting_player().is_none_or(|p| p.game_mode != 3)
            {
                let held = game.held().map(|s| s.item.to_string());
                let dim = game.dim.id().to_owned();
                with_state(|s| s.deployers.entry((dim, pos)).or_default().filter = held);
                return true;
            }
            // DeployerBlock.useItemOn: only its front (the hook has no hit
            // position: the clicked face stands for `location.length()`).
            if Dir::nearest(face) != facing {
                return false;
            }
            let dim = game.dim.id().to_owned();
            if holding_wrench {
                // onWrenched on the front: changeMode.
                with_state(|s| {
                    s.deployers.entry((dim, pos)).or_default().change_mode();
                });
                return true;
            }
            let hand = game.held_mut().take();
            let theirs = with_state(|s| {
                let d = s.deployers.entry((dim, pos)).or_default();
                let theirs = d.held.take();
                d.held = hand;
                d.dirty = true;
                theirs
            });
            *game.held_mut() = theirs;
            true
        }
        _ => false,
    }
}

/// The wrench on a vault or deployer away from a deployer's front
/// (`onWrenched`); true when it was one.
pub(super) fn wrench(game: &mut Game<'_>, pos: Pos, face: [i32; 3]) -> bool {
    let face = Dir::nearest(face);
    if let Some((facing, along)) = deployer_at(game, pos) {
        let (f, a) = super::create_deployer::wrenched(facing, along, face);
        if (f, a) != (facing, along) {
            set_block(
                game,
                pos,
                if a { DEPLOYER_ALONG } else { DEPLOYER },
                &[("facing", f.name())],
            );
        }
        return true;
    }
    if block_name(game, pos).as_deref() != Some(VAULT) {
        return false;
    }
    // ItemVaultBlock.onWrenched: from above or below it leaves its vault and
    // turns (HORIZONTAL_AXIS about Y).
    if face.axis() == Axis::Y {
        let axis = vault_axis(game, pos);
        set_block(
            game,
            pos,
            VAULT,
            &[("axis", if axis == Axis::X { "z" } else { "x" })],
        );
        game.native_with_mod_block_entity(pos, |be| {
            let mut part = read_part(axis, &be.nbt);
            part.controller = None;
            part.width = 1;
            part.length = 1;
            part.update_connectivity = true;
            write_part(&part, &mut be.nbt);
        });
    }
    true
}

// ---- The deployer -------------------------------------------------------------

/// The world in front of a deployer, for its fake player.
struct Front<'a, 'g> {
    game: &'a mut Game<'g>,
    dim: String,
    pos: Pos,
    facing: Dir,
}

impl Front<'_, '_> {
    fn clicked(&self) -> Pos {
        let n = self.facing.normal();
        [
            self.pos[0] + 2 * n[0],
            self.pos[1] + 2 * n[1],
            self.pos[2] + 2 * n[2],
        ]
    }
    /// A belt or depot below (`TransportedItemStackHandlerBehaviour`).
    fn transported_below(&self) -> bool {
        let at = self.clicked();
        self.facing == Dir::Down
            && block_name(self.game, at).is_some_and(|n| {
                super::create_logistics::is_belt(&n) || n == super::create_machines::DEPOT
            })
    }
}

impl DeployerWorld<ItemStack> for Front<'_, '_> {
    fn should_activate(&self, held: Option<&ItemStack>) -> bool {
        let target = self.clicked();
        if let Some(h) = held {
            // A block item in front of the same block.
            if self
                .game
                .items()
                .get(&h.item)
                .is_some_and(|i| i.block.is_some())
                && block_name(self.game, target).as_deref() == Some(&*h.item)
            {
                return false;
            }
            if self.transported_below() {
                return false;
            }
        }
        true
    }

    fn ray_distance(&self) -> f32 {
        // The ray from 1.5 to 2.5 blocks out meets the block in front's
        // outline (its collision boxes here) where it enters them.
        let target = self.clicked();
        let shape = self.game.block_collision_shape(target);
        let axis = super::create_machines::core_axis(self.facing.axis());
        let others: Vec<horizon_core::geometry::Axis> = Axis::ALL
            .into_iter()
            .filter(|a| *a != self.facing.axis())
            .map(super::create_machines::core_axis)
            .collect();
        let mut best = 1.0f32;
        for b in shape.boxes() {
            if !others.iter().all(|o| b.min(*o) <= 0.5 && b.max(*o) >= 0.5) {
                continue;
            }
            let entry = if self.facing.positive() {
                b.min(axis)
            } else {
                1.0 - b.max(axis)
            };
            best = best.min(entry.max(0.0) as f32);
        }
        best
    }

    fn activate(
        &mut self,
        mode: Mode,
        held: &mut Option<ItemStack>,
        inventory: &mut Vec<ItemStack>,
    ) {
        // Belt processing is BeltDeployerCallbacks'.
        if self.transported_below() {
            return;
        }
        // A mob in the block in front is punched (`player.attack`); using
        // items on mobs isn't ported (no hook interacts as a player).
        if mode == Mode::Punch && self.punch_mob(held.as_ref()) {
            return;
        }
        match mode {
            Mode::Punch => self.punch(held, inventory),
            Mode::Use => self.use_item(held),
        }
    }
}

impl Front<'_, '_> {
    /// `DeployerHandler.activate`'s entities in the clicked block: one at
    /// random takes the fist's 1 plus the held item's attack damage (no
    /// armour, knockback, enchantments or tool wear here; its drops fall
    /// where it dies). True when there was one.
    fn punch_mob(&mut self, held: Option<&ItemStack>) -> bool {
        let t = self.clicked().map(f64::from);
        let inside: Vec<(u32, f32)> = self
            .game
            .native_mobs(&self.dim)
            .into_iter()
            .filter(|m| {
                m.pos[0] + 0.3 > t[0]
                    && m.pos[0] - 0.3 < t[0] + 1.0
                    && m.pos[2] + 0.3 > t[2]
                    && m.pos[2] - 0.3 < t[2] + 1.0
                    && m.pos[1] + 1.8 > t[1]
                    && m.pos[1] < t[1] + 1.0
            })
            .map(|m| (m.id, m.health))
            .collect();
        if inside.is_empty() {
            return false;
        }
        let roll = horizon_core::random::RandomSource::next_int_bound(&mut self.game.world.random, inside.len() as i32);
        let (id, health) = inside[roll.unwrap_or(0).max(0) as usize % inside.len()];
        let bonus = held.and_then(|h| self.game.items().items.get(&*h.item).map(|i| i.attack_damage)).unwrap_or(0.0);
        let left = health - (1.0 + bonus);
        if left <= 0.0 {
            self.game.native_kill_mob(&self.dim, id);
        } else {
            self.game.native_set_mob_health(&self.dim, id, left);
        }
        true
    }

    /// `DeployerHandler.activate`'s left click: breaking progress, then the
    /// block harvested into the fake player's inventory.
    fn punch(&mut self, held: &mut Option<ItemStack>, inventory: &mut Vec<ItemStack>) {
        let target = self.clicked();
        let key = (self.dim.clone(), self.pos);
        if self.game.block_collision_shape(target).is_empty() {
            with_state(|s| s.breaking.remove(&key));
            return;
        }
        let Some(held_stack) = held.clone() else {
            return;
        };
        let state = self.game.world.block(target[0], target[1], target[2]);
        let facts = self.game.res.facts().get(state).clone();
        if facts.hardness < 0.0 {
            return;
        }
        // BlockBehaviour.getDestroyProgress with the held tool, times 16.
        let speed = self
            .game
            .items()
            .mining_speed(Some(&held_stack), &facts.name);
        let harvest = !facts.requires_tool
            || self
                .game
                .items()
                .correct_for_drops(Some(&held_stack), &facts.name);
        let step = if facts.hardness == 0.0 {
            1.0
        } else {
            speed / facts.hardness / if harvest { 30.0 } else { 100.0 }
        } * 16.0;
        let before = with_state(|s| {
            s.breaking
                .get(&key)
                .filter(|(p, _)| *p == target)
                .map_or(0.0, |b| b.1)
        });
        let progress = before + step;
        if progress >= 1.0 {
            with_state(|s| s.breaking.remove(&key));
            let first = self.game.dropped.len();
            let at = horizon_core::position::BlockPos::new(target[0], target[1], target[2]);
            self.game
                .with_level(|b, level, ticks| b.destroy_block(level, ticks, at, true));
            let _ = self.game.process_level_events();
            // Block.getDrops into the fake player's inventory.
            let [x, y, z] = target.map(f64::from);
            let mut i = first.min(self.game.dropped.len());
            while i < self.game.dropped.len() {
                let p = self.game.dropped[i].position;
                if (x - 0.5..x + 1.5).contains(&p.x)
                    && (y - 0.5..y + 1.5).contains(&p.y)
                    && (z - 0.5..z + 1.5).contains(&p.z)
                {
                    let d = self.game.dropped.remove(i);
                    if harvest {
                        inventory.push(d.stack);
                    }
                } else {
                    i += 1;
                }
            }
            // ItemStack.mineBlock: a tool wears.
            if let Some(h) = held.as_mut() {
                let max = self.game.items().get(&h.item).map_or(0, |i| i.max_damage);
                if max > 0 {
                    h.damage += 1;
                    if h.damage >= max {
                        *held = None;
                    }
                }
            }
            return;
        }
        if progress <= 0.0 {
            with_state(|s| s.breaking.remove(&key));
            return;
        }
        with_state(|s| s.breaking.insert(key, (target, progress)));
        self.game.destroy_block_progress(
            -(self.pos[0] ^ self.pos[2] ^ self.pos[1]).abs() - 1000,
            target,
            (progress * 10.0) as i32,
        );
    }

    /// `DeployerHandler.activate`'s right click: a block item is placed as a
    /// player would place it (`BlockItem.useOn` through vanilla's
    /// placement rules). Other uses (items' `useOn` and `use`, blocks' own
    /// use) need a fake player Horizon doesn't have.
    fn use_item(&mut self, held: &mut Option<ItemStack>) {
        let Some(stack) = held.clone() else {
            return;
        };
        if self
            .game
            .items()
            .get(&stack.item)
            .and_then(|i| i.block)
            .is_none()
        {
            return;
        }
        let target = self.clicked();
        let face = self.facing.opposite();
        let n = self.facing.normal();
        let centre = self.pos.map(|v| f64::from(v) + 0.5);
        // Where the ray from 1.5 out meets the block, or its far end.
        let reach = f64::from(self.ray_distance()).min(1.0 - 1.0 / 64.0) + 1.5 + 1.0 / 64.0;
        let hit = [0, 1, 2].map(|i| centre[i] + f64::from(n[i]) * reach);
        let yaw = match self.facing {
            Dir::South => 0.0,
            Dir::West => 90.0,
            Dir::North => 180.0,
            Dir::East => 270.0,
            _ => 0.0,
        };
        let pitch = match self.facing {
            Dir::Up => -90.0,
            Dir::Down => 90.0,
            _ => 0.0,
        };
        let ctx = horizon_world::level::PlaceContext {
            clicked_pos: horizon_core::position::BlockPos::new(target[0], target[1], target[2]),
            face: super::create_logistics::core_dir(face),
            hit: horizon_core::geometry::Vec3::new(hit[0], hit[1], hit[2]),
            inside: false,
            yaw,
            pitch,
            sneaking: false,
            item: stack.item.to_string(),
            can_use_game_master_blocks: false,
        };
        let outcome = self
            .game
            .with_level(|b, level, ticks| b.place_block(level, ticks, &ctx));
        let horizon_world::level::PlaceOutcome::Placed { pos, state } = outcome else {
            let _ = self.game.process_level_events();
            return;
        };
        let placed = [pos.x, pos.y, pos.z];
        let state = self
            .game
            .apply_item_block_state(placed, state, &stack)
            .unwrap_or(state);
        let name = self.game.res.facts().get(state).name.clone();
        if let Some(entity) = self.game.new_block_entity(&name) {
            self.game
                .world
                .block_entities
                .insert((placed[0], placed[1], placed[2]), entity);
            self.game.apply_placed_item(placed, &stack, false);
        }
        let _ = self.game.process_level_events();
        if let Some(h) = held.as_mut() {
            h.count -= 1;
            if h.count == 0 {
                *held = None;
            }
        }
    }
}

fn save_path(root: &Path, dimension: &str) -> PathBuf {
    root.join("data").join(format!(
        "create_deployers_{}.txt",
        dimension.replace(':', "_")
    ))
}

fn stack_line(tag: &str, s: &ItemStack) -> String {
    format!("{tag} {} {} {}\n", s.count, s.damage, s.item)
}

fn parse_stack(f: &[&str]) -> Option<ItemStack> {
    let mut s = ItemStack::new(f.get(3)?, f.get(1)?.parse().ok()?);
    s.damage = f.get(2)?.parse().ok()?;
    Some(s)
}

/// A deployer's save lines (`DeployerBlockEntity.write`).
fn deployer_text(p: Pos, d: &Deployer<ItemStack>) -> String {
    let mut out = format!(
        "deployer {} {} {} {} {} {} {} {}\n",
        p[0],
        p[1],
        p[2],
        d.state.code(),
        if d.mode == Mode::Punch { "punch" } else { "use" },
        d.timer,
        d.reach,
        u8::from(d.redstone_locked)
    );
    if let Some(f) = &d.filter {
        out.push_str(&format!("filter {f}\n"));
    }
    if let Some(h) = &d.held {
        out.push_str(&stack_line("held", h));
    }
    for o in &d.overflow {
        out.push_str(&stack_line("overflow", o));
    }
    for i in &d.inventory {
        out.push_str(&stack_line("inventory", i));
    }
    out
}

/// The deployers in save lines.
fn parse_deployers<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<(Pos, Deployer<ItemStack>)> {
    let mut out: Vec<(Pos, Deployer<ItemStack>)> = Vec::new();
    for line in lines {
        let f: Vec<&str> = line.split(' ').collect();
        match f.first().copied() {
            Some("deployer") if f.len() == 9 => {
                let n = |i: usize| f[i].parse::<i32>().unwrap_or(0);
                let mut d = Deployer::new();
                d.state = State::from_code(n(4) as u8);
                d.mode = if f[5] == "punch" { Mode::Punch } else { Mode::Use };
                d.timer = n(6);
                d.reach = f[7].parse().unwrap_or(0.0);
                d.redstone_locked = f[8] == "1";
                out.push(([n(1), n(2), n(3)], d));
            }
            Some("filter") if f.len() == 2 => {
                if let Some((_, d)) = out.last_mut() {
                    d.filter = Some(f[1].to_owned());
                }
            }
            Some("held") => {
                if let (Some((_, d)), Some(s)) = (out.last_mut(), parse_stack(&f)) {
                    d.held = Some(s);
                }
            }
            Some("overflow") => {
                if let (Some((_, d)), Some(s)) = (out.last_mut(), parse_stack(&f)) {
                    d.overflow.push(s);
                }
            }
            Some("inventory") => {
                if let (Some((_, d)), Some(s)) = (out.last_mut(), parse_stack(&f)) {
                    d.inventory.push(s);
                }
            }
            _ => {}
        }
    }
    out
}

/// A deployer taken onto a moving structure: its state goes with its block
/// (`DeployerBlockEntity`'s data), as save lines.
pub(super) fn take_deployer(dim: &str, pos: Pos) -> String {
    let d = with_state(|s| s.deployers.remove(&(dim.to_owned(), pos))).unwrap_or_default();
    deployer_text(pos, &d)
}

/// A deployer put back from a moving structure, with the state it carried.
pub(super) fn put_deployer(dim: &str, pos: Pos, text: &str) {
    let d = parse_deployers(text.lines()).pop().map(|(_, d)| d).unwrap_or_default();
    with_state(|s| s.deployers.insert((dim.to_owned(), pos), d));
}

/// A moving deployer's state from what its block carries.
pub(super) fn moving_deployer(text: &str) -> Deployer<ItemStack> {
    parse_deployers(text.lines()).pop().map(|(_, d)| d).unwrap_or_default()
}

/// What its block carries again.
pub(super) fn moving_deployer_text(d: &Deployer<ItemStack>) -> String {
    deployer_text([0; 3], d)
}

/// `DeployerHandler.activate` for a deployer on a moving structure, its hand
/// reaching `clicked` the way it faces in the world (`shouldActivate`
/// first in use mode). True while it is breaking a block there.
pub(super) fn deploy_moving(game: &mut Game<'_>, clicked: Pos, facing: Dir, d: &mut Deployer<ItemStack>) -> bool {
    let n = facing.normal();
    let pos = [0, 1, 2].map(|k| clicked[k] - 2 * n[k]);
    let dim = game.dim.id().to_owned();
    let mut front = Front {
        game,
        dim: dim.clone(),
        pos,
        facing,
    };
    if d.mode == Mode::Use && !front.should_activate(d.held.as_ref()) {
        return false;
    }
    front.activate(d.mode, &mut d.held, &mut d.inventory);
    with_state(|s| s.breaking.contains_key(&(dim, pos)))
}

/// `cancelStall`: a moving deployer stops breaking at `clicked`.
pub(super) fn stop_deploying(game: &mut Game<'_>, clicked: Pos, facing: Dir) {
    let n = facing.normal();
    let pos = [0, 1, 2].map(|k| clicked[k] - 2 * n[k]);
    let dim = game.dim.id().to_owned();
    if with_state(|s| s.breaking.remove(&(dim, pos))).is_some() {
        game.destroy_block_progress(-(pos[0] ^ pos[2] ^ pos[1]).abs() - 1000, clicked, -1);
    }
}

fn load(game: &Game<'_>, dimension: &str) {
    let root = game.save.root.clone();
    let fresh = with_state(|s| {
        s.root.get_or_insert_with(|| root.clone());
        s.loaded.insert(dimension.to_owned())
    });
    if !fresh {
        return;
    }
    let Ok(text) = std::fs::read_to_string(save_path(&root, dimension)) else {
        return;
    };
    let mut lines = text.lines();
    if lines.next() != Some("create-deployers 1") {
        return;
    }
    let out = parse_deployers(lines);
    with_state(|s| {
        for (p, d) in out {
            s.deployers.insert((dimension.to_owned(), p), d);
        }
    });
}

/// Saves every dimension's deployers beside the world (items keep their
/// id, count and damage).
pub(super) fn save_all() {
    with_state(|s| {
        let Some(root) = s.root.clone() else {
            return;
        };
        let _ = std::fs::create_dir_all(root.join("data"));
        let mut dims: Vec<String> = s.loaded.iter().cloned().collect();
        dims.sort();
        for dimension in dims {
            let mut out = String::from("create-deployers 1\n");
            let mut all: Vec<(&Pos, &Deployer<ItemStack>)> = s
                .deployers
                .iter()
                .filter(|((d, _), _)| *d == dimension)
                .map(|((_, p), d)| (p, d))
                .collect();
            all.sort_by_key(|(p, _)| **p);
            for (p, d) in all {
                out.push_str(&deployer_text(*p, d));
            }
            if let Err(e) = std::fs::write(save_path(&root, &dimension), out) {
                log::warn!("Create: can't save the deployers of {dimension}: {e}");
            }
        }
    });
}

/// The level's deployers after the kinetics' tick; placements finished;
/// what clients see of arms and deployers.
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str) {
    load(game, dimension);
    let placed: Vec<Placed> = with_state(|s| {
        let (here, rest): (Vec<Placed>, Vec<Placed>) = std::mem::take(&mut s.placed)
            .into_iter()
            .partition(|p| p.dim == dimension);
        s.placed = rest;
        here
    });
    for p in placed {
        finish_placed(game, p);
    }
    let ticks = with_state(|s| {
        s.ticks += 1;
        s.ticks
    });
    // Deployers the kinetics know of (placed, loaded or set by commands).
    let known: Vec<Pos> = super::create_glue::with_kinetics(game, |k, _| {
        k.positions()
            .into_iter()
            .filter(|p| matches!(k.get(*p).map(|e| e.kind), Some(Kind::Deployer(..))))
            .collect()
    });
    let dim = dimension.to_owned();
    with_state(|s| {
        for p in &known {
            s.deployers.entry((dim.clone(), *p)).or_default();
        }
    });
    let mine: Vec<Pos> = with_state(|s| {
        s.deployers
            .keys()
            .filter(|(d, _)| *d == dim)
            .map(|(_, p)| *p)
            .collect()
    });
    for pos in mine {
        if !game.world.loaded(pos[0], pos[2]) {
            continue;
        }
        let Some((facing, _)) = deployer_at(game, pos) else {
            // DeployerBlock.onRemove: discardPlayer drops everything.
            let gone = with_state(|s| {
                s.breaking.remove(&(dim.clone(), pos));
                s.deployers.remove(&(dim.clone(), pos))
            });
            if let Some(mut d) = gone {
                let c = pos.map(|v| f64::from(v) + 0.5);
                for st in d.discard() {
                    game.spawn_item(st, c, [0.0, 0.1, 0.0], 10);
                }
            }
            with_state(|s| s.pending.push((dim.clone(), pos, None)));
            continue;
        };
        let speed = speed_at(game, pos);
        let Some(mut d) = with_state(|s| s.deployers.remove(&(dim.clone(), pos))) else {
            continue;
        };
        // DeployerBlock.neighborChanged: a signal locks it.
        let locked = super::create_glue::signal(game, pos);
        if d.redstone_locked != locked {
            d.redstone_locked = locked;
            d.dirty = true;
        }
        // DeployerBlockEntity.tick: a held item the filter doesn't take goes to
        // the overflow (`item != stack || !filtering.test(item)`).
        if speed != 0.0
            && d.state == State::Waiting
            && d.timer <= 0
            && d.overflow.is_empty()
            && let (Some(h), Some(f)) = (&d.held, &d.filter)
            && &*h.item != f.as_str()
        {
            let rejected = d.held.take();
            d.overflow.extend(rejected);
            d.timer = timer_speed(speed) * 10;
            d.dirty = true;
        }
        d.tick(
            speed,
            &mut Front {
                game,
                dim: dim.clone(),
                pos,
                facing,
            },
        );
        let view = encode_deployer(&d, speed);
        with_state(|s| {
            note(s, &dim, pos, view);
            s.deployers.insert((dim.clone(), pos), d);
        });
    }
    if ticks % SAVE_EVERY == 0 {
        save_all();
    }
    send(game, dimension, ticks % SNAPSHOT_EVERY == 0);
}

/// The arm and deployer changes of `dimension` to its players; `full`
/// repeats every one.
fn send(game: &mut Game<'_>, dimension: &str, full: bool) {
    let entries: Vec<(Pos, Option<Vec<u8>>)> = with_state(|s| {
        // Arms that are gone.
        let gone: Vec<Pos> = s
            .shown
            .keys()
            .filter(|(d, p)| {
                d == dimension
                    && game.world.loaded(p[0], p[2])
                    && !matches!(
                        block_name(game, *p).as_deref(),
                        Some(ARM | DEPLOYER | DEPLOYER_ALONG | VAULT)
                    )
            })
            .map(|(_, p)| *p)
            .collect();
        for p in gone {
            s.shown.remove(&(dimension.to_owned(), p));
            s.arms.remove(&(dimension.to_owned(), p));
            s.pending.push((dimension.to_owned(), p, None));
        }
        let (here, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut s.pending)
            .into_iter()
            .partition(|(d, _, _)| d == dimension);
        s.pending = rest;
        if full {
            s.shown
                .iter()
                .filter(|((d, _), _)| d == dimension)
                .map(|((_, p), v)| (*p, Some(v.clone())))
                .collect()
        } else {
            here.into_iter().map(|(_, p, v)| (p, v)).collect()
        }
    });
    if entries.is_empty() && !full {
        return;
    }
    let mut message = Vec::from(&b"A1"[..]);
    message.push(u8::from(full));
    for (pos, view) in &entries {
        for v in pos {
            message.extend_from_slice(&v.to_le_bytes());
        }
        match view {
            Some(v) => {
                message.extend_from_slice(&(v.len() as u16).to_le_bytes());
                message.extend_from_slice(v);
            }
            None => message.extend_from_slice(&0u16.to_le_bytes()),
        }
        if message.len() > 28_000 {
            break;
        }
    }
    for player in game.native_server_players() {
        if player.dimension != dimension {
            continue;
        }
        if let Err(e) = game.native_net_send_to_player(player.uuid, ARMS, &message) {
            log::debug!("Create: arms to {}: {e}", player.name);
        }
    }
}

fn push_item(out: &mut Vec<u8>, stack: Option<&ItemStack>) {
    let id = stack.map_or("", |s| &*s.item);
    out.push(id.len().min(255) as u8);
    out.extend_from_slice(&id.as_bytes()[..id.len().min(255)]);
}

/// What a client draws of an arm: its angles and held item.
fn encode_arm(arm: &Arm<ItemStack>) -> Vec<u8> {
    let mut out = vec![1u8];
    for a in [
        arm.angles.base,
        arm.angles.lower,
        arm.angles.upper,
        arm.angles.head,
    ] {
        out.extend_from_slice(&a.to_le_bytes());
    }
    push_item(&mut out, arm.held.as_ref());
    out.push(u8::from(arm.goggles));
    out
}

/// What a client draws of a deployer: its cycle, timer and held item.
fn encode_deployer(d: &Deployer<ItemStack>, speed: f32) -> Vec<u8> {
    let mut out = vec![2u8, d.state.code(), u8::from(d.mode == Mode::Punch)];
    out.extend_from_slice(&d.timer.to_le_bytes());
    out.extend_from_slice(&timer_speed(speed).to_le_bytes());
    out.extend_from_slice(&d.reach.to_le_bytes());
    push_item(&mut out, d.held.as_ref());
    out.push(d.held.as_ref().map_or(0, |s| s.count.min(255) as u8));
    let filter = d.filter.as_deref().unwrap_or("");
    out.push(filter.len().min(255) as u8);
    out.extend_from_slice(&filter.as_bytes()[..filter.len().min(255)]);
    out
}

// ---- Belts and depots under a deployer ---------------------------------------------

fn deploying_recipes(game: &Game<'_>) -> Arc<Recipes> {
    if let Some(r) = with_state(|s| s.recipes.clone()) {
        return r;
    }
    let mut list: Recipes = Vec::new();
    for kind in ["create:deploying", "create:item_application"] {
        for r in game
            .recipes()
            .custom
            .get(kind)
            .map_or(&[][..], Vec::as_slice)
        {
            if let Some(d) = parse_deploying(&r.id, &r.json) {
                let target = ingredient(game, &d.recipe.ingredient);
                let held = ingredient(game, &d.held);
                list.push((d, target, held));
            }
        }
    }
    let list = Arc::new(list);
    with_state(|s| s.recipes = Some(list.clone()));
    list
}

/// `native_recipe_ingredient` without its player (as machines.rs reads them).
fn ingredient(game: &Game<'_>, choices: &[String]) -> Option<Ingredient> {
    use horizon_client::atlas_sources::qualified;
    let list = choices
        .iter()
        .map(|s| match s.strip_prefix('#') {
            Some(tag) => String::from("#") + &qualified(tag),
            None => qualified(s),
        })
        .map(serde_json::Value::String)
        .collect();
    let items = game.items();
    let known: Vec<String> = Ingredient::parse(&serde_json::Value::Array(list), items)?
        .order
        .into_iter()
        .filter(|item| items.get(item).is_some())
        .collect();
    (!known.is_empty()).then(|| Ingredient::of(known))
}

/// `DeployerBlockEntity.getRecipe`: deploying, then item application.
fn find_deploying(
    game: &Game<'_>,
    target: &ItemStack,
    held: &ItemStack,
) -> Option<DeployingRecipe> {
    deploying_recipes(game)
        .iter()
        .find(|(_, t, h)| {
            t.as_ref().is_some_and(|t| t.test(target)) && h.as_ref().is_some_and(|h| h.test(held))
        })
        .map(|(r, _, _)| r.clone())
}

/// The deployer at `deployer` as the `BeltProcessingBehaviour` of a belt
/// or depot below: `onItemReceived` (none when there is no deployer), else
/// whether it holds the item.
pub(super) fn deployer_received(game: &Game<'_>, deployer: Pos, item: &ItemStack) -> Option<bool> {
    let (facing, _) = deployer_at(game, deployer)?;
    let speed = speed_at(game, deployer);
    let dim = game.dim.id().to_owned();
    let (mode, st, locked, held) = with_state(|s| {
        let d = s.deployers.get(&(dim.clone(), deployer))?;
        Some((d.mode, d.state, d.redstone_locked, d.held.clone()))
    })?;
    let has_recipe = held
        .as_ref()
        .is_some_and(|h| find_deploying(game, item, h).is_some());
    let (result, start) = on_item_received(
        speed,
        mode,
        facing == Dir::Down,
        st,
        locked,
        held.is_some(),
        has_recipe,
    );
    if start {
        start_deployer(game, deployer, facing);
    }
    Some(result == ProcessingResult::Hold)
}

fn start_deployer(game: &Game<'_>, pos: Pos, facing: Dir) {
    let dim = game.dim.id().to_owned();
    let Some(mut d) = with_state(|s| s.deployers.remove(&(dim.clone(), pos))) else {
        return;
    };
    // `start` only reads the world (its ray): a read-only front.
    struct Ray(f32);
    impl DeployerWorld<ItemStack> for Ray {
        fn should_activate(&self, _: Option<&ItemStack>) -> bool {
            true
        }
        fn ray_distance(&self) -> f32 {
            self.0
        }
        fn activate(&mut self, _: Mode, _: &mut Option<ItemStack>, _: &mut Vec<ItemStack>) {}
    }
    let n = facing.normal();
    let target = [pos[0] + 2 * n[0], pos[1] + 2 * n[1], pos[2] + 2 * n[2]];
    let shape = game.block_collision_shape(target);
    let top = shape
        .boxes()
        .iter()
        .map(|b| b.max(horizon_core::geometry::Axis::Y))
        .fold(0.0f64, f64::max);
    let distance = if facing == Dir::Down {
        (1.0 - top).max(0.0) as f32
    } else {
        1.0
    };
    d.start(&Ray(distance));
    with_state(|s| s.deployers.insert((dim, pos), d));
}

/// `whileItemHeld` of the deployer at `deployer`: none when there is none,
/// else whether it holds the item and, when it applies its recipe, the
/// results and what is left of the stack (`BeltDeployerCallbacks.activate`,
/// its results not locked).
pub(super) fn deployer_held(
    game: &mut Game<'_>,
    deployer: Pos,
    item: &ItemStack,
) -> Option<(bool, Option<super::create_machines::Pressed>)> {
    let (facing, _) = deployer_at(game, deployer)?;
    let speed = speed_at(game, deployer);
    let dim = game.dim.id().to_owned();
    let (st, timer, locked, held) = with_state(|s| {
        let d = s.deployers.get(&(dim.clone(), deployer))?;
        Some((d.state, d.timer, d.redstone_locked, d.held.clone()))
    })?;
    let recipe = held.as_ref().and_then(|h| find_deploying(game, item, h));
    let (result, step) = when_item_held(
        speed,
        facing == Dir::Down,
        st,
        timer,
        locked,
        held.is_some(),
        recipe.is_some(),
    );
    let hold = result == ProcessingResult::Hold;
    match step {
        HeldStep::Nothing => Some((hold, None)),
        HeldStep::Start => {
            start_deployer(game, deployer, facing);
            Some((hold, None))
        }
        HeldStep::Apply => {
            let recipe = recipe?;
            let results: Vec<ItemStack> = {
                let mut guard = state();
                let s = guard.get_or_insert_with(Arms::default);
                super::create_processing::roll_results(&recipe.recipe, || {
                    s.random = s
                        .random
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    ((s.random >> 40) as f32) / (1u64 << 24) as f32
                })
                .into_iter()
                .map(|(id, count)| ItemStack::new(&id, count))
                .collect()
            };
            let results = super::create_machines::with_remainder(game, &item.item, results);
            let left = (item.count > 1).then(|| with_count(item, item.count - 1));
            // The held item is used (`keep_held_item`, damage, or one less).
            let held_stack = held?;
            let info = game.items().get(&held_stack.item);
            let max_damage = info.map_or(0, |i| i.max_damage);
            let remainder = info.and_then(|i| i.remainder.clone());
            with_state(|s| {
                let Some(d) = s.deployers.get_mut(&(dim.clone(), deployer)) else {
                    return;
                };
                d.dirty = true;
                let Some(h) = d.held.as_mut() else {
                    return;
                };
                match held_use(recipe.keep_held, max_damage) {
                    HeldUse::Keep => {}
                    HeldUse::Damage => {
                        h.damage += 1;
                        if h.damage >= max_damage {
                            d.held = None;
                        }
                    }
                    HeldUse::Shrink => {
                        h.count -= 1;
                        let leftover = remainder.map(|r| ItemStack::new(&r, 1));
                        if h.count == 0 {
                            d.held = leftover;
                        } else if let Some(l) = leftover {
                            d.inventory.push(l);
                        }
                    }
                }
            });
            Some((hold, Some((results, left))))
        }
    }
}

// ---- Clients -------------------------------------------------------------------

#[derive(Clone)]
enum ClientView {
    Arm {
        angles: [f32; 4],
        previous: [f32; 4],
        held: String,
        goggles: bool,
    },
    Deployer {
        state: State,
        punch: bool,
        timer: i32,
        timer_speed: i32,
        reach: f32,
        held: String,
        held_count: u32,
        filter: String,
    },
    Vault {
        along_x: bool,
        controller: Pos,
        width: i32,
    },
}

static CLIENT: Mutex<Option<HashMap<Pos, (ClientView, Instant)>>> = Mutex::new(None);

/// `native_net_from_server` on `create:arms`.
pub(super) fn from_server(data: &[u8]) {
    let Some(rest) = data.strip_prefix(b"A1") else {
        return;
    };
    let Some((&full, mut rest)) = rest.split_first() else {
        return;
    };
    let mut guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    let views = guard.get_or_insert_with(HashMap::new);
    if full != 0 {
        views.clear();
    }
    let now = Instant::now();
    while rest.len() >= 14 {
        let int = |b: &[u8]| i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let pos = [int(&rest[0..]), int(&rest[4..]), int(&rest[8..])];
        let len = usize::from(u16::from_le_bytes([rest[12], rest[13]]));
        rest = &rest[14..];
        if len == 0 {
            views.remove(&pos);
            continue;
        }
        if rest.len() < len {
            return;
        }
        let (v, tail) = rest.split_at(len);
        rest = tail;
        let float = |b: &[u8]| f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let item = |b: &[u8]| -> String {
            let Some((&n, s)) = b.split_first() else {
                return String::new();
            };
            String::from_utf8_lossy(&s[..usize::from(n).min(s.len())]).into_owned()
        };
        let view = match v.first() {
            Some(1) if v.len() >= 18 => {
                let angles = [
                    float(&v[1..]),
                    float(&v[5..]),
                    float(&v[9..]),
                    float(&v[13..]),
                ];
                let previous = match views.get(&pos) {
                    Some((ClientView::Arm { angles: a, .. }, _)) => *a,
                    _ => angles,
                };
                let after = 18 + usize::from(v[17]);
                ClientView::Arm {
                    angles,
                    previous,
                    held: item(&v[17..]),
                    goggles: v.get(after).is_some_and(|g| *g != 0),
                }
            }
            Some(3) if v.len() >= 15 => ClientView::Vault {
                along_x: v[1] != 0,
                controller: [int(&v[2..]), int(&v[6..]), int(&v[10..])],
                width: i32::from(v[14]),
            },
            Some(2) if v.len() >= 16 => ClientView::Deployer {
                state: State::from_code(v[1]),
                punch: v[2] != 0,
                timer: int(&v[3..]),
                timer_speed: int(&v[7..]),
                reach: float(&v[11..]),
                held: item(&v[15..]),
                held_count: v.get(16 + usize::from(v[15])).map_or(0, |c| u32::from(*c)),
                filter: v.get(17 + usize::from(v[15])..).map(item).unwrap_or_default(),
            },
            _ => continue,
        };
        views.insert(pos, (view, now));
    }
}

/// A world opened or closed.
pub(super) fn world_phase(closing: bool) {
    if closing {
        save_all();
    }
    *state() = None;
    *CLIENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn model_state(game: &Game<'_>, name: &str) -> Option<horizon_world::registry::BlockStateId> {
    game.pack.assets.registry.default_state(name)
}

fn draw_item(
    game: &Game<'_>,
    id: &str,
    m: glam::Mat4,
    at: [f64; 3],
    light: u8,
    owner: &str,
    out: &mut Vec<horizon_client::mesher::Vertex>,
) {
    let stack = ItemStack::new(id, 1);
    let owner = super::item_draw::owner_at(at, 0.0, owner);
    let state = game.resolve_item(
        &stack,
        horizon_assets::item_model::DisplayContext::Fixed,
        Some(&owner),
        at[0] as i32 ^ at[2] as i32,
    );
    game.submit_item_world(
        &state,
        m,
        horizon_client::block_entity_render::vertex_light(light),
        out,
    );
}

fn is_block_item(game: &Game<'_>, id: &str) -> bool {
    game.items().get(id).is_some_and(|i| i.block.is_some())
}

/// Each frame: the arms' bodies and claws (`ArmRenderer`) and the
/// deployers' poles, hands and held items (`DeployerRenderer`); their cog
/// and shaft are the kinetics' parts.
pub(super) fn draw(
    game: &Game<'_>,
    camera: glam::DVec3,
    out: &mut Vec<horizon_client::mesher::Vertex>,
) {
    let guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    let Some(views) = guard.as_ref() else {
        return;
    };
    let now = Instant::now();
    for (pos, (view, at)) in views {
        let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
        if (corner + glam::DVec3::splat(0.5)).distance_squared(camera) > 64.0 * 64.0 {
            continue;
        }
        let light = game
            .world
            .light(pos[0], pos[1], pos[2])
            .max(game.world.light(pos[0], pos[1] + 1, pos[2]));
        let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
        let origin = glam::Mat4::from_translation((corner - camera).as_vec3());
        let seed = horizon_core::random::position_seed(horizon_core::position::BlockPos::new(
            pos[0], pos[1], pos[2],
        ));
        let ticks = now.duration_since(*at).as_secs_f32() * 20.0;
        let centre = (corner + glam::DVec3::splat(0.5)).to_array();
        match view {
            ClientView::Arm {
                angles,
                previous,
                held,
                goggles,
            } => {
                // LerpedFloat.getValue(partialTicks) between the last two.
                let pt = ticks.min(1.0);
                let lerp = |i: usize| {
                    if i == 0 || i == 3 {
                        super::create_arm::angle_lerp(pt, previous[i], angles[i])
                    } else {
                        previous[i] + (angles[i] - previous[i]) * pt
                    }
                };
                let (base, lower, upper, head) =
                    (lerp(0), lerp(1) - 135.0, lerp(2) - 90.0, lerp(3));
                let deg = |d: f32| d.to_radians();
                let mut m = origin * glam::Mat4::from_translation(glam::Vec3::splat(0.5));
                // ArmRenderer: upside down on a ceiling (`rotateXDegrees(180)`).
                if arm_on_ceiling(game, *pos) {
                    m *= glam::Mat4::from_rotation_x(std::f32::consts::PI);
                }
                // transformBase
                m *= glam::Mat4::from_translation(glam::Vec3::new(0.0, 4.0 / 16.0, 0.0))
                    * glam::Mat4::from_rotation_y(deg(base));
                let mut parts = vec![("create:hz_part_arm_base", m)];
                // transformLowerArm
                m *= glam::Mat4::from_translation(glam::Vec3::new(0.0, 2.0 / 16.0, 0.0))
                    * glam::Mat4::from_rotation_x(deg(lower + 135.0));
                parts.push(("create:hz_part_arm_lower_body", m));
                // transformUpperArm
                m *= glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.0, -14.0 / 16.0))
                    * glam::Mat4::from_rotation_x(deg(upper - 90.0));
                parts.push(("create:hz_part_arm_upper_body", m));
                // transformHead
                m *= glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.0, -15.0 / 16.0))
                    * glam::Mat4::from_rotation_x(deg(head - 45.0));
                // ARM_CLAW_BASE_GOGGLES when it wears goggles.
                let claw = if *goggles {
                    "create:hz_part_arm_claw_base_goggles"
                } else {
                    "create:hz_part_arm_claw_base"
                };
                parts.push((claw, m));
                let has_item = !held.is_empty();
                let block_item = has_item && is_block_item(game, held);
                for flip in [1.0f32, -1.0] {
                    let grip = if has_item {
                        if block_item { 3.0 / 16.0 } else { 5.0 / 64.0 }
                    } else {
                        1.0 / 16.0
                    };
                    let half = m * glam::Mat4::from_translation(glam::Vec3::new(
                        0.0,
                        -flip * grip,
                        -6.0 / 16.0,
                    ));
                    let name = if flip > 0.0 {
                        "create:hz_part_arm_lower_claw_grip"
                    } else {
                        "create:hz_part_arm_upper_claw_grip"
                    };
                    parts.push((name, half));
                }
                for (name, pm) in parts {
                    if let Some(state) = model_state(game, name) {
                        game.block_model_vertices(state, seed, pm, packed, false, out);
                    }
                }
                if has_item {
                    let scale = if block_item { 0.5 } else { 0.625 };
                    let im = m
                        * glam::Mat4::from_rotation_x(deg(90.0))
                        * glam::Mat4::from_translation(glam::Vec3::new(
                            0.0,
                            if block_item {
                                -9.0 / 16.0
                            } else {
                                -10.0 / 16.0
                            },
                            0.0,
                        ))
                        * glam::Mat4::from_scale(glam::Vec3::splat(scale));
                    draw_item(game, held, im, centre, light, ARM, out);
                }
            }
            ClientView::Vault {
                along_x,
                controller,
                width,
            } => {
                // The vault's blockstate is blank: its model, turned along
                // its axis, with ItemVaultCTBehaviour's connected textures
                // (`LARGE` past 2 wide picks the large sheets).
                let holder = if *along_x { "create:hz_vault_x" } else { "create:hz_vault_z" };
                let Some(state) = model_state(game, holder) else {
                    continue;
                };
                let axis = if *along_x { Axis::X } else { Axis::Z };
                let large = *width > 2;
                // ItemVaultCTBehaviour.connectsTo: the same state, one vault.
                let connected = |other: Pos| {
                    matches!(views.get(&other), Some((ClientView::Vault { along_x: x, controller: c, width: w }, _))
                        if x == along_x && c == controller && (*w > 2) == large)
                };
                super::create_fluid_glue::draw_ct_model(
                    game,
                    state,
                    *pos,
                    origin,
                    |_| false,
                    |facing, name| {
                        let kind = if facing.axis() == axis {
                            "front"
                        } else if facing == Dir::Up {
                            "top"
                        } else if facing == Dir::Down {
                            "bottom"
                        } else {
                            "side"
                        };
                        let original = format!("create:block/vault/vault_{kind}_small");
                        if name != original {
                            return None;
                        }
                        let sheet = if large { "large" } else { "medium" };
                        let (up, right) = super::create_ct::vault_frame(facing, axis);
                        Some((format!("create:block/vault/vault_{kind}_{sheet}"), up, right))
                    },
                    connected,
                    out,
                );
            }
            ClientView::Deployer {
                state,
                punch,
                timer,
                timer_speed,
                reach,
                held,
                filter,
                ..
            } => {
                let Some((facing, along)) = deployer_at(game, *pos) else {
                    continue;
                };
                // DeployerFilterSlot: the filter item on each side that is
                // neither the front nor along the rotation axis.
                if !filter.is_empty() {
                    let rotation = super::create_deployer::rotation_axis(facing, along);
                    for side in Dir::ALL.into_iter().filter(|d| d.axis() != facing.axis() && d.axis() != rotation) {
                        let local = glam::Vec3::new(0.5, 0.5, 15.5 / 16.0) - glam::Vec3::splat(0.5);
                        let turned = glam::Mat4::from_rotation_x(side.vertical_angle().to_radians())
                            * glam::Mat4::from_rotation_y(side.horizontal_angle().to_radians());
                        let n = facing.normal();
                        let at = turned.transform_vector3(local) + glam::Vec3::splat(0.5)
                            - glam::Vec3::new(n[0] as f32, n[1] as f32, n[2] as f32) * (2.0 / 16.0);
                        let x_rot = if side == Dir::Up { 90.0 } else if side == Dir::Down { 270.0 } else { 0.0 };
                        let mut y_rot = side.horizontal_angle() + 180.0;
                        if side.axis() == super::create_kinetics::Axis::Y {
                            y_rot += 180.0 + facing.horizontal_angle();
                        }
                        super::create_logistics::draw_filter_item(game, camera, *pos, filter, at.to_array(), y_rot, x_rot, DEPLOYER, out);
                    }
                }
                // The client's own timer runs on between updates.
                let whole = ticks.floor();
                let mut t = *timer;
                let mut partial = ticks - whole;
                if *timer_speed > 0 && t > 0 {
                    t -= (whole as i32).saturating_mul(*timer_speed);
                    if t <= 0 {
                        t = 0;
                        partial = 0.0;
                    }
                } else {
                    partial = 0.0;
                }
                let mode = if *punch { Mode::Punch } else { Mode::Use };
                let pose = hand_pose(mode, !held.is_empty());
                let distance = hand_offset(*state, t, *timer_speed, partial, pose, *reach);
                let n = facing.normal();
                let offset = glam::Vec3::new(n[0] as f32, n[1] as f32, n[2] as f32) * distance;
                let y_rot = facing.horizontal_angle().to_radians();
                let x_rot = match facing {
                    Dir::Up => 270f32,
                    Dir::Down => 90.0,
                    _ => 0.0,
                }
                .to_radians();
                let centred = |r: glam::Mat4| {
                    glam::Mat4::from_translation(glam::Vec3::splat(0.5))
                        * r
                        * glam::Mat4::from_translation(glam::Vec3::splat(-0.5))
                };
                let turn = |axis_matters: bool| {
                    let z = if axis_matters && (along ^ (facing.axis() == Axis::Z)) {
                        90f32.to_radians()
                    } else {
                        0.0
                    };
                    origin
                        * glam::Mat4::from_translation(offset)
                        * centred(glam::Mat4::from_rotation_y(y_rot))
                        * centred(glam::Mat4::from_rotation_x(x_rot))
                        * centred(glam::Mat4::from_rotation_z(z))
                };
                if let Some(s) = model_state(game, "create:hz_part_deployer_pole") {
                    game.block_model_vertices(s, seed, turn(true), packed, false, out);
                }
                let hand = match pose {
                    HandPose::Pointing => "create:hz_part_deployer_hand_pointing",
                    HandPose::Punching => "create:hz_part_deployer_hand_punching",
                    HandPose::Holding => "create:hz_part_deployer_hand_holding",
                };
                if let Some(s) = model_state(game, hand) {
                    game.block_model_vertices(s, seed, turn(false), packed, false, out);
                }
                if !held.is_empty() {
                    // DeployerRenderer.renderItem (its display mode, a still
                    // deployer facing up spinning its item, isn't drawn).
                    let block_item = is_block_item(game, held);
                    let x2 = match facing {
                        Dir::Up => 90f32,
                        Dir::Down => 270.0,
                        _ => 0.0,
                    };
                    let mut im = origin
                        * glam::Mat4::from_translation(offset + glam::Vec3::splat(0.5))
                        * glam::Mat4::from_rotation_y(
                            (facing.horizontal_angle() + 180.0).to_radians(),
                        )
                        * glam::Mat4::from_rotation_x(x2.to_radians())
                        * glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.0, -11.0 / 16.0));
                    if *punch {
                        im *= glam::Mat4::from_translation(glam::Vec3::new(
                            0.0,
                            1.0 / 8.0,
                            -1.0 / 16.0,
                        ));
                    }
                    let scale = if *punch {
                        0.75
                    } else if block_item {
                        0.75 - 1.0 / 64.0
                    } else {
                        0.5
                    };
                    im *= glam::Mat4::from_scale(glam::Vec3::splat(scale));
                    draw_item(game, held, im, centre, light, DEPLOYER, out);
                }
            }
        }
    }
}
