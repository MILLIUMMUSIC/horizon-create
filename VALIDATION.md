# Validation

## Milestone 4, part 3: brass funnel and belt tunnels (cloud)

Ported in a cloud session on Linux (x86_64), Rust 1.99.1, Python 3.12, against the pinned Horizon (`aa27eca`) and
horizon-mixins (`3f99a5c`). **Not built with `tools/horizon.py build` and not run in the game.**

| Check | How | Result |
|---|---|---|
| Rule modules (all of `src/lib.rs`, now with `tunnel.rs`) | `cargo fmt --all --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` | 123 tests pass (8 new), no warnings |
| The bundle applies and the game compiles | `python3.12 tools/horizon.py check --horizon ../horizon --no-audio` (Linux: the container has no ALSA) | Pass, no warnings |
| `prepare_content.py`'s funnel and tunnel blocks | `fit_logistics` run on a scratch tree with Create 6.0.10's generated `andesite_tunnel`, `brass_tunnel`, `brass_funnel` and `andesite_funnel` blockstates (from the source tag, not committed) | Each tunnel shape gets a block with `axis=x/y/z` variants (y as x), its loot table in 26.3's format and its base's `mineable` tag; the brass funnel gets its extracting and collecting blocks as the andesite one |
| `tools/horizon.py build`, the content mod from the jar, the game | Not run | — |

What the unit tests cover: tunnel block names and states; `getTunnelState` (T left/right and cross from belts across
it and side inputs, a belt along it not counting, windows between tunnels, a closed window kept); sides and flaps of
straight, T and cross tunnels, flaps closed by a neighbouring tunnel; `canSurvive`; connected brass tunnels
(`getAdjacent`, `isConnected`, `hasDistributionBehaviour`), `gatherValidOutputs` (own sides first, the belt's end,
not into a solid block, nothing on a stopped belt); `distribute` in Split (nine items over three belts, three each;
overflow from a nearly full target to the others) and Round robin (whole stacks in turn, waiting on a full target);
the tick's timing (targets, then two ticks for a new item); a stack entering an andesite cross giving one item to
each side and going on, a single item passing, a brass tunnel taking a stack and holding the next item back at
0.99; the brass funnel's blocks, filter (`test`, set, cleared), amount (a full stack, one for an andesite
extractor) and filter slot positions.

Not checked yet (part 3):

- Crafting the brass funnel and both tunnels (their recipes should be carried from the jar as the andesite funnel's).
- Brass funnel: placing (extracting, sneaking for collecting), breaking with a pickaxe and dropping itself; the
  filter set and cleared by right clicks on its front (any side of an upright one), and not by clicks on other
  faces; the filter item drawn in its slot (position, tilt, size, flat items against block items) on horizontal
  and vertical funnels; extracting a whole stack of the filtered item from a chest with mixed items; collecting
  only the filtered item; as a belt funnel, pulling only the filtered item off the belt (others pass, or wait in
  front of a funnel facing them) and pushing whole stacks onto it; an arm depositing into it; the filter kept over
  a save and reload.
- Andesite tunnel: placing on a belt (the straight block turning to the belt's axis), its shape changing as belts,
  depots and funnels come and go beside it, the window between two tunnels and the wrench closing it; splitting a
  stack onto belts and depots at its sides; breaking off when the belt below goes; drops; models and collision.
- Brass tunnel: a T over a belt with a side belt splitting stacks; three tunnels side by side over parallel belts
  sharing stacks evenly; Round robin after the wrench (the action bar line, the whole group switching); a tunnel
  placed beside a group; an empty hand taking the stacks; a broken tunnel dropping its stack; items thrown off a
  belt's end through it; stacks and modes kept over a save and reload; brass funnels on the side belts filtering
  what a side takes.
- Regressions: andesite funnels and chutes (they now gather an item from every slot, up to a stack, as Create's
  `ItemHelper.extract`), belts without tunnels, arms depositing into andesite funnels.

## Milestone 4, part 2 and fluids: checked on the Mac (macOS, offscreen)

Both cloud PRs merged together, then built with `tools/horizon.py build` (Python 3.12) against the pinned Horizon
(`aa27eca`) and horizon-mixins (`3f99a5c`), the content mod made from Create 6.0.10's jar, and run offscreen in a
creative world. 115 unit tests pass, clippy has no warnings.

Fixed here:

- **The arm and the pump had the same wire code** (12) in `kinetics.rs`'s `create:kinetics` sync once both PRs were
  in: the arm is 14 now.
- **The content mod didn't build**: horizon-port-pack leaves out `mechanical_arm` (no template has its properties)
  and `fit` read its declaration as if it were carried; containers it leaves out are declared from nothing now.
- **Nothing drew a tank's or basin's fluid**: the fluid's sprite was looked up as `minecraft:block/water_still`, and the
  atlas keeps vanilla's sprites without the namespace (`block/water_still`); and the tint was packed as 0xRRGGBB
  where the vertex takes RGBA8 (red in the low byte), which turned water orange.
- **A right click on a deployer gave its item straight back**, and every Create block's click could run twice:
  Horizon's client doesn't run the mod, so for a block whose template has no use it sends the off hand's use too,
  where Create's client answers SUCCESS and sends none. `glue::right_click` drops the off hand's click on a block the
  main hand's click just took (same tick, same block).
- **The deployer's andesite casing never came** (nor any recipe naming `c:stripped_logs`, `c:cobblestones`,
  `c:stripped_woods`...): NeoForge's convention tags for vanilla items are added to the content mod
  (`CONVENTION_TAGS`); tags only other mods fill stay empty, as with NeoForge.
- **Vaults placed by a command never joined**: the vault's block entity starts `Uninitialized` now
  (`ItemVaultBlock.onPlace` runs however the block is placed), not only when a player places it.

Seen working after the fixes:

- Fluids: a mechanical pump driven by a cogwheel beside it (−16 RPM) between an open pipe end in a water source and a
  fluid tank: the source drained and its 1000 mB went into the tank within a few seconds, saved and loaded with the
  world (`create_fluids_*.txt`). Buckets: a water bucket fills the tank and an empty one takes it back in survival;
  in creative the bucket is kept and the tank still fills or drains, as Create's `FluidHelper` does. The water is
  drawn in the tank's window, tinted.
- Arm: the arm item selects a depot ("Extract from depot") and another ("Deposit to depot", after a second click),
  the placed arm takes both points, turns with a cogwheel beside it and carries iron ingots from one depot to the
  other; its base, arms, claw and cog are drawn.
- Deployer: placed facing down by a command over a depot, its shaft turned by a motor along Z; a right click on its
  front gives it andesite alloy; it waits while the depot is empty (as Create's `shouldActivate` with a transported
  item handler below) and applies the alloy to stripped oak logs on the depot, making andesite casing (8 alloy left
  6 after two logs); pole and hand drawn, its state saved in `create_deployers_*.txt`.
- Vault: three blocks in a row join; a hopper fills the first block, an andesite funnel on the last one takes the
  iron out of the whole vault.
- Crafting: horizon-port-pack drops the crafting recipes of the blocks it leaves out; `prepare_content.py --jar` now
  carries the ones for blocks it declares (andesite funnel, fluid pipe, fluid tank, item vault, mechanical arm,
  deployer), and NeoForge's vanilla convention tags those and the carried recipes name (`c:barrels/wooden`,
  `c:dyes/*`, `c:chests/wooden`...). In survival, alloy over dried kelp in the inventory grid gave 2 andesite
  funnels, and copper sheet, barrel, copper sheet at a crafting table a fluid tank.

