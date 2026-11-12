//! BFS  LEVEL    Domain 2 Autonomic - the runtime on the body's own computer  [crate brainos-bodyd]
//! BFS  PURPOSE  A small five-joint desk arm with a two-finger gripper, shaped like a LeRobot SO-101: its dimensions, forward kinematics and closed-form inverse kinematics
//! BFS  DEFINES  PAN, SHOULDER, ELBOW, WRIST, ROLL, GRIP, DOF, Pose, DeskArmConfig, Tool
//! BFS  USES     external: libm
//! BFS  USED BY  bodyd/armloop, bodyd/selfmodel, host/motor, host/motor_mind, host/motor_seat, host/tests/motor_mandate
//!
//! A desk arm with a gripper: pan, shoulder, elbow, wrist pitch, wrist
//! roll, and two fingers that move as one.
//!
//! **Every number here is plausible, not measured.** The layout is the
//! LeRobot SO-101's; the lengths, masses and torques are round figures of
//! the right size, in the same standing as the Mini-Blur geometry until
//! somebody puts a ruler on a real one. The first real-arm test replaces
//! this table with measurements before anything else.
//!
//! The layout is chosen so the inverse kinematics has a closed form: the
//! pan joint points the arm, and the next three joints all pitch in one
//! vertical plane. No iteration, no Jacobian, nothing that can fail to
//! converge on a microcontroller.

use libm::{acosf, atan2f, cosf, sinf, sqrtf};

const DEG: f32 = core::f32::consts::PI / 180.0;

/// Joint order, everywhere a desk-arm pose is an array.
pub const PAN: usize = 0;
pub const SHOULDER: usize = 1;
pub const ELBOW: usize = 2;
pub const WRIST: usize = 3;
pub const ROLL: usize = 4;
pub const GRIP: usize = 5;
pub const DOF: usize = 6;

/// A pose: five angles in degrees, then the finger opening (each
/// finger's travel from the centre line) in metres.
pub type Pose = [f32; DOF];

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DeskArmConfig {
    /// height of the shoulder axis above the table
    pub shoulder_h_m: f32,
    pub upper_m: f32,
    pub fore_m: f32,
    /// wrist pitch axis to the roll joint
    pub wrist_m: f32,
    /// roll joint to where the fingers start
    pub palm_m: f32,
    pub finger_m: f32,
    /// each finger's travel from the centre line
    pub grip_max_m: f32,
    /// degrees: [min, max] for the five revolute joints
    pub limits: [(f32, f32); 5],
    pub torque_nm: f32,
    pub rate_dps: f32,
    pub grip_force_n: f32,
    pub grip_rate_mps: f32,
    pub link_mass_kg: f32,
    pub hand_mass_kg: f32,
}

impl Default for DeskArmConfig {
    fn default() -> DeskArmConfig {
        DeskArmConfig {
            shoulder_h_m: 0.11,
            upper_m: 0.12,
            fore_m: 0.14,
            wrist_m: 0.03,
            palm_m: 0.03,
            finger_m: 0.06,
            grip_max_m: 0.045,
            limits: [
                (-150.0, 150.0),
                (0.0, 180.0),
                (-170.0, 0.0),
                (-120.0, 120.0),
                (-180.0, 180.0),
            ],
            torque_nm: 2.9,
            rate_dps: 270.0,
            grip_force_n: 20.0,
            grip_rate_mps: 0.10,
            link_mass_kg: 0.10,
            hand_mass_kg: 0.12,
        }
    }
}

/// Where the tool is.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Tool {
    /// the grasp centre, metres, in the base frame (table top is z = 0)
    pub pos: [f32; 3],
    /// pitch of the tool axis from horizontal, degrees; -90 is straight down
    pub pitch_deg: f32,
    /// heading of the line between the fingers, degrees, seen from above;
    /// only meaningful when the tool points down
    pub finger_yaw_deg: f32,
}

impl DeskArmConfig {
    /// Wrist pitch axis to the grasp centre.
    pub fn tool_m(&self) -> f32 {
        self.wrist_m + self.palm_m + 0.6 * self.finger_m
    }

    pub fn admits(&self, q: &Pose) -> bool {
        (0..5).all(|i| {
            q[i].is_finite() && q[i] >= self.limits[i].0 && q[i] <= self.limits[i].1
        }) && q[GRIP].is_finite()
            && q[GRIP] >= 0.0
            && q[GRIP] <= self.grip_max_m
    }

