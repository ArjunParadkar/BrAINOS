//! An arm — the second mechanism, and the reason the trait exists.
//!
//! The hexapod and this share a runtime, a wire, an envelope model, a
//! watchdog set and a refusal vocabulary. They share no physics at all.
//! A hexapod's invariant is a convex hull of feet; an arm's is a torque
//! budget and a workspace boundary. A hexapod holds torque when it stops
//! because releasing means falling; an arm releases because holding
//! means pressing. If `Mechanism` is the right abstraction, adding this
//! is four methods and no changes anywhere else — and it was.
//!
//! **The physics, and where it is honest about being a model.** A planar
//! `N`-link chain. Forward kinematics is exact. The torque check is the
//! static moment about the shoulder — payload and link masses times
//! their horizontal distances — which is the right first-order model for
//! an arm moving slowly and the *wrong* one for an arm moving fast,
//! because it ignores the inertial term entirely. That is stated here
//! rather than discovered later: this mechanism refuses fast motions
//! precisely because its model does not cover them.
//!
//! **Self-collision** is the crude version that is actually worth
//! having: the end effector may not come within a radius of the base
//! column. A full swept-volume check needs a mesh nobody has measured,
//! and a check that claims more than it can do is worse than one that
//! admits its shape.

use crate::mechanism::{Mechanism, Motion, Refusal, Sample};
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::time::Duration;
use brainos_abi::verb::Verb;
use brainos_rt::envelope::{Channel, Envelope, Limit};
use libm::{cosf, fabsf, sinf, sqrtf};

/// The largest chain this mechanism models.
pub const MAX_JOINTS: usize = 6;

const DEG: f32 = core::f32::consts::PI / 180.0;
const G: f32 = 9.81;

/// One revolute joint.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Joint {
    /// link length from this joint to the next, metres
    pub link_m: f32,
    /// mass of that link, kilograms
    pub mass_kg: f32,
    /// the angle envelope, degrees
    pub min_deg: f32,
    pub max_deg: f32,
    /// what this joint's actuator can hold, newton-metres
    pub torque_nm: f32,
}

impl Joint {
    pub fn is_sane(&self) -> bool {
        self.link_m > 0.0
            && self.link_m < 10.0
            && self.mass_kg >= 0.0
            && self.min_deg < self.max_deg
            && self.torque_nm > 0.0
            && [self.link_m, self.mass_kg, self.min_deg, self.max_deg, self.torque_nm]
                .iter()
                .all(|v| v.is_finite())
    }

