// Create's connected textures, ported from Create 6.0.10 (MIT, The Create
// Team): AllCTTypes' texture indices, ConnectedTextureBehaviour.buildContext
// and CTSpriteShiftEntry's place on a connected sheet. No Horizon types:
// whether a block connects to another is asked of the caller, and the
// result is a tile of the sheet the glue moves a face's texture onto.

use super::create_kinetics::{Axis, Dir, Pos};

/// `AllCTTypes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum CtType {
    Horizontal,
    HorizontalKryppers,
    Vertical,
    Omnidirectional,
    Roof,
    RoofStair,
    Cross,
    Rectangle,
}

/// `CTContext`: which neighbours across the face connect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Context {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub top_left: bool,
    pub top_right: bool,
    pub bottom_left: bool,
    pub bottom_right: bool,
}

impl CtType {
    /// `getSheetSize`: tiles along each side of the connected texture.
    pub fn sheet_size(self) -> i32 {
        match self {
            CtType::Horizontal | CtType::HorizontalKryppers | CtType::Vertical => 2,
            CtType::Omnidirectional => 8,
            CtType::Roof | CtType::RoofStair | CtType::Cross | CtType::Rectangle => 4,
        }
    }

    /// `getContextRequirement`: (left and right, up and down, corners).
    fn requirement(self) -> (bool, bool, bool) {
        match self {
            CtType::Horizontal | CtType::HorizontalKryppers => (true, false, false),
            CtType::Vertical => (false, true, false),
            CtType::Omnidirectional | CtType::Roof => (true, true, true),
            CtType::RoofStair | CtType::Cross | CtType::Rectangle => (true, true, false),
        }
    }

    /// `getTextureIndex`.
    pub fn index(self, c: &Context) -> i32 {
        let b = i32::from;
        match self {
            CtType::Horizontal => b(c.right) + 2 * b(c.left),
            CtType::HorizontalKryppers => match (c.left, c.right) {
                (false, false) => 0,
                (_, false) => 3,
                (false, _) => 2,
                _ => 1,
            },
            CtType::Vertical => b(c.up) + 2 * b(c.down),
            CtType::Omnidirectional => omni_index(c),
            CtType::Roof => roof_index(c),
            CtType::RoofStair => {
                const MAPPING: [[i32; 4]; 4] =
                    [[1, 6, 9, 4], [14, 12, 13, 15], [2, 10, 8, 0], [5, 5, 5, 5]];
                let ty = 2 * b(c.up) + b(c.right);
                let rot = 2 * b(c.left) + b(c.down);
                MAPPING[ty as usize][rot as usize]
            }
            CtType::Cross => b(c.up) + 2 * b(c.down) + 4 * b(c.left) + 8 * b(c.right),
            CtType::Rectangle => {
                let x = match (c.left, c.right) {
                    (true, true) => 2,
                    (true, false) => 3,
                    (false, true) => 1,
                    (false, false) => 0,
                };
                let y = match (c.up, c.down) {
                    (true, true) => 1,
                    (true, false) => 2,
                    (false, true) => 0,
                    (false, false) => 3,
                };
                x + y * 4
            }
        }
    }
}

fn omni_index(c: &Context) -> i32 {
    let b = i32::from;
    let mut x = 0;
    let mut y = 0;
    let borders = b(!c.up) + b(!c.down) + b(!c.left) + b(!c.right);
    if c.up {
        x += 1;
    }
    if c.down {
        x += 2;
    }
    if c.left {
        y += 1;
    }
    if c.right {
        y += 2;
    }
    if borders == 0 {
        if c.top_right {
            x += 1;
        }
        if c.top_left {
            x += 2;
        }
        if c.bottom_right {
            y += 2;
        }
        if c.bottom_left {
            y += 1;
        }
    }
    if borders == 1 {
        if !c.right && (c.top_left || c.bottom_left) {
            y = 4;
            x = -1 + b(c.bottom_left) + b(c.top_left) * 2;
        }
        if !c.left && (c.top_right || c.bottom_right) {
            y = 5;
            x = -1 + b(c.bottom_right) + b(c.top_right) * 2;
        }
        if !c.down && (c.top_left || c.top_right) {
            y = 6;
            x = -1 + b(c.top_left) + b(c.top_right) * 2;
        }
        if !c.up && (c.bottom_left || c.bottom_right) {
            y = 7;
            x = -1 + b(c.bottom_left) + b(c.bottom_right) * 2;
        }
    }
    if borders == 2
        && ((c.up && c.left && c.top_left)
            || (c.down && c.left && c.bottom_left)
            || (c.up && c.right && c.top_right)
            || (c.down && c.right && c.bottom_right))
    {
        x += 3;
    }
    x + 8 * y
}

