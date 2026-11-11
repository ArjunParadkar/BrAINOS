//! BFS  LEVEL    Section 3.8 - the three worlds (self, human, internet)  [crate brainos-worlds]
//! BFS  PURPOSE  Crate root for the three worlds the entity deals with (itself, its human, the internet): origin-tagging of incoming data and message filtering
//! BFS  DEFINES  advocate, ingress, world
//! BFS  USES     this crate: advocate, ingress, world
//! BFS  USED BY  crate entity, crate host, crate nostd
//!
//! brainos-worlds — self, internet, human.
//!
//! The entity inhabits three worlds with distinct trust posture. The
//! internet is treated as an alternate universe it *visits*, not a place
//! it lives: network access is off by default and enabled per session.
//!
//! Two things live here.
//!
//! **Ingress.** Everything arriving from outside the entity is tainted
//! with its origin before it can become a memory or a proposal. This is
//! the mechanical half of the prompt-injection defence — the other half
//! is in `brainos-memory`, where an origin's confidence ceiling and
//! policy-admissibility are enforced. Together they make the attack
//! structurally uninteresting: text from an untrusted world cannot
//! become the kind of memory the gate reads, however persuasive it is.
//!
//! **Advocacy.** The entity filters what reaches its human — offers,
//! requests, manipulation — because it is *theirs*, not a platform's.
//! This is the one capability in the architecture that is structurally
//! unavailable to any incumbent whose revenue depends on what reaches
//! the user, and the reason it is available to us is not virtue: it is
//! that the customer owns the entity, so the entity's interests and the
//! customer's are the same object.
//!
//! The honest limit, stated where the code is: `advocate` is a
//! heuristic classifier over surface features. It catches the common
//! shapes — urgency, artificial scarcity, authority claims, a request
//! for credentials or money — and it will miss a careful adversary and
//! occasionally flag a legitimate message. It is a *filter the human
//! controls*, not a guarantee, and everything it holds back is visible
//! to them on request.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod advocate;
pub mod ingress;
pub mod world;

pub use advocate::{Advocate, Judgement, Pressure};
pub use ingress::{Ingress, Tainted, Verdict as IngressVerdict};
pub use world::{Posture, Session, World};
