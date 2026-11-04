// Create's sequenced gearshift: `SequencedGearshiftBlockEntity`, `Instruction`,
// `SequencerInstructions`, `InstructionSpeedModifiers` and the scroll inputs of
// `SequencedGearshiftScreen`. No Horizon types.

/// `SequencerInstructions`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    TurnAngle,
    TurnDistance,
    Delay,
    Await,
    End,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::TurnAngle,
        Kind::TurnDistance,
        Kind::Delay,
        Kind::Await,
        Kind::End,
    ];
    pub fn from_index(i: i32) -> Kind {
        Kind::ALL[i.clamp(0, 4) as usize]
    }
    pub fn index(self) -> i32 {
        self as i32
    }
    /// `(maxValue, shiftStep, defaultValue)`; none without a value parameter.
    pub fn value_range(self) -> Option<(i32, i32, i32)> {
        match self {
            Kind::TurnAngle => Some((360, 45, 90)),
            Kind::TurnDistance => Some((128, 5, 5)),
            Kind::Delay => Some((600, 20, 10)),
            Kind::Await | Kind::End => None,
        }
    }
    /// `hasSpeedParameter`.
    pub fn has_speed(self) -> bool {
        matches!(self, Kind::TurnAngle | Kind::TurnDistance)
    }
    /// `needsPropagation`.
    pub fn needs_propagation(self) -> bool {
        self.has_speed()
    }
    /// `gui.sequenced_gearshift.instruction.*`.
    pub fn label(self) -> &'static str {
        match self {
            Kind::TurnAngle => "Turn",
            Kind::TurnDistance => "Piston",
            Kind::Delay => "Delay",
            Kind::Await => "Await",
            Kind::End => "End",
        }
    }
    /// `descriptiveTranslationKey`.
    pub fn description(self) -> &'static str {
        match self {
            Kind::TurnAngle => "Turn by angle",
            Kind::TurnDistance => "Turn to move Piston/Pulley/Gantry",
            Kind::Delay => "Timed Delay",
            Kind::Await => "Await new Redstone Pulse",
            Kind::End => "End",
        }
    }
    /// `parameterKey`.
    pub fn parameter(self) -> &'static str {
        match self {
            Kind::TurnAngle => "Angle",
            Kind::TurnDistance => "Distance",
            Kind::Delay => "Duration",
            Kind::Await | Kind::End => "",
        }
    }
    /// `formatValue`.
    pub fn format(self, value: i32) -> String {
        match self {
            Kind::TurnAngle => format!("{value}\u{b0}"),
            Kind::TurnDistance => format!("{value}m"),
            Kind::Delay if value >= 20 => format!("{}s", value / 20),
            Kind::Delay => format!("{value}t"),
            _ => String::new(),
        }
    }
    /// The row of `sequencer.png` it draws: its Y.
    pub fn background_y(self) -> f32 {
        match self {
            Kind::TurnAngle | Kind::TurnDistance => 16.0,
            Kind::Delay => 60.0,
            Kind::End => 82.0,
            Kind::Await => 162.0,
        }
    }
}

/// `InstructionSpeedModifiers`, by index: its value, label and description.
pub const MODIFIERS: [(i32, &str, &str); 4] = [
    (2, ">>", "Double speed, Forwards"),
    (1, "->", "Input speed, Forwards"),
    (-1, "<-", "Input speed, Reversed"),
    (-2, "<<", "Double speed, Reversed"),
];

/// `convertToAngular`.
pub fn to_angular(speed: f32) -> f32 {
    speed * 3.0 / 10.0
}

/// `convertToLinear`.
pub fn to_linear(speed: f32) -> f32 {
    speed / 512.0
}

/// `Instruction`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Instruction {
    pub kind: Kind,
    /// Index into `MODIFIERS`.
    pub modifier: usize,
    pub value: i32,
}

