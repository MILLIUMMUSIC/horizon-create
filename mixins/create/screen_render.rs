// A screen of Create's own: drawn over the world.
fn create_screen_render(game: &crate::game::Game<'_>, batch: &mut horizon_client::hud::HudBatch, screen: &horizon_client::mod_api::NativeScreenView<'_>) {
    crate::game::create_sequencer_glue::render(game, batch, screen.rect, screen.mouse);
    crate::game::create_detector_glue::render(game, batch, screen.rect, screen.mouse);
}
