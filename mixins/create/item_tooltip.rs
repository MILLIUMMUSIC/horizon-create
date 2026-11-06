// `SequencedAssemblyRecipe.addToTooltip`.
fn create_item_tooltip(
    game: &crate::game::Game<'_>,
    stack: &horizon_client::items::ItemStack,
    lines: &mut Vec<(horizon_client::text::Text, u32)>,
) {
    crate::game::create_sequenced_glue::tooltip(game, stack, lines);
}
