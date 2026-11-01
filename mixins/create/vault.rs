// Create's item vault, ported from Create 6.0.10 (MIT, The Create Team):
// ItemVaultBlock, ItemVaultBlockEntity and the ConnectivityHandler that joins
// vaults into one multiblock (up to 3 wide and 3 times as long along their
// axis). No Horizon types: the vaults are reached through `VaultLevel`, the
// glue keeps each block's part in its block entity.

use super::create_kinetics::{Axis, Pos};
use std::collections::{HashMap, HashSet};

/// `CLogistics.vaultCapacity`: stacks a vault holds per block.
pub const VAULT_CAPACITY: usize = 20;
/// `ItemVaultBlockEntity.getMaxWidth`.
pub const MAX_WIDTH: i32 = 3;

/// One vault block's `ItemVaultBlockEntity` multiblock fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VaultPart {
    /// `ItemVaultBlock.HORIZONTAL_AXIS` (X or Z).
    pub axis: Axis,
    /// `controller`: none when this block is its own controller.
    pub controller: Option<Pos>,
    /// `radius` and `length` (kept by the controller).
    pub width: i32,
    pub length: i32,
    /// `updateConnectivity`: look for a multiblock on the next tick.
    pub update_connectivity: bool,
}

impl VaultPart {
    pub fn new(axis: Axis) -> Self {
        VaultPart {
            axis,
            controller: None,
            width: 1,
            length: 1,
            update_connectivity: false,
        }
    }
    /// `isController`.
    pub fn is_controller(&self, pos: Pos) -> bool {
        self.controller.is_none_or(|c| c == pos)
    }
    /// `getController`.
    pub fn controller_of(&self, pos: Pos) -> Pos {
        if self.is_controller(pos) {
            pos
        } else {
            self.controller.unwrap_or(pos)
        }
    }
    /// `ItemVaultBlock.LARGE`, as `notifyMultiUpdated` sets it (no block
    /// property holds it here: the barrel template has none to spare).
    #[allow(dead_code)]
    pub fn large(&self) -> bool {
        self.width > 2
    }
}

/// The vaults of a level.
pub trait VaultLevel {
    fn part(&self, pos: Pos) -> Option<VaultPart>;
    fn set_part(&mut self, pos: Pos, part: VaultPart);
}

/// `IMultiBlockEntityContainer.getMaxLength(longAxis, width)` of a vault.
pub fn max_length(axis: Axis, width: i32) -> i32 {
    if axis == Axis::Y {
        MAX_WIDTH
    } else {
        width * 3
    }
}

/// The block `y` along `axis` and `x`, `z` across it from `origin`, as
/// ConnectivityHandler walks a multiblock.
pub fn offset(axis: Axis, origin: Pos, y: i32, x: i32, z: i32) -> Pos {
    let [ox, oy, oz] = origin;
    match axis {
        Axis::X => [ox + y, oy + x, oz + z],
        Axis::Y => [ox + x, oy + y, oz + z],
        Axis::Z => [ox + x, oy + z, oz + y],
    }
}

/// The blocks of the vault whose controller is at `controller`, in the
/// order of its combined inventory (`initCapability`: slot `i` is slot
/// `i % 20` of block `i / 20`).
pub fn members(controller: Pos, part: &VaultPart) -> Vec<Pos> {
    let r = part.width;
    let mut out = Vec::new();
    for y in 0..part.length {
        for x in 0..r {
            for z in 0..r {
                out.push(offset(part.axis, controller, y, x, z));
            }
        }
    }
    out
}

/// `ItemVaultBlock.getStateForPlacement`: the axis of a vault placed against
/// `placed_on` (its axis if it is a vault) unless sneaking, else the
/// player's horizontal facing's.
pub fn placement_axis(placed_on: Option<Axis>, sneaking: bool, horizontal_facing: Axis) -> Axis {
    match placed_on {
        Some(a) if !sneaking => a,
        _ => horizontal_facing,
    }
}

/// `SearchCache`: each position's controller, as first found.
#[derive(Default)]
struct SearchCache {
    controllers: HashMap<Pos, Option<Pos>>,
}

