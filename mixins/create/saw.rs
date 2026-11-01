// Create's mechanical saw rules, injected as `crate::game::create_saw`:
// `TreeCutter.findTree` (what falls with a cut log: logs and roots, then
// leaves by their distance, and attachments; bamboo, cane, cactus and kelp
// from the top down; chorus) and `SawBlockEntity.isSawable`. The world is
// asked through `TreeWorld`. No Horizon types.

use super::create_kinetics::Pos;
use std::collections::{HashSet, VecDeque};

/// What `TreeCutter` reads of a block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TreeBlock {
    /// `isLog`: `minecraft:logs`, `tconstruct:slimy_logs`, a mushroom stem.
    pub log: bool,
    /// `isRoot`: `create:roots`.
    pub root: bool,
    /// `isLeaf`: its `distance` (not scaffolding's).
    pub leaf: Option<i32>,
    /// `isVerticalPlant`: bamboo, cactus, sugar cane, kelp.
    pub vertical_plant: bool,
    /// `isChorus`: chorus plant or flower.
    pub chorus: bool,
    /// `create:tree_attachments`.
    pub attachment: bool,
    /// `nonDecayingLeafDistance`: mushroom blocks, wart blocks and
    /// weeping vines are taken this far from the cut; -1 otherwise.
    pub non_decaying: i32,
}

pub trait TreeWorld {
    fn tree_block(&self, pos: Pos) -> TreeBlock;
    /// `getHeight`: how far up a vertical plant is followed.
    fn height(&self) -> i32;
}

/// `TreeCutter.Tree`: broken attachments first, then logs, then leaves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    pub logs: Vec<Pos>,
    pub leaves: Vec<Pos>,
    pub attachments: Vec<Pos>,
}

impl Tree {
    /// `destroyBlocks`' order, each with its effect chance.
    pub fn in_order(&self) -> Vec<(Pos, f32)> {
        let a = self.attachments.iter().map(|p| (*p, 1.0 / 32.0));
        let l = self.logs.iter().map(|p| (*p, 1.0 / 2.0));
        let f = self.leaves.iter().map(|p| (*p, 1.0 / 8.0));
        a.chain(l).chain(f).collect()
    }
}

fn offset(p: Pos, x: i32, y: i32, z: i32) -> Pos {
    [p[0] + x, p[1] + y, p[2] + z]
}

/// `BlockPos.betweenClosed` over the box around `p` from `min_y` to
/// `max_y` above it, X fastest, then Y, then Z.
fn around(p: Pos, min_y: i32, max_y: i32) -> Vec<Pos> {
    let mut out = Vec::new();
    for z in -1..=1 {
        for y in min_y..=max_y {
            for x in -1..=1 {
                out.push(offset(p, x, y, z));
            }
        }
    }
    out
}

const UP: (i32, i32) = (0, 1);
const DOWN: (i32, i32) = (-1, 0);
const BOTH: (i32, i32) = (-1, 1);

/// `validateCut`: nothing on the layer over the cut still stands on logs.
fn validate_cut(world: &impl TreeWorld, pos: Pos) -> bool {
    let mut visited = HashSet::new();
    let mut frontier = VecDeque::from([pos, offset(pos, 0, 1, 0)]);
    while let Some(current) = frontier.pop_front() {
        let below = offset(current, 0, -1, 0);
        visited.insert(current);
        let lower_layer = current[1] == pos[1];
        let here = world.tree_block(current);
        let under = world.tree_block(below);
        if !here.log && !here.root {
            continue;
        }
        if !lower_layer && pos != below && (under.log || under.root) {
            return false;
        }
        // Up (on the lower layer only), then the four sides.
        let mut next = Vec::new();
        if lower_layer {
            next.push(offset(current, 0, 1, 0));
        }
        next.extend([
            offset(current, 0, 0, -1),
            offset(current, 0, 0, 1),
            offset(current, -1, 0, 0),
            offset(current, 1, 0, 0),
        ]);
        for n in next {
            if !visited.contains(&n) {
                frontier.push_back(n);
            }
        }
    }
    true
}

