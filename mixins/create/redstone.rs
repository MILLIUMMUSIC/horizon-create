// Create's redstone logic, ported from Create 6.0.10 (MIT, The Create Team):
// the pulse repeater, extender and timer (BrassDiodeBlockEntity), the latches
// (ToggleLatchBlock, PoweredLatchBlock) and the analog lever's state. Injected
// as `crate::game::create_redstone`; no Horizon types, tested with `cargo test`.

/// Which `BrassDiodeBlockEntity`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brass {
    Repeater,
    Extender,
    Timer,
}

impl Brass {
    /// `defaultValue`.
    pub fn default_max(self) -> i32 {
        if self == Brass::Timer { 20 } else { 2 }
    }
}

/// `BrassDiodeBlockEntity.tick` with its `updateState`: the new `state` and
/// the new `POWERING` when it changes.
pub fn brass_tick(
    kind: Brass,
    state: i32,
    max: i32,
    powered: bool,
    powering: bool,
) -> (i32, Option<bool>) {
    let at_min = state <= 0;
    match kind {
        Brass::Extender => {
            if at_min && !powered {
                return (state, None);
            }
            if at_min || powered {
                return (max, (!powering).then_some(true));
            }
            if state == 1 {
                return (0, powering.then_some(false));
            }
            (state - 1, None)
        }
        Brass::Repeater => {
            if at_min && !powered {
                return (state, None);
            }
            if state > max + 1 {
                return (if !powered && !powering { 0 } else { state }, None);
            }
            let next = state + 1;
            if next == max - 1 && !powering {
                return (next, Some(true));
            }
            if next == max + 1 && powering {
                return (next, Some(false));
            }
            (next, None)
        }
        Brass::Timer => {
            let next = if powered || state >= max - 1 {
                0
            } else {
                state + 1
            };
            let should = !powered && if max == 2 { next == 0 } else { next <= 1 };
            (next, (should != powering).then_some(should))
        }
    }
}

/// `getOutputSignal` of a brass diode.
pub fn brass_output(powering: bool, inverted: bool) -> i32 {
    if powering != inverted { 15 } else { 0 }
}

/// `ToggleLatchBlock.tick`: `POWERING` flips on the input's rising edge.
pub fn toggle_latch(powered: bool, should: bool, powering: bool) -> bool {
    if should && !powered {
        !powering
    } else {
        powering
    }
}

/// `PoweredLatchBlock.tick`: the new `POWERING`.
pub fn powered_latch(
    back: bool,
    should_back: bool,
    side: bool,
    should_side: bool,
    powering: bool,
) -> bool {
    let mut p = powering;
    if back != should_back {
        if should_back {
            p = true;
        } else if side {
            p = false;
        }
    }
    if side != should_side {
        if should_side {
            p = false;
        } else if back {
            p = true;
        }
    }
    p
}

/// `AnalogLeverBlockEntity.changeState`.
pub fn lever_state(state: i32, back: bool) -> i32 {
    (state + if back { -1 } else { 1 }).clamp(0, 15)
}

/// A wheel step of a brass diode's interval, over Create 6's value board's
/// rows: ticks to 60, seconds to 60, minutes to 60 (`setValueSettings`).
pub fn interval_step(value: i32, up: bool) -> i32 {
    const SECOND: i32 = 20;
    const MINUTE: i32 = 1200;
    let next = if up {
        if value < 60 {
            value + 1
        } else if value < MINUTE {
            (value / SECOND + 1) * SECOND
        } else {
            (value / MINUTE + 1) * MINUTE
        }
    } else if value > MINUTE {
        (value / MINUTE - 1) * MINUTE
    } else if value > 60 {
        (value / SECOND - 1) * SECOND
    } else {
        value - 1
    };
    next.clamp(2, 60 * 20 * 60)
}

/// `format`.
pub fn interval_label(value: i32) -> String {
    if value < 60 {
        format!("{value}t")
    } else if value < 1200 {
        format!("{}s", value / 20)
    } else {
        format!("{}m", value / 1200)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeater_delays_a_pulse() {
        // Interval 4: on at state 3, off at 5.
        let (mut s, mut powering) = (0, false);
        let mut on_at = None;
        let mut off_at = None;
        for t in 0..8 {
            let (n, p) = brass_tick(Brass::Repeater, s, 4, t < 2, powering);
            s = n;
            if let Some(v) = p {
                powering = v;
                if v {
                    on_at = Some(t);
                } else {
                    off_at = Some(t);
                }
            }
        }
        assert_eq!(on_at, Some(2));
        assert_eq!(off_at, Some(4));
        // Past the interval it waits for the input to end, then rests.
        let (n, _) = brass_tick(Brass::Repeater, 6, 4, false, false);
        assert_eq!(n, 0);
    }

    #[test]
    fn an_extender_holds_after_its_input_goes() {
        let (s, p) = brass_tick(Brass::Extender, 0, 4, true, false);
        assert_eq!((s, p), (4, Some(true)));
        assert_eq!(brass_tick(Brass::Extender, 4, 4, false, true), (3, None));
        assert_eq!(
            brass_tick(Brass::Extender, 1, 4, false, true),
            (0, Some(false))
        );
        assert_eq!(brass_tick(Brass::Extender, 0, 4, false, false), (0, None));
    }

    #[test]
    fn a_timer_pulses_each_interval_until_powered() {
        let (mut s, mut powering) = (0, false);
        let mut ons = 0;
        for _ in 0..40 {
            let (n, p) = brass_tick(Brass::Timer, s, 20, false, powering);
            s = n;
            if let Some(v) = p {
                powering = v;
                ons += i32::from(v);
            }
        }
        // At once, then every 20 ticks.
        assert_eq!(ons, 3);
        assert_eq!(brass_tick(Brass::Timer, 5, 20, true, false), (0, None));
    }

    #[test]
    fn latches_and_lever_and_intervals() {
        assert!(toggle_latch(false, true, false));
        assert!(!toggle_latch(true, true, false));
        assert!(powered_latch(false, true, false, false, false));
        assert!(!powered_latch(false, false, false, true, true));
        assert!(powered_latch(true, true, true, false, false));
        assert_eq!(lever_state(15, false), 15);
        assert_eq!(lever_state(0, true), 0);
        assert_eq!(interval_step(2, false), 2);
        assert_eq!(interval_step(59, true), 60);
        assert_eq!(interval_step(60, true), 80);
        assert_eq!(interval_step(1200, true), 2400);
        assert_eq!(interval_step(80, false), 60);
        assert_eq!(interval_label(80), "4s");
        assert_eq!(brass_output(true, true), 0);
        assert_eq!(brass_output(false, true), 15);
    }
}
