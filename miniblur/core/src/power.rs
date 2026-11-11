//! BFS  LEVEL    Phase 0 - Mini-Blur hexapod as numbers (shared by firmware and seat)  [crate miniblur-core]
//! BFS  PURPOSE  Estimates a gait plan's peak servo current against the supply rail limit, and checks battery voltage against the floor for walking
//! BFS  DEFINES  PowerLimits
//! BFS  USES     this crate: MAX_SERVOS, gait, servo
//! BFS  USED BY  bodyd/hexapod, bodyd/tests/bodyd_suite, core/policy, core/schema
//!
//! The servo rail budget and the battery floor. MG90S draws ~0.35 A moving
//! and ~0.7 A stalled; a small UBEC feeds 3 A. A gait that moves too many
//! servos at once browns out the rail, and a brown-out mid-write is exactly
//! the failure the two-slot journal exists for — better to refuse the step.

use crate::gait::Plan;
use crate::servo::ServoTable;
use crate::MAX_SERVOS;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PowerLimits {
    /// what the UBEC can deliver, A
    pub rail_amps: f32,
    /// per moving servo, A (worst case, under load)
    pub servo_moving_amps: f32,
    /// idle draw per servo (holding), A
    pub servo_hold_amps: f32,
    /// below this the body sits down and refuses to walk, V
    pub vbat_floor: f32,
    /// a servo counts as moving if its target changes by more than this per sample, deg
    pub moving_threshold_deg: f32,
}

impl Default for PowerLimits {
    fn default() -> PowerLimits {
        PowerLimits {
            rail_amps: 3.0,
            servo_moving_amps: 0.35,
            servo_hold_amps: 0.08,
            vbat_floor: 6.6,
            moving_threshold_deg: 0.75,
        }
    }
}

impl PowerLimits {
    /// Peak rail current a plan will draw: the sample with the most servos
    /// moving at once. Returns (peak_amps, servos_moving_at_peak).
    pub fn plan_peak_amps(&self, table: &ServoTable, plan: &Plan) -> Result<(f32, usize), alloc::string::String> {
        let mut prev: Option<[(u8, f32); MAX_SERVOS]> = None;
        let mut peak = 0.0f32;
        let mut peak_n = 0usize;
        for pose in plan.iter() {
            let cur = table.targets(&pose)?;
            if let Some(p) = prev {
                let mut moving = 0usize;
                for i in 0..table.len {
                    if (cur[i].1 - p[i].1).abs() > self.moving_threshold_deg {
                        moving += 1;
                    }
                }
                let amps = moving as f32 * self.servo_moving_amps
                    + (table.len - moving) as f32 * self.servo_hold_amps;
                if amps > peak {
                    peak = amps;
                    peak_n = moving;
                }
            }
            prev = Some(cur);
        }
        Ok((peak, peak_n))
    }

    pub fn rail_ok(&self, amps: f32) -> bool {
        amps <= self.rail_amps
    }

    /// NaN means "not measured" (a board with no divider wired): the
    /// policy cannot refuse on a sense it does not have, and telemetry
    /// says so rather than inventing a number.
    pub fn battery_ok(&self, vbat: f32) -> bool {
        vbat.is_nan() || vbat >= self.vbat_floor
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gait::{Dir, GaitParams, Walk};

    #[test]
    fn the_default_gait_fits_a_3a_rail() {
        let lim = PowerLimits::default();
        let t = ServoTable::bom_10_servo();
        let plan = Plan::new(GaitParams::default(), Walk { dir: Dir::Fwd, steps: 1, speed: 1.0 });
        let (peak, n) = lim.plan_peak_amps(&t, &plan).unwrap();
        assert!(n <= 10);
        assert!(lim.rail_ok(peak), "peak {peak} A with {n} moving");
    }

    #[test]
    fn a_weak_rail_refuses_the_same_gait() {
        let lim = PowerLimits { rail_amps: 1.0, ..PowerLimits::default() };
        let t = ServoTable::bom_10_servo();
        let plan = Plan::new(GaitParams::default(), Walk { dir: Dir::Fwd, steps: 1, speed: 1.0 });
        let (peak, _) = lim.plan_peak_amps(&t, &plan).unwrap();
        assert!(!lim.rail_ok(peak));
    }

    #[test]
    fn battery_floor() {
        let lim = PowerLimits::default();
        assert!(lim.battery_ok(7.4));
        assert!(!lim.battery_ok(6.5));
    }
}
