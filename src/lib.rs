// The rule modules as a plain library, for `cargo test`. In the game they are
// injected side by side under `crate::game` (horizon-mixin.toml), so each
// reaches the other as `super::create_*` in both places.

#[path = "../mixins/create/kinetics.rs"]
pub mod create_kinetics;
#[path = "../mixins/create/processing.rs"]
pub mod create_processing;
