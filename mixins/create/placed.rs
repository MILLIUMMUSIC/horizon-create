// A player placed a block: ours get their block entity.
fn create_placed(game: &mut crate::game::Game<'_>, pos: [i32; 3]) {
    crate::game::create_glue::placed(game, pos);
}
