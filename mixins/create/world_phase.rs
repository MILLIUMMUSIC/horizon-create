// Kinetics are per world: start fresh on open, save on close.
fn create_world_phase(phase: crate::game::native_lifecycle::WorldPhase) {
    crate::game::create_glue::world_phase(phase);
}
