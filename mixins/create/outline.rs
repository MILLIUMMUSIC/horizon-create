// Catnip's Outliner as Create uses it (outlines of boxes shown for a slot,
// kept alive by showing them again each tick, fading out over eight ticks
// once they aren't), and AABBOutline's edges as thin boxes. No Horizon types:
// the glue draws the boxes and tells clients what to show.

use std::collections::HashMap;

/// `Outliner.OutlineEntry.FADE_TICKS`.
pub const FADE_TICKS: i32 = 8;
/// `OutlineParams.lineWidth`'s default.
pub const LINE_WIDTH: f32 = 1.0 / 32.0;

/// `OutlineParams`: the colour (0xRRGGBB) and line width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub rgb: u32,
    pub width: f32,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            rgb: 0xFFFFFF,
            width: LINE_WIDTH,
        }
    }
}

/// One outline: its box (`[x0, y0, z0, x1, y1, z1]`) and parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub bounds: [f64; 6],
    /// `showCluster`: the blocks whose outer edges are outlined instead of
    /// `bounds`' own.
    pub cluster: Option<Vec<[i32; 3]>>,
    pub params: Params,
    ticks_till_removal: i32,
    fade: i32,
}

impl Entry {
    /// `isFading`.
    #[allow(dead_code)]
    pub fn fading(&self) -> bool {
        self.ticks_till_removal < 0
    }
    /// The outline's alpha this tick: 1, then down to 0 as it fades.
    #[allow(dead_code)]
    pub fn alpha(&self) -> f32 {
        if self.fading() {
            (self.fade as f32 / FADE_TICKS as f32).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}

/// `Outliner`: the outlines shown, by slot.
#[derive(Clone, Debug, Default)]
pub struct Outliner {
    pub entries: HashMap<String, Entry>,
}

impl Outliner {
    /// `showAABB`: the slot's outline, shown for `ticks` more ticks (one
    /// unless kept), then fading.
    pub fn show_aabb(&mut self, slot: &str, bounds: [f64; 6], params: Params, ticks: i32) {
        self.entries.insert(
            slot.to_owned(),
            Entry {
                bounds,
                cluster: None,
                params,
                ticks_till_removal: ticks.max(1),
                fade: FADE_TICKS,
            },
        );
    }
    /// `showCluster`: the outer edges of the blocks' union, shown as
    /// `show_aabb` shows a box.
    pub fn show_cluster(&mut self, slot: &str, blocks: Vec<[i32; 3]>, params: Params, ticks: i32) {
        self.show_aabb(slot, [0.0; 6], params, ticks);
        if let Some(e) = self.entries.get_mut(slot) {
            e.cluster = Some(blocks);
        }
    }
    /// `keep`: the slot's outline stays another tick.
    pub fn keep(&mut self, slot: &str) {
        if let Some(e) = self.entries.get_mut(slot) {
            e.ticks_till_removal = e.ticks_till_removal.max(1);
            e.fade = FADE_TICKS;
        }
    }
    /// `remove`.
    pub fn remove(&mut self, slot: &str) {
        self.entries.remove(slot);
    }
    /// `tickOutlines`: each outline counts down, then fades and goes.
    pub fn tick(&mut self) {
        self.entries.retain(|_, e| {
            if e.ticks_till_removal >= 0 {
                e.ticks_till_removal -= 1;
            }
            if e.ticks_till_removal < 0 {
                e.fade -= 1;
            }
            e.fade >= 0
        });
    }
}

/// `AABBOutline.renderBox`'s twelve edges, each a box `width` thick around
/// the edge of `b` grown by 1/128 (so lines on a block's faces show).
pub fn edge_boxes(b: [f64; 6], width: f32) -> Vec<[f64; 6]> {
    let g = 1.0 / 128.0;
    let lo = [b[0] - g, b[1] - g, b[2] - g];
    let hi = [b[3] + g, b[4] + g, b[5] + g];
    let h = f64::from(width) / 2.0;
    let mut out = Vec::with_capacity(12);
    for axis in 0..3 {
        let (a, c) = ((axis + 1) % 3, (axis + 2) % 3);
        for ea in [lo[a], hi[a]] {
            for ec in [lo[c], hi[c]] {
                let mut min = [0.0; 3];
                let mut max = [0.0; 3];
                min[axis] = lo[axis] - h;
                max[axis] = hi[axis] + h;
                min[a] = ea - h;
                max[a] = ea + h;
                min[c] = ec - h;
                max[c] = ec + h;
                out.push([min[0], min[1], min[2], max[0], max[1], max[2]]);
            }
        }
    }
    out
}

/// `BlockClusterOutline`: the edges of every face of the blocks that has no
/// block beside it, each a unit-long box `width` thick, none twice.
pub fn cluster_boxes(blocks: &[[i32; 3]], width: f32) -> Vec<[f64; 6]> {
    use std::collections::HashSet;
    let set: HashSet<[i32; 3]> = blocks.iter().copied().collect();
    // An edge of a face: its start corner, its axis and the face's axis. An
    // edge two coplanar faces share is drawn by neither (it toggles).
    let mut edges: HashSet<([i32; 3], usize, usize)> = HashSet::new();
    for &b in &set {
        for axis in 0..3 {
            for sign in [-1, 1] {
                let mut n = b;
                n[axis] += sign;
                if set.contains(&n) {
                    continue;
                }
                // The exposed face on `axis`: its four edges run along the
                // other two axes.
                let base = if sign > 0 { 1 } else { 0 };
                for along in (0..3).filter(|&k| k != axis) {
                    let across = 3 - axis - along;
                    for hi in [0, 1] {
                        let mut at = b;
                        at[axis] += base;
                        at[across] += hi;
                        if !edges.remove(&(at, along, axis)) {
                            edges.insert((at, along, axis));
                        }
                    }
                }
            }
        }
    }
    let h = f64::from(width) / 2.0;
    // The edge where two faces meet at a right angle is drawn once.
    let edges: HashSet<([i32; 3], usize)> = edges
        .into_iter()
        .map(|(at, along, _)| (at, along))
        .collect();
    let mut out: Vec<[f64; 6]> = edges
        .into_iter()
        .map(|(at, along)| {
            let mut min = at.map(f64::from);
            let mut max = min;
            for k in 0..3 {
                if k == along {
                    max[k] += 1.0;
                }
                min[k] -= h;
                max[k] += h;
            }
            [min[0], min[1], min[2], max[0], max[1], max[2]]
        })
        .collect();
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cluster_outlines_its_outer_edges_only() {
        assert_eq!(cluster_boxes(&[[0, 0, 0]], 1.0 / 16.0).len(), 12);
        // Two blocks side by side: a 2 x 1 x 1 box, its edges along the
        // join left out: four of two unit pieces and eight single ones.
        let two = cluster_boxes(&[[0, 0, 0], [1, 0, 0]], 1.0 / 16.0);
        assert_eq!(two.len(), 16);
        // A 2 x 2 x 2 cube: 12 edges of two units each.
        let cube: Vec<[i32; 3]> = (0..8).map(|i| [i & 1, i >> 1 & 1, i >> 2]).collect();
        assert_eq!(cluster_boxes(&cube, 1.0 / 16.0).len(), 24);
    }

    #[test]
    fn outlines_last_while_kept_then_fade() {
        let mut o = Outliner::default();
        o.show_aabb("a", [0.0, 0.0, 0.0, 1.0, 1.0, 1.0], Params::default(), 1);
        o.tick();
        assert!(!o.entries["a"].fading());
        o.keep("a");
        o.tick();
        o.tick();
        assert!(o.entries["a"].fading());
        for _ in 0..FADE_TICKS {
            o.tick();
        }
        assert!(o.entries.is_empty());
    }

    #[test]
    fn a_box_has_twelve_thin_edges() {
        let e = edge_boxes([0.0, 0.0, 0.0, 2.0, 1.0, 1.0], 1.0 / 16.0);
        assert_eq!(e.len(), 12);
        // Each edge is thin across two axes and long along one.
        for b in e {
            let size = [b[3] - b[0], b[4] - b[1], b[5] - b[2]];
            assert_eq!(
                size.iter()
                    .filter(|s| (**s - 1.0 / 16.0).abs() < 1e-9)
                    .count(),
                2
            );
        }
    }
}
