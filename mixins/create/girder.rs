// The rules of metal girders and brackets (`GirderBlock`, `GirderEncasedShaftBlock`,
// `GirderWrenchBehavior`, `GirderPlacementHelper`, `BracketBlock`), with no Horizon
// types.

use super::create_kinetics::{Axis, Dir};

/// `GirderBlock`'s properties.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Girder {
    pub x: bool,
    pub z: bool,
    pub top: bool,
    pub bottom: bool,
    pub axis: Axis,
}

/// What a girder needs to know of a neighbour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Side {
    /// A girder's `x` and `z`.
    pub girder: Option<(bool, bool)>,
    /// An encased girder's `HORIZONTAL_AXIS`.
    pub encased: Option<Axis>,
    /// A nixie tube or placard attached towards this side (`getFacing`,
    /// `connectedDirection`).
    pub attached_towards: bool,
    /// A bracket on the block there, facing this way (`isFacingBracket`).
    pub bracket_towards: bool,
    /// `WallBlock.UP`.
    pub wall_up: bool,
    /// `FlapDisplayBlock`.
    pub flap_display: bool,
    /// `LanternBlock`'s `HANGING`.
    pub lantern_hanging: Option<bool>,
    /// A `ChainBlock` along Y.
    pub chain_y: bool,
    /// `FACE`: true for floor, false for ceiling, none for walls or no such
    /// property; `has_face` says whether it has one.
    pub has_face: bool,
    pub face_floor: Option<bool>,
    pub air: bool,
    /// `Block.isFaceFull` on the near side and `isSolid`.
    pub full_face: bool,
    /// `AbstractChuteBlock.getChuteFacing` is down.
    pub chute_down: bool,
    /// `getBlockSupportShape` is empty.
    pub no_support: bool,
    /// Nothing at all in `getShape`.
    pub no_shape: bool,
}

/// `updateVerticalProperty`'s `canAttach`: `own_y` for a `GirderBlock` on Y.
pub fn can_attach_vertically(own_y: bool, side: &Side, d: Dir) -> bool {
    if own_y
        || side.girder.is_some()
        || side.encased.is_some()
        || side.wall_up
        || side.attached_towards
        || side.flap_display
    {
        return true;
    }
    if side.lantern_hanging.is_some_and(|h| (d == Dir::Down) == h) || side.chain_y {
        return true;
    }
    if side.has_face {
        return match side.face_floor {
            Some(false) => d == Dir::Down,
            Some(true) => d == Dir::Up,
            None => false,
        };
    }
    side.bracket_towards
}

