//! BFS  LEVEL    Phase 0 - Mini-Blur hexapod as numbers (shared by firmware and seat)  [crate miniblur-core]
//! BFS  PURPOSE  Crate root for the hexapod model shared by firmware and seat (legs, poses, gait, limits, wire format) so both run identical safety checks
//! BFS  DEFINES  gait, geometry, json, policy, power, runtime, schema, servo, stability, watchdog, wire, LEGS, +5 more
//! BFS  USES     nothing outside core/alloc
//! BFS  USED BY  crate bodyd, crate host, crate miniblur-a, crate operator, crate proof, crate sim
//!
//! miniblur-core — the Phase 0 body as numbers (Execution Plan §4).
//!
//! Everything a hexapod's Domain 2 needs to keep the body alive, and
//! everything the seat's KIRA needs to refuse a step before it is sent:
//! leg geometry, the servo table, the tripod gait, the tip-over and current
//! invariants, the lost-link watchdog, and the wire codec the body speaks to
//! the seat. `no_std`, one pure-Rust dependency (`libm`), fixed-size tables —
//! it links into the ESP32-S3 firmware and into the operator unchanged.
//!
//! The rule this crate exists for: **the same envelope is checked twice.**
//! KIRA at the seat runs `policy::validate/simulate/policy` before granting;
//! the board runs the same functions in its reflex loop before moving. A
//! compromised seat cannot push the body past what its own reflexes allow.
//!
//! Nothing here remembers on the entity's behalf (§13.2). Calibration is a
//! value handed in at incorporation; the body holds it only while powered.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod gait;
pub mod geometry;
pub mod json;
pub mod policy;
pub mod power;
pub mod runtime;
pub mod schema;
pub mod servo;
pub mod stability;
pub mod watchdog;
pub mod wire;

/// Six legs, fixed order. Tripod A = {L1, R2, L3}; tripod B = {R1, L2, R3}.
pub const LEGS: usize = 6;
/// Servo channels the table may name (MCPWM on the S3 gives 12; the BOM has 10).
pub const MAX_SERVOS: usize = 12;

/// Leg indices in gait order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum LegId {
    L1 = 0,
    R1 = 1,
    L2 = 2,
    R2 = 3,
    L3 = 4,
    R3 = 5,
}

impl LegId {
    pub const ALL: [LegId; LEGS] = [LegId::L1, LegId::R1, LegId::L2, LegId::R2, LegId::L3, LegId::R3];
    pub fn name(self) -> &'static str {
        match self {
            LegId::L1 => "L1",
            LegId::R1 => "R1",
            LegId::L2 => "L2",
            LegId::R2 => "R2",
            LegId::L3 => "L3",
            LegId::R3 => "R3",
        }
    }
    pub fn from_name(s: &str) -> Option<LegId> {
        LegId::ALL.iter().copied().find(|l| l.name() == s)
    }
    /// Tripod A lifts in the first half of the cycle, B in the second.
    pub fn tripod(self) -> Tripod {
        match self {
            LegId::L1 | LegId::R2 | LegId::L3 => Tripod::A,
            LegId::R1 | LegId::L2 | LegId::R3 => Tripod::B,
        }
    }
    pub fn is_left(self) -> bool {
        matches!(self, LegId::L1 | LegId::L2 | LegId::L3)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tripod {
    A,
    B,
}

/// A joint axis on a leg. `Swing` is the coxa yaw (forward/back), `Lift` the
/// femur pitch (up/down). A 1-DOF leg has only `Lift`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    Swing,
    Lift,
}

impl Axis {
    pub fn name(self) -> &'static str {
        match self {
            Axis::Swing => "swing",
            Axis::Lift => "lift",
        }
    }
    pub fn from_name(s: &str) -> Option<Axis> {
        match s {
            "swing" => Some(Axis::Swing),
            "lift" => Some(Axis::Lift),
            _ => None,
        }
    }
}

/// Joint targets for the whole body, in degrees **from each joint's
/// calibrated center** (so they are body-relative, not servo-relative).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Pose {
    pub swing: [f32; LEGS],
    pub lift: [f32; LEGS],
}

impl Pose {
    pub const STAND: Pose = Pose { swing: [0.0; LEGS], lift: [0.0; LEGS] };
    /// Sitting: every femur pitched down so the body rests on the ground.
    pub fn sit(lift_down_deg: f32) -> Pose {
        Pose { swing: [0.0; LEGS], lift: [-lift_down_deg; LEGS] }
    }
}
