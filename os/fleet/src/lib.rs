//! brainos-fleet — Stage 7, and the thing that makes it different.
//!
//! A fleet is *thousands of entities, each with many bodies, each
//! sovereign*. That last word is the whole design, and it is what makes
//! this crate small rather than large.
//!
//! The obvious way to build fleet management is a central authority: a
//! server that knows every robot, holds their configuration, pushes
//! updates, and can reach in. That is how every fleet product works and
//! it is the wrong shape here, because it reintroduces exactly the thing
//! the architecture removed — a party above the entity that owns it.
//! An entity whose existence depends on an administrator's goodwill is
//! not an entity.
//!
//! So a `Registry` here is a **directory, not a controller**. It knows
//! which entities exist and how to reach them. It cannot make one do
//! anything: there is no method on it that produces an action, and the
//! only way to influence an entity is to send it a proposal that faces
//! its own gate. Operations that genuinely need to reach in —
//! deactivation, revocation — go through the quorum in
//! `brainos-crypto`, which needs other people's hardware keys.
//!
//! The **marketplace** follows from the same constraint. A third party
//! publishes a capability package; an entity acquires it; every
//! acquisition passes that entity's KIRA. There is no push. A package
//! is signed by its publisher and *countersigned by the entity's owner*
//! before it can be installed, because "the vendor can add a capability
//! to your robot" is the failure this design exists to make
//! impossible.
//!
//! **Hosted placement** is the datacenter business arriving as an
//! architectural consequence. A host runs an entity's Tier 2–3 cognition
//! and holds nothing: the Key stays with the customer, the state graph
//! is the entity's, and the host is attested per session. Revoking a
//! host is one quorum operation and the entity carries on degraded.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod hosting;
pub mod marketplace;
pub mod registry;

pub use hosting::{Host, HostError, Placement as HostPlacement, Tenancy};
pub use marketplace::{Acquisition, Manifest, MarketError, Package, Publisher};
pub use registry::{Entry, Fleet, Health, Registry, RegistryError};