impl Girder {
    fn flag(&mut self, d: Dir, v: bool) {
        match d {
            Dir::East | Dir::West => self.x = v,
            Dir::North | Dir::South => self.z = v,
            Dir::Up => self.top = v,
            Dir::Down => self.bottom = v,
        }
    }
    /// `updateState`.
    pub fn update_state(mut self, d: Dir, side: &Side) -> Girder {
        if d.axis() == Axis::Y {
            if can_attach_vertically(self.axis == Axis::Y, side, d) {
                self.flag(d, true);
            }
            return self;
        }
        let axis = d.axis();
        let same_girder = side
            .girder
            .is_some_and(|(x, z)| if axis == Axis::X { x } else { z });
        if self.axis == axis
            || side.encased.is_some_and(|a| a != axis)
            || same_girder
            || side.attached_towards
            || side.bracket_towards
        {
            self.flag(d, true);
        }
        self
    }
    /// `isConnected`.
    pub fn connected(&self, d: Dir, side: &Side) -> bool {
        if !(if d.axis() == Axis::X { self.x } else { self.z }) || side.air {
            return false;
        }
        if side.attached_towards || side.bracket_towards {
            return true;
        }
        if side.no_shape {
            return false;
        }
        side.full_face || side.chute_down
    }
    /// `updateShape` on `d`; `side` reads each neighbour.
    pub fn update_shape(mut self, d: Dir, side: &dyn Fn(Dir) -> Side) -> Girder {
        let axis = d.axis();
        if axis != Axis::Y {
            if self.axis != axis
                && !self.connected(d, &side(d))
                && !self.connected(d.opposite(), &side(d.opposite()))
            {
                self.flag(d, false);
            }
        } else if self.axis != Axis::Y {
            if side(Dir::Up).no_support {
                self.top = false;
            }
            if side(Dir::Down).no_support {
                self.bottom = false;
            }
        }
        for o in [d, d.opposite()] {
            self = self.update_state(o, &side(o));
        }
        self
    }
    /// `getStateForPlacement`, without waterlogging.
    pub fn placed(face: Dir, side: &dyn Fn(Dir) -> Side) -> Girder {
        let axis = face.axis();
        Girder {
            x: axis == Axis::X,
            z: axis == Axis::Z,
            top: false,
            bottom: false,
            axis,
        }
        .around(side)
    }
    fn around(mut self, side: &dyn Fn(Dir) -> Side) -> Girder {
        for d in Dir::ALL {
            self = self.update_state(d, &side(d));
        }
        self
    }
    /// `GirderPlacementHelper.withAxis`, then `updateFromNeighbourShapes`.
    pub fn extended(axis: Axis, side: &dyn Fn(Dir) -> Side) -> Girder {
        let mut g = Girder {
            x: axis == Axis::X,
            z: axis == Axis::Z,
            top: false,
            bottom: false,
            axis,
        };
        for d in Dir::ALL {
            g = g.update_shape(d, side);
        }
        g
    }
    /// `GirderWrenchBehavior.postProcess`.
    pub fn post_process(mut self) -> Girder {
        if !(self.top && self.bottom) && self.axis == Axis::Y {
            self.axis = if self.x { Axis::X } else { Axis::Z };
        }
        self
    }
    /// `useItemOn` with a shaft: the encased girder's `HORIZONTAL_AXIS`.
    pub fn encased_axis(&self, hit: Dir) -> Axis {
        if self.x || hit.axis() == Axis::Z {
            Axis::Z
        } else {
            Axis::X
        }
    }
    /// `GirderEncasedShaftBlock.getRotatedBlockState`.
    pub fn from_encased(horizontal: Axis, top: bool, bottom: bool) -> Girder {
        Girder {
            x: horizontal == Axis::Z,
            z: horizontal == Axis::X,
            top,
            bottom,
            axis: if horizontal == Axis::X {
                Axis::Z
            } else {
                Axis::X
            },
        }
    }
}

/// `IPlacementHelper.orderedByDistance`: `hit` from the block's centre.
pub fn ordered_by_distance(hit: [f64; 3], dirs: &[Dir]) -> Vec<Dir> {
    let mut with: Vec<(f64, Dir)> = dirs
        .iter()
        .map(|d| {
            let n = d.normal();
            (
                (0..3)
                    .map(|k| (f64::from(n[k]) - hit[k]).powi(2))
                    .sum::<f64>(),
                *d,
            )
        })
        .collect();
    with.sort_by(|a, b| a.0.total_cmp(&b.0));
    with.into_iter().map(|p| p.1).collect()
}

/// `GirderWrenchBehavior.Action`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Single,
    Pair,
}

/// `getValidDirections`: `other` is the girder above or below, if any.
pub fn wrench_directions(
    g: &Girder,
    other: &dyn Fn(Dir) -> Option<(bool, bool)>,
) -> Vec<(Dir, Action)> {
    if !g.x && !g.z {
        return Vec::new();
    }
    let mut out = Vec::new();
    for d in [Dir::Down, Dir::Up] {
        match other(d) {
            None => {
                if g.x != g.z {
                    out.push((d, Action::Single));
                }
            }
            Some((ox, oz)) => {
                if g.x != g.z && ox != oz {
                    out.push((d, Action::Pair));
                }
            }
        }
    }
    out
}

/// `placementAssistRange`'s default.
pub const PLACEMENT_RANGE: usize = 12;

/// `GirderPlacementHelper.canExtendToward`, for a girder's `x`, `z`.
pub fn can_extend(x: bool, z: bool, side: Dir) -> bool {
    let axis = side.axis();
    match (x, z) {
        (false, false) => axis == Axis::Y,
        (true, true) => true,
        _ => axis == (if x { Axis::X } else { Axis::Z }),
    }
}

/// The same for an encased girder.
pub fn can_extend_encased(horizontal: Axis, side: Dir) -> bool {
    side.axis() != Axis::Y && side.axis() != horizontal
}

/// `BracketType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BracketType {
    Pipe,
    Cog,
    Shaft,
}

