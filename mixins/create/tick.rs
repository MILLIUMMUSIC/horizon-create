// Once a tick in every game: the window's carries its player on moving
// contraptions.
fn create_tick(game: &mut crate::game::Game<'_>) {
    crate::game::create_contraptions::client_tick(game);
    crate::game::create_logistics::belt_client_tick(game);
    crate::game::create_glue::send_scrolled_values(game);
}
