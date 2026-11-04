// Create's cuckoo clock: `CuckooClockBlockEntity` and `CuckooClockRenderer`'s
// timings. No Horizon types.

/// `CuckooClockBlockEntity.Animation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Animation {
    None,
    Pig,
    Creeper,
    Surprise,
}

impl Animation {
    pub fn code(self) -> u32 {
        self as u32
    }
    pub fn of(code: u32) -> Animation {
        match code {
            1 => Animation::Pig,
            2 => Animation::Creeper,
            3 => Animation::Surprise,
            _ => Animation::None,
        }
    }
}

/// `tick`'s hours and minutes: the day time, sped up in a dimension with no day.
pub fn time(day_time: i64, natural: bool) -> (i32, i32) {
    let day = (day_time * if natural { 1 } else { 24 }).rem_euclid(24000) as i32;
    ((day / 1000 + 6) % 24, (day % 1000) * 60 / 1000)
}

/// What the block entity does a tick on, with its speed not 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clock {
    pub animation: Animation,
    pub progress: f32,
}

impl Clock {
    /// `tick` on the server: true when the surprise goes off now.
    pub fn tick(&mut self, hours: i32, minutes: i32, mysterious: bool) -> bool {
        if self.animation == Animation::None {
            let start = if hours == 12 && minutes < 5 {
                Some(Animation::Pig)
            } else if hours == 18 && minutes < 36 && minutes > 31 {
                Some(Animation::Creeper)
            } else {
                None
            };
            if let Some(a) = start {
                self.animation = if mysterious { Animation::Surprise } else { a };
                self.progress = 0.0;
            }
            return false;
        }
        let value = self.progress;
        self.progress = value + 1.0;
        if value > 100.0 {
            self.animation = Animation::None;
        }
        self.animation == Animation::Surprise && (self.progress - 50.0).abs() < 1e-5
    }
}

/// `moveHands`: the angles the hands chase.
pub fn hand_targets(hours: i32, minutes: i32) -> (f32, f32) {
    ((30 * (hours % 12)) as f32, (6 * minutes) as f32)
}

/// `LerpedFloat.angular` chasing by `Chaser.EXP` at 0.2: the shortest way round.
pub fn chase_angle(value: f32, target: f32) -> f32 {
    let diff = ((target - value + 180.0).rem_euclid(360.0)) - 180.0;
    value + diff * 0.2
}

/// `CuckooClockRenderer`: the doors' angle at `value`.
pub fn door_angle(animation: Animation, value: f32) -> f32 {
    if animation == Animation::None {
        return 0.0;
    }
    let step = if animation == Animation::Surprise {
        3
    } else {
        15
    };
    let third = (step / 3) as f32;
    let mut angle = 0.0;
    let mut phase = 30;
    while phase <= 60 {
        let local = value - phase as f32;
        if local < -third {
        } else if local < 0.0 {
            angle = (value - (phase - 5) as f32) / 5.0 * 135.0;
        } else if local < third {
            angle = 135.0;
        } else if local < 2.0 * third {
            angle = 135.0 - (value - (phase + 5) as f32) / 5.0 * 135.0;
        }
        phase += step;
    }
    angle
}

/// The figure's offset out of the doors.
pub fn figure_offset(door: f32) -> f32 {
    -(door / 135.0) / 2.0 + 10.0 / 16.0
}

