// The engineer's goggles' overlay, in the HUD.
fn create_goggles(game: &crate::game::Game<'_>, batch: &mut horizon_client::hud::HudBatch) {
    crate::game::create_glue::hud(game, batch);
}
