# Porting Create to Horizon: the plan

Create 6.0.10 (1.21.1, NeoForge): ~2,000 Java files, plus Flywheel (instanced rendering), Ponder (scenes) and
Registrate (registration). Horizon is Minecraft 26.3 in Rust: no JVM, so every line of behaviour is ported to Rust.

`horizon-port-pack check` on the jar, the starting inventory:

| Carried by Horizon already (data) | Left to port (code) |
|---|---|
| 542 blocks (template blocks), 142 items, 931 recipes, 640 loot tables, 73 worldgen files, 1,293 textures | 101 blocks with no vanilla template, 130 block entities, 23 menus, 26 screens, 21 renderers, 11 entities, 5 recipe types (879 recipes), 13 particle types, 3 network messages, 30 events, 64 Mixins |

## Architecture

- **Rules in plain Rust** (`kinetics.rs` and the modules after it): a port of Create's classes over a small `Level`
  trait, unit-tested with `cargo test`, no Horizon types. Block entities are rows keyed by position, one table per
  dimension, as `TorquePropagator` keeps one network map per level.
- **Glue** (`glue.rs`): Horizon's native hook points (block changes, level ticks, right clicks, world lifecycle,
  the mod message channel) plus source mixins where no hook exists yet (drawing). The bundle pins one Horizon
  Nightly; each hook point we need upstream is one less pinned mixin.
- **Content from the player's jar** (`tools/prepare_content.py`): Create's assets are All Rights Reserved, so they
  are never redistributed.
- **Fidelity**: Create's code is the spec. Where the port knowingly differs, the code says so. Next step for
  proof: a NeoForge oracle (Horizon ships one for data, `tools/neoforge_oracle.py`) and per-feature harnesses
  comparing tick-by-tick speeds and stress.

## Milestones

1. **Kinetic core** (done in this repo): propagation, networks and stress, generators (creative motor, hand
   crank), relays (shaft, cogwheels, gearbox), a consumer (press), saving, client sync, rotating parts drawn.
