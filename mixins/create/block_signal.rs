// The redstone signal one of Create's blocks gives.
fn create_block_signal(
    game: &crate::game::GameLevel<'_>,
    dimension: horizon_world::level::Dimension,
    pos: horizon_core::position::BlockPos,
    state: horizon_world::registry::BlockStateId,
    face: horizon_core::geometry::Direction,
    direct: bool,
    signal: &mut i32,
) {
    crate::game::create_glue::block_signal(game, dimension, [pos.x, pos.y, pos.z], state, face, direct, signal);
}
