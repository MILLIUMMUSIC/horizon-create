// What clients get of Create's funnel and tunnel block entities.
fn create_block_entity_update_tag(
    entity: &horizon_client::containers::ModBlockEntity,
    tag: &mut Option<horizon_world::nbt::Compound>,
) {
    crate::game::create_logistics::update_tag(entity, tag);
}
