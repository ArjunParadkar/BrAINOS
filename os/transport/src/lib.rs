//! brainos-transport — the nerve, not the mind.
//!
//! This crate carries bytes between an entity and a body and knows
//! nothing about what they mean. That separation is the MHS insight
//! turned into a module boundary: *nerves without a self are a puppet,
//! and wiring is exactly what we should never build ourselves* — but the
//! wiring we do build for our own bodies should at least be honest
//! about what it guarantees.
//!
//! Four guarantees, and no others:
//!
//! 1. **Framing.** A frame is delivered whole or not at all. A torn
//!    frame is detected, not half-parsed.
//! 2. **Integrity.** Every frame carries a MAC under a per-session key
//!    derived from the Key. A frame from something that does not hold
//!    that key does not arrive.
//! 3. **Freshness.** Every frame carries a counter. A replayed frame is
//!    refused. This matters physically: replaying a "step forward"
//!    command is how a robot walks off a table.
//! 4. **Liveness.** The link knows when it last heard from the other
//!    side, and says so. Everything above it decides what to do about
//!    that; this layer never decides to stop a body, because deciding is
//!    not a nerve's job.
//!
//! What it deliberately does **not** provide: ordering beyond the
//! counter, retransmission, or confidentiality. A body's command stream
//! is not secret — it is observable by anyone in the room with eyes —
//! and building encryption here would add a key schedule, a nonce
//! discipline and a failure mode, in exchange for hiding something that
//! is not hidden.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod frame;
pub mod link;
pub mod loopback;
pub mod session;

pub use frame::{Frame, FrameError, Kind, MAX_PAYLOAD};
pub use link::{Health, Link, LinkStats};
pub use loopback::Loopback;
pub use session::{Peer, SessionKey, Wire, WireError};
