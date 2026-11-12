//! BFS  LEVEL    Section 8 Acquisition - a device becomes a region of the body  [crate brainos-body]
//! BFS  PURPOSE  Crate root: lists the modules that turn a device into a region of the body and re-exports their main types
//! BFS  DEFINES  acquire, calibration, hal, policy, region, schema, vital
//! BFS  USES     this crate: acquire, calibration, hal, policy, region, schema, vital
//! BFS  USED BY  crate adapters, crate blur, crate bodyd, crate bpu, crate entity, crate fleet, crate host, crate kira, +4 more
//!
//! brainos-body — devices as regions you become.
//!
//! This replaces device drivers. A device is not a tool the entity uses;
//! it is a region of the body the entity *is*, the way a brain learns it
//! has a hand. §8 is the protocol by which that happens, and the whole
//! point of this crate is that it is **one** protocol: a hexapod we
//! built, a ROS 2 robot we did not, and an MHS lab instrument all arrive
//! through the same five steps, and the difference between them is an
//! adapter that produces a `CapabilitySchema`, not a special case in the
//! acquisition logic.
//!
//! ```text
//!   1 DISCOVER     find a device
//!   2 HANDSHAKE    attest identities, both directions
//!   3 SCHEMA       read what it can do, and within what limits
//!   4 INCORPORATE  bind a KIRA policy and an envelope; the entity now has a limb
//!   5 CALIBRATE    learn this specific body; the result goes in the state graph
//! ```
//!
//! Two properties are load-bearing and are enforced by types here rather
//! than by care:
//!
//! **A body's identity includes its capabilities.** `BodyId` is the hash
//! of the attestation key *and* the schema digest. A device that comes
//! back advertising a verb it did not have before is a different body and
//! must be acquired again. The alternative — trusting a reconnecting
//! device's new claims — is how a limb silently acquires an ability
//! nobody authorized.
//!
//! **Calibration belongs to the entity, not the body.** The body holds
//! it only while powered (the Memory-Integrity Law). Step 5 writes it
//! into the state graph and pushes a copy down; on the next power-on the
//! entity pushes it again. A body that is replaced by an identical one
//! gets the same calibration, and a body that remembers its own
//! calibration is a body whose memory the entity does not control.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod acquire;
pub mod calibration;
pub mod hal;
pub mod policy;
pub mod region;
pub mod schema;
pub mod vital;

pub use acquire::{Acquisition, Step, StepError};
pub use calibration::{Calibration, JointTrim};
pub use hal::{Actuator, Command, Reading, Sensor, Telemetry};
pub use policy::{BodyPolicy, Posture, Refusal};
pub use region::{BodyMap, Region, RegionClass};
pub use schema::{CapabilitySchema, ParamKind, ParamSpec, VerbSpec};
pub use vital::{Band, Felt, Health};