impl SearchCache {
    fn put(&mut self, pos: Pos, controller: Pos) {
        self.controllers.insert(pos, Some(controller));
    }
    fn get_or_cache(&mut self, level: &dyn VaultLevel, pos: Pos) -> Option<Pos> {
        if let Some(c) = self.controllers.get(&pos) {
            return *c;
        }
        let found = level
            .part(pos)
            .map(|p| p.controller_of(pos))
            .filter(|c| level.part(*c).is_some());
        self.controllers.insert(pos, found);
        found
    }
}

/// `ItemVaultBlockEntity.updateConnectivity` of the controller at `pos`
/// (`ConnectivityHandler.formMulti`).
pub fn form_multi(level: &mut dyn VaultLevel, pos: Pos) {
    let mut cache = SearchCache::default();
    form_multi_from(level, &mut cache, vec![pos]);
}

fn form_multi_from(level: &mut dyn VaultLevel, cache: &mut SearchCache, mut frontier: Vec<Pos>) {
    let Some(first) = frontier.first().and_then(|p| level.part(*p)) else {
        return;
    };
    let main = first.axis;
    let vertical = main == Axis::Y;
    let mut min_x = if vertical {
        i64::MAX
    } else {
        i64::from(i32::MIN)
    };
    let mut min_y = if !vertical {
        i64::MAX
    } else {
        i64::from(i32::MIN)
    };
    let mut min_z = if vertical {
        i64::MAX
    } else {
        i64::from(i32::MIN)
    };
    for p in &frontier {
        min_x = min_x.min(i64::from(p[0]));
        min_y = min_y.min(i64::from(p[1]));
        min_z = min_z.min(i64::from(p[2]));
    }
    let w = i64::from(MAX_WIDTH);
    if vertical {
        min_x -= w;
        min_z -= w;
    } else {
        min_y -= w;
    }
    // The creation queue (a PriorityQueue by amount, largest first).
    let mut queue: Vec<(i32, Pos)> = Vec::new();
    let mut visited: HashSet<Pos> = HashSet::new();
    while !frontier.is_empty() {
        let part = frontier.remove(0);
        if !visited.insert(part) {
            continue;
        }
        let amount = try_to_form(level, part, cache, true);
        if amount > 1 {
            queue.push((amount, part));
        }
        for axis in Axis::ALL {
            let mut next = part;
            next[axis as usize] -= 1;
            if i64::from(next[0]) <= min_x
                || i64::from(next[1]) <= min_y
                || i64::from(next[2]) <= min_z
            {
                continue;
            }
            if visited.contains(&next) || level.part(next).is_none() {
                continue;
            }
            frontier.push(next);
        }
    }
    visited.clear();
    queue.sort_by_key(|(amount, _)| std::cmp::Reverse(*amount));
    for (_, part) in queue {
        if !visited.insert(part) {
            continue;
        }
        try_to_form(level, part, cache, false);
    }
}

/// `tryToFormNewMulti`: the biggest multiblock `pos` (a controller) can
/// start, made when not simulating; its block count.
fn try_to_form(
    level: &mut dyn VaultLevel,
    pos: Pos,
    cache: &mut SearchCache,
    simulate: bool,
) -> i32 {
    let Some(part) = level.part(pos) else {
        return 0;
    };
    if !part.is_controller(pos) {
        return 0;
    }
    let mut best_width = 1;
    let mut best_amount = -1;
    for w in 1..=MAX_WIDTH {
        let amount = try_width(level, pos, w, cache, true);
        if amount < best_amount {
            continue;
        }
        best_width = w;
        best_amount = amount;
    }
    if !simulate {
        if part.width == best_width && part.width * part.width * part.length == best_amount {
            return best_amount;
        }
        split(level, pos, Some(cache));
        try_width(level, pos, best_width, cache, false);
        if let Some(mut p) = level.part(pos) {
            p.update_connectivity = false;
            p.width = best_width;
            p.length = best_amount / best_width / best_width;
            level.set_part(pos, p);
        }
    }
    best_amount
}

