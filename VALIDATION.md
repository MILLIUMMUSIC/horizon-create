# Validation

## Milestone 4, fluids

Ported in a cloud session on Linux (x86_64), Rust 1.99.1, Python 3.12, against the pinned Horizon (`aa27eca`) and
horizon-mixins (`3f99a5c`). **Compiled and unit-tested only: nothing of it has been run in the game yet.**

| Check | How | Result |
|---|---|---|
| Rule modules (`kinetics.rs`, `processing.rs`, `fan.rs`, `obj.rs`, `belt.rs`, `chute.rs`, `funnel.rs`, `fluids.rs`) | `cargo fmt --all --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` | 89 tests pass, no warnings |
| The bundle applies and the game compiles | `tools/horizon.py check --no-audio` (Linux; the container has no ALSA, so the audio crates are left out) | Pass, no warnings |
| `prepare_content.py`'s straight-pipe blockstate | Run by hand on Create 6.0.10's generated `fluid_pipe.json` (from the source tag, not committed) | Each axis gets the two body parts of a straight pipe |
| Content mod from Create 6.0.10's jar, `build`, the game | Not done (no jar or Minecraft resources in the cloud) | — |

What the unit tests cover: NeoForge's `FluidTank` fill and drain; straight pipes and pumps open along their axis;
open ends draining a source and pouring by the bucket (`removeFluidFromSpace`, `provideFluidToSpace`, the
obsidian/cobblestone/stone collisions, water evaporating in the nether, Create's honey from a full hive); a pump
between a water source and a tank putting pressure on both sides (`distributePressureTo`, values per face), then
moving water into the tank at half its speed a tick (16 mB a tick at 32 RPM) once the flows are complete; a stopped
pump moving nothing; a pump turned round emptying the tank into the world, as a source block once the open end holds
a bucket; removing a pipe wiping the pressure; saving pipes and tanks; the tank's fluid box (`FluidTankRenderer`);
the pump as a cog about its facing (no shaft, 4 su, wrench turns it round, saved); the basin's tanks (inputs kept
apart, outputs drained first, `CombinedTankWrapper`'s loss of a second fluid), its fluid boxes (`BasinRenderer`)
and recipes with fluid ingredients (one tank per ingredient, `SizedFluidIngredient`), and which mixing recipes are
supported (dough and cardboard pulp yes; tea, chocolate and honey no: no milk or Create fluids here).

Not checked yet (all in the game, for the Mac pass):

- `prepare_content.py --jar` makes the fluid pipe (straight, chain template, its model per axis), the pump (lightning
  rod template, its cog partial), the tank (glass template, the single window model) with their loot and tags; the
  pipe and pump break with a pickaxe and drop themselves.
- Placing pipes along the clicked axis; the pump's cog turns when a cogwheel beside it turns (and not from a shaft at
  its back); goggles show the pump's 4 su per RPM.
- A pump (any speed) between an open pipe end in water and a tank: the tank fills (watch it go up, about 16 mB a
  tick at 32 RPM, 8000 mB to full) and the water source is drained (and refilled by an infinite pool).
- The pump wrenched round: the tank empties out of the open end, a water source appears there after 1000 mB.
- Lava the same way; water and lava meeting in a pipe breaking it into cobblestone; pouring water onto lava making
  obsidian; water poured in the nether vanishing.
- Pipes longer than 16 blocks from the pump (range), two branches splitting the flow, a pump pushing into another
  pump.
- Breaking a pipe mid-flow stops it; placing a block in front of an open end; closing and reopening the world keeps
  tanks' contents and the pipes keep working (pressure comes back from the pumps).
- Buckets on a tank and on a basin (fill, take back; creative keeps the bucket); the tank's fluid drawn at its level,
  water tinted, lava untinted; the basin's fluids drawn side by side.
- A mixer over a basin with wheat flour and 1000 mB of water piped or poured in making dough; cardboard pulp.
- Cauldrons and beehives drained by an open end.
- A LAN guest seeing tanks and basins fill (the `create:fluids` channel).

## Milestone 4, arm and deployer

Ported in a cloud session on Linux (x86_64), Rust 1.99.1, Python 3.12, against the pinned Horizon (`aa27eca`) and
horizon-mixins (`3f99a5c`). **Compiled and unit-tested only: nothing of this part has been run in the game yet**
(the cloud has no Minecraft resources and no Create jar).

