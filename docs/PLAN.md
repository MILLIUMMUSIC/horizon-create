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
   Part 2 (**compiled and unit-tested only, not yet checked in the game**):
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
4. **Logistics**: belts (their own model and item transport), chutes, funnels, tunnels, mechanical arm,
   deployer, item vaults, fluid pipes and tanks.
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
