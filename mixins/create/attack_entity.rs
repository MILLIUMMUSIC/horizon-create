// A guest's blow on an entity.
fn create_attack_entity(
    game: &mut crate::game::Game<'_>,
    entity: i32,
    canceled: &mut bool,
) {
    crate::game::create_glue::attack_entity(game, entity, canceled);
}
