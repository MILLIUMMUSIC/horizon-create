// Create's encased chain drive and adjustable chain gearshift, ported from
// Create 6.0.10 (MIT, The Create Team): ChainDriveBlock's `part` and connection
// along its neighbours, areBlocksConnected, and ChainGearshiftBlockEntity's
// speed modifier from a redstone signal. Injected as `crate::game::create_chain`;
// no Horizon types, tested with `cargo test`.

use super::create_kinetics::{Axis, Dir};

/// `ChainDriveBlock.Part`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Start,
    Middle,
    End,
    None,
}

impl Part {
    pub fn name(self) -> &'static str {
        match self {
            Part::Start => "start",
            Part::Middle => "middle",
            Part::End => "end",
            Part::None => "none",
        }
    }
    pub fn parse(s: &str) -> Part {
        match s {
            "start" => Part::Start,
            "middle" => Part::Middle,
            "end" => Part::End,
            _ => Part::None,
        }
    }
}

/// A chain block's `axis`, `part` and `axis_along_first`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chain {
    pub axis: Axis,
    pub part: Part,
    pub along_first: bool,
}

impl Chain {
    pub fn new(axis: Axis) -> Chain {
        Chain {
            axis,
            part: Part::None,
            along_first: false,
        }
    }
    /// `getConnectionAxis`.
    pub fn connection_axis(self) -> Axis {
        match (self.along_first, self.axis) {
            (true, Axis::X) => Axis::Y,
            (true, _) => Axis::X,
            (false, Axis::Z) => Axis::Y,
            (false, _) => Axis::Z,
        }
    }
    /// `updateShape`: the neighbour on `face` is `neighbour` (none when it
    /// isn't a chain block).
    pub fn update_shape(self, face: Dir, neighbour: Option<Chain>) -> Chain {
        let face_axis = face.axis();
        if self.axis == face_axis {
            return self;
        }
        let facing_along_first = if self.axis == Axis::X {
            face_axis == Axis::Y
        } else {
            face_axis == Axis::X
        };
        let positive = face.positive();
        let Some(other) = neighbour else {
            if facing_along_first != self.along_first || self.part == Part::None {
                return self;
            }
            if self.part == Part::Middle {
                return Chain {
                    part: if positive { Part::End } else { Part::Start },
                    ..self
                };
            }
            if (self.part == Part::Start) == positive {
                return Chain {
                    part: Part::None,
                    ..self
                };
            }
            return self;
        };
        if other.axis == face_axis
            || (other.part != Part::None && other.connection_axis() != face_axis)
        {
            return self;
        }
        let mut part = self.part;
        let mut along_first = self.along_first;
        if part == Part::None {
            part = if positive { Part::Start } else { Part::End };
            along_first = facing_along_first;
        } else if self.connection_axis() != face_axis {
            return self;
        }
        if (part == Part::Start) != positive {
            part = Part::Middle;
        }
        Chain {
            part,
            along_first,
            ..self
        }
    }
    /// `areBlocksConnected`: toward `other` on `facing`.
    pub fn connected_to(self, other: Chain, facing: Dir) -> bool {
        if other.connection_axis() != self.connection_axis()
            || facing.axis() != self.connection_axis()
        {
            return false;
        }
        if facing.positive() {
            matches!(self.part, Part::Middle | Part::Start)
        } else {
            matches!(self.part, Part::Middle | Part::End)
        }
    }
}

/// `ChainGearshiftBlockEntity.getModifierForSignal`.
pub fn modifier_for_signal(signal: i32) -> f32 {
    if signal == 0 {
        1.0
    } else {
        1.0 + (signal + 1) as f32 / 16.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_of_chains_joins_and_parts() {
        // Three along X, each on axis Z.
        let lone = Chain::new(Axis::Z);
        let west = lone.update_shape(Dir::East, Some(lone));
        assert_eq!(west.part, Part::Start);
        assert!(west.along_first);
        assert_eq!(west.connection_axis(), Axis::X);
        let east = lone.update_shape(Dir::West, Some(west));
        assert_eq!(east.part, Part::End);
        let middle = east.update_shape(Dir::East, Some(lone));
        assert_eq!(middle.part, Part::Middle);
        assert!(west.connected_to(middle, Dir::East));
        assert!(middle.connected_to(east, Dir::East));
        assert!(east.connected_to(middle, Dir::West));
        assert!(!west.connected_to(middle, Dir::West));
        assert!(!west.connected_to(middle, Dir::Up));
        // The neighbour gone: the middle ends, the end is alone.
        assert_eq!(middle.update_shape(Dir::East, None).part, Part::End);
        assert_eq!(west.update_shape(Dir::East, None).part, Part::None);
        // Not across a neighbour on its own axis.
        assert_eq!(
            lone.update_shape(Dir::East, Some(Chain::new(Axis::X))).part,
            Part::None
        );
        assert_eq!(lone.update_shape(Dir::South, Some(lone)), lone);
    }

    #[test]
    fn the_signal_slows_what_it_gives() {
        assert_eq!(modifier_for_signal(0), 1.0);
        assert_eq!(modifier_for_signal(15), 2.0);
        assert_eq!(modifier_for_signal(7), 1.5);
    }
}