impl Instruction {
    pub fn new(kind: Kind) -> Instruction {
        Instruction {
            kind,
            modifier: 1,
            value: kind.value_range().map_or(1, |r| r.2),
        }
    }
    /// `getDuration`.
    pub fn duration(&self, progress: f32, speed: f32) -> i32 {
        let speed = (speed * MODIFIERS[self.modifier].0 as f32).abs();
        let target = f64::from(self.value) - f64::from(progress);
        match self.kind {
            Kind::TurnAngle => {
                ((target / f64::from(to_angular(speed))).ceil() as i32).wrapping_add(2)
            }
            Kind::TurnDistance => {
                ((target / f64::from(to_linear(speed))).ceil() as i32).wrapping_add(2)
            }
            Kind::Delay => target as i32,
            Kind::Await => -1,
            Kind::End => 0,
        }
    }
    /// `getTickProgress`.
    pub fn tick_progress(&self, speed: f32) -> f32 {
        match self.kind {
            Kind::TurnAngle => to_angular(speed),
            Kind::TurnDistance => to_linear(speed),
            Kind::Delay => 1.0,
            _ => 0.0,
        }
    }
    /// `getSpeedModifier`.
    pub fn speed_modifier(&self) -> i32 {
        if self.kind.has_speed() {
            MODIFIERS[self.modifier].0
        } else {
            0
        }
    }
    /// A row of the configuration, in one number: `ConfigureSequencedGearshiftPacket`.
    pub fn encode(&self, row: usize, count: usize) -> i32 {
        CONFIG_FLAG
            | (count as i32 & 7) << 27
            | (row as i32 & 7) << 24
            | self.kind.index() << 20
            | (self.modifier as i32 & 15) << 16
            | (self.value & 0xFFFF)
    }
    /// What `encode` made: the row, the count and the instruction, in range.
    pub fn decode(v: i32) -> Option<(usize, usize, Instruction)> {
        if v & CONFIG_FLAG == 0 || v < 0 {
            return None;
        }
        let (count, row) = ((v >> 27 & 7) as usize, (v >> 24 & 7) as usize);
        let kind = (v >> 20 & 15).min(4);
        let modifier = ((v >> 16 & 15) as usize).min(3);
        if row >= 5 || count == 0 || count > 5 {
            return None;
        }
        let kind = Kind::from_index(kind);
        let value = (v & 0xFFFF).clamp(1, kind.value_range().map_or(1, |r| r.0));
        Some((
            row,
            count,
            Instruction {
                kind,
                modifier,
                value,
            },
        ))
    }
}

/// Marks a number sent as a configuration row.
pub const CONFIG_FLAG: i32 = 1 << 30;

/// What the block entity asks of the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Run {
    /// Nothing changed.
    Stay,
    /// Instruction `index` starts: `STATE` is `index + 1`, the modifier applies.
    Started {
        index: i32,
        modifier: i32,
        context: Option<(Kind, f64)>,
    },
    /// The sequence ended: the modifier is gone; `STATE` 0 without a signal.
    Ended { was_turning: bool },
}

/// `SequencedGearshiftBlockEntity`.
#[derive(Clone, Debug, PartialEq)]
pub struct Sequencer {
    pub instructions: Vec<Instruction>,
    pub current: i32,
    pub duration: i32,
    pub progress: f32,
    pub timer: i32,
    pub powered_previously: bool,
}

impl Default for Sequencer {
    fn default() -> Self {
        Sequencer {
            instructions: vec![
                Instruction::new(Kind::TurnAngle),
                Instruction::new(Kind::End),
            ],
            current: -1,
            duration: -1,
            progress: 0.0,
            timer: 0,
            powered_previously: false,
        }
    }
}

