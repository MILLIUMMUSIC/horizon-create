// Create's steam whistle: `WhistleBlock`, `WhistleExtenderBlock` and
// `WhistleBlockEntity`'s rules. No Horizon types.

use super::create_kinetics::Dir;

/// `WhistleSize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Small,
    Medium,
    Large,
}

impl Size {
    pub fn name(self) -> &'static str {
        match self {
            Size::Small => "small",
            Size::Medium => "medium",
            Size::Large => "large",
        }
    }
    pub fn parse(s: &str) -> Option<Size> {
        match s {
            "small" => Some(Size::Small),
            "medium" => Some(Size::Medium),
            "large" => Some(Size::Large),
            _ => None,
        }
    }
    pub fn ordinal(self) -> u32 {
        self as u32
    }
    /// `BlockState.cycle(SIZE)`.
    pub fn next(self) -> Size {
        match self {
            Size::Small => Size::Medium,
            Size::Medium => Size::Large,
            Size::Large => Size::Small,
        }
    }
    /// `WhistleSoundInstance`'s event.
    pub fn sound(self) -> &'static str {
        match self {
            Size::Small => "create:whistle_high",
            Size::Medium => "create:whistle",
            Size::Large => "create:whistle_low",
        }
    }
}

/// `WhistleExtenderShape`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Single,
    Double,
    DoubleConnected,
}

impl Shape {
    pub fn name(self) -> &'static str {
        match self {
            Shape::Single => "single",
            Shape::Double => "double",
            Shape::DoubleConnected => "double_connected",
        }
    }
    pub fn parse(s: &str) -> Option<Shape> {
        match s {
            "single" => Some(Shape::Single),
            "double" => Some(Shape::Double),
            "double_connected" => Some(Shape::DoubleConnected),
            _ => None,
        }
    }
}

/// `getAttachedDirection`.
pub fn attached(wall: bool, facing: Dir) -> Dir {
    if wall { facing } else { Dir::Down }
}

/// `getStateForPlacement`: `wall` and `facing` for a click on `clicked`,
/// `horizontal` the way the placer faces.
pub fn placement(clicked: Dir, horizontal: Dir) -> (bool, Dir) {
    if clicked == Dir::Up || clicked == Dir::Down {
        (false, horizontal)
    } else {
        (true, clicked.opposite())
    }
}

/// What is above a whistle, block by block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Above {
    Extension(Shape),
    Free,
    Blocked,
}

/// What `incrementSize` does: at the `i`-th block above (from 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grow {
    /// A single extension becomes double.
    Double(i32),
    /// A new extension is placed.
    Place(i32),
    Nothing,
}

/// `incrementSize`.
pub fn grow(above: impl Fn(i32) -> Above) -> Grow {
    for i in 1..=6 {
        match above(i) {
            Above::Extension(Shape::Single) => return Grow::Double(i),
            Above::Extension(_) => {}
            Above::Free => return Grow::Place(i),
            Above::Blocked => return Grow::Nothing,
        }
    }
    Grow::Nothing
}

/// `incrementSize`'s sound's pitch.
pub fn grow_pitch(g: Grow) -> f32 {
    let semitones = match g {
        Grow::Double(i) => i * 2,
        Grow::Place(i) => i * 2 - 1,
        Grow::Nothing => 0,
    };
    2f32.powf(-(semitones as f32) / 12.0)
}

/// `updatePitch`, from what is above it (an extension's shape, or none).
pub fn pitch(above: impl Fn(i32) -> Option<Shape>) -> i32 {
    let mut pitch = 0;
    let mut i = 1;
    while pitch <= 24 {
        let Some(shape) = above(i) else {
            break;
        };
        if shape == Shape::Single {
            pitch += 1;
            break;
        }
        pitch += 2;
        i += 1;
    }
    pitch
}

/// `generic.notes`, as `addToGoggleTooltip` picks.
pub fn note(pitch: i32) -> &'static str {
    const NOTES: [&str; 12] = [
        "F#", "F", "E", "D#", "D", "C#", "C", "B", "A#", "A", "G#", "G",
    ];
    NOTES[pitch.rem_euclid(12) as usize]
}

