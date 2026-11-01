# Create for Horizon

[Create](https://github.com/Creators-of-Create/Create) (the Minecraft mod, 6.0.10 for 1.21.1) ported to
[Horizon](https://github.com/horizon-rust/horizon), Minecraft: Java Edition rewritten in Rust. Horizon runs no
Java, so Create's code is ported to Rust and built into the game as a **native mod** (a
[horizon-mixins](https://github.com/horizon-rust/horizon-mixins) bundle), and its blocks, items, recipes, models
and textures come from **your own Create jar** as a content mod.

Status: **milestones 1, 2 and 3 (part 1): kinetics, their visuals, and the first machines**, checked in the game.
**Milestone 3 part 2** (encased fan, mixer and basin, the creative motor's value box, the wrench, the water wheel's
OBJ model) is checked in the game in part (see VALIDATION.md for what was seen). **Milestone 4 part 1** (mechanical belt, chute,
andesite funnel) is checked in the game. **Milestone 4, fluids** (straight fluid pipes, mechanical pump,
single fluid tanks, basin fluids) and **milestone 4 part 2** (item vault, mechanical arm, deployer) are ported and
unit-tested; VALIDATION.md says what of them was seen in the game. See
[docs/PLAN.md](docs/PLAN.md) for what is ported and what comes next, and [VALIDATION.md](VALIDATION.md) for what was
checked how.

## What is in milestone 1

| Create | Here |
|---|---|
| `RotationPropagator`, `KineticNetwork`, `TorquePropagator`, `KineticBlockEntity`, `GeneratingKineticBlockEntity` | `mixins/create/kinetics.rs`, a line-by-line port with unit tests |
| Shaft, cogwheel, large cogwheel, gearbox, creative motor, hand crank, mechanical press (as a stress consumer) | Turn, transfer speed (gears reverse, large/small ratio 2:1, gearboxes turn sideways shafts), overstress, break on conflicts |
| `KineticBlockEntityRenderer` (Flywheel instances) | Rotating parts drawn each frame with Horizon's own block model baker |
| Block entity NBT | Saved per dimension beside the world (`data/create_kinetics_*.txt`) |
| Client sync (`sendData`) | Horizon's mod channel `create:kinetics` (hook API 40) |
| `GearboxRenderer`, `CreativeMotorRenderer`, `HandCrankRenderer`, `MechanicalPressRenderer` | Partial models (`shaft_half`, crank handle, press head) turned with `CachedBuffers.rotateToFace` |
| Engineer's goggles overlay | Stress impact, generator capacity, network stress, overstressed, beside the crosshair |
| `DepotBehaviour`, `PressingBehaviour`, `MillstoneBlockEntity`, `WaterWheelBlockEntity`, processing recipes | `mixins/create/processing.rs` (rules, tested) and `machines.rs` (Horizon block entities with item slots, rendering of held items) |
| `AirCurrent`, `FanProcessing`, the fan processing types, `EncasedFanRenderer` (not yet run in the game) | `mixins/create/fan.rs` (rules, tested), `machines.rs` (items and depots in the current), `glue.rs` (propeller) |
| `MechanicalMixerBlockEntity`, `BasinBlockEntity`, `BasinRecipe`, `MechanicalMixerRenderer`, `BasinRenderer` (not yet run in the game) | `processing.rs` (cycle, recipe matching, tested), `machines.rs` (basin block entity), `glue.rs` (cog, pole, head) |
| `ScrollValueBehaviour` on the creative motor, `IWrenchable` (not yet run in the game) | `kinetics.rs` (value box, steps, wrench turns, tested), `glue.rs` (mouse wheel, `create:values` channel, wrench) |
| `WaterWheelRenderer` with NeoForge's OBJ loader (not yet run in the game) | `mixins/create/obj.rs` (OBJ/MTL parsing, tested) and `glue.rs` (the wheel in spruce, turned with `rotateToFaceVertical`) |
| `BeltBlock`, `BeltBlockEntity`, `BeltInventory`, `TransportedItemStack`, `BeltHelper`, `BeltConnectorItem`, `BeltRenderer` (not yet run in the game) | `mixins/create/belt.rs` (chain, movement, items, connector, tested), `logistics.rs` (items in the world, saving, drawing), `glue.rs` (the belt's partials with the scrolling texture) |
| `ChuteBlockEntity` (falling items only), `FunnelBlockEntity`/`BeltFunnelInteractionHandler` for the andesite funnel (not yet run in the game) | `mixins/create/chute.rs`, `funnel.rs` (rules, tested), `logistics.rs` |
| `FluidTransportBehaviour`, `PipeConnection`, `FluidNetwork`, `FluidPropagator`, `OpenEndedPipe`, `PumpBlockEntity`, `FluidTankBlockEntity` (single tanks), `FluidTankRenderer`, the basin's tanks, fluid `create:mixing` recipes (not yet run in the game) | `mixins/create/fluids.rs` (rules, tested; fluid holders behind the `FluidAccess` trait), `fluid_glue.rs` (pipes, pumps and tanks in the world, buckets, drawing), `processing.rs` (fluid ingredients) |
| `ItemVaultBlock`/`ItemVaultBlockEntity` and `ConnectivityHandler`, `ArmBlockEntity`/`ArmInteractionPoint`/`ArmAngleTarget`/`ArmRenderer`, `DeployerBlockEntity`/`BeltDeployerCallbacks`/`DeployerRenderer` (compiled and unit-tested only) | `mixins/create/vault.rs`, `arm.rs`, `deployer.rs` (rules, tested), `arms.rs` (block entities, the item handler, the deployer's actions, drawing) |

Hand crank: right click to turn it (10 ticks per click, as in Create). Creative motor: look at the value box on one
of its sides and turn the mouse wheel (Create 6 opens a value board there, which Horizon can't show yet); right click
still steps its speed 16 → 32 → 64 → 128 → 256 → −256 …. Wrench: right click turns our blocks, sneak + right click
picks them up. The mixer needs at least 30 RPM (a creative motor's default 16 is too slow). Belt connector: right
click a shaft, then another one in line with it (same axis, horizontal or one up per block); an empty hand takes
items off a belt. Andesite funnel: placed normally it extracts, placed sneaking (or wrenched) it collects.
Fluid pipes are straight, along the clicked face's axis like a chain. A mechanical pump is driven by a cogwheel beside
it (it has no shaft) and pushes out of its front, whatever way it turns (the wrench turns it round); an open pipe end
in water draws it, one in the air pours it. Buckets fill and empty fluid tanks and basins.

Mechanical arm: before placing it, right click the depots, belts, basins, chutes, deployers, millstones or collecting
funnels it should use (once: take from, twice: put into); drive it with a cogwheel beside it. Deployer: right click
its front to give it an item, wrench its front to switch between using and punching. Item vault: sneak to place
blocks against it (a plain right click on it does nothing, as the barrel template's menu is stopped).

## Layout

```text
mixins/create/
  horizon-mixin.toml   the bundle: modules and injections, pinned to Horizon Nightly 2026-10-31 (aa27eca)
  kinetics.rs          Create's kinetic rules, the wrench's turns, the motor's value box, no Horizon types
  processing.rs        pressing and mixing cycles, processing and basin recipes, millstone and water wheel rules
  fan.rs               the encased fan's air current and fan processing rules, no Horizon types
  obj.rs               OBJ/MTL model parsing (the water wheel's), no Horizon types
  belt.rs              the mechanical belt: chain, movement, items, belt funnels, connector, no Horizon types
  chute.rs, funnel.rs  the chute's falling item and the andesite funnel's modes, no Horizon types
  fluids.rs            fluid pipes, networks, pumps' pressure, open ends, tanks, no Horizon types
  fluid_glue.rs        Horizon's side of fluids: pipes, pumps and tanks in the world, buckets, drawing
  vault.rs, arm.rs,    the item vault's multiblock, the mechanical arm's phases and angles, the deployer's
  deployer.rs          cycle and belt callbacks, no Horizon types
  arms.rs              Horizon's side of vaults, arms and deployers: block entities, the item handler, drawing
  logistics.rs         Horizon's side of belts, chutes and funnels: items in the world, saving, drawing
  machines.rs          Horizon's side of the machines: depot, millstone and basin block entities, press, mixer
                       and fan work, water wheel flow
  glue.rs              Horizon's side: hooks, per-dimension state, network messages, drawing
  *.rs                 one small helper per injection
src/lib.rs             the rule modules as a library, for `cargo test`
tools/
  prepare_content.py   builds the content mod from your Create jar (via horizon-port-pack)
  horizon.py           export pinned Horizon → apply the bundle → cargo check/test/build
  package.py           writes the .hnmod (from the native mod template, not yet tried on this bundle)
```

## Build and run

Needs Rust 1.99.1, Python 3.12 (for `tools/horizon.py`), Git, a Horizon checkout and a horizon-mixins checkout at
`3f99a5c` (the revision the Horizon launcher builds native mods with).

```sh
cargo test                                   # the kinetic core, standalone
HORIZON_MIXINS=../horizon-mixins python3 tools/horizon.py build --horizon ../horizon
```

The content mod, from your own Create jar (`create-1.21.1-6.0.10.jar` from Modrinth or CurseForge) and the
prepared resources of the same Horizon version (the launcher keeps them in
`<launcher data>/versions/<version>/.local/horizon-client/26.3`):

```sh
cargo build --release -p horizon-port-pack --manifest-path ../horizon/Cargo.toml
HORIZON_PORT_PACK=../horizon/target/release/horizon-port-pack \
  python3 tools/prepare_content.py --jar create-1.21.1-6.0.10.jar --resources <prepared 26.3>
target/horizon-cargo/fast/horizon-client --resources <prepared 26.3> --dev-mod target/content/create
```

Add `--offscreen` to run it with no window (driven by `horizon-ctl`, screenshots still work).

In the launcher: put the content mod folder in the game folder's `horizon-mods/` (switch it on under Options →
Mods) and this repository's `mixins/create` in `horizon-native-mods/`; the launcher builds the game with it.

## Licences

Code: MIT. The Rust port follows Create's code, which is MIT (© The Create Team). Create's **assets are All Rights
Reserved**: none are in this repository, and `prepare_content.py` reads them from the player's own jar, for local use
only, like Horizon reads Minecraft's from the player's own game (the water wheel's OBJ model included, which the
native mod reads from the content mod at runtime). Build tooling is adapted from the MIT-licensed
Horizon native mod template. Not affiliated with the Create team, Horizon, Mojang or Microsoft.
