//! BFS  LEVEL    Phase 0 - Mini-Blur hexapod as numbers (shared by firmware and seat)  [crate miniblur-core]
//! BFS  PURPOSE  Servo table: which channel drives which leg joint, with center, direction and angle limits; converts a pose to per-channel angles and pulses
//! BFS  DEFINES  ServoCal, ServoTable
//! BFS  USES     this crate: Axis | external: Axis, LegId
//! BFS  USED BY  bodyd/hexapod, bodyd/selfmodel, bodyd/tests/bodyd_suite, core/power, core/runtime, core/schema, host/bench, host/fullstack, +4 more
//!
//! The servo table — which channel drives which joint, and each channel's
//! calibrated envelope. This is §8 step 5 (calibration) as a value: the
//! seat keeps it in the state graph, hands it to the body at incorporation,
//! and the body forgets it at power-off.
//!
//! A channel may drive more than one leg's joint (cheap hexapods link the
//! front and rear swing of one side through a bar). The gait guarantees the
//! linked legs always want the same angle — they are in the same tripod —
//! and `targets()` refuses a pose that would tear a linkage.

use crate::{Axis, LegId, Pose, LEGS, MAX_SERVOS};
use alloc::string::String;
use alloc::format;

/// One servo channel and its calibration.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ServoCal {
    pub ch: u8,
    /// absolute servo degrees the joint may reach (after calibration)
    pub min_deg: f32,
    pub max_deg: f32,
    /// absolute servo degrees at the joint's neutral pose
    pub center_deg: f32,
    /// +1 if increasing servo degrees increases the joint angle, -1 if mirrored
    pub dir: i8,
    /// the joints this channel drives (up to two: a linked pair)
    pub joints: [Option<(LegId, Axis)>; 2],
}

impl ServoCal {
    pub const fn one(ch: u8, leg: LegId, axis: Axis, center_deg: f32, dir: i8) -> ServoCal {
        ServoCal { ch, min_deg: 20.0, max_deg: 160.0, center_deg, dir, joints: [Some((leg, axis)), None] }
    }
    pub const fn pair(ch: u8, a: LegId, b: LegId, axis: Axis, center_deg: f32, dir: i8) -> ServoCal {
        ServoCal { ch, min_deg: 20.0, max_deg: 160.0, center_deg, dir, joints: [Some((a, axis)), Some((b, axis))] }
    }
    /// Joint degrees (relative to centre) → absolute servo degrees.
    pub fn servo_deg(&self, joint_rel_deg: f32) -> f32 {
        self.center_deg + (self.dir as f32) * joint_rel_deg
    }
    pub fn in_envelope(&self, servo_deg: f32) -> bool {
        servo_deg >= self.min_deg && servo_deg <= self.max_deg
    }
    /// Standard hobby-servo pulse: 500 µs at 0°, 2500 µs at 180°.
    pub fn pulse_us(servo_deg: f32) -> u16 {
        let d = if servo_deg < 0.0 { 0.0 } else if servo_deg > 180.0 { 180.0 } else { servo_deg };
        (500.0 + d * (2000.0 / 180.0)) as u16
    }
}

/// The whole table. Fixed capacity; `len` channels are in use.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ServoTable {
    pub cal: [ServoCal; MAX_SERVOS],
    pub len: usize,
}

impl ServoTable {
    pub fn from_slice(c: &[ServoCal]) -> ServoTable {
        let blank = ServoCal::one(255, LegId::L1, Axis::Lift, 90.0, 1);
        let mut cal = [blank; MAX_SERVOS];
        let n = if c.len() > MAX_SERVOS { MAX_SERVOS } else { c.len() };
        cal[..n].copy_from_slice(&c[..n]);
        ServoTable { cal, len: n }
    }

    /// The report's BOM: 10 × MG90S on six legs. Hypothesis for the joint
    /// layout until the wiring doc says otherwise: every leg has its own
    /// lift (6), the front and rear swing of each side are linked (2), and
    /// the middle legs swing on their own (2). Linked pairs {L1,L3} and
    /// {R1,R3} are each inside one tripod, so the tripod gait never asks a
    /// linked pair for two different angles.
    pub fn bom_10_servo() -> ServoTable {
        use Axis::*;
        use LegId::*;
        ServoTable::from_slice(&[
            ServoCal::one(0, L1, Lift, 90.0, 1),
            ServoCal::one(1, R1, Lift, 90.0, -1),
            ServoCal::one(2, L2, Lift, 90.0, 1),
            ServoCal::one(3, R2, Lift, 90.0, -1),
            ServoCal::one(4, L3, Lift, 90.0, 1),
            ServoCal::one(5, R3, Lift, 90.0, -1),
            ServoCal::pair(6, L1, L3, Swing, 90.0, 1),
            ServoCal::pair(7, R1, R3, Swing, 90.0, -1),
            ServoCal::one(8, L2, Swing, 90.0, 1),
            ServoCal::one(9, R2, Swing, 90.0, -1),
        ])
    }

