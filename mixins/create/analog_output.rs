// What a comparator reads from one of Create's blocks.
fn create_analog_output(
    game: &crate::game::GameLevel<'_>,
    dimension: horizon_world::level::Dimension,
    pos: horizon_core::position::BlockPos,
    output: &mut i32,
) {
    crate::game::create_glue::analog_output(game, dimension, [pos.x, pos.y, pos.z], output);
}