impl BracketType {
    pub fn name(self) -> &'static str {
        match self {
            BracketType::Pipe => "pipe",
            BracketType::Cog => "cog",
            BracketType::Shaft => "shaft",
        }
    }
    pub fn parse(s: &str) -> Option<BracketType> {
        match s {
            "pipe" => Some(BracketType::Pipe),
            "cog" => Some(BracketType::Cog),
            "shaft" => Some(BracketType::Shaft),
            _ => None,
        }
    }
}

/// A bracket's block state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bracket {
    pub metal: bool,
    pub ty: BracketType,
    pub facing: Dir,
    pub axis_along_first: bool,
}

impl Bracket {
    pub fn block(&self) -> &'static str {
        if self.metal {
            "create:metal_bracket"
        } else {
            "create:wooden_bracket"
        }
    }
    pub fn to_line(&self) -> String {
        format!(
            "{} {} {} {}",
            u8::from(self.metal),
            self.ty.name(),
            self.facing.name(),
            u8::from(self.axis_along_first)
        )
    }
    pub fn parse_line(f: &[&str]) -> Option<Bracket> {
        let [metal, ty, facing, along] = f else {
            return None;
        };
        Some(Bracket {
            metal: *metal == "1",
            ty: BracketType::parse(ty)?,
            facing: Dir::parse(facing)?,
            axis_along_first: *along == "1",
        })
    }
}