    pub fn channels(&self) -> &[ServoCal] {
        &self.cal[..self.len]
    }

    pub fn find(&self, ch: u8) -> Option<&ServoCal> {
        self.channels().iter().find(|c| c.ch == ch)
    }

    pub fn find_mut(&mut self, ch: u8) -> Option<&mut ServoCal> {
        self.cal[..self.len].iter_mut().find(|c| c.ch == ch)
    }

    /// Does any channel drive this joint? (a 1-DOF leg has no swing)
    pub fn has_joint(&self, leg: LegId, axis: Axis) -> bool {
        self.channels().iter().any(|c| c.joints.iter().flatten().any(|j| *j == (leg, axis)))
    }

    /// Absolute servo targets for a pose. Err names the first joint outside
    /// its envelope or the first linkage the pose would tear.
    pub fn targets(&self, pose: &Pose) -> Result<[(u8, f32); MAX_SERVOS], String> {
        let mut out = [(255u8, 0.0f32); MAX_SERVOS];
        for (i, c) in self.channels().iter().enumerate() {
            let mut want: Option<f32> = None;
            for j in c.joints.iter().flatten() {
                let li = j.0 as usize;
                debug_assert!(li < LEGS);
                let rel = match j.1 {
                    Axis::Swing => pose.swing[li],
                    Axis::Lift => pose.lift[li],
                };
                match want {
                    None => want = Some(rel),
                    Some(w) if (w - rel).abs() > 0.5 => {
                        return Err(format!(
                            "ch{} links {} and {} {}: pose asks {w:.1}° and {rel:.1}°",
                            c.ch, c.joints[0].unwrap().0.name(), j.0.name(), j.1.name()
                        ))
                    }
                    _ => {}
                }
            }
            let rel = want.unwrap_or(0.0);
            let deg = c.servo_deg(rel);
            if !c.in_envelope(deg) {
                let j = c.joints[0].unwrap();
                return Err(format!(
                    "ch{} ({} {}) target {deg:.1}° is outside its envelope [{:.0}, {:.0}]",
                    c.ch, j.0.name(), j.1.name(), c.min_deg, c.max_deg
                ));
            }
            out[i] = (c.ch, deg);
        }
        Ok(out)
    }

    /// Envelope check for a raw `servo.set` (calibration mode).
    pub fn check_raw(&self, ch: u8, deg: f32) -> Result<(), String> {
        match self.find(ch) {
            None => Err(format!("no servo on channel {ch}")),
            Some(c) if !c.in_envelope(deg) => Err(format!(
                "channel {ch}: {deg:.1}° is outside its envelope [{:.0}, {:.0}]",
                c.min_deg, c.max_deg
            )),
            Some(_) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bom_table_covers_every_lift_and_every_swing() {
        let t = ServoTable::bom_10_servo();
        assert_eq!(t.len, 10);
        for leg in LegId::ALL {
            assert!(t.has_joint(leg, Axis::Lift), "{} lift", leg.name());
            assert!(t.has_joint(leg, Axis::Swing), "{} swing", leg.name());
        }
    }

    #[test]
    fn standing_is_every_servo_at_centre() {
        let t = ServoTable::bom_10_servo();
        let out = t.targets(&Pose::STAND).unwrap();
        for (ch, deg) in out.iter().take(t.len) {
            assert_eq!(*deg, 90.0, "ch{ch}");
        }
    }

    #[test]
    fn a_torn_linkage_is_refused() {
        let t = ServoTable::bom_10_servo();
        let mut p = Pose::STAND;
        p.swing[LegId::L1 as usize] = 20.0;
        p.swing[LegId::L3 as usize] = -20.0;
        let e = t.targets(&p).unwrap_err();
        assert!(e.contains("links L1 and L3"), "{e}");
    }

    #[test]
    fn outside_the_envelope_is_refused_by_name() {
        let t = ServoTable::bom_10_servo();
        let mut p = Pose::STAND;
        p.lift[LegId::R2 as usize] = 90.0; // 90 - 90 = 0° absolute: below min 20
        let e = t.targets(&p).unwrap_err();
        assert!(e.contains("R2 lift"), "{e}");
        assert!(t.check_raw(3, 10.0).is_err());
        assert!(t.check_raw(3, 90.0).is_ok());
        assert!(t.check_raw(11, 90.0).is_err());
    }

    #[test]
    fn pulses_are_standard() {
        assert_eq!(ServoCal::pulse_us(0.0), 500);
        assert_eq!(ServoCal::pulse_us(90.0), 1500);
        assert_eq!(ServoCal::pulse_us(180.0), 2500);
        assert_eq!(ServoCal::pulse_us(999.0), 2500);
    }
}