    /// Forward kinematics: exact.
    pub fn tool(&self, q: &Pose) -> Tool {
        let a1 = q[SHOULDER] * DEG;
        let a12 = a1 + q[ELBOW] * DEG;
        let phi = a12 + q[WRIST] * DEG;
        let r = self.upper_m * cosf(a1) + self.fore_m * cosf(a12) + self.tool_m() * cosf(phi);
        let z = self.shoulder_h_m
            + self.upper_m * sinf(a1)
            + self.fore_m * sinf(a12)
            + self.tool_m() * sinf(phi);
        let pan = q[PAN] * DEG;
        Tool {
            pos: [r * cosf(pan), r * sinf(pan), z],
            pitch_deg: phi / DEG,
            finger_yaw_deg: q[PAN] + 90.0 - q[ROLL],
        }
    }

    /// Inverse kinematics, closed form, elbow up. `None` when the point
    /// is out of reach or the answer is outside a joint limit: this
    /// never returns the nearest pose it can manage, because "nearly
    /// where you asked" is how an arm ends up inside something.
    pub fn solve(&self, want: &Tool, grip_m: f32) -> Option<Pose> {
        let [x, y, z] = want.pos;
        let phi = want.pitch_deg * DEG;
        let r = sqrtf(x * x + y * y);
        if r < 1e-4 {
            return None;
        }
        let pan = atan2f(y, x);
        let rw = r - self.tool_m() * cosf(phi);
        let zw = z - self.shoulder_h_m - self.tool_m() * sinf(phi);
        let d2 = rw * rw + zw * zw;
        let c = (d2 - self.upper_m * self.upper_m - self.fore_m * self.fore_m)
            / (2.0 * self.upper_m * self.fore_m);
        if !(-1.0..=1.0).contains(&c) {
            return None;
        }
        let a2 = -acosf(c);
        let a1 = atan2f(zw, rw) - atan2f(self.fore_m * sinf(a2), self.upper_m + self.fore_m * cosf(a2));
        let a3 = phi - a1 - a2;
        // the fingers are symmetric, so the roll is only defined up to a
        // half turn; take the one nearest zero
        let mut roll = pan / DEG + 90.0 - want.finger_yaw_deg;
        while roll > 90.0 {
            roll -= 180.0;
        }
        while roll < -90.0 {
            roll += 180.0;
        }
        let q = [pan / DEG, a1 / DEG, a2 / DEG, a3 / DEG, roll, grip_m];
        if self.admits(&q) {
            Some(q)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solving_then_moving_lands_where_asked() {
        let c = DeskArmConfig::default();
        let mut solved = 0;
        for i in 0..400 {
            let f = i as f32;
            let want = Tool {
                pos: [
                    0.10 + 0.12 * ((f * 0.37) % 1.0),
                    -0.12 + 0.24 * ((f * 0.61) % 1.0),
                    0.01 + 0.10 * ((f * 0.83) % 1.0),
                ],
                pitch_deg: -90.0,
                finger_yaw_deg: -80.0 + 160.0 * ((f * 0.29) % 1.0),
            };
            let Some(q) = c.solve(&want, 0.02) else { continue };
            solved += 1;
            let got = c.tool(&q);
            for k in 0..3 {
                assert!((got.pos[k] - want.pos[k]).abs() < 2e-5, "{got:?} vs {want:?}");
            }
            assert!((got.pitch_deg + 90.0).abs() < 1e-2);
            let d = (got.finger_yaw_deg - want.finger_yaw_deg).rem_euclid(180.0);
            assert!(d < 1e-2 || d > 180.0 - 1e-2, "yaw {d}");
        }
        assert!(solved > 300, "only {solved} of 400 reachable");
    }

    #[test]
    fn out_of_reach_is_refused_not_approximated() {
        let c = DeskArmConfig::default();
        let far = Tool { pos: [0.60, 0.0, 0.05], pitch_deg: -90.0, finger_yaw_deg: 0.0 };
        assert!(c.solve(&far, 0.0).is_none());
        let on_the_axis = Tool { pos: [0.0, 0.0, 0.2], pitch_deg: -90.0, finger_yaw_deg: 0.0 };
        assert!(c.solve(&on_the_axis, 0.0).is_none());
    }
}
