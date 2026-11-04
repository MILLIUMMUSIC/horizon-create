// The creative crate (`CreativeCrateBlockEntity`, `BottomlessItemHandler`) and
// the item hatch (`ItemHatchBlock`).

use super::Game;
use super::create_kinetics::{Dir, Pos};
use horizon_client::containers::{BlockEntity, ModBlockEntity};
use horizon_client::items::ItemStack;
use horizon_world::nbt::{JavaString, Tag};

pub(super) const CRATE: &str = "create:creative_crate";
pub(super) const HATCH: &str = "create:item_hatch";

fn name_at(game: &Game<'_>, pos: Pos) -> Option<String> {
    game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2])).map(|s| s.name.clone())
}

fn prop(game: &Game<'_>, pos: Pos, key: &str) -> Option<String> {
    game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2]))?.properties.get(key).cloned()
}

fn filter_of(be: &ModBlockEntity) -> Option<String> {
    match be.nbt.get(&JavaString::from("Filter")) {
        Some(Tag::String(t)) => Some(t.to_string_lossy()).filter(|t| !t.is_empty()),
        _ => None,
    }
}

/// `CreativeCrateBlockEntity`'s handler, as a container: its first slot always
/// a full stack of the filter, what goes into the second gone by the next tick.
pub(super) fn crate_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    if entity.items.len() < 2 {
        entity.items.resize(2, None);
    }
    let filter = filter_of(entity);
    entity.items[1] = None;
    entity.items[0] = filter.as_ref().map(|id| {
        let stack = ItemStack::new(id.as_str(), 1);
        let max = super::create_machines::max_stack(game, &stack);
        ItemStack::new(id.as_str(), max)
    });
    super::create_machines::note_view(game, pos, vec![(filter.unwrap_or_default(), 1)]);
}

/// A click on a filter slot: the held item is the filter, an empty hand clears
/// it. `at` is the slot, `radius` its reach.
fn filter_click(game: &mut Game<'_>, pos: Pos, at: [f32; 3], radius: f32) -> bool {
    let local = game.pick().filter(|h| h.pos == pos).map(|h| [0, 1, 2].map(|k| (h.point[k] - f64::from(pos[k])) as f32));
    let on_slot = local.is_some_and(|l| (0..3).map(|k| (l[k] - at[k]).powi(2)).sum::<f32>().sqrt() < radius);
    if !on_slot {
        return false;
    }
    let text = game.held().map(|s| s.item.to_string()).unwrap_or_default();
    game.native_with_mod_block_entity(pos, |be| {
        be.nbt.insert(JavaString::from("Filter"), Tag::String(text.as_str().into()));
    });
    true
}

/// `getLocalOffset` of `HatchFilterSlot`, and the angle it turns by.
fn hatch_slot(facing: Dir) -> ([f32; 3], f32) {
    let angle = facing.horizontal_angle();
    (super::create_funnel::rotate_centered_y([0.5, 5.15 / 16.0, 9.5 / 16.0], angle), angle)
}

/// `native_right_click_block` on these two.
pub(super) fn right_click(game: &mut Game<'_>, pos: Pos) -> bool {
    match name_at(game, pos).as_deref() {
        Some(CRATE) => filter_click(game, pos, [0.5, 13.5 / 16.0, 0.5], 0.2),
        Some(HATCH) => hatch_click(game, pos),
        _ => false,
    }
}

