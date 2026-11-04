// The mouse wheel over a screen of Create's own.
fn create_screen_scrolled(game: &crate::game::Game<'_>, screen: &horizon_client::mod_api::NativeScreenView<'_>, scroll: [f64; 2], modifiers: u32, canceled: &mut bool) {
    let (up, shift) = (scroll[1] > 0.0, modifiers & 1 != 0);
    if crate::game::create_sequencer_glue::scrolled(game, screen.rect, screen.mouse, up, shift)
        || crate::game::create_detector_glue::scrolled(game, screen.rect, screen.mouse, up, shift)
    {
        *canceled = true;
    }
}