2. **Visual completeness of kinetics** (done): shaft halves on motors and gearboxes (gearbox halves turn by
   their source's side, as `GearboxRenderer`), the hand crank's base and handle (the handle chases the network's
   speed, as `HandCrankBlockEntity.independentAngle`), the press's shaft and head (at rest), engineer's goggles
   overlay (stress impact, generator capacity, network stress, overstressed). Left for later: encased shafts and
   cogs (no vanilla template has `top_shaft`/`bottom_shaft`), rotation particles and sounds.
3. **First processing chain**, part 1 (done): the depot (held item and 8-slot output buffer as Horizon container
   slots: hoppers, breaking and saving work as for any container; items placed from the top, falling on it, given
   back to the player), the press over a depot or over items on the ground (`PressingBehaviour`'s 240-tick cycle,
   `create:pressing` recipes, head animation), the millstone (`create:milling`, items thrown on it, outputs to an
   empty hand), the water wheel (flow score from the fluid's flow around it, 8 RPM, 32 su).
   Part 2 (checked in the game in part: see VALIDATION.md):
   - Encased fan (`fan.rs`, `AirCurrent`): 2 su, shaft behind it; its current's reach from its speed (3 to 20
     blocks), stopped by full blocks and reaching into partial ones (`findMaxDepth`), passing `create:fan_transparent`
     blocks; segments of splashing (water), haunting (soul fire, lit soul campfire), smoking (fire, lit campfire)
     and blasting (lava), by Create's priorities; item entities in it pushed along and processed after 150 ticks
     per 16 items (`create:splashing`, `create:haunting`, vanilla smoking, smelting/blasting, items without a
     smelting result burnt unless fire resistant); a depot's held item in it (or a block below a horizontal
     current) processed the same way. Rebuilt every 30 ticks and when its speed changes. The propeller partial
     turns at five times the speed (80 to 1280). Not yet: pushing mobs and players, burning and hurting them,
     haunting horses, the air particles and sound, crafting remainders of the results, chutes and nozzles.
   - Mechanical mixer and basin: the mixer (4 su, a small cog, needs 30 RPM) runs `MechanicalMixerBlockEntity`'s
     40-tick cycle with its `processingTicks` (`log2(512 / speed) * 15 + 1` at the bottom) over a basin two below;
     the basin is a mod block entity with 9 input and 9 output slots (hoppers in and out, items thrown in, an empty
     hand takes all). `create:mixing` recipes with items only and no heat (andesite alloy from iron or zinc
     nuggets...), most ingredients first, results rolled into the output slots (all must fit), mixing again while
     another batch is there. Pole and head partials go down and up, the head whisking at the head's speed.
     Skipped and noted: fluids (dough, mud, chocolate, tea, honey, lava, cardboard pulp: fluid ingredients or
     results), heat (no blaze burner yet: brass and the other heated recipes), shapeless crafting recipes in the
     mixer (`allowShapelessInMixer`), potion brewing, the basin's filter slot, pouring out sideways (spouts: the
     template has no `facing`, so it keeps its outputs), its fluid tanks and their rendering. Basins draw their
     items as `BasinRenderer` places them (no ingredient spin yet).
   - Value boxes: the creative motor's speed (`ScrollValueBehaviour`, `MotorValueBox` on its sides): looking at
     the box shows its value over the hotbar, the mouse wheel changes it (Create 0.5's steps: 1, 4, 16, 64; never
     0), sent on the mod channel `create:values` and checked by the server (reach, game mode, range). Create 6
     opens a value board on a held right click instead (engine gap below). Right-click stepping is kept.
   - Wrench (`IWrenchable`): turns shafts, cogwheels, gearboxes (about the clicked face), motors, cranks, water
     wheels, fans (a quarter turn off the face's axis) and presses (from above or below); sneaking takes ours into
     the inventory (not in creative), a block entity's items dropped. Through `native_use_item_on`, and through
     `native_right_click_block` for blocks whose template has a use of its own (press, mixer, millstone, depot,
     basin), which would take the click first. Not yet: sounds, the wrench's own render and attack on minecarts.
   - Water wheel model (`obj.rs`): an OBJ/MTL loader reads `wheel.json` (its `model`, `flip_v`, texture parents),
     `water_wheel.obj` and `.mtl` from the game's resources (the content mod made from the player's jar) at
     runtime, client side, and draws it turned with `rotateToFaceVertical` and spun with the network, in spruce
     (the default material: oak planks and log swapped for spruce's). The block's own model (its frame) stays in
     the chunk mesh. Not yet: other materials (right-clicking planks), the large water wheel.
4. **Logistics**. Part 1 (**compiled and unit-tested only, not yet run in the game**):
   - Mechanical belt (`belt.rs`: `BeltBlock`, `BeltBlockEntity`, `BeltInventory`, `TransportedItemStack`,
     `BeltHelper`, `ItemHandlerBeltSegment`, `BeltConnectorItem`; `logistics.rs`): the belt connector joins two
     shafts of one axis in a line (horizontal, or sloped one up per block), up to 20 blocks, through replaceable
     blocks and shafts (which become pulleys); the first click remembers the shaft (an action bar line says so:
     there is no preview line). Each segment is a kinetic block (`Kind::Belt`): pulleys connect to shafts along
     the belt's axis, segments of one belt pass their speed on (`propagateRotationTo`), sloped ones diagonally; no
     stress impact (`CStress.setNoImpact`). The controller (its start) keeps the items: they move at
     `speed / 480` blocks a tick, keep a block apart, stop at walls (`BLOCKED`), hand over to the next belt, a depot,
     a chute or a collecting funnel facing up (`INSERT`) and are thrown off the end otherwise (`EJECT`). Items
     dropped on a belt go on it (`updateEntityAfterFallOn`), an empty hand takes a segment's items back, a shaft on
     a middle segment makes a pulley. A press over a belt holds and presses its items one at a time with the
     depot's callbacks (`BeltPressingCallbacks`, shared with the depot now); a fan's current processes items on
     belt segments (`handleProcessingOnAllItems`) as on depots. Breaking a segment breaks the chain into shafts
     (at pulleys) and air and drops the items (`onRemove`, `ejectAll`); it drops the connector, and a shaft where
     it had a pulley. Drawn as `BeltRenderer` does: its partials turned its way, their texture swapped for the
     scrolling sprite and moved with the speed (`shiftUVScrolling`), the pulley turning, and the items lying
     along it, moved on between the server's updates (`create:belts` channel). Saved beside the world
     (`data/create_logistics_*.txt`). Not yet: vertical and sideways belts, casings and covers, dyeing, the
     wrench (`BeltSlicer`), extending belts with the connector, entities and players riding belts
     (`BeltMovementHandler`), upright items (`create:upright_on_belt`), tunnels, crushers, the item's
     stacked look (`renderItem`'s count), sounds.
   - Chute (`chute.rs`: `ChuteBlockEntity`'s item and `handleDownwardOutput`/`handleInputFromAbove`; a mod block
     entity): items fall at 4 blocks a second, into the inventory below (containers, depots, belt segments), into
     the chute below, or out of the bottom onto the ground; an empty chute takes up to 16 from the inventory above
     and items falling on top; its item drawn at its height. Not yet: fans pushing or pulling through chutes
     (`push`/`pull`), diagonal chutes, window and intersection shapes, smart chutes.
   - Andesite funnel (`funnel.rs`; no block entity): extracting (placed normally) puts one item every 8 ticks
     from the inventory behind in front of it, unless an item still lies there; collecting (placed sneaking, or
     wrenched) puts items at its opening into the inventory behind; a horizontal one over a belt is a belt
     funnel (`getShapeForPosition`): pulling items off the belt into the inventory behind
     (`BeltFunnelInteractionHandler`), or pushing one at a time onto it, by its shape and the belt's movement. The
     wrench switches it. Not yet: redstone (paused when powered), the belt funnel's own model and flap, clicking an
     item into it, brass funnels and filters.
   Part 2 and later: tunnels, mechanical arm, deployer, item vaults, brass funnels, fluid pipes and tanks.
5. **Contraptions**: moving block structures (pistons, bearings, gantries, minecart contraptions) — need
   entities with block collision, their own rendering and network sync. The biggest engine gap; expect
   upstream Horizon work (hook points for custom entities and their rendering).
6. **Trains**: track graph, signals, stations, schedules. Depends on 5.
7. **Everything else**: Ponder scenes, schematics and the schematicannon, decoration sets, Create's
   compatibility with other mods.

## Engine gaps to raise upstream (as hook point requests)

- A mod-drawn screen outside a container menu, with a free cursor: Create 6's value settings board (hold right
  click on a value box, pick a value with the mouse) has nowhere to open. The value box takes the mouse wheel
  instead (`native_wheel`, as Create 0.5 did).
- Block placement by a template: blocks are placed as their template places itself (a lightning rod faces the
  clicked face; Create's fan faces away from the player), until a mod can answer `getStateForPlacement`.
- Basins with a `facing` (pouring sideways onto belts and depots) need a template with a hopper's facing and a
  block entity; none has both. Basin and depot shapes stay the enchanting table's (12/16 high), so items rest on
  top instead of inside.
- Container insertion rules: hoppers fill a basin's input slots as any container's, not as `BasinInventory`
  (one slot per item kind).
- Pushing and hurting mobs and players from a native mod's server tick (the fan's current): `native_push_mob`
  reaches mobs only, players not at all.

- Draw hook for mod block entities (today: a mixin into `transient_block_geometry`), and a per-state "hide from
  the chunk mesh" flag (today: blank blockstate models plus hidden `hz_*` blocks holding the real models).
- Block templates with arbitrary property sets (encased cogs, belts, funnels have no vanilla template).
- Sneaking and the clicked hit position in `native_right_click_block` (Create's crank turns backwards when
  sneaking; value boxes need the hit position).
- Custom entities with block collision and rendering (contraptions, trains).
- **Recipes in subfolders don't load** (`Recipes::load` reads `data/<ns>/recipe/*.json` only; vanilla reads the
  folder recursively): 1198 of Create's recipes, crafting included. Worked around by flattening them
  (`pressing/iron_ingot` → `pressing.iron_ingot`) in `prepare_content.py`.