impl Sequencer {
    pub fn is_idle(&self) -> bool {
        self.current == -1
    }
    fn get(&self, i: i32) -> Option<Instruction> {
        usize::try_from(i)
            .ok()
            .and_then(|i| self.instructions.get(i))
            .copied()
    }
    /// `getModifier`.
    pub fn modifier(&self) -> i32 {
        self.get(self.current).map_or(0, |i| i.speed_modifier())
    }
    /// `tick`, with the speed it has.
    pub fn tick(&mut self, speed: f32, theoretical: f32) -> Run {
        if self.is_idle() || self.duration < 0 {
            return Run::Stay;
        }
        if self.timer < self.duration {
            self.timer += 1;
            if let Some(i) = self.get(self.current) {
                self.progress += i.tick_progress(speed);
            }
            return Run::Stay;
        }
        self.run(self.current + 1, theoretical)
    }
    /// `onSpeedChanged`.
    pub fn speed_changed(&mut self, previous: f32, speed: f32, theoretical: f32) -> Run {
        if self.is_idle() || previous.abs() == speed.abs() {
            return Run::Stay;
        }
        let Some(i) = self.get(self.current) else {
            return Run::Stay;
        };
        let ran = if speed == 0.0 {
            self.run(-1, theoretical)
        } else {
            Run::Stay
        };
        self.duration = i.duration(self.progress, theoretical);
        self.timer = 0;
        ran
    }
    /// `onRedstoneUpdate`: `running` is whether `STATE` isn't 0.
    pub fn redstone(
        &mut self,
        powered: bool,
        running: bool,
        speed: f32,
        theoretical: f32,
    ) -> (Run, bool) {
        let mut run = Run::Stay;
        if !self.powered_previously && powered {
            // risingFlank: Await goes on to the next.
            if self
                .get(self.current)
                .is_some_and(|i| i.kind == Kind::Await)
            {
                run = self.run(self.current + 1, theoretical);
            }
        }
        self.powered_previously = powered;
        if !self.is_idle() || powered == running {
            return (run, false);
        }
        if !powered {
            return (run, true);
        }
        if speed == 0.0 {
            return (run, false);
        }
        (self.run(0, theoretical), false)
    }
    /// `run`.
    pub fn run(&mut self, index: i32, theoretical: f32) -> Run {
        let instruction = self.get(index).filter(|i| i.kind != Kind::End);
        let Some(i) = instruction else {
            let was_turning = self.modifier() != 0;
            self.current = -1;
            self.duration = -1;
            self.progress = 0.0;
            self.timer = 0;
            return Run::Ended { was_turning };
        };
        // `getTheoreticalSpeed() * getModifier()`, the new instruction's.
        let kinetic = f64::from(theoretical) * f64::from(i.speed_modifier());
        self.duration = i.duration(0.0, theoretical);
        self.current = index;
        self.progress = 0.0;
        self.timer = 0;
        let context = i.kind.needs_propagation().then(|| {
            (
                i.kind,
                if kinetic == 0.0 {
                    0.0
                } else {
                    f64::from(i.value) / kinetic
                },
            )
        });
        Run::Started {
            index,
            modifier: i.speed_modifier(),
            context,
        }
    }
    /// `ConfigureSequencedGearshiftPacket.applySettings`.
    pub fn configure(&mut self, instructions: Vec<Instruction>, theoretical: f32) -> Run {
        let run = self.run(-1, theoretical);
        self.instructions = instructions;
        run
    }
}

/// `SequenceContext.getEffectiveValue`.
pub fn effective(relative: f64, speed_at_target: f32) -> f64 {
    (relative * f64::from(speed_at_target)).abs()
}

/// `SequencedGearshiftScreen.instructionUpdated`: row `row` becomes `kind`.
pub fn change_kind(rows: &mut Vec<Instruction>, row: usize, kind: Kind) {
    let Some(r) = rows.get_mut(row) else {
        return;
    };
    r.kind = kind;
    r.value = kind.value_range().map_or(-1, |v| v.2);
    if kind == Kind::End {
        rows.truncate(row + 1);
    } else if row + 1 < 5 && row + 1 == rows.len() {
        rows.push(Instruction::new(Kind::End));
    }
}

/// `ScrollInput.mouseScrolled` on a row's value: its new value.
pub fn scroll_value(kind: Kind, value: i32, forward: bool, shift: bool) -> i32 {
    let Some((max, shift_step, _)) = kind.value_range() else {
        return value;
    };
    let step = if kind == Kind::Delay {
        // The screen's own step function for delays.
        let v = if forward { value } else { value - 1 };
        match (v < 20, shift) {
            (true, true) => 20,
            (true, false) => 1,
            (false, true) => 100,
            (false, false) => 20,
        }
    } else if shift {
        shift_step
    } else {
        1
    };
    let mut state = value + if forward { step } else { -step };
    if shift {
        state -= state % shift_step;
    }
    // withRange(1, maxValue + 1): `clampState` to max - 1 of that range.
    state.clamp(1, max)
}

