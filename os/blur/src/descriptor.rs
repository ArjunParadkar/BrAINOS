//! BFS  LEVEL    Domain 2 Autonomic - the BLUR quadruped body (wire, acquisition, supervisor)  [crate brainos-blur]
//! BFS  PURPOSE  Types and lookups for BLUR v36's design data (26 actuators, motor nodes, buses, springs, torque and range limits); the tables are generated in gen.rs
//! BFS  DEFINES  Bus, MotorKind, EncoderSide, NodeKind, ActuatorSpec, NodeSpec, SpringKind, SpringSpec, PassiveJoint, N_ACT, ENC_BITS, ENC_COUNTS, +4 more
//! BFS  USES     this crate: gen
//! BFS  USED BY  blur/acquire, blur/autonomic, blur/bus, blur/calibrate, blur/census, blur/policy, blur/schema, blur/session, +8 more
//!
//! The body descriptor: BLUR v36 as numbers.
//!
//! The tables themselves are generated (`gen.rs`, from the MJCF by
//! `blur/gen_descriptor.py`); this module is the types and the few
//! derived quantities everything else asks for. The descriptor is the
//! *design* — what a correctly assembled BLUR is. What a particular
//! assembled BLUR turned out to be (which ESC has which silicon UID,
//! which way each motor was wired, where each encoder magnet was glued)
//! is the build sheet and the calibration, and those live in the
//! entity's state graph, never here and never on the body.

/// Which of the Teensy's three CAN controllers a node hangs on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
#[repr(u8)]
pub enum Bus {
    A = 0,
    B = 1,
    C = 2,
}

impl Bus {
    pub const ALL: [Bus; 3] = [Bus::A, Bus::B, Bus::C];
    pub fn from_u8(v: u8) -> Option<Bus> {
        Bus::ALL.get(v as usize).copied()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MotorKind {
    /// 2806.5 1300 KV + 16:1 on a B-G431B-ESC1
    Esc2806,
    /// 4015 hollow gimbal, MT6701 on the motor, 16:1
    Gimbal4015,
    /// 2204 260 KV + 16:1, joint-side MT6701
    Gimbal2204,
    /// 2204 driving a 5 mm cable drum; the "joint" is the cable length (m)
    Winch2204,
    /// N20 gearmotor on a lead screw; no encoder
    N20,
}

impl MotorKind {
    pub const fn name(self) -> &'static str {
        match self {
            MotorKind::Esc2806 => "2806.5+ESC1",
            MotorKind::Gimbal4015 => "4015",
            MotorKind::Gimbal2204 => "2204",
            MotorKind::Winch2204 => "2204-winch",
            MotorKind::N20 => "N20",
        }
    }
}

/// Where the absolute encoder sits, which decides what "absolute" means
/// at power-on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EncoderSide {
    /// on the joint: one reading is the joint angle up to a fixed offset
    Joint,
    /// on the motor, ahead of the gearbox: the joint angle is known only
    /// modulo 2π/gear until it is resolved, every boot
    Motor,
    /// no encoder at all (the gripper)
    None,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeKind {
    /// one B-G431B-ESC1, one motor
    Esc1,
    /// an ESP32-S3 bus node with SimpleFOC drivers, several motors by slot
    Esp32S3,
}

#[derive(Clone, Copy, Debug)]
pub struct ActuatorSpec {
    pub name: &'static str,
    pub motor: MotorKind,
    /// drives a tendon (winch) rather than a joint
    pub tendon: bool,
    /// output torque limit, N·m (N for a winch) — the MJCF forcerange
    pub tau_max: f32,
    /// joint range from the MJCF, rad (m for a winch)
    pub q_min: f32,
    pub q_max: f32,
    /// value at the MJCF `stand` keyframe
    pub q_stand: f32,
    /// output torque per phase amp, N·m/A (N/A for a winch)
    pub kt_out: f32,
    /// no-load output speed on a sagged pack, rad/s (m/s for a winch)
    pub w0: f32,
    /// the driver's peak phase current
    pub drv_peak_a: f32,
    pub gear: f32,
    pub bus: Bus,
    pub node: u8,
    pub slot: u8,
    pub encoder: EncoderSide,
    /// home on the high stop (else the low one): the side the passive load
    /// in the jig pushes toward, computed from the MJCF by the generator
    pub home_high: bool,
    /// winches only: the cable length with no winch force, body held at
    /// the stand pose in the jig — the tail's zero reference (it has no
    /// hard stop). NaN for every other actuator.
    pub q_rest: f32,
}

impl ActuatorSpec {
    /// Torque available at output speed `w`. Flat to half the no-load
    /// speed, then linear to zero at no-load — the shape of a FOC drive
    /// that is current-limited at low speed and voltage-limited at high
    /// speed. The knee at w0/2 is an ASSUMPTION (no winding resistance
    /// in the parts list); it is the one number to replace from a bench
    /// sweep on load day.
    pub fn tau_available(&self, w: f32) -> f32 {
        let a = if w < 0.0 { -w } else { w };
        if a <= 0.5 * self.w0 {
            self.tau_max
        } else if a >= self.w0 {
            0.0
        } else {
            self.tau_max * (self.w0 - a) / (0.5 * self.w0)
        }
    }