/// `ItemHatchBlock.useItemOn`.
fn hatch_click(game: &mut Game<'_>, pos: Pos) -> bool {
    let Some(facing) = prop(game, pos, "facing").and_then(|f| Dir::parse(&f)) else {
        return false;
    };
    let (slot, _) = hatch_slot(facing);
    // testHit: its offset down an eighth.
    if filter_click(game, pos, [slot[0], slot[1] - 0.125, slot[2]], 0.4 * 0.965 / 2.0) {
        return true;
    }
    let n = facing.normal();
    let target = [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]];
    if !super::create_logistics::has_inventory_at(game, target) {
        return true;
    }
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    if sneaking && game.held().is_some_and(|s| &*s.item == super::create_glue::WRENCH) {
        return false;
    }
    let filter = match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(BlockEntity::Mod(be)) => filter_of(be),
        _ => None,
    };
    let slots: Vec<usize> = if sneaking { (9..36).collect() } else { vec![game.held_slot()] };
    let mut any = false;
    for i in slots {
        let Some(item) = game.stack_in_mut(i).take() else {
            continue;
        };
        // canFitInsideContainerItems, and the filter.
        let unfit = item.item.ends_with("shulker_box");
        if unfit || filter.as_deref().is_some_and(|f| f != &*item.item) {
            *game.stack_in_mut(i) = Some(item);
            continue;
        }
        let count = item.count;
        let left = super::create_logistics::inventory_insert(game, target, facing.opposite(), item);
        any |= left.as_ref().is_none_or(|l| l.count != count);
        *game.stack_in_mut(i) = left;
    }
    if any {
        let at = pos.map(|v| f64::from(v) + 0.5);
        game.with_level(|_, level, _| {
            level.native_play_sound("minecraft:block.barrel.open", at, super::create_sounds::BLOCKS, 0.25, 1.4);
            level.native_play_sound("minecraft:block.netherite_block.place", at, super::create_sounds::BLOCKS, 0.75, 1.15);
        });
        game.native_set_block_property(pos, "open", "true");
        game.native_with_mod_block_entity(pos, |be| {
            be.nbt.insert(JavaString::from("Close"), Tag::Int(10));
        });
        if let Some(player) = game.native_acting_player() {
            let line = if sneaking { "Inventory items deposited" } else { "Held item deposited" };
            game.native_action_bar(player.uuid, horizon_client::text::Text::literal(line.to_owned()));
        }
    }
    true
}

/// `ItemHatchBlock.tick`: shut ten ticks on; its filter told to clients.
pub(super) fn hatch_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    if let Some(Tag::Int(left)) = entity.nbt.get(&JavaString::from("Close")).cloned() {
        if left <= 1 {
            entity.nbt.remove(&JavaString::from("Close"));
            game.native_set_block_property(pos, "open", "false");
        } else {
            entity.nbt.insert(JavaString::from("Close"), Tag::Int(left - 1));
        }
    }
    super::create_machines::note_view(game, pos, vec![(filter_of(entity).unwrap_or_default(), 1)]);
}

/// `getStateForPlacement`: only on a wall.
pub(super) fn hatch_placement(
    level: &super::GameLevel<'_>,
    state: horizon_world::registry::BlockStateId,
    clicked: Dir,
) -> Option<horizon_world::registry::BlockStateId> {
    if clicked == Dir::Up || clicked == Dir::Down {
        return None;
    }
    level.native_state_with_property(state, "facing", clicked.opposite().name())
}

/// `FilteringRenderer` on either, from what clients were told.
pub(super) fn draw(game: &Game<'_>, camera: glam::DVec3, out: &mut Vec<horizon_client::mesher::Vertex>) {
    for pos in super::create_machines::client_positions() {
        let Some(name) = name_at(game, pos) else {
            continue;
        };
        if name != CRATE && name != HATCH {
            continue;
        }
        let Some(id) = super::create_machines::client_slots(pos).and_then(|s| s.first().map(|v| v.0.clone())).filter(|i| !i.is_empty()) else {
            continue;
        };
        if name == CRATE {
            super::create_logistics::draw_filter_item(game, camera, pos, &id, [0.5, 13.5 / 16.0, 0.5], 0.0, 90.0, CRATE, out);
        } else if let Some(facing) = prop(game, pos, "facing").and_then(|f| Dir::parse(&f)) {
            let (at, angle) = hatch_slot(facing);
            super::create_logistics::draw_filter_item(game, camera, pos, &id, at, angle, -45.0, HATCH, out);
        }
    }
}