/// `TreeCutter.findTree`: the block at `pos` (`broken`) was just cut.
pub fn find_tree(world: &impl TreeWorld, pos: Pos, broken: TreeBlock) -> Tree {
    let mut tree = Tree::default();
    let above = world.tree_block(offset(pos, 0, 1, 0));
    if broken.vertical_plant {
        if !above.vertical_plant {
            return tree;
        }
        tree.logs.push(offset(pos, 0, 1, 0));
        for i in 1..world.height() {
            let current = offset(pos, 0, i, 0);
            if !world.tree_block(current).vertical_plant {
                break;
            }
            tree.logs.push(current);
        }
        tree.logs.reverse();
        return tree;
    }
    if broken.chorus {
        if !above.chorus {
            return tree;
        }
        let mut visited = HashSet::new();
        let mut frontier = VecDeque::from([offset(pos, 0, 1, 0)]);
        while let Some(current) = frontier.pop_front() {
            visited.insert(current);
            tree.logs.push(current);
            for d in [
                [0, -1, 0],
                [0, 1, 0],
                [0, 0, -1],
                [0, 0, 1],
                [-1, 0, 0],
                [1, 0, 0],
            ] {
                let n = offset(current, d[0], d[1], d[2]);
                if visited.contains(&n) || !world.tree_block(n).chorus {
                    continue;
                }
                frontier.push_back(n);
            }
        }
        tree.logs.reverse();
        return tree;
    }
    if !validate_cut(world, pos) {
        return tree;
    }
    let mut visited: HashSet<Pos> = HashSet::from([pos]);
    let mut frontier: VecDeque<Pos> = around(pos, 0, 1).into();
    // Logs and roots, upwards.
    let mut has_roots = false;
    while let Some(current) = frontier.pop_front() {
        if !visited.insert(current) {
            continue;
        }
        let b = world.tree_block(current);
        if b.root {
            has_roots = true;
        } else if !b.log {
            continue;
        }
        tree.logs.push(current);
        frontier.extend(
            around(current, UP.0, UP.1)
                .into_iter()
                .filter(|p| !visited.contains(p)),
        );
    }
    visited = tree.logs.iter().copied().collect();
    frontier = tree.logs.iter().copied().collect();
    // Roots, downwards.
    if has_roots {
        let old: HashSet<Pos> = tree.logs.iter().copied().collect();
        while let Some(current) = frontier.pop_front() {
            if !world.tree_block(current).root {
                continue;
            }
            if !old.contains(&current) {
                tree.logs.push(current);
            }
            for n in around(current, DOWN.0, DOWN.1) {
                if visited.insert(n) {
                    frontier.push_back(n);
                }
            }
        }
        visited = tree.logs.iter().copied().collect();
        frontier = tree.logs.iter().copied().collect();
    }
    // Leaves, as long as their distance grows away from the logs.
    while let Some(prev) = frontier.pop_front() {
        let prev_distance = world.tree_block(prev).leaf.unwrap_or(0);
        for current in around(prev, BOTH.0, BOTH.1) {
            if visited.contains(&current) {
                continue;
            }
            let b = world.tree_block(current);
            if b.attachment {
                tree.attachments.push(current);
                visited.insert(current);
                continue;
            }
            let horizontal = (current[0] - pos[0]).abs().max((current[2] - pos[2]).abs());
            if horizontal <= b.non_decaying && visited.insert(current) {
                tree.leaves.push(current);
                frontier.push_back(current);
                continue;
            }
            if b.leaf.is_some_and(|d| d > prev_distance) && visited.insert(current) {
                tree.leaves.push(current);
                frontier.push_back(current);
            }
        }
    }
    tree
}

/// What `SawBlockEntity.isSawable` reads of a block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SawFacts {
    pub sapling: bool,
    pub log: bool,
    /// `minecraft:leaves`.
    pub leaves: bool,
    pub root: bool,
    /// Bamboo, `create:solid_crops`, cactus, sugar cane, kelp, chorus plant.
    pub plant: bool,
}

