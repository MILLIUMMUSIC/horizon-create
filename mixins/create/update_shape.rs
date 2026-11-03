// A neighbour of one of Create's blocks changed (pipes turn to it).
fn create_update_shape(
    game: &crate::game::GameLevel<'_>,
    pos: horizon_core::position::BlockPos,
    state: horizon_world::registry::BlockStateId,
    direction: horizon_core::geometry::Direction,
    new: &mut Option<horizon_world::registry::BlockStateId>,
) {
    crate::game::create_glue::update_shape(game, [pos.x, pos.y, pos.z], state, direction, new);
}
