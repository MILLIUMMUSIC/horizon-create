// Which of Create's blocks give a redstone signal.
fn create_is_signal_source(
    game: &crate::game::GameLevel<'_>,
    state: horizon_world::registry::BlockStateId,
    source: &mut bool,
) {
    crate::game::create_glue::is_signal_source(game, state, source);
}
