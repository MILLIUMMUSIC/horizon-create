// Create's spout and item drain, ported from Create 6.0.10 (MIT, The Create
// Team): SpoutBlockEntity's processing as a belt or depot's
// BeltProcessingBehaviour, FillingBySpout and GenericItemFilling for the
// items Horizon has, GenericItemEmptying, and how SpoutRenderer draws the
// tank and the nozzle. Injected as `crate::game::create_spout`; no Horizon
// types, tested with `cargo test`.

use super::create_fluids::{FluidStack, WATER, bucket_fluid, filled_bucket};

/// `SpoutBlockEntity`'s tank (`SmartFluidTankBehaviour.single(this, 1000)`).
pub const SPOUT_CAPACITY: i32 = 1000;
/// `ItemDrainBlockEntity`'s tank.
#[allow(dead_code)]
pub const DRAIN_CAPACITY: i32 = 1500;
/// `SpoutBlockEntity.FILLING_TIME` (and the drain's).
pub const FILLING_TIME: i32 = 20;
/// `PotionFluidHandler.getRequiredAmountForFilledBottle`.
pub const BOTTLE: i32 = 250;

pub const GLASS_BOTTLE: &str = "minecraft:glass_bottle";
pub const BUCKET: &str = "minecraft:bucket";
pub const POTION: &str = "minecraft:potion";
/// `Potions.WATER`.
pub const WATER_POTION: &str = "minecraft:water";

/// `GenericItemFilling.canItemBeFilled` for vanilla's items: a glass bottle,
/// and an empty bucket (a `FluidBucketWrapper` with room). The milk bucket
/// never, and a full bucket has no room.
pub fn generic_can_fill(item: &str) -> bool {
    item == GLASS_BOTTLE || item == BUCKET
}

/// `GenericItemFilling.getRequiredAmountForItem`: a bottle takes 250 mB of
/// water (`canFillGlassBottleInternally`; potions and tea have no fluid
/// here), a bucket 1000 mB of a fluid that has a bucket.
pub fn generic_fill_amount(item: &str, fluid: &FluidStack) -> Option<i32> {
    if fluid.is_empty() {
        return None;
    }
    if item == GLASS_BOTTLE {
        return (fluid.fluid == WATER).then_some(BOTTLE);
    }
    if item == BUCKET {
        return filled_bucket(&fluid.fluid).map(|_| 1000);
    }
    None
}

/// What `GenericItemFilling.fillItem` makes of one item: its id and, for a
/// bottle, the potion it holds (`Potions.WATER`).
pub fn generic_fill_result(
    item: &str,
    fluid: &str,
) -> Option<(&'static str, Option<&'static str>)> {
    if item == GLASS_BOTTLE && fluid == WATER {
        return Some((POTION, Some(WATER_POTION)));
    }
    if item == BUCKET {
        return filled_bucket(fluid).map(|b| (b, None));
    }
    None
}

/// `GenericItemEmptying.emptyItem` for vanilla's items: a bucket of water or
/// lava gives its 1000 mB and the empty bucket; a plain water bottle
/// (`PotionFluidHandler.getFluidFromPotionItem`) 250 mB of water and the
/// glass bottle. Other potions are Create's potion fluid, which Horizon has
/// no fluid for: they aren't taken.
#[allow(dead_code)]
pub fn generic_empty(item: &str, potion: Option<&str>) -> Option<(FluidStack, &'static str)> {
    if let Some(fluid) = bucket_fluid(item) {
        return Some((FluidStack::new(fluid, 1000), BUCKET));
    }
    if item == POTION && potion == Some(WATER_POTION) {
        return Some((FluidStack::new(WATER, BOTTLE), GLASS_BOTTLE));
    }
    None
}

/// What the spout does with an item two blocks below
/// (`BeltProcessingBehaviour.ProcessingResult`, and the step it takes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spouting {
    Pass,
    Hold,
    /// `processingTicks = FILLING_TIME`, the spouting sound: hold.
    Start,
    /// The process is done: fill one item with this much, hold.
    Fill(i32),
}

/// `SpoutBlockEntity.onItemReceived`: whether it holds an item arriving
/// below. `fillable` is `FillingBySpout.canItemBeFilled`, `required` its
/// `getRequiredAmountForItem` for what the tank holds.
pub fn on_item_received(fillable: bool, tank: &FluidStack, required: Option<i32>) -> bool {
    if !fillable {
        return false;
    }
    if tank.is_empty() {
        return true;
    }
    required.is_some()
}

/// `SpoutBlockEntity.whenItemHeld`, with its `processingTicks`.
pub fn when_item_held(
    processing_ticks: i32,
    fillable: bool,
    tank: &FluidStack,
    required: Option<i32>,
) -> Spouting {
    if processing_ticks != -1 && processing_ticks != 5 {
        return Spouting::Hold;
    }
    if !fillable {
        return Spouting::Pass;
    }
    if tank.is_empty() {
        return Spouting::Hold;
    }
    let Some(required) = required else {
        return Spouting::Pass;
    };
    if required > tank.amount {
        return Spouting::Hold;
    }
    if processing_ticks == -1 {
        return Spouting::Start;
    }
    Spouting::Fill(required)
}

