// Create's mechanical crafter, ported from Create 6.0.10 (MIT, The Create
// Team): MechanicalCrafterBlock's `pointing` and target direction, its
// placement against another crafter, RecipeGridHandler's chains and
// GroupedItems, MechanicalCraftingInput and MechanicalCraftingRecipe's
// matching, and the block entity's phases and countdowns. Injected as
// `crate::game::create_crafter`; no Horizon types, tested with `cargo test`.

use super::create_kinetics::{Axis, Dir, Pos};
use std::collections::{BTreeMap, HashSet};

/// catnip's `Pointing`, in its declaration order (`state.cycle`), with its
/// `xRotation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pointing {
    Up,
    Left,
    Down,
    Right,
}

impl Pointing {
    pub const ALL: [Pointing; 4] = [
        Pointing::Up,
        Pointing::Left,
        Pointing::Down,
        Pointing::Right,
    ];

    pub fn x_rotation(self) -> f32 {
        match self {
            Pointing::Up => 0.0,
            Pointing::Left => 270.0,
            Pointing::Down => 180.0,
            Pointing::Right => 90.0,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Pointing::Up => "up",
            Pointing::Left => "left",
            Pointing::Down => "down",
            Pointing::Right => "right",
        }
    }
    pub fn parse(s: &str) -> Option<Pointing> {
        Pointing::ALL.into_iter().find(|p| p.name() == s)
    }
    /// `state.cycle(POINTING)`: the wrench on the crafter's front.
    pub fn next(self) -> Pointing {
        let i = Pointing::ALL.iter().position(|p| *p == self).unwrap_or(0);
        Pointing::ALL[(i + 1) % 4]
    }
}

/// `AngleHelper.horizontalAngle`.
fn horizontal_angle(f: Dir) -> f32 {
    match f {
        Dir::South => 0.0,
        Dir::West => -90.0,
        Dir::North => 180.0,
        Dir::East => -270.0,
        _ => 0.0,
    }
}

/// `VecHelper.rotate` about one axis, in degrees.
fn rotate(v: [f32; 3], deg: f32, axis: Axis) -> [f32; 3] {
    let a = deg.to_radians();
    let (sin, cos) = a.sin_cos();
    let [x, y, z] = v;
    match axis {
        Axis::X => [x, y * cos - z * sin, z * cos + y * sin],
        Axis::Y => [x * cos + z * sin, y, z * cos - x * sin],
        Axis::Z => [x * cos - y * sin, y * cos + x * sin, z],
    }
}

/// `MechanicalCrafterBlock.getTargetDirection`: where the crafter passes its
/// items on.
pub fn target_direction(facing: Dir, pointing: Pointing) -> Dir {
    let v = rotate([0.0, 1.0, 0.0], -pointing.x_rotation(), Axis::Z);
    let v = rotate(v, horizontal_angle(facing), Axis::Y);
    let r = |c: f32| c.round() as i32;
    Dir::nearest([r(v[0]), r(v[1]), r(v[2])])
}

/// `MechanicalCrafterBlock.pointingFromFacing`.
pub fn pointing_from_facing(pointing_face: Dir, block_facing: Dir) -> Pointing {
    let positive = block_facing.positive();
    match pointing_face {
        Dir::Down => Pointing::Up,
        Dir::East | Dir::North => {
            if positive {
                Pointing::Left
            } else {
                Pointing::Right
            }
        }
        Dir::West | Dir::South => {
            if positive {
                Pointing::Right
            } else {
                Pointing::Left
            }
        }
        Dir::Up => Pointing::Down,
    }
}

/// `MechanicalCrafterBlock.getStateForPlacement`: against another crafter
/// (`against`: its facing) unless sneaking, its facing and the pointing back
/// to it; else `HorizontalKineticBlock`'s (facing the placer), pointing at
/// the clicked block when the click wasn't on the front.
pub fn placement(
    face: Dir,
    against: Option<Dir>,
    sneaking: bool,
    placer_horizontal: Dir,
) -> (Dir, Pointing) {
    match against {
        Some(other) if !sneaking => (other, pointing_from_facing(face, other)),
        _ => {
            let facing = placer_horizontal.opposite();
            let pointing = if facing != face {
                pointing_from_facing(face, facing)
            } else {
                Pointing::Up
            };
            (facing, pointing)
        }
    }
}

/// `MechanicalCrafterBlock.isValidTarget`: a crafter facing the same way,
/// not pointing straight back.
pub fn is_valid_target(this: (Dir, Pointing), target: Option<(Dir, Pointing)>) -> bool {
    let Some(t) = target else {
        return false;
    };
    this.0 == t.0 && (this.1.x_rotation() - t.1.x_rotation()).abs() != 180.0
}