fn roof_index(c: &Context) -> i32 {
    let up_drops = c.down && !c.up && (c.left || c.right);
    let down_drops = !c.down && c.up && (c.left || c.right);
    let left_drops = !c.left && c.right && (c.up || c.down);
    let right_drops = c.left && !c.right && (c.up || c.down);
    if up_drops {
        if left_drops {
            return if c.bottom_right { 0 } else { 5 };
        }
        if right_drops {
            return if c.bottom_left { 2 } else { 5 };
        }
        return 1;
    }
    if down_drops {
        if left_drops {
            return if c.top_right { 8 } else { 5 };
        }
        if right_drops {
            return if c.top_left { 10 } else { 5 };
        }
        return 9;
    }
    if left_drops {
        return 4;
    }
    if right_drops {
        return 6;
    }
    if !c.up || !c.down || !c.left || !c.right {
        return 5;
    }
    if c.bottom_left && c.top_right {
        if c.top_left && !c.bottom_right {
            return 12;
        }
        if c.bottom_right && !c.top_left {
            return 15;
        }
        if !c.bottom_right && !c.top_left {
            return 7;
        }
    }
    if c.bottom_right && c.top_left {
        if c.top_right && !c.bottom_left {
            return 13;
        }
        if c.bottom_left && !c.top_right {
            return 14;
        }
        if !c.bottom_left && !c.top_right {
            return 11;
        }
    }
    5
}

/// `Direction.fromAxisAndDirection`.
pub fn from_axis(axis: Axis, positive: bool) -> Dir {
    match (axis, positive) {
        (Axis::X, true) => Dir::East,
        (Axis::X, false) => Dir::West,
        (Axis::Y, true) => Dir::Up,
        (Axis::Y, false) => Dir::Down,
        (Axis::Z, true) => Dir::South,
        (Axis::Z, false) => Dir::North,
    }
}

/// `ConnectedTextureBehaviour.getUpDirection` and `getRightDirection`'s
/// defaults for a face.
pub fn default_frame(face: Dir) -> (Dir, Dir) {
    let up = if face.axis() == Axis::Y {
        Dir::North
    } else {
        Dir::Up
    };
    let right = if face.axis() == Axis::X {
        Dir::South
    } else {
        Dir::West
    };
    (up, right)
}

/// `ItemVaultCTBehaviour`'s up and right on a face of a vault along
/// `axis` (X or Z).
pub fn vault_frame(face: Dir, axis: Axis) -> (Dir, Dir) {
    let (up, right) = default_frame(face);
    let along_x = axis == Axis::X;
    let fa = face.axis();
    let up = if fa == Axis::Y && along_x {
        up.clockwise(Axis::Y)
    } else if fa == axis || fa == Axis::Y {
        up
    } else {
        from_axis(axis, along_x)
    };
    let right = if fa == Axis::Y && along_x {
        right.clockwise(Axis::Y)
    } else if fa == axis || fa == Axis::Y {
        right
    } else {
        from_axis(Axis::Y, face.positive())
    };
    (up, right)
}

fn step(pos: Pos, d: Dir, n: i32) -> Pos {
    let v = d.normal();
    [pos[0] + v[0] * n, pos[1] + v[1] * n, pos[2] + v[2] * n]
}

