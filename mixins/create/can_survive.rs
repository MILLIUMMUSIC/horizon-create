// Whether one of Create's blocks stands where it is (the redstone link).
fn create_can_survive(
    game: &crate::game::GameLevel<'_>,
    pos: horizon_core::position::BlockPos,
    state: horizon_world::registry::BlockStateId,
    survives: &mut Option<bool>,
) {
    crate::game::create_glue::can_survive(game, [pos.x, pos.y, pos.z], state, survives);
}