## Milestone 4, fluids (cloud)

Ported in a cloud session on Linux (x86_64), Rust 1.99.1, Python 3.12, against the pinned Horizon (`aa27eca`) and
horizon-mixins (`3f99a5c`). Then checked on the Mac: see above.

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

Not checked yet (fluids):

- The pump's cog turning, seen; goggles on the pump; the pipe and pump breaking with a pickaxe and dropping
  themselves; placing pipes by hand along the clicked axis.
- The pump wrenched round emptying the tank out of the open end; lava; water and lava meeting in a pipe; pouring onto
  lava; the nether.
- An infinite pool feeding the pump for long (the test pool had air under it, so its source didn't come back: a
  test rig problem, not the mod's), the tank filling to 8000 mB.
- Pipes longer than 16 blocks, two branches, a pump into a pump, breaking a pipe mid-flow, a block in front of an open
  end.
- Buckets on a basin and its fluids drawn; a mixer making dough from flour and piped water; cardboard pulp.
- Cauldrons and beehives drained by an open end; a LAN guest seeing tanks and basins fill.

## Milestone 4, arm and deployer (cloud)

Ported in a cloud session on Linux (x86_64), Rust 1.99.1, Python 3.12, against the pinned Horizon (`aa27eca`) and
horizon-mixins (`3f99a5c`). Then checked on the Mac: see above.

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

Not checked yet (arm, deployer, vault):

- Crafting the fluid pipe, item vault, mechanical arm and deployer at a table (their recipes are carried from the
  jar now as the funnel's and tank's, which were crafted: see above).
- Vault: placing by hand (its axis from the player or a lone vault beside it), a 2×2×2 vault, chutes and arms on a
  vault, breaking a block of a multiblock, the wrench from above, right clicks not opening the barrel's menu.
- Arm: belts, basins, millstones, chutes, deployers and funnels as points; round robin; out-of-range points on
  placing; an empty hand taking its item; the held item drawn in the claw while moving; saving mid-move; a second
  player.
- Deployer: placing by hand (facing, sneaking, `axis_along_first` by a shaft); the wrench switching punch/use;
  using (placing a held block), punching (breaking the block in front); deploying on a belt; an axe on copper; the
  pole and hand compared closely against Create's (the pole stands out of the back while retracted, as the
  partial's 9 px behind the block suggest, but its movement wasn't compared frame by frame); dropping its items when
  broken.
- Regressions: belts, chutes and funnels into chests and depots, presses over belts and depots (they now look for
  vaults and deployers first). The combined pass below was made before this merge.

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
