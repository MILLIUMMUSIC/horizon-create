// Create's own sound events (AllSoundEvents), ported from Create 6.0.10 (MIT,
// The Create Team): each `create:<name>` the content's sounds.json defines,
// with the `_compounded_<n>` parts a WrappedSoundEntry plays beside it, each at
// its own volume and pitch. No Horizon types: the glue plays what `parts`
// lists.

/// `SoundSource`, in vanilla's order (`horizon_client::settings::SOUND_SOURCES`).
pub const BLOCKS: usize = 4;
pub const PLAYERS: usize = 7;

/// A sound entry's category and its parts' (volume, pitch), the first part
/// being `create:<name>` itself.
pub fn entry(name: &str) -> Option<(usize, &'static [(f32, f32)])> {
    Some(match name {
        "wrench_rotate" => (BLOCKS, &[(0.25, 1.25)]),
        "wrench_remove" => (BLOCKS, &[(0.25, 0.75), (0.25, 0.75)]),
        "depot_slide" => (BLOCKS, &[(0.125, 1.5)]),
        "depot_plop" => (BLOCKS, &[(0.25, 1.25)]),
        "funnel_flap" => (BLOCKS, &[(0.125, 1.5), (0.0425, 0.75)]),
        "slime_added" => (BLOCKS, &[(1.0, 1.0)]),
        "mechanical_press_activation" => (BLOCKS, &[(0.125, 1.0), (0.5, 1.0)]),
        "mechanical_press_activation_belt" => (BLOCKS, &[(0.75, 1.0), (0.15, 0.75)]),
        "mixing" => (BLOCKS, &[(0.125, 0.5), (0.125, 0.5)]),
        "cranking" => (BLOCKS, &[(0.075, 0.5), (0.025, 0.5)]),
        "contraption_assemble" => (BLOCKS, &[(0.5, 0.5), (0.045, 0.74)]),
        "contraption_disassemble" => (BLOCKS, &[(0.35, 0.75)]),
        "saw_activate_wood" => (BLOCKS, &[(0.75, 1.5)]),
        "saw_activate_stone" => (BLOCKS, &[(0.125, 1.25)]),
        "crafter_click" => (BLOCKS, &[(0.25, 1.0), (0.125, 1.0)]),
        "crafter_craft" => (BLOCKS, &[(0.125, 0.75)]),
        "desk_bell" => (BLOCKS, &[(1.0, 1.0)]),
        "haunted_bell_convert" => (BLOCKS, &[(1.0, 1.0)]),
        "haunted_bell_use" => (BLOCKS, &[(1.0, 1.0)]),
        "scroll_value" => (PLAYERS, &[(0.124, 1.0)]),
        "confirm" => (PLAYERS, &[(0.5, 0.8)]),
        "deny" => (PLAYERS, &[(1.0, 0.5)]),
        _ => return None,
    })
}

/// `WrappedSoundEntry.play` at `volume` and `pitch`: each part's event
/// (`create:<name>`, then `create:<name>_compounded_<n>`), its category and
/// its own volume and pitch times those.
pub fn parts(name: &str, volume: f32, pitch: f32) -> Vec<(String, usize, f32, f32)> {
    let Some((category, list)) = entry(name) else {
        return Vec::new();
    };
    list.iter()
        .enumerate()
        .map(|(i, (v, p))| {
            let id = if i == 0 {
                format!("create:{name}")
            } else {
                format!("create:{name}_compounded_{i}")
            };
            (id, category, v * volume, p * pitch)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compound_sounds_play_every_part() {
        let p = parts("wrench_remove", 1.0, 0.5);
        assert_eq!(p.len(), 2);
        assert_eq!(p[1].0, "create:wrench_remove_compounded_1");
        assert_eq!((p[1].2, p[1].3), (0.25, 0.375));
        assert!(parts("nothing", 1.0, 1.0).is_empty());
    }
}
