// The wrench used on one of Create's blocks.
fn create_use_item_on(
    game: &mut crate::game::Game<'_>,
    stack: &horizon_client::items::ItemStack,
    pos: [i32; 3],
    face: [i32; 3],
    claimed: &mut bool,
) {
    crate::game::create_glue::use_item_on(game, stack, pos, face, claimed);
}
