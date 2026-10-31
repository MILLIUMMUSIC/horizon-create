# Create for Horizon

[Create](https://github.com/Creators-of-Create/Create) (the Minecraft mod, 6.0.10 for 1.21.1) ported to
[Horizon](https://github.com/horizon-rust/horizon), Minecraft: Java Edition rewritten in Rust. Horizon runs no
Java, so Create's code is ported to Rust and built into the game as a **native mod** (a
[horizon-mixins](https://github.com/horizon-rust/horizon-mixins) bundle), and its blocks, items, recipes, models
and textures come from **your own Create jar** as a content mod.

Status: **milestones 1, 2 and 3 (part 1): kinetics, their visuals, and the first machines.** See [docs/PLAN.md](docs/PLAN.md) for what is ported and what comes next.

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

Hand crank: right click to turn it (10 ticks per click, as in Create). Creative motor: right click steps its speed
16 → 32 → 64 → 128 → 256 → −256 … (Create's scroll value box comes later).

## Layout

```text
mixins/create/
  horizon-mixin.toml   the bundle: modules and injections, pinned to Horizon Nightly 2026-10-31 (aa27eca)
  kinetics.rs          Create's kinetic rules, no Horizon types
  processing.rs        pressing cycle, processing recipes, millstone and water wheel rules, no Horizon types
  machines.rs          Horizon's side of the machines: block entities, depot and press work, water wheel flow
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
only, like Horizon reads Minecraft's from the player's own game. Build tooling is adapted from the MIT-licensed
Horizon native mod template. Not affiliated with the Create team, Horizon, Mojang or Microsoft.