/// `getSuitableBracket`: for a block along `target` (none for a pipe that isn't
/// straight), from the side `direction`.
pub fn suitable_bracket(
    metal: bool,
    target: Option<Axis>,
    direction: Dir,
    ty: BracketType,
) -> Option<Bracket> {
    let target = target?;
    let axis = direction.axis();
    if target == axis {
        return None;
    }
    let along_first = if axis != Axis::Z {
        target == Axis::Z
    } else {
        target == Axis::Y
    };
    Some(Bracket {
        metal,
        ty,
        facing: direction,
        axis_along_first: !along_first,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty(_: Dir) -> Side {
        Side {
            air: true,
            no_support: true,
            no_shape: true,
            ..Side::default()
        }
    }

    #[test]
    fn placed_along_the_clicked_axis() {
        let g = Girder::placed(Dir::East, &empty);
        assert!(g.x && !g.z && !g.top && !g.bottom);
        assert_eq!(g.axis, Axis::X);
        let g = Girder::placed(Dir::Up, &empty);
        assert!(!g.x && !g.z && g.top && g.bottom);
    }

    #[test]
    fn a_beam_joins_a_girder_on_its_side() {
        let side = |d: Dir| {
            if d == Dir::North {
                Side {
                    girder: Some((false, true)),
                    ..Side::default()
                }
            } else {
                empty(d)
            }
        };
        let g = Girder::placed(Dir::East, &side);
        assert!(g.x && g.z);
    }

    #[test]
    fn a_beam_drops_a_side_with_nothing_holding_it() {
        let holding = Girder {
            x: true,
            z: true,
            top: false,
            bottom: false,
            axis: Axis::X,
        };
        let g = holding.update_shape(Dir::North, &empty);
        assert!(g.x && !g.z);
        let wall = |d: Dir| {
            if d == Dir::North {
                Side {
                    full_face: true,
                    ..Side::default()
                }
            } else {
                empty(d)
            }
        };
        assert!(holding.update_shape(Dir::North, &wall).z);
        // Along its own axis it never lets go.
        assert!(holding.update_shape(Dir::East, &empty).x);
    }

    #[test]
    fn top_and_bottom_need_support() {
        let g = Girder {
            x: true,
            z: false,
            top: true,
            bottom: true,
            axis: Axis::X,
        };
        let n = g.update_shape(Dir::Up, &empty);
        assert!(!n.top && !n.bottom);
        // A pole keeps both.
        let p = Girder {
            x: false,
            z: false,
            top: true,
            bottom: true,
            axis: Axis::Y,
        };
        let n = p.update_shape(Dir::Up, &empty);
        assert!(n.top && n.bottom);
    }

    #[test]
    fn vertical_attachments() {
        assert!(can_attach_vertically(true, &Side::default(), Dir::Up));
        assert!(can_attach_vertically(
            false,
            &Side {
                girder: Some((true, false)),
                ..Side::default()
            },
            Dir::Up
        ));
        assert!(can_attach_vertically(
            false,
            &Side {
                wall_up: true,
                ..Side::default()
            },
            Dir::Up
        ));
        assert!(can_attach_vertically(
            false,
            &Side {
                lantern_hanging: Some(true),
                ..Side::default()
            },
            Dir::Down
        ));
        assert!(!can_attach_vertically(
            false,
            &Side {
                lantern_hanging: Some(true),
                ..Side::default()
            },
            Dir::Up
        ));
        let floor = Side {
            has_face: true,
            face_floor: Some(true),
            ..Side::default()
        };
        assert!(can_attach_vertically(false, &floor, Dir::Up));
        assert!(!can_attach_vertically(false, &floor, Dir::Down));
        assert!(!can_attach_vertically(false, &Side::default(), Dir::Up));
    }

    #[test]
    fn wrench_toggles_top_and_bottom() {
        let beam = Girder {
            x: true,
            z: false,
            top: false,
            bottom: false,
            axis: Axis::X,
        };
        let dirs = wrench_directions(&beam, &|_| None);
        assert_eq!(
            dirs,
            vec![(Dir::Down, Action::Single), (Dir::Up, Action::Single)]
        );
        let dirs = wrench_directions(&beam, &|d| {
            (d == Dir::Up)
                .then_some((true, false))
                .or(Some((true, true)))
        });
        assert_eq!(dirs, vec![(Dir::Up, Action::Pair)]);
        let pole = Girder {
            x: false,
            z: false,
            top: true,
            bottom: true,
            axis: Axis::Y,
        };
        assert!(wrench_directions(&pole, &|_| None).is_empty());
    }

    #[test]
    fn post_process_leaves_a_pole_only_with_both() {
        let g = Girder {
            x: true,
            z: false,
            top: true,
            bottom: false,
            axis: Axis::Y,
        };
        assert_eq!(g.post_process().axis, Axis::X);
        let g = Girder {
            x: true,
            z: false,
            top: true,
            bottom: true,
            axis: Axis::Y,
        };
        assert_eq!(g.post_process().axis, Axis::Y);
    }

    #[test]
    fn encasing_round_trip() {
        let g = Girder {
            x: true,
            z: false,
            top: true,
            bottom: false,
            axis: Axis::X,
        };
        assert_eq!(g.encased_axis(Dir::Up), Axis::Z);
        let back = Girder::from_encased(Axis::Z, true, false);
        assert_eq!(back, g);
    }

    #[test]
    fn extends_along_what_it_is() {
        assert!(can_extend(false, false, Dir::Up));
        assert!(!can_extend(false, false, Dir::East));
        assert!(can_extend(true, false, Dir::West));
        assert!(!can_extend(true, false, Dir::North));
        assert!(can_extend(true, true, Dir::North));
        assert!(can_extend_encased(Axis::X, Dir::North));
        assert!(!can_extend_encased(Axis::X, Dir::East));
    }

    #[test]
    fn nearest_direction_first() {
        assert_eq!(
            ordered_by_distance([0.1, 0.4, 0.0], &[Dir::Down, Dir::Up]),
            vec![Dir::Up, Dir::Down]
        );
    }

    #[test]
    fn brackets_go_across() {
        assert_eq!(
            suitable_bracket(true, Some(Axis::X), Dir::East, BracketType::Shaft),
            None
        );
        assert_eq!(
            suitable_bracket(true, None, Dir::East, BracketType::Pipe),
            None
        );
        let b = suitable_bracket(true, Some(Axis::Z), Dir::Up, BracketType::Shaft).unwrap();
        assert_eq!(b.facing, Dir::Up);
        assert!(!b.axis_along_first);
        let b = suitable_bracket(false, Some(Axis::X), Dir::Up, BracketType::Cog).unwrap();
        assert!(b.axis_along_first);
        let b = suitable_bracket(true, Some(Axis::Y), Dir::North, BracketType::Pipe).unwrap();
        assert!(!b.axis_along_first);
        let f: Vec<String> = b.to_line().split(' ').map(str::to_owned).collect();
        let r: Vec<&str> = f.iter().map(String::as_str).collect();
        assert_eq!(Bracket::parse_line(&r), Some(b));
    }
}
