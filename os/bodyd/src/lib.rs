//! brainos-bodyd — what runs on the machine with the motors.
//!
//! Everything else in this workspace runs at the seat: the entity, its
//! memory, its gate. This crate runs on the other end of the wire, on a
//! microcontroller, next to the servos, and it has one job that is worth
//! stating as a refusal rather than a capability:
//!
//! > **It checks the same envelope the seat checked, and refuses
//! > anyway.**
//!
//! That duplication is the design, not an oversight. KIRA at the seat
//! evaluates a proposal against the body's declared limits and mints a
//! token; the body then evaluates the *same numbers* before it moves.
//! A compromised seat, a corrupted token, a bug in the gate — none of
//! them can push the body past what its own reflex layer allows, because
//! the reflex layer is not asking the seat's opinion.
//!
//! **Nothing here thinks.** There is no memory, no model, no state graph.
//! The body holds calibration only while powered, because the
//! Memory-Integrity Law says memory lives in the entity and a body that
//! remembered its own calibration would be a body whose memory the
//! entity does not control. On every power-on the entity pushes it back
//! down.
//!
//! **Nothing here decides what to do.** It receives a motion, plays it
//! inside an envelope, and stops. Stopping is the only escalation it
//! has, and it is enough: the seat notices through ordinary telemetry
//! that the body stopped and why, and cognition decides what to do about
//! it with all the time in the world, because the body is already safe.
//!
//! The hexapod mechanism links `miniblur-core` — the same crate the
//! ESP32-S3 firmware links, with the same geometry, the same tripod
//! gait, and the same convex-hull tip-over check. What is tested here is
//! what would run on the board.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod arm;
pub mod hexapod;
pub mod mechanism;
pub mod protocol;
pub mod runtime;

pub use arm::{Arm, ArmConfig, Joint};
pub use hexapod::{Hexapod, HexapodConfig};
pub use mechanism::{Mechanism, Motion, Refusal, Sample};
pub use protocol::{Command, Report, Wire as Protocol};
pub use runtime::{BodyRuntime, Pass, State};
