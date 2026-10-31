// Hand cranks, creative motors, depots, basins, millstones and the wrench
// take a right click.
fn create_right_click(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    face: [i32; 3],
    canceled: &mut bool,
) {
    crate::game::create_glue::right_click(game, pos, face, canceled);
}
