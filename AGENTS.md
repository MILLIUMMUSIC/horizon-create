# Working on Create for Horizon

Read README.md, docs/PLAN.md and compatibility.json before editing. Keep this
repository separate from Horizon and horizon-mixins. Only export committed
pinned Horizon sources into ignored target trees; never modify the original
checkout or player resources. Never commit binaries, exported sources,
Minecraft assets or Create's assets (All Rights Reserved: they come from the
player's own jar through tools/prepare_content.py).

Create's Java code is the specification. Port rules into kinetics.rs (and
sibling rule modules) with no Horizon types, behind small traits, with unit
tests; Horizon-specific code goes in glue.rs. The Cargo library and the
injected `create_kinetics` module share kinetics.rs. Module payloads cannot
use `//!` comments, external `mod` files or `include!`.

Every commit pushed to `main` updates the progress estimate in README.md
("Progress: about N %" and its table): adjust the milestone's "Done" share for
what the push ports or fixes, recompute the total, and keep the weights unless
docs/PLAN.md's milestones change.

Validate with cargo fmt, clippy with warnings denied, cargo test, then
`python3 tools/horizon.py check|build --horizon ../horizon` (Python 3.12) for
the full game. Keep validation claims and supported revisions honest.

## How the port is wired (read this before exploring)

Each injection is one small `*.rs` helper in `mixins/create/` that calls a
`pub(super) fn` in `glue.rs` (or `machines.rs`); `horizon-mixin.toml` lists the
modules and every injection with its target and bindings. Hooks in use:
`native_level_tick_post` (server tick: kinetics, machines, messages),
`GameLevel::native_block_changed` and `native_place_block` (attach/detach
kinetic entities), `native_right_click_block` and `native_use_item_on` (cranks,
motors, depots, basins, the wrench), `native_world_lifecycle` (reset and save),
`native_net_from_server` / `native_net_from_client` (channels),
`transient_blocks::transient_block_geometry` (drawing moving parts),
`native_hud` (goggles, value box tips), `native_wheel` (value box scroll),
`native_left_click_block` (super glue taken off),
`native_block_entity*` (mod block entities with inventories).

Adding a block:

- **Content** (`tools/prepare_content.py`): pick the table by what the block
  needs. `ROTATING` (`iron_chain` template, `axis`) for shaft-like blocks,
  `DIRECTIONAL` (`lightning_rod`, `facing`) for motors, fans, wheels,
  `HORIZONTAL` (`grindstone`, `face`/`facing`) for press-like blocks,
  `CONTAINERS` (`enchanting_table`) for block entities with item slots.
  Rotating blocks get a blank blockstate and a hidden `create:hz_<name>`
  block that keeps the real model for the native mod to draw; extra moving
  pieces go in `PARTIALS` (hidden `create:hz_part_<name>` blocks). Shapes are
  only keyed by the template's own property.
- **Kinetics** (`kinetics.rs`): a `Kind` variant, then every `match` on it
  (`from_block`/`block`, `rotation_axis`, `has_shaft_towards`,
  `is_small_cog`/`is_large_cog`, `stress_impact`, `stress_capacity`,
  `is_generator`, `state_property`, the save key code near `kind_key`) and a
  unit test. Copy each answer from the Create class (e.g. the mixer has no
  shaft: it is driven by a cog beside it).
- **Drawing** (`glue.rs`, `parts`): return `Part`s (holder state, orientation,
  axis, angle) for the moving pieces; `machines.rs` draws held items
  (`draw_depots`).
- **Block entity with slots** (`machines.rs`): answer `block_entity`,
  `block_entity_ticks`, `block_entity_container` for the block id, tick in
  `block_entity_tick`, sync to clients over `create:depots` (format D1,
  `encode_depots`/`from_server`).

Channels: `create:kinetics` (format K3: speeds, sources, stress, `aux`),
`create:depots` (D1: items held by depots, basins, millstones),
`create:values` (client → server value box changes, checked by the server),
`create:fluids` (F1: tanks' and basins' fluids),
`create:contraptions` (C1: running pistons' offsets and speeds, their blocks when they change, then the glue,
then turning bearings' angles and blocks, then running rope pulleys' lengths and loads, then moving gantries),
`create:chassis` (client → server: a chassis' range scrolled with the wrench).

Contraptions: `contraption.rs` (rules: structure search, `PistonContraption`,
the linear actuator) and `contraptions.rs` (Horizon's side: blocks taken out
and put back, drawn moving, saved in `data/create_contraptions_*.txt`). The
window's player is carried on its own side from `native_tick` (`tick.rs`):
in single player the server's copy of the player is not the one that moves.
Chassis ranges and super glue live in `contraptions.rs`'s state (no block
entity or entity), saved in the same file and moved with the structure.
Bearings tick there too (`bearings_tick`); a windmill's generated speed is
the kinetics' `flow_score`.
Actors (drill, harvester, plough): rules in `actors.rs`, Horizon's side in
`actor_glue.rs` (`tick_actors` runs before each structure moves and tells
it whether it is stalled; standing drills tick from `glue::level_tick`).

Fluids: `fluids.rs` (rules) reaches every fluid holder through the
`FluidAccess` trait; `fluid_glue.rs` answers it (tanks kept in the native
mod, basins' tanks in their block entity's data). Pipes and pumps tick from
`glue::level_tick` with the pumps' speeds; block changes reach it through
`glue::block_changed`.
`create:arms` (A1: arm angles and held items, deployer cycles; `arms.rs`).

Recipes: `prepare_content.py` flattens subfolders (`pressing/x` →
`pressing.x`, Horizon reads only the top folder); `machines.rs` reads
`game.recipes()` directly (`find_recipe`), because Horizon's recipe helpers
answer only inside a player's hook. Engine gaps go in `docs/PLAN.md`.

Pure rules for a new machine go in their own module (as `processing.rs`,
`fan.rs`, `obj.rs`): add it as a `[[module]]` in `horizon-mixin.toml` and with
`#[path]` in `src/lib.rs` so `cargo test` covers it. `cargo test` does not
compile `glue.rs`/`machines.rs`; only `tools/horizon.py check` does. Run
`check` after Horizon-side edits; `build` is needed only to run the game.