/// `tickAudio`'s `f`.
pub fn sound_pitch(pitch: i32) -> f32 {
    2f32.powf(-(pitch as f32) / 12.0)
}

/// `animation`: a tick on, towards out when blowing.
pub fn animate(value: f32, powered: bool) -> f32 {
    if powered {
        value + (1.0 - value) * 0.5
    } else {
        (value - 0.4).max(0.0)
    }
}

/// `WhistleRenderer`: how far up the mouth is, wiggling while it blows; `ticks`
/// with the frame's part.
pub fn mouth_offset(value: f32, blowing: bool, ticks: f32, size: Size) -> f32 {
    let mut offset = value;
    if blowing && value > 0.5 {
        let wiggle = ticks / 8.0;
        offset -= (wiggle * std::f32::consts::TAU * (4 - size.ordinal()) as f32).sin() / 16.0;
    }
    offset * 4.0 / 16.0
}

/// `WhistleExtenderBlock.updateShape` from above: connected to an extension
/// above or not.
pub fn shape_with_above(shape: Shape, extension_above: bool) -> Shape {
    match (shape, extension_above) {
        (Shape::DoubleConnected, false) => Shape::Double,
        (Shape::Single, _) | (Shape::Double, false) => shape,
        (_, true) => Shape::DoubleConnected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_on_a_tank() {
        assert_eq!(placement(Dir::Up, Dir::North), (false, Dir::North));
        assert_eq!(placement(Dir::East, Dir::North), (true, Dir::West));
        assert_eq!(attached(true, Dir::West), Dir::West);
        assert_eq!(attached(false, Dir::West), Dir::Down);
    }

    #[test]
    fn it_grows_a_semitone_at_a_time() {
        use Above::{Blocked, Extension, Free};
        assert_eq!(grow(|_| Free), Grow::Place(1));
        assert_eq!(grow(|_| Blocked), Grow::Nothing);
        assert_eq!(
            grow(|i| if i == 1 {
                Extension(Shape::Single)
            } else {
                Free
            }),
            Grow::Double(1)
        );
        assert_eq!(
            grow(|i| if i == 1 {
                Extension(Shape::Double)
            } else {
                Free
            }),
            Grow::Place(2)
        );
        assert_eq!(grow(|_| Extension(Shape::DoubleConnected)), Grow::Nothing);
        assert_eq!(pitch(|_| None), 0);
        assert_eq!(pitch(|i| (i == 1).then_some(Shape::Single)), 1);
        assert_eq!(pitch(|i| (i == 1).then_some(Shape::Double)), 2);
        assert_eq!(
            pitch(|i| match i {
                1 => Some(Shape::DoubleConnected),
                2 => Some(Shape::Single),
                _ => None,
            }),
            3
        );
        assert_eq!(note(0), "F#");
        assert_eq!(note(13), "F");
        assert!((sound_pitch(12) - 0.5).abs() < 1e-5);
        assert!((grow_pitch(Grow::Place(1)) - 2f32.powf(-1.0 / 12.0)).abs() < 1e-5);
    }

    #[test]
    fn the_mouth_opens_and_closes() {
        assert_eq!(animate(0.0, true), 0.5);
        assert!((animate(0.5, false) - 0.1).abs() < 1e-5);
        assert_eq!(animate(0.1, false), 0.0);
        assert_eq!(mouth_offset(0.5, false, 3.0, Size::Small), 0.125);
        assert_eq!(mouth_offset(1.0, true, 0.0, Size::Large), 0.25);
    }

    #[test]
    fn extensions_connect_upwards() {
        assert_eq!(
            shape_with_above(Shape::Double, true),
            Shape::DoubleConnected
        );
        assert_eq!(
            shape_with_above(Shape::DoubleConnected, false),
            Shape::Double
        );
        assert_eq!(shape_with_above(Shape::Single, false), Shape::Single);
        assert_eq!(Size::Large.next(), Size::Small);
    }
}
