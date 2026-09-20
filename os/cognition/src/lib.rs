//! brainos-cognition — Domain 4.
//!
//! The entity does not have agents. It has cognitive resources it
//! escalates to, placed where latency and consequence dictate. The
//! distinction is architectural rather than semantic: an agent is a
//! function with a loop and its own goals; a resource is something the
//! entity reaches for, like a person reaching for a calculator. The
//! entity is the continuous thing; the resource is not.
//!
//! Four tiers, and the boundary that matters is between the first and
//! the rest:
//!
//! ```text
//!   0 Autonomic   < 10 ms   on-body MCU     gait, servo limits, balance
//!   1 Fast        < 100 ms  on-body compute obstacle → replan, intent
//!   2 Deliberate  < 1 s     edge            task planning, skill choice
//!   3 Frontier    seconds   datacenter      novel situations, dreaming
//! ```
//!
//! **Tier 0 never proposes into KIRA** — it runs inside an envelope the
//! gate already granted (see `brainos-rt`). Tiers 1–3 always propose and
//! never act. There is no tier that both thinks and acts.
//!
//! **Placement is Stage 5 and it falls out of this.** A resource is
//! identified by what it can do and how long it takes, not by where it
//! runs. Moving deliberation from the body to a datacenter changes a
//! `Placement` field and nothing else, which is why "the datacenter
//! business arrives as an architectural consequence, not a pivot" is a
//! claim this crate can actually back.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod ambient;
pub mod escalate;
pub mod placement;
pub mod predict;
pub mod resource;
pub mod salience;

pub use ambient::{Ambient, Attention, Intent, SenseFrame};
pub use escalate::{Escalation, Reason as EscalationReason};
pub use placement::{Placement, Plan, Site};
pub use predict::{Forward, Prediction, Surprise};
pub use resource::{Capability as ResourceCapability, Resource, ResourcePool, Tier};
pub use salience::{Salience, Salient};
