// `SequencedAssemblyItem`'s progress bar.
fn create_item_bar(stack: &horizon_client::items::ItemStack, bar: &mut Option<(i32, u32)>) {
    crate::game::create_sequenced_glue::item_bar(stack, bar);
}