    /// Phase current for an output torque, A. NaN for the N20 (no model).
    pub fn current_for(&self, tau: f32) -> f32 {
        let t = if tau < 0.0 { -tau } else { tau };
        t / self.kt_out
    }

    /// Does the driver's peak current reach the MJCF torque limit? False
    /// for the 4015s under the assumed KV and driver — the descriptor
    /// says so instead of quietly granting 3.8 N·m.
    pub fn driver_reaches_tau_max(&self) -> bool {
        let i = self.current_for(self.tau_max);
        !(i > self.drv_peak_a * 1.02)
    }

    /// The torque the driver can actually deliver: the lesser of the
    /// MJCF limit and the driver's peak current.
    pub fn tau_deliverable(&self) -> f32 {
        if self.kt_out.is_nan() {
            return self.tau_max;
        }
        let t = self.drv_peak_a * self.kt_out;
        if t < self.tau_max {
            t
        } else {
            self.tau_max
        }
    }

    pub fn range_contains(&self, q: f32) -> bool {
        q >= self.q_min && q <= self.q_max
    }
}

#[derive(Clone, Copy, Debug)]
pub struct NodeSpec {
    pub id: u8,
    pub bus: Bus,
    pub kind: NodeKind,
    pub name: &'static str,
    pub slots: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpringKind {
    /// compression coilover on a passive girdle joint
    Coilover,
    /// steel extension spring at a distal joint
    Extension,
    /// TPU cable tendon, rear knees
    TpuKneeExtensor,
    /// steel jump spring, slack at full extension
    Jump,
    Other,
}

#[derive(Clone, Copy, Debug)]
pub struct SpringSpec {
    pub name: &'static str,
    pub kind: SpringKind,
    /// N/m for spatial tendons, N·m/rad for jump springs
    pub stiffness: f32,
    pub rest_lo: f32,
    pub rest_hi: f32,
}

/// A joint with no motor: the girdles and the tail segments, sprung.
#[derive(Clone, Copy, Debug)]
pub struct PassiveJoint {
    pub name: &'static str,
    pub stiffness: f32,
    pub springref: f32,
    pub q_min: f32,
    pub q_max: f32,
}

pub use crate::gen::{ACTUATORS, MASS_KG, MJCF_SHA256, NODES, PASSIVE_JOINTS, SPRINGS, V_SAG};

/// How many actuators BLUR has. Every per-joint array is this long.
pub const N_ACT: usize = 26;

/// MT6701 resolution.
pub const ENC_BITS: u32 = 14;
pub const ENC_COUNTS: u32 = 1 << ENC_BITS;

/// Index of an actuator by name.
pub fn actuator_index(name: &str) -> Option<usize> {
    ACTUATORS.iter().position(|a| a.name == name)
}

/// The actuator on (node, slot).
pub fn actuator_at(node: u8, slot: u8) -> Option<usize> {
    ACTUATORS
        .iter()
        .position(|a| a.node == node && a.slot == slot)
}

pub fn node(id: u8) -> Option<&'static NodeSpec> {
    NODES.iter().find(|n| n.id == id)
}

/// Joints whose absolute angle is ambiguous at power-on.
pub fn ambiguous_at_power_on() -> impl Iterator<Item = (usize, &'static ActuatorSpec)> {
    ACTUATORS
        .iter()
        .enumerate()
        .filter(|(_, a)| a.encoder == EncoderSide::Motor)
}
