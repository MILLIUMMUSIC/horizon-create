// A mouse button over a screen of Create's own.
fn create_screen_pressed(game: &crate::game::Game<'_>, screen: &horizon_client::mod_api::NativeScreenView<'_>, button: i32, canceled: &mut bool) {
    if crate::game::create_sequencer_glue::pressed(game, screen.rect, screen.mouse, button)
        || crate::game::create_detector_glue::pressed(game, screen.rect, screen.mouse, button)
    {
        *canceled = true;
    }
}
