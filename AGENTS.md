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

Validate with cargo fmt, clippy with warnings denied, cargo test, then
`python3 tools/horizon.py check|build --horizon ../horizon` (Python 3.12) for
the full game. Keep validation claims and supported revisions honest.
