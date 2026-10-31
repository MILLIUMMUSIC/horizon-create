// The rule modules as a plain library, for `cargo test`. In the game they are
// injected side by side under `crate::game` (horizon-mixin.toml), so each
// reaches the other as `super::create_*` in both places.

#[path = "../mixins/create/fan.rs"]
pub mod create_fan;
#[path = "../mixins/create/kinetics.rs"]
pub mod create_kinetics;
#[path = "../mixins/create/obj.rs"]
pub mod create_obj;
#[path = "../mixins/create/processing.rs"]
pub mod create_processing;
