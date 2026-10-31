// The kinetics tick at the end of each server level's tick.
fn create_level_tick(game: &mut crate::game::Game<'_>, dimension: &str) {
    crate::game::create_glue::level_tick(game, dimension);
}