/// `SawBlockEntity.isSawable`.
pub fn is_sawable(f: &SawFacts) -> bool {
    if f.sapling {
        return false;
    }
    f.log || f.leaves || f.root || f.plant
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct World(HashMap<Pos, TreeBlock>);

    impl TreeWorld for World {
        fn tree_block(&self, pos: Pos) -> TreeBlock {
            self.0.get(&pos).copied().unwrap_or(TreeBlock {
                non_decaying: -1,
                ..TreeBlock::default()
            })
        }
        fn height(&self) -> i32 {
            384
        }
    }

    fn log() -> TreeBlock {
        TreeBlock {
            log: true,
            non_decaying: -1,
            ..TreeBlock::default()
        }
    }

    fn leaf(d: i32) -> TreeBlock {
        TreeBlock {
            leaf: Some(d),
            non_decaying: -1,
            ..TreeBlock::default()
        }
    }

    #[test]
    fn a_cut_oak_falls_whole() {
        // A trunk of four over the cut at y 0, leaves around the top two.
        let mut m = HashMap::new();
        for y in 1..=4 {
            m.insert([0, y, 0], log());
        }
        for (x, z) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            m.insert([x, 4, z], leaf(1));
            m.insert([2 * x, 4, 2 * z], leaf(2));
        }
        m.insert([0, 5, 0], leaf(1));
        // A neighbour's leaves, nearer to their own log, aren't taken.
        m.insert([3, 4, 0], leaf(1));
        let world = World(m);
        let tree = find_tree(&world, [0, 0, 0], log());
        assert_eq!(tree.logs, vec![[0, 1, 0], [0, 2, 0], [0, 3, 0], [0, 4, 0]]);
        assert_eq!(tree.leaves.len(), 9);
        assert!(!tree.leaves.contains(&[3, 4, 0]));
        let order = tree.in_order();
        assert_eq!(order[0], ([0, 1, 0], 0.5));
    }

    #[test]
    fn a_trunk_still_standing_on_another_log_stays() {
        let mut m = HashMap::new();
        // A 2x1 trunk: cutting one side leaves the other holding it up.
        for y in 0..=3 {
            m.insert([0, y, 0], log());
            m.insert([1, y, 0], log());
        }
        let world = World(m);
        assert_eq!(find_tree(&world, [0, 0, 0], log()), Tree::default());
    }

    #[test]
    fn vertical_plants_fall_from_the_top() {
        let cane = TreeBlock {
            vertical_plant: true,
            non_decaying: -1,
            ..TreeBlock::default()
        };
        let mut m = HashMap::new();
        for y in 1..=3 {
            m.insert([0, y, 0], cane);
        }
        let world = World(m);
        let tree = find_tree(&world, [0, 0, 0], cane);
        assert_eq!(tree.logs.first(), Some(&[0, 3, 0]));
        assert!(tree.logs.contains(&[0, 1, 0]));
        // Nothing over it: nothing more falls.
        assert_eq!(find_tree(&world, [0, 3, 0], cane), Tree::default());
    }

    #[test]
    fn chorus_falls_together() {
        let chorus = TreeBlock {
            chorus: true,
            non_decaying: -1,
            ..TreeBlock::default()
        };
        let mut m = HashMap::new();
        m.insert([0, 1, 0], chorus);
        m.insert([0, 2, 0], chorus);
        m.insert([1, 2, 0], chorus);
        let world = World(m);
        let tree = find_tree(&world, [0, 0, 0], chorus);
        assert_eq!(tree.logs.len(), 3);
        assert_eq!(tree.logs.last(), Some(&[0, 1, 0]));
    }

    #[test]
    fn what_a_saw_cuts() {
        assert!(is_sawable(&SawFacts {
            log: true,
            ..SawFacts::default()
        }));
        assert!(!is_sawable(&SawFacts {
            sapling: true,
            leaves: true,
            ..SawFacts::default()
        }));
        assert!(!is_sawable(&SawFacts::default()));
    }
}
