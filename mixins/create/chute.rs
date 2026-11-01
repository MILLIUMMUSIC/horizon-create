// Create's chute, injected as `crate::game::create_chute`: the item a chute
// holds and how it falls through (`ChuteBlockEntity.tick`,
// `handleDownwardOutput`, `handleInputFromAbove`, `getItemMotion`). No
// Horizon types: the world comes in through `ChuteWorld`. Straight chutes:
// items fall by gravity, or are blown up or down by fans at the ends of
// the column (`pull`, `push`, `handleUpwardOutput`, `handleInputFromBelow`).

/// `ChuteBlockEntity.getExtractionAmount`: up to 16 at a time from above.
pub const CHUTE_EXTRACTION: u32 = 16;

/// `getItemMotion`, in chutes a tick, from the fans' `push` and `pull`.
pub fn item_motion(push: f32, pull: f32) -> f32 {
    let fan_speed_modifier = 1.0 / 64.0;
    let max_item_speed = 20.0;
    let gravity = 4.0;
    let motion = (push + pull) * fan_speed_modifier;
    (motion.clamp(-max_item_speed, max_item_speed) + if motion <= 0.0 { -gravity } else { 0.0 })
        / 20.0
}

/// What a chute needs of the blocks around it.
pub trait ChuteWorld<S> {
    /// `grabCapability(DOWN)` and `insertItemStacked` into it: none when
    /// there is no inventory below (a chute below has none for this),
    /// else what is left.
    fn insert_below(&mut self, stack: &S, simulate: bool) -> Option<Option<S>>;
    /// `getTargetChute`: an empty chute below (`canAcceptItem`); none
    /// when there is no chute below.
    fn chute_below_accepts(&self) -> Option<bool>;
    /// `targetChute.setItem(item, 1)`.
    fn give_chute_below(&mut self, stack: S);
    /// A funnel below facing down takes nothing from a chute.
    fn funnel_below_facing_down(&self) -> bool;
    /// `Block.canSupportRigidBlock` below.
    fn rigid_below(&self) -> bool;
    /// An item entity dropped out of the bottom (`centre - 12/16`, falling
    /// at a quarter block a tick).
    fn drop_below(&mut self, stack: S);
    /// `grabCapability(UP)` and `ItemHelper.extract(UPTO, 16)`: what came
    /// out of the inventory above.
    fn extract_above(&mut self, max: u32) -> Option<S>;
    /// `grabCapability(UP)` and `insertItemStacked` into it, as below.
    fn insert_above(&mut self, _stack: &S, _simulate: bool) -> Option<Option<S>> {
        None
    }
    /// The chute above (an input chute): whether it takes the item, its
    /// item moving up or standing (`getItemMotion() >= 0`); none: no chute.
    fn chute_above_accepts(&self) -> Option<bool> {
        None
    }
    /// `bestOutput.setItem(item, 0)`.
    fn give_chute_above(&mut self, _stack: S) {}
    /// A funnel above facing up, or a solid face above: nothing goes out.
    fn blocked_above(&self) -> bool {
        true
    }
    /// An item entity blown out of the top (`centre + 8/16`, at twice the
    /// motion).
    fn drop_above(&mut self, _stack: S, _motion: f32) {}
    /// `grabCapability(DOWN)` and `ItemHelper.extract(UPTO, 16)`.
    fn extract_below(&mut self, _max: u32) -> Option<S> {
        None
    }
}

/// `updateAirFlow`'s `maxPullDistance`: how far below the chute a fan's
/// pull (`pull - push`) reaches for items (none with a chute below).
pub fn max_pull_distance(speed: f32) -> f32 {
    if speed >= 128.0 {
        3.0
    } else if speed >= 64.0 {
        2.0
    } else if speed >= 32.0 {
        1.0
    } else {
        (speed / 32.0).clamp(0.0, 1.0)
    }
}

/// `ChuteBlockEntity`'s item and its place: 1 at the top, 0 at the bottom.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chute<S> {
    pub item: Option<S>,
    pub position: f32,
}

impl<S: Clone + PartialEq> Chute<S> {
    /// `setItem(stack, insertionPos)`.
    pub fn set_item(&mut self, stack: Option<S>, position: f32) {
        self.item = stack;
        self.position = position;
    }