/// A sound's event, volume and pitch.
pub type Sound = (&'static str, f32, f32);

/// `tick` on a client: the sounds at `value` of an animation.
pub fn animation_sounds(animation: Animation, value: i32) -> Vec<Sound> {
    let surprise = animation == Animation::Surprise;
    let mut out = Vec::new();
    if value == 1 {
        out.push(("minecraft:block.note_block.chime", 2.0, 0.5));
    }
    if value == 21 {
        out.push(("minecraft:block.note_block.chime", 2.0, 0.793701));
    }
    if value == 40 && surprise {
        out.push(("minecraft:entity.tnt.primed", 1.0, 1.0));
    }
    let step = if surprise { 3 } else { 15 };
    let mut phase = 30;
    while phase <= 60 {
        if value == phase - step / 3 {
            out.push(("minecraft:block.chest.open", 1.0 / 16.0, 2.0));
        }
        if value == phase {
            if animation == Animation::Pig {
                out.push(("minecraft:entity.pig.ambient", 0.25, 1.0));
            } else {
                out.push(("minecraft:entity.creeper.hurt", 0.25, 3.0));
            }
        }
        if value == phase + step / 3 {
            out.push(("minecraft:block.chest.close", 1.0 / 16.0, 2.0));
        }
        phase += step;
    }
    out
}

/// The ticking: its pitch at game tick `ticks`, if it ticks then.
pub fn tick_pitch(ticks: i64, natural: bool) -> Option<f32> {
    let (slow, fast) = if natural { (32, 16) } else { (6, 3) };
    if ticks % slow == 0 {
        Some(2.0)
    } else if ticks % fast == 0 {
        Some(1.5)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_runs_from_six_in_the_morning() {
        assert_eq!(time(0, true), (6, 0));
        assert_eq!(time(6000, true), (12, 0));
        assert_eq!(time(12500, true), (18, 30));
        assert_eq!(time(18000, true), (0, 0));
        assert_eq!(time(1000, false), (6, 0));
        assert_eq!(hand_targets(13, 30), (30.0, 180.0));
    }

    #[test]
    fn it_calls_at_noon_and_dusk() {
        let mut c = Clock {
            animation: Animation::None,
            progress: 0.0,
        };
        assert!(!c.tick(11, 59, false));
        assert_eq!(c.animation, Animation::None);
        c.tick(12, 0, false);
        assert_eq!(c.animation, Animation::Pig);
        for _ in 0..102 {
            assert!(!c.tick(12, 3, false));
        }
        assert_eq!(c.animation, Animation::None);
        c.tick(18, 33, false);
        assert_eq!(c.animation, Animation::Creeper);
    }

    #[test]
    fn the_mysterious_one_goes_off() {
        let mut c = Clock {
            animation: Animation::None,
            progress: 0.0,
        };
        c.tick(12, 0, true);
        assert_eq!(c.animation, Animation::Surprise);
        let mut went = 0;
        for i in 1..=60 {
            if c.tick(12, 1, true) {
                went = i;
            }
        }
        assert_eq!(went, 50);
    }

    #[test]
    fn doors_open_for_each_call() {
        assert_eq!(door_angle(Animation::Pig, 10.0), 0.0);
        assert_eq!(door_angle(Animation::Pig, 30.0), 135.0);
        assert_eq!(door_angle(Animation::Pig, 25.0), 0.0);
        assert!((door_angle(Animation::Pig, 27.5) - 67.5).abs() < 1e-3);
        assert_eq!(door_angle(Animation::Pig, 37.0), 135.0 - 2.0 / 5.0 * 135.0);
        assert_eq!(door_angle(Animation::None, 30.0), 0.0);
        assert_eq!(figure_offset(135.0), 10.0 / 16.0 - 0.5);
    }

    #[test]
    fn sounds() {
        assert_eq!(animation_sounds(Animation::Pig, 1).len(), 1);
        assert_eq!(
            animation_sounds(Animation::Pig, 30)[0].0,
            "minecraft:entity.pig.ambient"
        );
        assert_eq!(
            animation_sounds(Animation::Creeper, 25)[0].0,
            "minecraft:block.chest.open"
        );
        assert_eq!(tick_pitch(32, true), Some(2.0));
        assert_eq!(tick_pitch(16, true), Some(1.5));
        assert_eq!(tick_pitch(5, true), None);
        assert!((chase_angle(350.0, 10.0) - 354.0).abs() < 1e-3);
    }
}