/// `SpoutBlockEntity.tick`'s countdown (both sides): -1 when idle.
pub fn tick(processing_ticks: i32) -> i32 {
    if processing_ticks >= 0 {
        processing_ticks - 1
    } else {
        processing_ticks
    }
}

/// `SpoutRenderer`: the tank's fluid box (from the block's corner, in
/// blocks: min x, y, z, max x, y, z), as full as `level` (0 to 1), never
/// under 0.175 of it; none when empty.
pub fn tank_box(level: f32) -> Option<[f32; 6]> {
    if level <= 0.0 {
        return None;
    }
    let level = level.max(0.175);
    let min = 2.5 / 16.0;
    let max = min + 11.0 / 16.0;
    let y_offset = 11.0 / 16.0 * level;
    // translate(0, yOffset) then a box from min - yOffset to min.
    Some([min, min, min, max, min + y_offset, max])
}

/// `SpoutRenderer`'s stream and nozzle at `processing_pt`
/// (`processingTicks - partialTicks`; `None` when idle): the stream's
/// radius (`bb.inflate(radius / 32)`, none when there is none) and how far
/// each nozzle bit moves down (`-3 * squeeze / 32` each).
pub fn nozzle(processing_pt: Option<f32>, has_fluid: bool) -> (Option<f32>, f32) {
    let Some(pt) = processing_pt else {
        return (None, 0.0);
    };
    let progress = (1.0 - (pt - 5.0) / 10.0).clamp(0.0, 1.0);
    let radius = if has_fluid {
        (2.0 * progress - 1.0).powi(2) - 1.0
    } else {
        0.0
    };
    let squeeze = if pt < 0.0 {
        0.0
    } else if pt < 2.0 {
        // Mth.lerp(pt / 2, 0, -1)
        -pt / 2.0
    } else if pt < 10.0 {
        -1.0
    } else {
        radius
    };
    (has_fluid.then_some(radius), squeeze)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bottles_and_buckets_fill_as_generic_item_filling() {
        let water = FluidStack::new(WATER, 1000);
        let lava = FluidStack::new(super::super::create_fluids::LAVA, 1000);
        assert!(generic_can_fill(GLASS_BOTTLE) && generic_can_fill(BUCKET));
        assert!(
            !generic_can_fill("minecraft:milk_bucket")
                && !generic_can_fill("minecraft:water_bucket")
        );
        assert_eq!(generic_fill_amount(GLASS_BOTTLE, &water), Some(250));
        assert_eq!(generic_fill_amount(GLASS_BOTTLE, &lava), None);
        assert_eq!(generic_fill_amount(BUCKET, &lava), Some(1000));
        assert_eq!(
            generic_fill_result(GLASS_BOTTLE, WATER),
            Some((POTION, Some(WATER_POTION)))
        );
        assert_eq!(
            generic_fill_result(BUCKET, WATER),
            Some(("minecraft:water_bucket", None))
        );
    }

    #[test]
    fn buckets_and_water_bottles_empty() {
        assert_eq!(
            generic_empty("minecraft:lava_bucket", None),
            Some((
                FluidStack::new(super::super::create_fluids::LAVA, 1000),
                BUCKET
            ))
        );
        assert_eq!(
            generic_empty(POTION, Some(WATER_POTION)),
            Some((FluidStack::new(WATER, 250), GLASS_BOTTLE))
        );
        assert_eq!(generic_empty(POTION, Some("minecraft:swiftness")), None);
        assert_eq!(generic_empty(BUCKET, None), None);
    }

    #[test]
    fn the_spout_holds_starts_then_fills_at_five() {
        let water = FluidStack::new(WATER, 1000);
        let empty = FluidStack::empty();
        // An empty spout holds what it could fill, passes the rest.
        assert!(on_item_received(true, &empty, None));
        assert!(!on_item_received(false, &water, Some(250)));
        assert!(!on_item_received(true, &water, None));
        assert_eq!(when_item_held(-1, true, &water, Some(250)), Spouting::Start);
        assert_eq!(when_item_held(12, true, &water, Some(250)), Spouting::Hold);
        assert_eq!(
            when_item_held(5, true, &water, Some(250)),
            Spouting::Fill(250)
        );
        assert_eq!(
            when_item_held(-1, true, &water.with_amount(100), Some(250)),
            Spouting::Hold
        );
        assert_eq!(when_item_held(-1, true, &water, None), Spouting::Pass);
        // Twenty ticks counted down to -1.
        let mut t = FILLING_TIME;
        for _ in 0..21 {
            t = tick(t);
        }
        assert_eq!(t, -1);
        assert_eq!(tick(-1), -1);
    }

    #[test]
    fn the_nozzle_squeezes_while_spouting() {
        assert_eq!(nozzle(None, true), (None, 0.0));
        assert_eq!(nozzle(Some(8.0), true).1, -1.0);
        assert_eq!(nozzle(Some(1.0), true).1, -0.5);
        // At the start the stream is thin, at five it is widest.
        let (r0, _) = nozzle(Some(15.0), true);
        let (r5, _) = nozzle(Some(10.0), true);
        assert_eq!(r0, Some(0.0));
        assert_eq!(r5, Some(-1.0));
        let b = tank_box(0.5).unwrap();
        assert!((b[4] - (2.5 / 16.0 + 5.5 / 16.0)).abs() < 1e-6);
        assert!(tank_box(0.0).is_none());
    }
}