    /// `ChuteBlockEntity.tick` on the server, at `motion` chutes a tick.
    /// True when the item changed (`notifyUpdate`).
    pub fn tick(&mut self, motion: f32, world: &mut dyn ChuteWorld<S>) -> bool {
        if self.item.is_none() {
            if motion < 0.0
                && let Some(stack) = world.extract_above(CHUTE_EXTRACTION)
            {
                self.set_item(Some(stack), 1.0);
                return true;
            }
            if motion > 0.0
                && let Some(stack) = world.extract_below(CHUTE_EXTRACTION)
            {
                self.set_item(Some(stack), 0.0);
                return true;
            }
            return false;
        }
        let mut next = self.position + motion;
        let mut changed = false;
        if motion < 0.0 && next < 0.5 {
            if !self.downward_output(world, true) {
                next = 0.5;
            } else if next < 0.0 {
                // Horizon can't simulate an insertion into a container, so
                // the simulation above is hopeful: an item the real one
                // refuses waits in the middle, as a refused simulation's.
                if self.downward_output(world, false) {
                    next = self.position;
                    changed = true;
                } else {
                    next = 0.5;
                }
            }
        }
        if motion > 0.0 && next > 0.5 {
            if !self.upward_output(world, true) {
                next = 0.5;
            } else if next > 1.0 {
                if self.upward_output(world, false) {
                    next = self.position;
                    changed = true;
                } else {
                    next = 0.5;
                }
            }
        }
        self.position = next;
        changed
    }

    /// `handleUpwardOutput` for an open chute.
    fn upward_output(&mut self, world: &mut dyn ChuteWorld<S>, simulate: bool) -> bool {
        let Some(item) = self.item.clone() else {
            return false;
        };
        if let Some(remainder) = world.insert_above(&item, simulate) {
            let took = remainder.as_ref() != Some(&item);
            if !simulate {
                let position = self.position;
                self.set_item(remainder, position);
            }
            return took;
        }
        if let Some(accepts) = world.chute_above_accepts() {
            if accepts && !simulate {
                world.give_chute_above(item);
                self.set_item(None, 0.0);
            }
            // An input chute above that doesn't take it: it waits.
            return accepts;
        }
        if world.blocked_above() {
            return false;
        }
        if !simulate {
            world.drop_above(item, 0.0);
            self.set_item(None, 0.0);
        }
        true
    }