/// What `RecipeGridHandler` reads of the world: the crafter at a position
/// (its facing and pointing).
pub trait CrafterWorld {
    fn crafter(&self, pos: Pos) -> Option<(Dir, Pointing)>;
}

fn offset(p: Pos, d: Dir) -> Pos {
    let n = d.normal();
    [p[0] + n[0], p[1] + n[1], p[2] + n[2]]
}

/// `RecipeGridHandler.getTargetingCrafter`: the crafter `pos` passes to.
pub fn targeting(world: &impl CrafterWorld, pos: Pos) -> Option<Pos> {
    let (facing, pointing) = world.crafter(pos)?;
    let target = offset(pos, target_direction(facing, pointing));
    let (target_facing, _) = world.crafter(target)?;
    (target_facing == facing).then_some(target)
}

/// `RecipeGridHandler.getPrecedingCrafters`: the crafters passing to `pos`.
pub fn preceding(world: &impl CrafterWorld, pos: Pos) -> Vec<Pos> {
    let Some((facing, pointing)) = world.crafter(pos) else {
        return Vec::new();
    };
    let own_target = target_direction(facing, pointing);
    Dir::ALL
        .into_iter()
        .filter(|d| d.axis() != facing.axis() && *d != own_target)
        .filter_map(|d| {
            let n = offset(pos, d);
            let (nf, np) = world.crafter(n)?;
            (target_direction(nf, np) == d.opposite() && nf == facing).then_some(n)
        })
        .collect()
}

/// `RecipeGridHandler.getAllCraftersOfChainIf`: every crafter joined to
/// `root`, breadth first; none when the chain loops, when one fails `test`
/// (unless a powered start), or when none passes.
pub fn chain_if(
    world: &impl CrafterWorld,
    root: Pos,
    test: impl Fn(Pos) -> bool,
    powered_start: bool,
) -> Option<Vec<Pos>> {
    let mut crafters = Vec::new();
    let mut frontier: Vec<(Pos, Option<Pos>)> = vec![(root, None)];
    let mut visited = HashSet::new();
    let (mut empty, mut all_empty) = (false, true);
    while !frontier.is_empty() {
        let (current, last) = frontier.remove(0);
        if visited.contains(&current) {
            return None;
        }
        if test(current) {
            all_empty = false;
        } else {
            empty = true;
        }
        crafters.push(current);
        visited.insert(current);
        let target = targeting(world, current);
        if target != last
            && let Some(t) = target
        {
            frontier.push((t, Some(current)));
        }
        for p in preceding(world, current) {
            if Some(p) != last {
                frontier.push((p, Some(current)));
            }
        }
    }
    if (empty && !powered_start) || all_empty {
        None
    } else {
        Some(crafters)
    }
}

/// `RecipeGridHandler.GroupedItems`: items by their place in the grid (x to
/// the crafter's right, y up), as they are passed along.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Grouped<S> {
    pub grid: BTreeMap<(i32, i32), S>,
}

impl<S: Clone> Grouped<S> {
    pub fn of(stack: S) -> Self {
        let mut grid = BTreeMap::new();
        grid.insert((0, 0), stack);
        Grouped { grid }
    }
    /// `mergeOnto`: moved one step the way the crafter points.
    pub fn merge_onto(&self, other: &mut Grouped<S>, pointing: Pointing) {
        let dx = match pointing {
            Pointing::Left => 1,
            Pointing::Right => -1,
            _ => 0,
        };
        let dy = match pointing {
            Pointing::Down => 1,
            Pointing::Up => -1,
            _ => 0,
        };
        for ((x, y), s) in &self.grid {
            other.grid.insert((x + dx, y + dy), s.clone());
        }
    }
    /// `MechanicalCraftingInput.of`: the occupied box (`is_empty` leaves a
    /// cell out of it), rows top first (`maxY - y`), its width and height.
    pub fn input(&self, is_empty: impl Fn(&S) -> bool) -> Option<(usize, usize, Vec<Option<S>>)> {
        let filled: Vec<(i32, i32)> = self
            .grid
            .iter()
            .filter(|(_, s)| !is_empty(s))
            .map(|(p, _)| *p)
            .collect();
        let min_x = filled.iter().map(|p| p.0).min()?;
        let max_x = filled.iter().map(|p| p.0).max()?;
        let min_y = filled.iter().map(|p| p.1).min()?;
        let max_y = filled.iter().map(|p| p.1).max()?;
        let (w, h) = ((max_x - min_x + 1) as usize, (max_y - min_y + 1) as usize);
        let mut cells = Vec::with_capacity(w * h);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                cells.push(
                    self.grid
                        .get(&(x + min_x, max_y - y))
                        .filter(|s| !is_empty(s))
                        .cloned(),
                );
            }
        }
        Some((w, h, cells))
    }
}

