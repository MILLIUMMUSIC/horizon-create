# Create for Horizon

[Create](https://github.com/Creators-of-Create/Create) (the Minecraft mod, 6.0.10 for 1.21.1) ported to
[Horizon](https://github.com/horizon-rust/horizon), Minecraft: Java Edition rewritten in Rust. Horizon runs no
Java, so Create's code is ported to Rust and built into the game as a **native mod** (a
[horizon-mixins](https://github.com/horizon-rust/horizon-mixins) bundle), and its blocks, items, recipes, models
and textures come from **your own Create jar** as a content mod.

Status: **milestones 1, 2 and 3 (part 1): kinetics, their visuals, and the first machines**, checked in the game.
**Milestone 3 part 2** (encased fan, mixer and basin, the creative motor's value box, the wrench, the water wheel's
OBJ model) is ported, unit-tested and compiles into the game, but **has not been run in the game yet**. See
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

Hand crank: right click to turn it (10 ticks per click, as in Create). Creative motor: look at the value box on one
of its sides and turn the mouse wheel (Create 6 opens a value board there, which Horizon can't show yet); right click
still steps its speed 16 → 32 → 64 → 128 → 256 → −256 …. Wrench: right click turns our blocks, sneak + right click
picks them up. The mixer needs at least 30 RPM (a creative motor's default 16 is too slow).

## Layout

```text
mixins/create/
  horizon-mixin.toml   the bundle: modules and injections, pinned to Horizon Nightly 2026-10-31 (aa27eca)
  kinetics.rs          Create's kinetic rules, the wrench's turns, the motor's value box, no Horizon types
  processing.rs        pressing and mixing cycles, processing and basin recipes, millstone and water wheel rules
  fan.rs               the encased fan's air current and fan processing rules, no Horizon types
  obj.rs               OBJ/MTL model parsing (the water wheel's), no Horizon types
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