/// `tryToFormNewMultiOfWidth`.
fn try_width(
    level: &mut dyn VaultLevel,
    origin: Pos,
    width: i32,
    cache: &mut SearchCache,
    simulate: bool,
) -> i32 {
    let Some(be) = level.part(origin) else {
        return 0;
    };
    let axis = be.axis;
    let mut amount = 0;
    let mut height = 0;
    'search: for y in 0..max_length(axis, width) {
        for x in 0..width {
            for z in 0..width {
                let pos = offset(axis, origin, y, x, z);
                let Some(con_pos) = cache.get_or_cache(level, pos) else {
                    break 'search;
                };
                let Some(controller) = level.part(con_pos) else {
                    break 'search;
                };
                let other = controller.width;
                if other > width {
                    break 'search;
                }
                if other == width && controller.length == max_length(axis, width) {
                    break 'search;
                }
                if axis != controller.axis {
                    break 'search;
                }
                if con_pos != origin {
                    let [cx, cy, cz] = con_pos;
                    let [ox, oy, oz] = origin;
                    let outside = if axis == Axis::Y {
                        cx < ox || cz < oz || cx + other > ox + width || cz + other > oz + width
                    } else {
                        (axis == Axis::Z && cx < ox)
                            || cy < oy
                            || (axis == Axis::X && cz < oz)
                            || (axis == Axis::Z && cx + other > ox + width)
                            || cy + other > oy + width
                            || (axis == Axis::X && cz + other > oz + width)
                    };
                    if outside {
                        break 'search;
                    }
                }
            }
        }
        amount += width * width;
        height += 1;
    }
    if simulate {
        return amount;
    }
    for y in 0..height {
        for x in 0..width {
            for z in 0..width {
                let pos = offset(axis, origin, y, x, z);
                if pos == origin || level.part(pos).is_none() {
                    continue;
                }
                split(level, pos, Some(cache));
                if let Some(mut p) = level.part(pos) {
                    p.controller = Some(origin);
                    p.update_connectivity = false;
                    p.length = height;
                    p.width = width;
                    level.set_part(pos, p);
                }
                cache.put(pos, origin);
            }
        }
    }
    amount
}

/// `ConnectivityHandler.splitMulti` of the multiblock `pos` is part of:
/// every block of it back on its own, looking for a multiblock again on its
/// next tick (`removeController`).
pub fn split_multi(level: &mut dyn VaultLevel, pos: Pos) {
    split(level, pos, None);
}

fn split(level: &mut dyn VaultLevel, pos: Pos, mut cache: Option<&mut SearchCache>) {
    let Some(part) = level.part(pos) else {
        return;
    };
    let origin = part.controller_of(pos);
    let Some(be) = level.part(origin) else {
        return;
    };
    if be.width == 1 && be.length == 1 {
        return;
    }
    for p in members(origin, &be) {
        let Some(at) = level.part(p) else {
            continue;
        };
        if at.controller_of(p) != origin {
            continue;
        }
        remove_controller(level, p);
        if let Some(c) = cache.as_deref_mut() {
            c.put(p, p);
        }
    }
}

/// `ItemVaultBlockEntity.removeController`.
pub fn remove_controller(level: &mut dyn VaultLevel, pos: Pos) {
    if let Some(mut p) = level.part(pos) {
        p.update_connectivity = true;
        p.controller = None;
        p.width = 1;
        p.length = 1;
        level.set_part(pos, p);
    }
}

/// What a vault block's tick finds of its multiblock when a block of it
/// went without its `onRemove` (`splitMulti`) being heard: true when the
/// multiblock it belongs to is whole.
pub fn multi_is_whole(level: &dyn VaultLevel, pos: Pos) -> bool {
    let Some(part) = level.part(pos) else {
        return false;
    };
    let origin = part.controller_of(pos);
    let Some(be) = level.part(origin) else {
        return false;
    };
    if !be.is_controller(origin) {
        return false;
    }
    let all = members(origin, &be);
    all.contains(&pos)
        && all.iter().all(|p| {
            level
                .part(*p)
                .is_some_and(|q| q.controller_of(*p) == origin && q.axis == be.axis)
        })
}