| Check | How | Result |
|---|---|---|
| Rule modules (all of `src/lib.rs`, now with `vault.rs`, `arm.rs`, `deployer.rs`) | `cargo fmt --all --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` | 101 tests pass (26 new), no warnings |
| The bundle applies and the game compiles | `tools/horizon.py check --no-audio` (Linux: the container has no ALSA, so the server build without audio; nothing of the mod is behind that feature) | Pass, no warnings |
| `prepare_content.py`'s new `fit_arms` | Run on a scratch tree with Create 6.0.10's generated `deployer.json` blockstate | Both deployer blocks get their `facing` variants, the extra one a 26.3-format loot table dropping the deployer |
| `tools/horizon.py build`, the content mod from the jar, the game | Not run | — |

What the unit tests cover: vaults joining along their axis, the 3×width length cap, crossing axes, a 2×2 vault and its
slot order, splitting when a block goes and forming again (`ConnectivityHandler`); the arm's full cycle (search,
move, collect, search outputs, deposit), its move time (`speed / 1024` a tick), not taking what no output can take,
a stopped or redstone-locked arm, funnels being deposit-only, the 5-block range, `ArmAngleTarget`'s base angle and
reach, backing up to rest before reaching out, points saved relative to the arm; the deployer's cycle timings
(`getTimerSpeed`, 1000 out, activation, 1000 back, 500), the overflow, the hand offset, its rotation axis,
placement (`axis_along_first` by neighbouring shafts), the wrench, `BeltDeployerCallbacks`, reading deploying
recipes; the arm and deployer as kinetic blocks (a cog drives the arm, the deployer's shaft follows
`axis_along_first`, saved and synced kinds).

Not checked yet (every in-game check, for the Mac pass):

- Content: the vault, arm and both deployer blocks load with their models (the vault turned east–west by `facing`,
  the deployer's twelve variants), the eleven new `hz_part_*` partials bake (the arm's are modelled around the origin,
  some with negative coordinates), each drops itself (the extra deployer block drops a deployer) with the right tool.
- Vault: placing one (its axis from the player, or from a lone vault beside it), hoppers into and out of one block,
  a line of three joining into one vault and a 2×2×2 one; funnels, chutes and arms filling and emptying the whole
  vault; breaking a block of a multiblock (its items drop, the rest joins again); the wrench from above (it leaves its
  vault and turns); right clicks don't open the barrel template's menu; saving and reloading.
- Arm: the arm item's selection (a click on a depot, belt, basin, chute, deployer, millstone or collecting funnel:
  "Extract from" / "Deposit to" over the hotbar, a second click switches; funnels only deposit); placing it takes
  the selection (out of range ones dropped with a message); driven by a cog beside it, 2 su; carrying items from a
  depot to a depot, a belt to a basin, a millstone's outputs to a chute, into a deployer's hand; round-robin
  between outputs; the belt item it reaches for waiting; an empty hand taking the held item back; the arm's base,
  lower and upper arm, claw and grips drawn and moving as `ArmRenderer` does (backing up to rest halfway), its held
  item in the claw, the cog turning; saving and reloading mid-move; a second player seeing it move.
- Deployer: placed facing away from the player (sneaking: towards), `axis_along_first` by a shaft beside it, its
  shaft turning; right click on its front swaps hands, the wrench on its front switches punch/use, elsewhere turns
  it or its shaft; using: placing a held block two blocks out (or against the block there), not when that block is
  already there; punching: breaking the block in front over several cycles with the held tool, its drops in the
  deployer, then taken by a funnel or arm; a deploying recipe on a depot under it (e.g. an axe on exposed chiseled
  copper, an `item_application` such as andesite casing) and on a belt passing under it, one item per cycle, the
  held item used or kept as the recipe says; the pole and hand (pointing, holding, punching) drawn going out and
  back with its held item; its state saved beside the world (`data/create_deployers_*.txt`) and its items dropped
  when it is broken.
- Regressions: belts, chutes and funnels into chests, depots and other inventories (their insertion and extraction
  now look for vaults and deployers first); presses over belts and depots (a deployer is looked for when no press is
  there).

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
