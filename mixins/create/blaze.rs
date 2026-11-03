// Create's blaze burner, ported from Create 6.0.10 (MIT, The Create Team):
// BlazeBurnerBlock.HeatLevel, BlazeBurnerBlockEntity's fuel (its types,
// burn times and the heat they give), HeatCondition.testBlazeBurner and the
// head's pose in BlazeBurnerRenderer. Injected as `crate::game::create_blaze`;
// no Horizon types, tested with `cargo test`.

/// `BlazeBurnerBlock.HeatLevel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HeatLevel {
    None,
    Smouldering,
    Fading,
    Kindled,
    Seething,
}

impl HeatLevel {
    pub const ALL: [HeatLevel; 5] = [
        HeatLevel::None,
        HeatLevel::Smouldering,
        HeatLevel::Fading,
        HeatLevel::Kindled,
        HeatLevel::Seething,
    ];
    pub fn name(self) -> &'static str {
        match self {
            HeatLevel::None => "none",
            HeatLevel::Smouldering => "smouldering",
            HeatLevel::Fading => "fading",
            HeatLevel::Kindled => "kindled",
            HeatLevel::Seething => "seething",
        }
    }
    pub fn parse(s: &str) -> Option<HeatLevel> {
        Self::ALL.into_iter().find(|l| l.name() == s)
    }
    pub fn is_at_least(self, other: HeatLevel) -> bool {
        self >= other
    }
    /// `nextActiveLevel`: the next level up that isn't `none`, wrapping to
    /// smouldering.
    pub fn next_active_level(self) -> HeatLevel {
        let i = Self::ALL.iter().position(|l| *l == self).unwrap_or(0);
        let next = Self::ALL[(i + 1) % Self::ALL.len()];
        if next == HeatLevel::None {
            HeatLevel::Smouldering
        } else {
            next
        }
    }
}

/// `BlazeBurnerBlockEntity.FuelType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Fuel {
    #[default]
    None,
    Normal,
    Special,
}

pub const MAX_HEAT_CAPACITY: i32 = 10000;
pub const INSERTION_THRESHOLD: i32 = 500;

/// What a burner holds (`activeFuel`, `remainingBurnTime`, `isCreative`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Burner {
    pub fuel: Fuel,
    pub remaining: i32,
    pub creative: bool,
}

impl Burner {
    /// `getHeatLevel`.
    pub fn heat_level(&self) -> HeatLevel {
        match self.fuel {
            Fuel::Special => HeatLevel::Seething,
            Fuel::Normal => {
                if f64::from(self.remaining) / f64::from(MAX_HEAT_CAPACITY) < 0.0125 {
                    HeatLevel::Fading
                } else {
                    HeatLevel::Kindled
                }
            }
            Fuel::None => HeatLevel::Smouldering,
        }
    }

    /// `tick` on the server: one tick of burning. True when the block's
    /// heat level should be looked at again (`updateBlockState`).
    pub fn tick(&mut self) -> bool {
        if self.creative {
            return false;
        }
        if self.remaining > 0 {
            self.remaining -= 1;
        }
        let burning = self.fuel == Fuel::Normal;
        if self.remaining > 0 {
            return burning;
        }
        if self.fuel == Fuel::Special {
            self.fuel = Fuel::Normal;
            self.remaining = MAX_HEAT_CAPACITY / 2;
        } else {
            self.fuel = Fuel::None;
        }
        true
    }

    /// `tryUpdateFuel` for an item of `kind` burning `burn_time` ticks: true
    /// when it was taken (an item to use up).
    pub fn try_update_fuel(
        &mut self,
        kind: Fuel,
        burn_time: i32,
        force_overflow: bool,
        simulate: bool,
    ) -> bool {
        if self.creative || kind == Fuel::None || kind < self.fuel {
            return false;
        }
        let mut new_time = burn_time;
        if kind == self.fuel {
            if self.remaining <= INSERTION_THRESHOLD {
                new_time += self.remaining;
            } else if force_overflow && kind == Fuel::Normal {
                new_time = if self.remaining < MAX_HEAT_CAPACITY {
                    (self.remaining + new_time).min(MAX_HEAT_CAPACITY)
                } else {
                    self.remaining
                };
            } else {
                return false;
            }
        }
        if !simulate {
            self.fuel = kind;
            self.remaining = new_time;
        }
        true
    }
}

