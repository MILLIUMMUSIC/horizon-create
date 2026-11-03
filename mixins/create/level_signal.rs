// `SignalGetter.getSignal(pos, direction)` for Create's diodes: the level's
// redstone queries are private to its crate, and only this one is needed.
use super::*;

impl Behaviours {
    pub fn create_signal<L: LevelAccess + ?Sized>(
        &self,
        level: &mut L,
        ticks: &mut Ticks,
        pos: horizon_core::position::BlockPos,
        direction: horizon_core::geometry::Direction,
    ) -> i32 {
        self.sim(level, ticks).signal(pos, direction)
    }
}
