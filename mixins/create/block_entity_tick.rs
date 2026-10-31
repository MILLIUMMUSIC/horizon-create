// A depot's or millstone's server tick.
fn create_block_entity_tick(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    entity: &mut horizon_client::containers::ModBlockEntity,
) {
    crate::game::create_machines::block_entity_tick(game, pos, entity);
}