/// The vault block's tick (`ItemVaultBlockEntity.tick`): a multiblock that
/// lost a block is split, then `updateConnectivity` forms one again.
pub fn tick(level: &mut dyn VaultLevel, pos: Pos) {
    let Some(part) = level.part(pos) else {
        return;
    };
    if (part.width > 1 || part.length > 1 || !part.is_controller(pos))
        && !multi_is_whole(level, pos)
    {
        if part.is_controller(pos) {
            split_multi(level, pos);
        }
        remove_controller(level, pos);
    }
    let Some(mut part) = level.part(pos) else {
        return;
    };
    if part.update_connectivity {
        part.update_connectivity = false;
        level.set_part(pos, part);
        if part.is_controller(pos) {
            form_multi(level, pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Vaults(HashMap<Pos, VaultPart>);

    impl VaultLevel for Vaults {
        fn part(&self, pos: Pos) -> Option<VaultPart> {
            self.0.get(&pos).copied()
        }
        fn set_part(&mut self, pos: Pos, part: VaultPart) {
            self.0.insert(pos, part);
        }
    }

    impl Vaults {
        /// A vault placed (`onPlace` → `updateConnectivity`).
        fn place(&mut self, pos: Pos, axis: Axis) {
            self.0.insert(pos, VaultPart::new(axis));
            form_multi(self, pos);
        }
        fn tick_all(&mut self) {
            let mut all: Vec<Pos> = self.0.keys().copied().collect();
            all.sort();
            for p in all {
                tick(self, p);
            }
        }
    }

    #[test]
    fn a_line_of_vaults_joins_along_its_axis() {
        let mut v = Vaults::default();
        for x in 0..3 {
            v.place([x, 64, 0], Axis::X);
            v.tick_all();
        }
        let c = v.part([0, 64, 0]).unwrap();
        assert!(c.is_controller([0, 64, 0]));
        assert_eq!((c.width, c.length), (1, 3));
        for x in 1..3 {
            assert_eq!(v.part([x, 64, 0]).unwrap().controller, Some([0, 64, 0]));
        }
        assert_eq!(
            members([0, 64, 0], &c),
            vec![[0, 64, 0], [1, 64, 0], [2, 64, 0]]
        );
    }

    #[test]
    fn length_is_capped_at_three_times_the_width() {
        let mut v = Vaults::default();
        for x in 0..4 {
            v.place([x, 64, 0], Axis::X);
            v.tick_all();
        }
        let c = v.part([0, 64, 0]).unwrap();
        assert_eq!((c.width, c.length), (1, 3));
        let last = v.part([3, 64, 0]).unwrap();
        assert!(last.is_controller([3, 64, 0]));
        assert_eq!((last.width, last.length), (1, 1));
    }

    #[test]
    fn crossing_axes_dont_join() {
        let mut v = Vaults::default();
        v.place([0, 64, 0], Axis::X);
        v.place([1, 64, 0], Axis::Z);
        v.tick_all();
        assert!(v.part([0, 64, 0]).unwrap().is_controller([0, 64, 0]));
        assert!(v.part([1, 64, 0]).unwrap().is_controller([1, 64, 0]));
    }

    #[test]
    fn a_two_by_two_vault_forms_and_is_large_at_three() {
        let mut v = Vaults::default();
        for y in 0..2 {
            for z in 0..2 {
                for x in 0..2 {
                    v.place([x, 64 + y, z], Axis::X);
                    v.tick_all();
                }
            }
        }
        v.tick_all();
        let c = v.part([0, 64, 0]).unwrap();
        assert!(c.is_controller([0, 64, 0]));
        assert_eq!((c.width, c.length), (2, 2));
        assert!(!c.large());
        assert_eq!(members([0, 64, 0], &c).len(), 8);
        // Slot order: along the axis first (yOffset), then up, then across.
        assert_eq!(members([0, 64, 0], &c)[1], [0, 64, 1]);
        assert_eq!(members([0, 64, 0], &c)[4], [1, 64, 0]);
    }

    #[test]
    fn removing_a_block_splits_and_the_rest_forms_again() {
        let mut v = Vaults::default();
        for x in 0..3 {
            v.place([x, 64, 0], Axis::X);
            v.tick_all();
        }
        v.0.remove(&[0, 64, 0]);
        v.tick_all();
        v.tick_all();
        let c = v.part([1, 64, 0]).unwrap();
        assert!(c.is_controller([1, 64, 0]));
        assert_eq!((c.width, c.length), (1, 2));
        assert_eq!(v.part([2, 64, 0]).unwrap().controller, Some([1, 64, 0]));
    }

    #[test]
    fn placement_follows_the_vault_it_is_placed_on() {
        assert_eq!(placement_axis(Some(Axis::Z), false, Axis::X), Axis::Z);
        assert_eq!(placement_axis(Some(Axis::Z), true, Axis::X), Axis::X);
        assert_eq!(placement_axis(None, false, Axis::X), Axis::X);
    }
}