    pub fn admits(&self, deg: f32) -> bool {
        deg.is_finite() && deg >= self.min_deg && deg <= self.max_deg
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ArmConfig {
    pub joints: [Joint; MAX_JOINTS],
    pub used: usize,
    /// what is currently in the gripper, kilograms
    pub payload_kg: f32,
    /// the end effector may not come within this of the base column
    pub keep_out_m: f32,
    /// the fastest joint rate the static torque model is trusted for
    pub max_rate_dps: f32,
    /// how many samples a motion is broken into
    pub samples: usize,
}

impl ArmConfig {
    /// A Doosan-M0609-shaped arm: six joints, about 900 mm reach, 6 kg
    /// payload. The numbers are plausible rather than measured, which is
    /// the same status the Mini-Blur geometry has until the frame is
    /// measured, and is labelled the same way.
    pub fn collaborative_six_axis() -> ArmConfig {
        let j = |link_m, mass_kg, min_deg, max_deg, torque_nm| Joint {
            link_m,
            mass_kg,
            min_deg,
            max_deg,
            torque_nm,
        };
        ArmConfig {
            joints: [
                j(0.17, 4.0, -360.0, 360.0, 180.0),
                j(0.44, 6.0, -95.0, 95.0, 180.0),
                j(0.35, 3.5, -145.0, 145.0, 90.0),
                j(0.12, 1.5, -360.0, 360.0, 40.0),
                j(0.10, 1.0, -135.0, 135.0, 40.0),
                j(0.06, 0.5, -360.0, 360.0, 20.0),
            ],
            used: 6,
            payload_kg: 0.0,
            keep_out_m: 0.15,
            max_rate_dps: 90.0,
            samples: 24,
        }
    }

    pub fn is_sane(&self) -> bool {
        self.used > 0
            && self.used <= MAX_JOINTS
            && self.joints[..self.used].iter().all(|j| j.is_sane())
            && self.payload_kg >= 0.0
            && self.keep_out_m > 0.0
            && self.max_rate_dps > 0.0
            && self.samples > 0
    }

    /// Total reach with every joint straight.
    pub fn reach_m(&self) -> f32 {
        self.joints[..self.used].iter().map(|j| j.link_m).sum()
    }
}

/// An arm.
pub struct Arm {
    pub config: ArmConfig,
    /// where the joints are now, degrees
    pose: [f32; MAX_JOINTS],
    /// whether the entity has pushed a calibration down
    calibrated: bool,
}

impl Arm {
    pub fn new(config: ArmConfig) -> Arm {
        Arm {
            config,
            pose: [0.0; MAX_JOINTS],
            calibrated: false,
        }
    }

    /// §8 step 5. As with the hexapod, the body holds this only while
    /// powered; the entity's state graph is where it lives.
    pub fn calibrate(&mut self, home: [f32; MAX_JOINTS]) -> bool {
        for i in 0..self.config.used {
            if !self.config.joints[i].admits(home[i]) {
                return false;
            }
        }
        self.pose = home;
        self.calibrated = true;
        true
    }

    pub fn is_calibrated(&self) -> bool {
        self.calibrated
    }

    pub fn forget_calibration(&mut self) {
        self.calibrated = false;
    }

    pub fn pose(&self) -> [f32; MAX_JOINTS] {
        self.pose
    }

    /// Where each joint sits, in the plane. Exact forward kinematics for
    /// a planar chain: angles accumulate, each link steps from the last.
    pub fn joint_positions(&self, pose: &[f32; MAX_JOINTS]) -> [(f32, f32); MAX_JOINTS] {
        let mut out = [(0.0f32, 0.0f32); MAX_JOINTS];
        let mut angle = 0.0f32;
        let mut x = 0.0f32;
        let mut y = 0.0f32;
        for i in 0..self.config.used {
            angle += pose[i] * DEG;
            x += self.config.joints[i].link_m * cosf(angle);
            y += self.config.joints[i].link_m * sinf(angle);
            out[i] = (x, y);
        }
        out
    }

    pub fn end_effector(&self, pose: &[f32; MAX_JOINTS]) -> (f32, f32) {
        self.joint_positions(pose)[self.config.used - 1]
    }

    /// The static moment about the shoulder, newton-metres.
    ///
    /// Payload and link masses times their horizontal distances. This
    /// ignores the inertial term entirely, which is why the mechanism
    /// refuses motions faster than `max_rate_dps` — a model that does
    /// not cover a regime should not be asked about it.
    pub fn shoulder_torque_nm(&self, pose: &[f32; MAX_JOINTS]) -> f32 {
        let positions = self.joint_positions(pose);
        let mut moment = 0.0f32;
        let mut previous_x = 0.0f32;
        for i in 0..self.config.used {
            // a link's mass acts at its midpoint
            let midpoint_x = (previous_x + positions[i].0) * 0.5;
            moment += self.config.joints[i].mass_kg * G * fabsf(midpoint_x);
            previous_x = positions[i].0;
        }
        let (ex, _) = positions[self.config.used - 1];
        moment += self.config.payload_kg * G * fabsf(ex);
        moment
    }

    /// Does the end effector clear the base column?
    pub fn clears_the_column(&self, pose: &[f32; MAX_JOINTS]) -> bool {
        let (x, y) = self.end_effector(pose);
        sqrtf(x * x + y * y) >= self.config.keep_out_m
    }

    fn arg(args: &[(&str, f32)], name: &str) -> Option<f32> {
        args.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
    }

    /// Check one pose against everything the arm knows.
    fn check_pose(&self, pose: &[f32; MAX_JOINTS]) -> Result<f32, Refusal> {
        for i in 0..self.config.used {
            if !self.config.joints[i].admits(pose[i]) {
                return Err(Refusal::OutsideEnvelope);
            }
        }
        if !self.clears_the_column(pose) {
            return Err(Refusal::WouldBreakInvariant);
        }
        let torque = self.shoulder_torque_nm(pose);
        if torque > self.config.joints[0].torque_nm {
            return Err(Refusal::WouldBreakInvariant);
        }
        // margin: how much torque is left, as the number the runtime
        // reports and the invariant re-checks per sample
        Ok(self.config.joints[0].torque_nm - torque)
    }

    /// Interpolate from the current pose to a target, checking every
    /// step. A motion is only accepted if every pose along it is legal —
    /// an arm that checked only its endpoints would happily swing
    /// through its own column.
    fn interpolate(
        &self,
        verb: Verb,
        target: [f32; MAX_JOINTS],
        rate_dps: f32,
        what: &str,
    ) -> Result<Motion, Refusal> {
        if !self.calibrated {
            return Err(Refusal::Uncalibrated);
        }
        if !rate_dps.is_finite() || rate_dps <= 0.0 {
            return Err(Refusal::Malformed);
        }
        if rate_dps > self.config.max_rate_dps {
            // the static torque model does not cover this regime
            return Err(Refusal::OutsideEnvelope);
        }

        let travel = (0..self.config.used)
            .map(|i| fabsf(target[i] - self.pose[i]))
            .fold(0.0f32, f32::max);
        if travel <= 0.0 {
            return Err(Refusal::Malformed);
        }
        let seconds = travel / rate_dps;
        if seconds > 10.0 {
            return Err(Refusal::TooLong);
        }

        let n = self.config.samples;
        let hold = Duration::from_millis(((seconds * 1000.0) as u64 / n as u64).max(1));
        let mut samples = Vec::with_capacity(n);
        let mut worst = f32::INFINITY;

        for step in 1..=n {
            let t = step as f32 / n as f32;
            let mut pose = [0.0f32; MAX_JOINTS];
            for i in 0..self.config.used {
                pose[i] = self.pose[i] + (target[i] - self.pose[i]) * t;
            }
            let margin = self.check_pose(&pose)?;
            if margin < worst {
                worst = margin;
            }
            let largest = (0..self.config.used)
                .map(|i| fabsf(pose[i]))
                .fold(0.0f32, f32::max);
            samples.push(
                Sample {
                    index: step - 1,
                    hold,
                    commands: [None; 8],
                    margin,
                }
                .with(Channel::Angle, largest)
                .with(Channel::Rate, rate_dps),
            );
        }

        Ok(Motion {
            verb,
            duration: Duration::from_millis((seconds * 1000.0) as u64),
            worst_margin: worst,
            summary: alloc::format!(
                "{what}: {} samples over {:.1} s, worst torque headroom {:.1} Nm",
                n,
                seconds,
                worst
            ),
            samples,
        })
    }
}

impl Mechanism for Arm {
    fn name(&self) -> &'static str {
        "arm"
    }

    fn channels(&self) -> &[Channel] {
        &[Channel::Angle, Channel::Rate]
    }

    fn plan(&self, verb: &Verb, args: &[(&str, f32)]) -> Result<Motion, Refusal> {
        match verb.as_str() {
            "arm.move_joint" => {
                let joint = Arm::arg(args, "joint").ok_or(Refusal::Malformed)?;
                let angle = Arm::arg(args, "angle").ok_or(Refusal::Malformed)?;
                let rate = Arm::arg(args, "speed").unwrap_or(30.0);
                if !joint.is_finite() || !angle.is_finite() {
                    return Err(Refusal::Malformed);
                }
                let idx = joint as usize;
                if joint < 0.0 || idx >= self.config.used {
                    return Err(Refusal::OutsideEnvelope);
                }
                let mut target = self.pose;
                target[idx] = angle;
                self.interpolate(*verb, target, rate, "move one joint")
            }
            "arm.home" => {
                let rate = Arm::arg(args, "speed").unwrap_or(20.0);
                // home is every joint at zero, which for this geometry is
                // fully extended — the worst case for the torque budget,
                // and therefore the motion most likely to be refused when
                // something heavy is in the gripper
                self.interpolate(*verb, [0.0; MAX_JOINTS], rate, "go home")
            }
            "arm.stow" => {
                let rate = Arm::arg(args, "speed").unwrap_or(20.0);
                let mut target = [0.0f32; MAX_JOINTS];
                // folded back over itself, which brings the end effector
                // in toward the column and is exactly the motion the
                // keep-out radius exists to bound
                for i in 0..self.config.used {
                    target[i] = self.config.joints[i].max_deg.min(90.0) * 0.8;
                }
                self.interpolate(*verb, target, rate, "stow")
            }
            _ => Err(Refusal::UnknownVerb),
        }
    }

    fn invariant(&self, sample: &Sample) -> Result<(), Refusal> {
        // the margin is torque headroom in newton-metres; a sample that
        // has run out of it would be the arm holding more than the
        // shoulder can
        if sample.margin.is_finite() && sample.margin <= 0.0 {
            return Err(Refusal::WouldBreakInvariant);
        }
        Ok(())
    }

    fn safe(&self) -> Sample {
        // An arm releases. Holding a pose against gravity with whatever
        // is in the gripper is how an arm that stopped safely cooks its
        // shoulder actuator — and unlike a hexapod, letting go does not
        // make it fall over.
        Sample::hold_for(0, Duration::from_millis(100))
            .with(Channel::Rate, 0.0)
    }

    fn holds_itself_up(&self) -> bool {
        false
    }

    fn native_envelope(&self, id: u16) -> Envelope {
        let mut e = Envelope::new(
            id,
            Duration::from_millis(10),
            Duration::from_millis(100),
        );
        // the widest angle any joint can reach, from the joint table
        // rather than from a number somebody typed
        let (mut lo, mut hi) = (0.0f32, 0.0f32);
        for j in &self.config.joints[..self.config.used] {
            lo = lo.min(j.min_deg);
            hi = hi.max(j.max_deg);
        }
        e.bound(Channel::Angle, Limit::new(lo, hi));
        e.bound(Channel::Rate, Limit::new(0.0, self.config.max_rate_dps));
        e
    }
}

/// A description of this body in the vocabulary §8 speaks, for the case
/// where the arm is ours rather than a manufacturer's.
pub fn native_schema(config: &ArmConfig) -> brainos_abi::error::Result<String> {
    let mut s = String::new();
    s.push_str("arm with ");
    let mut n = config.used;
    let mut buf = [0u8; 4];
    let mut i = 0;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        s.push(buf[i] as char);
    }
    s.push_str(" joints");
    Ok(s)
}