/// `HeatCondition.testBlazeBurner`: the recipe's heat requirement (0 none,
/// 1 heated, 2 superheated) against the burner's level.
pub fn heat_met(requirement: u8, level: HeatLevel) -> bool {
    match requirement {
        2 => level == HeatLevel::Seething,
        1 => level != HeatLevel::Smouldering && level != HeatLevel::None,
        _ => true,
    }
}

/// `BlazeBurnerRenderer.getBlazeModel`'s name for a head: which of the
/// blaze partials (`inert`, `idle`, `active`, `super`, `super_active`).
pub fn head_model(level: HeatLevel, block_above: bool) -> &'static str {
    if level.is_at_least(HeatLevel::Seething) {
        if block_above { "super_active" } else { "super" }
    } else if level.is_at_least(HeatLevel::Fading) {
        if block_above && level.is_at_least(HeatLevel::Kindled) {
            "active"
        } else {
            "idle"
        }
    } else {
        "inert"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuel_sets_the_heat_and_runs_out() {
        let mut b = Burner::default();
        assert_eq!(b.heat_level(), HeatLevel::Smouldering);
        assert!(b.try_update_fuel(Fuel::Normal, 1600, false, false));
        assert_eq!(b.heat_level(), HeatLevel::Kindled);
        // More normal fuel is refused until it is nearly used up.
        assert!(!b.try_update_fuel(Fuel::Normal, 1600, false, false));
        b.remaining = 400;
        assert!(b.try_update_fuel(Fuel::Normal, 1600, false, false));
        assert_eq!(b.remaining, 2000);
        // Special fuel takes over, then falls back to half a capacity.
        assert!(b.try_update_fuel(Fuel::Special, 3200, false, false));
        assert_eq!(b.heat_level(), HeatLevel::Seething);
        assert!(!b.try_update_fuel(Fuel::Normal, 1600, false, false));
        b.remaining = 1;
        assert!(b.tick());
        assert_eq!((b.fuel, b.remaining), (Fuel::Normal, MAX_HEAT_CAPACITY / 2));
        // Normal fuel running low fades, then goes out.
        b.remaining = 100;
        assert_eq!(b.heat_level(), HeatLevel::Fading);
        b.remaining = 2;
        b.tick();
        assert_eq!(b.fuel, Fuel::Normal);
        b.tick();
        assert_eq!(b.fuel, Fuel::None);
        assert_eq!(b.heat_level(), HeatLevel::Smouldering);
    }

    #[test]
    fn overflow_tops_up_normal_fuel_to_the_capacity() {
        let mut b = Burner {
            fuel: Fuel::Normal,
            remaining: 9000,
            creative: false,
        };
        assert!(!b.try_update_fuel(Fuel::Normal, 1600, false, false));
        assert!(b.try_update_fuel(Fuel::Normal, 1600, true, false));
        assert_eq!(b.remaining, MAX_HEAT_CAPACITY);
        let before = b;
        assert!(b.try_update_fuel(Fuel::Normal, 1600, true, true));
        assert_eq!(b, before);
    }

    #[test]
    fn heat_requirements() {
        assert!(heat_met(0, HeatLevel::Smouldering));
        assert!(!heat_met(1, HeatLevel::Smouldering));
        assert!(heat_met(1, HeatLevel::Fading));
        assert!(heat_met(1, HeatLevel::Seething));
        assert!(!heat_met(2, HeatLevel::Kindled));
        assert!(heat_met(2, HeatLevel::Seething));
        assert_eq!(
            HeatLevel::Seething.next_active_level(),
            HeatLevel::Smouldering
        );
        assert_eq!(head_model(HeatLevel::Kindled, true), "active");
        assert_eq!(head_model(HeatLevel::Fading, true), "idle");
        assert_eq!(head_model(HeatLevel::Smouldering, true), "inert");
    }
}
