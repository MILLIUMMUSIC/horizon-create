// Which of Create's blocks comparators read.
fn create_has_analog_output(
    game: &crate::game::GameLevel<'_>,
    state: horizon_world::registry::BlockStateId,
    output: &mut bool,
) {
    crate::game::create_glue::has_analog_output(game, state, output);
}
