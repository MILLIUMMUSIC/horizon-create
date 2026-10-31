// Rotating parts, drawn with the frame's moving blocks.
fn create_draw(
    game: &crate::game::Game<'_>,
    camera: glam::DVec3,
    out: &mut Vec<horizon_client::mesher::Vertex>,
) {
    crate::game::create_glue::draw(game, camera, out);
}
