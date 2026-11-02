// The wrench breaking a minecart in one blow.
fn create_wrench_cart(game: &mut crate::game::Game<'_>) -> core::ops::ControlFlow<bool> {
    crate::game::create_glue::wrench_attacks_cart(game)
}
