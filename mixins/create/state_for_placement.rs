// The state Create's kinetic blocks are placed in.
fn create_state_for_placement(
    game: &crate::game::GameLevel<'_>,
    state: horizon_world::registry::BlockStateId,
    context: &horizon_world::level::PlacementContext,
    answer: &mut Option<Option<horizon_world::registry::BlockStateId>>,
) {
    crate::game::create_glue::state_for_placement(game, state, context, answer);
}