/// `buildContext` for `face` of the block at `pos`, `up` and `right` its
/// behaviour's directions; `connects(other)` is `connectsTo` for the block
/// at `other` (no UVs reversed: none of the ported behaviours do).
pub fn build_context(
    ty: CtType,
    pos: Pos,
    face: Dir,
    up: Dir,
    right: Dir,
    connects: impl Fn(Pos) -> bool,
) -> Context {
    let mut h = if face.positive() {
        right.opposite()
    } else {
        right
    };
    let mut v = up;
    if face == Dir::Down {
        v = v.opposite();
        h = h.opposite();
    }
    let test = |sh: i32, sv: i32| connects(step(step(pos, h, sh), v, sv));
    let (horizontal, vertical, corners) = ty.requirement();
    let mut c = Context::default();
    if vertical {
        c.up = test(0, 1);
        c.down = test(0, -1);
    }
    if horizontal {
        c.left = test(-1, 0);
        c.right = test(1, 0);
    }
    if corners {
        c.top_left = c.up && c.left && test(-1, 1);
        c.top_right = c.up && c.right && test(1, 1);
        c.bottom_left = c.down && c.left && test(-1, -1);
        c.bottom_right = c.down && c.right && test(1, -1);
    }
    c
}

/// `CTSpriteShiftEntry.getTargetU` / `getTargetV`: a face's texture
/// coordinate within its own sprite (0 to 1) moved onto tile `index` of the
/// connected sheet, as a coordinate within the connected sprite.
pub fn target_uv(local: [f32; 2], index: i32, ty: CtType) -> [f32; 2] {
    let size = ty.sheet_size();
    let (du, dv) = ((index % size) as f32, (index / size) as f32);
    let s = size as f32;
    [(local[0] + du) / s, (local[1] + dv) / s]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn rectangle_indices_follow_the_neighbours() {
        let lone = Context::default();
        assert_eq!(CtType::Rectangle.index(&lone), 12);
        let middle = Context {
            up: true,
            down: true,
            left: true,
            right: true,
            ..Context::default()
        };
        assert_eq!(CtType::Rectangle.index(&middle), 6);
        let bottom_left_corner = Context {
            up: true,
            right: true,
            ..Context::default()
        };
        assert_eq!(CtType::Rectangle.index(&bottom_left_corner), 9);
    }

    #[test]
    fn a_tank_side_connects_to_the_blocks_beside_and_above() {
        // A 2-wide, 2-high wall of one tank along X, seen from the south.
        let tank: HashSet<Pos> = [[0, 0, 0], [1, 0, 0], [0, 1, 0], [1, 1, 0]].into();
        let (up, right) = default_frame(Dir::South);
        let c = build_context(CtType::Rectangle, [0, 0, 0], Dir::South, up, right, |p| {
            tank.contains(&p)
        });
        // From the south, east is right: the block at x 1 is to the right.
        assert!(c.up && !c.down && c.right && !c.left);
        let c = build_context(CtType::Rectangle, [0, 0, 0], Dir::North, up, right, |p| {
            tank.contains(&p)
        });
        assert!(c.up && c.left && !c.right);
    }

    #[test]
    fn omni_and_sheet_places() {
        let all = Context {
            up: true,
            down: true,
            left: true,
            right: true,
            top_left: true,
            top_right: true,
            bottom_left: true,
            bottom_right: true,
        };
        assert_eq!(CtType::Omnidirectional.index(&all), 6 + 6 * 8);
        assert_eq!(
            target_uv([0.5, 0.5], 6, CtType::Rectangle),
            [2.5 / 4.0, 1.5 / 4.0]
        );
    }

    #[test]
    fn a_vault_along_x_turns_its_top_frame() {
        let (up, right) = vault_frame(Dir::Up, Axis::X);
        assert_eq!((up, right), (Dir::East, Dir::North));
        let (up, right) = vault_frame(Dir::North, Axis::X);
        assert_eq!((up, right), (Dir::East, Dir::Down));
    }
}
