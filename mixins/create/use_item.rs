// An item's use right after a click one of Create's blocks took.
fn create_use_item(
    game: &mut crate::game::Game<'_>,
    stack: &horizon_client::items::ItemStack,
    claimed: &mut bool,
) {
    crate::game::create_glue::use_item(game, stack, claimed);
}