/// `SelectionScrollInput`: scrolling up goes back through the options.
pub fn scroll_option(index: i32, count: i32, up: bool) -> i32 {
    (index + if up { -1 } else { 1 }).clamp(0, count - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_overshoot_a_little() {
        let a = Instruction::new(Kind::TurnAngle);
        // 90 degrees at 16 rpm: 4.8 a tick.
        assert_eq!(a.duration(0.0, 16.0), 21);
        let d = Instruction {
            kind: Kind::Delay,
            modifier: 1,
            value: 40,
        };
        assert_eq!(d.duration(0.0, 16.0), 40);
        assert_eq!(Instruction::new(Kind::Await).duration(0.0, 16.0), -1);
        let fast = Instruction {
            kind: Kind::TurnDistance,
            modifier: 0,
            value: 5,
        };
        assert_eq!(fast.duration(0.0, 16.0), 82);
        assert_eq!(fast.speed_modifier(), 2);
        assert_eq!(d.speed_modifier(), 0);
    }

    #[test]
    fn it_runs_through_its_instructions() {
        let mut s = Sequencer::default();
        assert_eq!(s.redstone(false, false, 16.0, 16.0), (Run::Stay, false));
        // A pulse starts it.
        let (run, _) = s.redstone(true, false, 16.0, 16.0);
        assert!(matches!(
            run,
            Run::Started {
                index: 0,
                modifier: 1,
                ..
            }
        ));
        assert_eq!(s.modifier(), 1);
        for _ in 0..21 {
            assert_eq!(s.tick(16.0, 16.0), Run::Stay);
        }
        assert_eq!(s.tick(16.0, 16.0), Run::Ended { was_turning: true });
        assert!(s.is_idle());
        // Still powered: it stays idle, STATE back to 0 when the signal goes.
        assert_eq!(s.redstone(false, true, 16.0, 16.0), (Run::Stay, true));
    }

    #[test]
    fn await_goes_on_at_a_new_pulse() {
        let mut s = Sequencer {
            instructions: vec![
                Instruction::new(Kind::Await),
                Instruction::new(Kind::TurnAngle),
                Instruction::new(Kind::End),
            ],
            ..Sequencer::default()
        };
        s.redstone(true, false, 16.0, 16.0);
        assert_eq!(s.current, 0);
        assert_eq!(s.tick(16.0, 16.0), Run::Stay);
        s.redstone(false, true, 16.0, 16.0);
        assert_eq!(s.current, 0);
        let (run, _) = s.redstone(true, true, 16.0, 16.0);
        assert!(matches!(run, Run::Started { index: 1, .. }));
    }

    #[test]
    fn a_stop_ends_it_and_a_change_of_speed_retimes() {
        let mut s = Sequencer::default();
        s.run(0, 16.0);
        for _ in 0..5 {
            s.tick(16.0, 16.0);
        }
        assert_eq!(s.speed_changed(16.0, 32.0, 32.0), Run::Stay);
        assert_eq!(s.timer, 0);
        assert_eq!(
            s.duration,
            Instruction::new(Kind::TurnAngle).duration(s.progress, 32.0)
        );
        assert!(matches!(s.speed_changed(32.0, 0.0, 0.0), Run::Ended { .. }));
        assert!(s.is_idle());
    }

    #[test]
    fn rows_travel_in_one_number() {
        let i = Instruction {
            kind: Kind::Delay,
            modifier: 3,
            value: 600,
        };
        assert_eq!(Instruction::decode(i.encode(4, 5)), Some((4, 5, i)));
        assert_eq!(Instruction::decode(600), None);
        assert_eq!(Instruction::decode(-5), None);
    }

    #[test]
    fn the_screen_keeps_an_end_after_the_last_row() {
        let mut rows = vec![
            Instruction::new(Kind::TurnAngle),
            Instruction::new(Kind::End),
        ];
        change_kind(&mut rows, 1, Kind::Delay);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].value, 10);
        assert_eq!(rows[2].kind, Kind::End);
        change_kind(&mut rows, 0, Kind::End);
        assert_eq!(rows.len(), 1);
        for k in 0..4 {
            change_kind(&mut rows, k, Kind::Await);
        }
        assert_eq!(rows.len(), 5);
        change_kind(&mut rows, 4, Kind::Await);
        assert_eq!(rows.len(), 5);
    }

    #[test]
    fn scrolling_values() {
        assert_eq!(scroll_value(Kind::TurnAngle, 90, true, false), 91);
        assert_eq!(scroll_value(Kind::TurnAngle, 90, true, true), 135);
        assert_eq!(scroll_value(Kind::TurnAngle, 360, true, false), 360);
        assert_eq!(scroll_value(Kind::TurnAngle, 1, false, false), 1);
        assert_eq!(scroll_value(Kind::Delay, 10, true, false), 11);
        assert_eq!(scroll_value(Kind::Delay, 20, true, false), 40);
        assert_eq!(scroll_value(Kind::Delay, 20, false, false), 19);
        assert_eq!(scroll_option(1, 5, true), 0);
        assert_eq!(scroll_option(0, 5, true), 0);
        assert_eq!(scroll_option(4, 5, false), 4);
        assert_eq!(Kind::Delay.format(40), "2s");
        assert_eq!(Kind::Delay.format(10), "10t");
    }

    #[test]
    fn context_scales_by_the_speed_at_its_target() {
        assert_eq!(effective(90.0 / 32.0, 16.0), 45.0);
        let mut s = Sequencer::default();
        assert_eq!(
            s.run(0, 16.0),
            Run::Started {
                index: 0,
                modifier: 1,
                context: Some((Kind::TurnAngle, 90.0 / 16.0))
            }
        );
    }
}
