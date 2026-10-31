// Every committed `Level.setBlock`: ours are handled at the tick end.
fn create_block_changed(
    dimension: horizon_world::level::Dimension,
    pos: horizon_core::position::BlockPos,
    old: horizon_world::registry::BlockStateId,
    new: horizon_world::registry::BlockStateId,
) {
    crate::game::create_glue::block_changed(dimension, pos, old, new);
}
