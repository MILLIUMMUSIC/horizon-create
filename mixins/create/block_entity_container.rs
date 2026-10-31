// The depot's and millstone's item slots.
fn create_block_entity_container(
    block: &str,
    container: &mut Option<horizon_client::containers::ModContainer>,
) {
    crate::game::create_machines::block_entity_container(block, container);
}
