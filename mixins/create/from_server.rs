// The server's kinetics for this client to draw.
fn create_from_server(channel: &str, data: &[u8]) {
    crate::game::create_glue::from_server(channel, data);
}