    /// `handleDownwardOutput` for a chute facing down.
    fn downward_output(&mut self, world: &mut dyn ChuteWorld<S>, simulate: bool) -> bool {
        let Some(item) = self.item.clone() else {
            return false;
        };
        if let Some(remainder) = world.insert_below(&item, simulate) {
            let took = remainder.as_ref() != Some(&item);
            if !simulate {
                let position = self.position;
                self.set_item(remainder, position);
            }
            // A chute facing down waits for room below.
            return took;
        }
        if let Some(accepts) = world.chute_below_accepts() {
            if !simulate && accepts {
                world.give_chute_below(item);
                self.set_item(None, 0.0);
            }
            return accepts;
        }
        if world.funnel_below_facing_down() || world.rigid_below() {
            return false;
        }
        if !simulate {
            world.drop_below(item);
            self.set_item(None, 0.0);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Below {
        inventory: Option<Vec<u32>>,
        chute: Option<Option<u32>>,
        rigid: bool,
        dropped: Vec<u32>,
        above: Vec<u32>,
    }

    impl ChuteWorld<u32> for Below {
        fn insert_below(&mut self, stack: &u32, simulate: bool) -> Option<Option<u32>> {
            let inv = self.inventory.as_mut()?;
            if inv.len() >= 2 {
                return Some(Some(*stack));
            }
            if !simulate {
                inv.push(*stack);
            }
            Some(None)
        }
        fn chute_below_accepts(&self) -> Option<bool> {
            self.chute.map(|c| c.is_none())
        }
        fn give_chute_below(&mut self, stack: u32) {
            self.chute = Some(Some(stack));
        }
        fn funnel_below_facing_down(&self) -> bool {
            false
        }
        fn rigid_below(&self) -> bool {
            self.rigid
        }
        fn drop_below(&mut self, stack: u32) {
            self.dropped.push(stack);
        }
        fn extract_above(&mut self, _max: u32) -> Option<u32> {
            self.above.pop()
        }
    }

    #[test]
    fn gravity() {
        assert_eq!(item_motion(0.0, 0.0), -0.2);
        assert_eq!(item_motion(64.0 * 20.0 * 4.0, 0.0), 1.0);
    }

    #[test]
    fn items_fall_through_and_out() {
        let mut w = Below::default();
        let mut c = Chute::default();
        c.set_item(Some(7), 1.0);
        let m = item_motion(0.0, 0.0);
        let mut ticks = 0;
        while c.item.is_some() && ticks < 20 {
            c.tick(m, &mut w);
            ticks += 1;
        }
        assert_eq!(w.dropped, vec![7]);
        // 1 -> below 0 at a fifth a tick.
        assert_eq!(ticks, 6);
    }

    #[test]
    fn into_an_inventory_and_waiting_when_full() {
        let mut w = Below {
            inventory: Some(Vec::new()),
            ..Default::default()
        };
        let mut c = Chute::default();
        let m = item_motion(0.0, 0.0);
        for item in [1, 2, 3] {
            c.set_item(Some(item), 1.0);
            for _ in 0..10 {
                c.tick(m, &mut w);
            }
        }
        assert_eq!(w.inventory, Some(vec![1, 2]));
        // The third waits in the middle.
        assert_eq!(c.item, Some(3));
        assert_eq!(c.position, 0.5);
    }

    #[test]
    fn chute_to_chute_and_held_over_solid_ground() {
        let mut w = Below {
            chute: Some(None),
            ..Default::default()
        };
        let mut c = Chute::default();
        c.set_item(Some(4), 1.0);
        for _ in 0..10 {
            c.tick(-0.2, &mut w);
        }
        assert_eq!(w.chute, Some(Some(4)));
        let mut w = Below {
            rigid: true,
            ..Default::default()
        };
        c.set_item(Some(5), 1.0);
        for _ in 0..10 {
            c.tick(-0.2, &mut w);
        }
        assert_eq!(c.item, Some(5));
        assert!(w.dropped.is_empty());
    }

    #[test]
    fn takes_from_above_when_empty() {
        let mut w = Below {
            above: vec![9],
            ..Default::default()
        };
        let mut c: Chute<u32> = Chute::default();
        assert!(c.tick(-0.2, &mut w));
        assert_eq!((c.item, c.position), (Some(9), 1.0));
    }

    #[derive(Default)]
    struct Column {
        above: Option<Vec<u32>>,
        chute_above: Option<bool>,
        given_up: Vec<u32>,
        blown: Vec<u32>,
        below: Vec<u32>,
    }

    impl ChuteWorld<u32> for Column {
        fn insert_below(&mut self, _: &u32, _: bool) -> Option<Option<u32>> {
            None
        }
        fn chute_below_accepts(&self) -> Option<bool> {
            None
        }
        fn give_chute_below(&mut self, _: u32) {}
        fn funnel_below_facing_down(&self) -> bool {
            false
        }
        fn rigid_below(&self) -> bool {
            true
        }
        fn drop_below(&mut self, _: u32) {}
        fn extract_above(&mut self, _: u32) -> Option<u32> {
            None
        }
        fn insert_above(&mut self, stack: &u32, simulate: bool) -> Option<Option<u32>> {
            let inv = self.above.as_mut()?;
            if !simulate {
                inv.push(*stack);
            }
            Some(None)
        }
        fn chute_above_accepts(&self) -> Option<bool> {
            self.chute_above
        }
        fn give_chute_above(&mut self, stack: u32) {
            self.given_up.push(stack);
        }
        fn blocked_above(&self) -> bool {
            false
        }
        fn drop_above(&mut self, stack: u32, _: f32) {
            self.blown.push(stack);
        }
        fn extract_below(&mut self, _: u32) -> Option<u32> {
            self.below.pop()
        }
    }

    #[test]
    fn fans_blow_items_up() {
        // A fan below at 64 rpm pushing up: 1 chute a second.
        let motion = item_motion(64.0, 0.0);
        assert!((motion - 1.0 / 20.0).abs() < 1e-6);
        let mut c: Chute<u32> = Chute::default();
        let mut w = Column {
            below: vec![7],
            ..Column::default()
        };
        assert!(c.tick(motion, &mut w));
        assert_eq!((c.item, c.position), (Some(7), 0.0));
        let mut ticks = 0;
        while c.item.is_some() {
            c.tick(motion, &mut w);
            ticks += 1;
            assert!(ticks < 100);
        }
        assert_eq!(w.blown, vec![7]);
        // Into the chute above, or an inventory above.
        let mut w2 = Column {
            chute_above: Some(true),
            ..Column::default()
        };
        c.set_item(Some(3), 0.9);
        for _ in 0..3 {
            c.tick(motion, &mut w2);
        }
        assert_eq!(w2.given_up, vec![3]);
        let mut w3 = Column {
            above: Some(Vec::new()),
            ..Column::default()
        };
        c.set_item(Some(4), 0.9);
        for _ in 0..3 {
            c.tick(motion, &mut w3);
        }
        assert_eq!(w3.above, Some(vec![4]));
        assert_eq!(max_pull_distance(100.0), 2.0);
        assert_eq!(max_pull_distance(16.0), 0.5);
    }
}