/// A `create:mechanical_crafting` recipe's pattern: each cell's choices
/// (ids and `#tags`, none for a space), its size and whether a mirrored
/// grid matches too.
#[derive(Clone, Debug, PartialEq)]
pub struct MechanicalRecipe {
    pub id: String,
    pub width: usize,
    pub height: usize,
    pub cells: Vec<Option<Vec<String>>>,
    pub result: String,
    pub count: u32,
    pub accept_mirrored: bool,
}

/// A `create:mechanical_crafting` recipe file (`pattern`, `key`, `result`,
/// `accept_mirrored`).
pub fn parse_mechanical(id: &str, json: &serde_json::Value) -> Option<MechanicalRecipe> {
    let pattern: Vec<&str> = json
        .get("pattern")?
        .as_array()?
        .iter()
        .filter_map(|r| r.as_str())
        .collect();
    let width = pattern.iter().map(|r| r.chars().count()).max()?;
    let height = pattern.len();
    let key = json.get("key")?.as_object()?;
    let mut cells = Vec::with_capacity(width * height);
    for row in &pattern {
        let chars: Vec<char> = row.chars().collect();
        for x in 0..width {
            match chars.get(x) {
                None | Some(' ') => cells.push(None),
                Some(c) => {
                    let choices =
                        super::create_processing::ingredient_choices(key.get(&c.to_string())?);
                    if choices.is_empty() {
                        return None;
                    }
                    cells.push(Some(choices));
                }
            }
        }
    }
    let result = json.get("result")?;
    Some(MechanicalRecipe {
        id: id.to_owned(),
        width,
        height,
        cells,
        result: result
            .get("id")
            .or_else(|| result.get("item"))?
            .as_str()?
            .to_owned(),
        count: result
            .get("count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1) as u32,
        accept_mirrored: json
            .get("accept_mirrored")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
    })
}

impl MechanicalRecipe {
    /// `MechanicalCraftingRecipe.matches` (`ShapedRecipe.matches` when
    /// mirrored grids count): the input box against the pattern at each
    /// offset, `test(cell, stack)` for a filled cell.
    pub fn matches<S>(
        &self,
        w: usize,
        h: usize,
        input: &[Option<S>],
        test: impl Fn(&[String], &S) -> bool,
    ) -> bool {
        if w < self.width || h < self.height {
            return false;
        }
        let at = |dx: usize, dy: usize, mirror: bool| {
            (0..w).all(|i| {
                (0..h).all(|j| {
                    let (k, l) = (i as i64 - dx as i64, j as i64 - dy as i64);
                    let cell = if k >= 0
                        && l >= 0
                        && (k as usize) < self.width
                        && (l as usize) < self.height
                    {
                        let k = if mirror {
                            self.width - 1 - k as usize
                        } else {
                            k as usize
                        };
                        self.cells[k + l as usize * self.width].as_deref()
                    } else {
                        None
                    };
                    match (cell, &input[i + j * w]) {
                        (None, None) => true,
                        (Some(c), Some(s)) => test(c, s),
                        _ => false,
                    }
                })
            })
        };
        (0..=w - self.width).any(|dx| {
            (0..=h - self.height)
                .any(|dy| at(dx, dy, false) || (self.accept_mirrored && at(dx, dy, true)))
        })
    }
}

/// `MechanicalCrafterBlockEntity.Phase`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Accepting,
    Assembling,
    Exporting,
    Waiting,
    Crafting,
    Inserting,
}

impl Phase {
    pub const ALL: [Phase; 7] = [
        Phase::Idle,
        Phase::Accepting,
        Phase::Assembling,
        Phase::Exporting,
        Phase::Waiting,
        Phase::Crafting,
        Phase::Inserting,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Phase::Idle => "IDLE",
            Phase::Accepting => "ACCEPTING",
            Phase::Assembling => "ASSEMBLING",
            Phase::Exporting => "EXPORTING",
            Phase::Waiting => "WAITING",
            Phase::Crafting => "CRAFTING",
            Phase::Inserting => "INSERTING",
        }
    }
    pub fn parse(s: &str) -> Phase {
        Phase::ALL
            .into_iter()
            .find(|p| p.name() == s)
            .unwrap_or(Phase::Idle)
    }
}