- `native_recipes_of_type` and `native_recipe_ingredient` answer only inside a player's hook: a block entity's tick
  gets nothing. Worked around by reading `Game::recipes()` directly.
- Block shapes can only be keyed by a template property: a template with none (the enchanting table that hosts the
  depot and millstone block entities) keeps its own shape.
- No template has a belt's properties (`facing`, `slope`, `part`, `casing`): a block per part on a grindstone's
  template, its `face` holding the slope; vertical and sideways belts and belt casings need more. Belt shapes
  follow `face` alone (no `facing`): sloped belts are a lower half block. Funnels have the same problem
  (`extracting`, `powered`): a block per `extracting` on a lightning rod's template, and belt funnels have no
  block (a horizontal funnel over a belt acts as one, with the funnel's model).
- Mod block entities only on templates that have one: belts and funnels have none, so their items and timers live
  beside the world in the native mod (only an item's id, count and damage are saved), and hoppers can't put
  items onto belts.
- No native hook to take items out of a container (`native_insert_item` puts them in, and can't simulate):
  funnels and chutes take from chests and barrels (Horizon's `BlockEntity::Chest`), mod block entities and belts
  only, with no comparator update; a chute's "will it fit" simulation into a container is hopeful.
- No redstone signal query from a native mod: funnels are never paused by redstone.
- No `entityInside` / `updateEntityAfterFallOn` hook for mod blocks: belts, funnels and chutes look for item
  entities each tick.
- Mod containers can't say which stack a slot takes: hoppers stack onto a chute's item (`ChuteItemHandler` takes
  only into an empty chute).
- **No shared item, fluid or energy handlers between mods.** NeoForge's capabilities (`Capabilities.Item.BLOCK`,
  fluids, energy) are how Create talks to Mekanism, AE2 and others. Horizon has only a block entity's fixed
  container slots (hoppers and other mods can use them: this is how the depot, millstone and basin are reached
  today); fluids and energy have nothing. Horizon's `docs/native-hooks-neoforge.md` proposes a
  `block_capability(pos, kind, side, out)` hook, not built as of `aa27eca`. Plan: keep each machine's inventory
  and tank access behind one small trait in the port, so moving to that hook (or contributing it upstream) touches
  one place; until then, a shared registry module with the proposal's signature is the fallback if another
  ported mod (Mekanism, AE2) must reach Create's fluids.
- Loot tables a native port writes itself must be in 26.3's format (`"condition": {"type": ...}` per pool), as
  `horizon-port-pack` upgrades carried ones; 1.21.1's `conditions` list drops nothing. And a template's required
  tool applies to every block made from it: extra blocks (belt parts, the collecting funnel) need their base
  block's `mineable/*` tags or they drop nothing (`prepare_content.share_block_tags`).
