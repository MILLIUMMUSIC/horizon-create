# Validation

## Milestone 3, part 2: compiled and unit-tested only

The encased fan, the mixer and basin, the creative motor's value box, the wrench and the water wheel's OBJ model
were ported in a cloud session on Linux (x86_64), Rust 1.99.1 and Python 3.12, against the same pinned Horizon
(`aa27eca`) and horizon-mixins (`3f99a5c`). There were no prepared Minecraft resources and no Create jar there, so
**none of it has been run in the game**: everything below "Seen in the game" is from milestones 1 to 3 part 1.

| Check | How | Result |
|---|---|---|
| Rule modules (`kinetics.rs`, `processing.rs`, `fan.rs`, `obj.rs`) | `cargo fmt --all --check`, `cargo clippy --all-targets --locked -D warnings`, `cargo test --locked` | 54 tests pass, no warnings |
| The bundle applies and the game compiles | `tools/horizon.py check` and `build` (Python 3.12) | Pass, no warnings (`check` and the `fast` build, Linux x86_64) |
| `prepare_content.py` | Run on a stub carried folder (placeholder files, no Create assets) and a stub jar | New blocks, holders, shapes and the basin's blockstate written; OBJ files copied from the jar when missing |

To check in the game (macOS session): see the pull request's list.

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

Not checked yet: hoppers into and out of the depot and millstone, the press over items on the ground, a LAN or
dedicated server with a second player, building through the Horizon launcher, and anything against Create running
on NeoForge (no oracle yet: values are compared by hand with Create's code).
