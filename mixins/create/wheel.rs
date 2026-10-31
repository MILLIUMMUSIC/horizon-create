// The mouse wheel over a value box changes its value.
fn create_wheel(game: &mut crate::game::Game<'_>, steps: f32, claimed: &mut bool) {
    crate::game::create_glue::wheel(game, steps, claimed);
}
