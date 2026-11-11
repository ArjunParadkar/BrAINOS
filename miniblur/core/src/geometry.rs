//! BFS  LEVEL    Phase 0 - Mini-Blur hexapod as numbers (shared by firmware and seat)  [crate miniblur-core]
//! BFS  PURPOSE  Leg dimensions and forward kinematics: turns swing/lift joint angles into each foot's position in the body frame, for the tip-over check
//! BFS  DEFINES  Vec2, LegGeom
//! BFS  USES     this crate: LegId | external: libm
//! BFS  USED BY  bodyd/hexapod, bodyd/tests/bodyd_suite, core/policy, core/schema, core/stability, host/bench, host/fullstack, host/tests/fullstack_suite, +2 more
//!
//! Leg geometry and forward kinematics — enough to know where each foot is
//! for a given pose, which is all the stability check needs.
//!
//! Body frame: origin at the body centre, +x forward, +y left, +z up, mm.
//! A leg is coxa (yaw at the mount) → femur (pitch) → tibia (fixed, hangs
//! down). Two servos per leg at most: `swing` drives the coxa yaw, `lift` the
//! femur pitch. With `lift = 0` the femur is horizontal and the foot is on
//! the ground at `-tibia_mm`; positive lift raises the foot.

use crate::{LegId, Pose, LEGS};
use libm::{cosf, sinf, sqrtf};

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const fn new(x: f32, y: f32) -> Vec2 {
        Vec2 { x, y }
    }
    pub fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
    pub fn cross(self, o: Vec2) -> f32 {
        self.x * o.y - self.y * o.x
    }
    pub fn len(self) -> f32 {
        sqrtf(self.x * self.x + self.y * self.y)
    }
}

/// Link lengths and body size, mm. Defaults are the chihuahua-scale
/// Mini-Blur from the Technical Execution Report §3.10 — a hypothesis until
/// Nischal's wiring doc v0.1 measures the printed frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LegGeom {
    pub coxa_mm: f32,
    pub femur_mm: f32,
    pub tibia_mm: f32,
    pub body_w_mm: f32,
    pub body_l_mm: f32,
    /// centre of mass offset from the body origin (batteries sit aft)
    pub com: Vec2,
}

impl Default for LegGeom {
    fn default() -> LegGeom {
        LegGeom {
            coxa_mm: 12.0,
            femur_mm: 30.0,
            tibia_mm: 45.0,
            body_w_mm: 70.0,
            body_l_mm: 110.0,
            com: Vec2::new(-5.0, 0.0),
        }
    }
}

const DEG: f32 = core::f32::consts::PI / 180.0;

impl LegGeom {
    /// Where the coxa is mounted and which way it points at swing = 0.
    pub fn mount(&self, leg: LegId) -> (Vec2, f32) {
        let hw = self.body_w_mm * 0.5;
        let hl = self.body_l_mm * 0.5;
        let (x, side) = match leg {
            LegId::L1 => (hl, 1.0),
            LegId::R1 => (hl, -1.0),
            LegId::L2 => (0.0, 1.0),
            LegId::R2 => (0.0, -1.0),
            LegId::L3 => (-hl, 1.0),
            LegId::R3 => (-hl, -1.0),
        };
        // front legs angle forward, rear legs back, middle legs straight out
        let yaw = match leg {
            LegId::L1 => 45.0,
            LegId::R1 => -45.0,
            LegId::L2 => 90.0,
            LegId::R2 => -90.0,
            LegId::L3 => 135.0,
            LegId::R3 => -135.0,
        };
        (Vec2::new(x, side * hw), yaw)
    }

    /// Foot position for one leg: (xy in the body frame, z below the body).
    /// `swing_deg` and `lift_deg` are relative to the calibrated centre.
    pub fn foot(&self, leg: LegId, swing_deg: f32, lift_deg: f32) -> (Vec2, f32) {
        let (m, yaw0) = self.mount(leg);
        let yaw = (yaw0 + swing_deg) * DEG;
        let lift = lift_deg * DEG;
        let reach = self.coxa_mm + self.femur_mm * cosf(lift);
        let z = -self.tibia_mm + self.femur_mm * sinf(lift);
        (Vec2::new(m.x + reach * cosf(yaw), m.y + reach * sinf(yaw)), z)
    }

    /// All six feet for a pose.
    pub fn feet(&self, pose: &Pose) -> [(Vec2, f32); LEGS] {
        let mut out = [(Vec2::default(), 0.0); LEGS];
        for (i, leg) in LegId::ALL.iter().enumerate() {
            out[i] = self.foot(*leg, pose.swing[i], pose.lift[i]);
        }
        out
    }

    /// Standing foot height (lift = 0).
    pub fn ground_z(&self) -> f32 {
        -self.tibia_mm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standing_feet_are_on_the_ground_and_outside_the_body() {
        let g = LegGeom::default();
        for (leg, (xy, z)) in LegId::ALL.iter().zip(g.feet(&Pose::STAND).iter()) {
            assert!((z - g.ground_z()).abs() < 1e-4, "{}: z={z}", leg.name());
            assert!(xy.len() > g.body_w_mm * 0.5, "{} foot inside the body", leg.name());
            assert_eq!(xy.y > 0.0, leg.is_left(), "{} on the wrong side", leg.name());
        }
    }

    #[test]
    fn lifting_raises_the_foot_and_shortens_reach() {
        let g = LegGeom::default();
        let (xy0, z0) = g.foot(LegId::L2, 0.0, 0.0);
        let (xy1, z1) = g.foot(LegId::L2, 0.0, 30.0);
        assert!(z1 > z0);
        assert!(xy1.len() < xy0.len());
    }

    #[test]
    fn swing_moves_the_foot_forward_on_both_sides() {
        let g = LegGeom::default();
        // positive swing = toward the front for a left leg; mirrored for right
        let (l0, _) = g.foot(LegId::L2, 0.0, 0.0);
        let (l1, _) = g.foot(LegId::L2, -20.0, 0.0);
        assert!(l1.x > l0.x, "left leg: negative swing should move forward");
        let (r0, _) = g.foot(LegId::R2, 0.0, 0.0);
        let (r1, _) = g.foot(LegId::R2, 20.0, 0.0);
        assert!(r1.x > r0.x, "right leg: positive swing should move forward");
    }
}