/// `getCountDownSpeed`.
pub fn count_down_speed(speed: f32) -> i32 {
    if speed == 0.0 {
        0
    } else {
        (speed.abs() as i32).clamp(4, 250)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct W(HashMap<Pos, (Dir, Pointing)>);
    impl CrafterWorld for W {
        fn crafter(&self, pos: Pos) -> Option<(Dir, Pointing)> {
            self.0.get(&pos).copied()
        }
    }

    #[test]
    fn pointing_and_placement_agree() {
        // Facing north, pointing left: towards east (the viewer's left).
        assert_eq!(target_direction(Dir::North, Pointing::Left), Dir::East);
        assert_eq!(target_direction(Dir::North, Pointing::Up), Dir::Up);
        assert_eq!(target_direction(Dir::North, Pointing::Down), Dir::Down);
        // A crafter put on another's east side points back west at it.
        for facing in [Dir::North, Dir::South, Dir::East, Dir::West] {
            for face in Dir::ALL {
                if face.axis() == facing.axis() {
                    continue;
                }
                let (f, p) = placement(face, Some(facing), false, Dir::North);
                assert_eq!(f, facing);
                assert_eq!(
                    target_direction(f, p),
                    face.opposite(),
                    "{facing:?} {face:?}"
                );
            }
        }
        assert_eq!(Pointing::Right.next(), Pointing::Up);
    }

    #[test]
    fn a_two_by_two_chain_is_found_and_items_meet_at_its_end() {
        // Facing south, four crafters in an L feeding into the bottom right.
        let mut m = HashMap::new();
        m.insert([0, 1, 0], (Dir::South, Pointing::Down));
        m.insert([1, 1, 0], (Dir::South, Pointing::Down));
        m.insert([0, 0, 0], (Dir::South, Pointing::Down));
        // Pointing west, at the end.
        m.insert(
            [1, 0, 0],
            (Dir::South, pointing_from_facing(Dir::East, Dir::South)),
        );
        let w = W(m);
        assert_eq!(targeting(&w, [0, 1, 0]), Some([0, 0, 0]));
        assert_eq!(targeting(&w, [1, 0, 0]), Some([0, 0, 0]));
        assert_eq!(targeting(&w, [0, 0, 0]), None);
        assert_eq!(preceding(&w, [0, 0, 0]).len(), 2);
        let chain = chain_if(&w, [1, 1, 0], |_| true, false).unwrap();
        assert_eq!(chain.len(), 4);
        // One empty crafter stops it, unless powered.
        assert!(chain_if(&w, [1, 1, 0], |p| p != [1, 0, 0], false).is_none());
        assert!(chain_if(&w, [1, 1, 0], |p| p != [1, 0, 0], true).is_some());
    }

    #[test]
    fn grouped_items_merge_and_flip_into_an_input() {
        let mut a = Grouped::of("a");
        Grouped::of("b").merge_onto(&mut a, Pointing::Down);
        // `b` pointing down lands one above.
        assert_eq!(a.grid.get(&(0, 1)), Some(&"b"));
        let (w, h, cells) = a.input(|s| s.is_empty()).unwrap();
        assert_eq!((w, h), (1, 2));
        assert_eq!(cells, vec![Some("b"), Some("a")]);
    }

    #[test]
    fn mechanical_recipes_match_with_or_without_mirror() {
        let json = serde_json::json!({
            "pattern": ["AB", "A "],
            "key": {"A": {"item": "x:a"}, "B": {"item": "x:b"}},
            "result": {"id": "x:r", "count": 2},
            "accept_mirrored": false
        });
        let r = parse_mechanical("r", &json).unwrap();
        assert_eq!((r.width, r.height, r.count), (2, 2, 2));
        let test = |c: &[String], s: &&str| c.iter().any(|i| i == *s);
        let grid = [Some("x:a"), Some("x:b"), Some("x:a"), None];
        assert!(r.matches(2, 2, &grid, test));
        let mirrored = [Some("x:b"), Some("x:a"), None, Some("x:a")];
        assert!(!r.matches(2, 2, &mirrored, test));
        let mut r2 = r.clone();
        r2.accept_mirrored = true;
        assert!(r2.matches(2, 2, &mirrored, test));
        assert_eq!(count_down_speed(0.0), 0);
        assert_eq!(count_down_speed(-2.0), 4);
        assert_eq!(count_down_speed(256.0), 250);
        assert!(is_valid_target(
            (Dir::North, Pointing::Up),
            Some((Dir::North, Pointing::Left))
        ));
        assert!(!is_valid_target(
            (Dir::North, Pointing::Up),
            Some((Dir::North, Pointing::Down))
        ));
    }
}
