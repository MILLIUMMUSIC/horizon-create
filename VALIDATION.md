# Validation

## Combined pass over earlier milestones (macOS, offscreen)

Checked in one world after milestone 4 part 1 was merged, with the same build:

- Depot: a hopper above fills it and a hopper below empties it (3 iron ingots through to a chest).
- Millstone: a hopper above feeds it wheat, an andesite funnel on its side takes the flour out.
- Press over items on the ground: 2 iron ingots pressed into 2 iron sheets. **Fixed here**: the scan tested the
  item's feet against the deflated box under the press, so an item lying on the floor was never found; it now
  tests the item's bounding box, as `getEntitiesOfClass` does.
- Fan: pushes items on the ground along its current; through a lit soul campfire it turns sand into soul sand.
- Wrench: a creative motor and a press turn a quarter about the clicked top face; a fan clicked on a face along its
  axis stays (as Create's `getRotatedBlockState`); sneak + wrench in survival picks up a depot into the inventory and
  drops its items.
- Water wheel in a water trough: −8 RPM from the flow, driving two shafts beyond it.
- Belt: the far pulley drives a shaft beyond it (16 RPM); an andesite funnel on a chest over the belt pushes the
  chest's items onto it, one at a time.
- Andesite funnels: one on a chest's or millstone's side extracts one item and waits until it is gone (as Create's
  funnel with no filter); a collecting funnel on a chest takes items dropped onto it.
- Chute: a hopper above it feeds a chest below through it.

## Milestone 4, part 1

Ported in a cloud session on Linux (x86_64), then built, fixed and checked on macOS (Apple M5), Rust 1.99.1,
Python 3.12, against the pinned Horizon (`aa27eca`) and horizon-mixins (`3f99a5c`).

| Check | How | Result |
|---|---|---|
| Rule modules (`kinetics.rs`, `processing.rs`, `fan.rs`, `obj.rs`, `belt.rs`, `chute.rs`, `funnel.rs`) | `cargo fmt --all --check`, `cargo clippy --all-targets --locked -D warnings`, `cargo test --locked` | 75 tests pass, no warnings |
| The bundle applies and the game compiles | `tools/horizon.py check` (Linux) and `build` (macOS) | Pass, no warnings |
| Content mod from Create 6.0.10's jar | `tools/prepare_content.py --jar` | Pass: belt part blocks, chute, both funnel blocks, loot tables |
| In the game (offscreen, `horizon-ctl`, macOS) | Rigs built with commands, screenshots and frame differences, `/data get block` | See below |

Seen in the game:

- Belt connector: shaft, then a shaft 6 blocks along: a belt of start, 5 middles and end; used up in survival. A
  sloped belt (one up per block) is made and drawn diagonally.
- Items dropped on it lie flat, move apart at the belt's speed, stop at a wall or are thrown off the end; the
  belt texture scrolls (23 % of its pixels change between frames, the floor's none). Belt into depot and into a
  chute over a chest hand items over. Belt items are back after closing and reopening the world. An empty hand
  takes a segment's items.
- A press over the belt presses a stack of 3 iron ingots into 3 iron sheets, one per cycle. A fan through a lit
  campfire smokes 2 raw beef waiting at the belt's end.
- Breaking a segment in survival with a pickaxe: the connector drops, the chain becomes air with shafts at both
  ends, its items drop.
- Chute: items fall through three chutes and wait in the lowest over stone (as in Create).
- Andesite funnel on a chest extracts its items.

Fixed on the Mac: breaking a belt left the start pulley as air (the chain was removed over several ticks and the
controller broke itself first; now the chain is walked at once, as `onRemove` does); belt parts dropped nothing
(loot tables in 1.21.1's format, and the parts missing from `mineable/*`); a press on a belt let the rest of a
stack pass after the first item (its results were not locked, as `BeltPressingCallbacks` does).

Not checked yet: a belt funnel pulling items off the belt into its inventory, a second player.

## Milestone 3, part 2

Ported in a cloud session on Linux (x86_64), then checked on macOS (Apple M5), Rust 1.99.1, Python 3.12, against
the same pinned Horizon (`aa27eca`) and horizon-mixins (`3f99a5c`).

| Check | How | Result |
|---|---|---|
| Rule modules (`kinetics.rs`, `processing.rs`, `fan.rs`, `obj.rs`) | `cargo fmt --all --check`, `cargo clippy --all-targets --locked -D warnings`, `cargo test --locked` | 54 tests pass, no warnings (Linux and macOS) |
| The bundle applies and the game compiles | `tools/horizon.py check` and `build` (Python 3.12) | Pass, no warnings (Linux and macOS) |
| Content mod from Create 6.0.10's jar | `tools/prepare_content.py --jar` | Pass; the water wheel's OBJ, MTL and `wheel.json` are in it |
| In the game (offscreen, `horizon-ctl`, macOS) | Rigs built with commands, screenshots, `/data get block` | See below |

Seen in the game:

- Encased fan behind a creative motor at 16 RPM: the current finds water and lava in front of it. 2 raw iron on a
  depot behind lava became 2 iron ingots; 4 gravel on a depot behind water became 1 flint (the recipe's 25 % flint
  and 12.5 % nugget rolls), after about 7.5 s.
- Mechanical mixer driven by a cogwheel beside it (it has no shaft, as in Create) at 32 RPM, over a basin:
  2 andesite and 2 iron nuggets dropped into the basin became 2 andesite alloy in its output slots. Pole, head,
  casing and the basin are drawn.
- Creative motor's value box: "Rotation Speed / Scroll to change" over the crosshair; the mouse wheel steps
  16 → 20 → 24, and ⟲ renders in Horizon's font.
- Wrench: a click on a shaft's south face turns its axis x → y; sneak + click in survival picks it up into the
  inventory.
- Water wheel: its frame and OBJ wheel drawn in spruce.

Not checked yet: the fan pulling (reversed), slabs in the current; the OBJ wheel seen turning; a LAN guest changing
a value box.

## Milestones 1 to 3, part 1

What has been checked, on macOS (Apple M5), Rust 1.99.1, against Horizon Nightly 2026-10-31 (`aa27eca`) and
horizon-mixins `3f99a5c` (the revisions in `compatibility.json`).

| Check | How | Result |
|---|---|---|
| Rule modules (`kinetics.rs`, `processing.rs`) | `cargo fmt --check`, `cargo clippy --all-targets -D warnings`, `cargo test` | 30 tests pass, no warnings |
| The bundle applies and the game compiles | `tools/horizon.py check` / `build` (Python 3.12) | Pass, no warnings |
| Content mod from Create 6.0.10's jar | `tools/prepare_content.py`, then Horizon's `cargo horizon-mod build` | Pass |
| In the game (offscreen, `horizon-ctl`) | Rigs built with commands, screenshots and frame differences, `/data get block` | See below |

Seen in the game:

- Shafts, cogwheels (reversing), large cogwheels, gearbox (sides by source), creative motor, hand crank turn and
  are drawn turning; overstress and breaking on conflict per the unit tests.
- Engineer's goggles: press 128 su at 16 RPM of a creative motor's 262144 su; water wheel 256 su at 8 RPM.
- Depot: an item placed from the top; 3 iron ingots pressed into 3 iron sheets under a press, one per cycle, the
  head going down.
- Millstone: wheat thrown on it milled into wheat flour.
- Water wheel: water flowing over it gives a flow score of 2 and 8 RPM.

Not checked yet: a LAN or dedicated server with a second player, building through the Horizon launcher, and anything against Create running
on NeoForge (no oracle yet: values are compared by hand with Create's code).
