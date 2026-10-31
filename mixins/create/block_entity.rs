// The depot's and millstone's block entities.
fn create_block_entity(block: &str, entity: &mut Option<(String, horizon_world::nbt::Compound)>) {
    crate::game::create_machines::block_entity(block, entity);
}
