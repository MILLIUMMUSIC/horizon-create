// Funnels' and tunnels' flaps, drawn by their block entities.
fn create_block_entity_submit(
    game: &crate::game::Game<'_>,
    pos: [i32; 3],
    entity: &horizon_client::containers::ModBlockEntity,
    draw: &mut crate::game::native_block_entity_draw::BlockEntityDraw,
) {
    crate::game::create_logistics::submit(game, pos, entity, draw);
}
