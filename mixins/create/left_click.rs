// Super glue in hand takes a left click: the glue around the block comes
// off and the block isn't hit.
fn create_left_click(game: &mut crate::game::Game<'_>, pos: [i32; 3], face: [i32; 3], canceled: &mut bool) {
    if crate::game::create_contraptions::left_click(game, pos, face) {
        *canceled = true;
    }
}
