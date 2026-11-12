//! BFS  LEVEL    Parts and nerves - every piece of hardware as a body part  [crate brainos-part]
//! BFS  PURPOSE  Crate root: lists the part-model modules and states the five rules every part and nerve must obey
//! BFS  DEFINES  class, homeostat, intero, nerve, pack, policy, schema, sim
//! BFS  USES     this crate: class, homeostat, intero, nerve, pack, policy, schema
//! BFS  USED BY  crate bpu, crate entity, crate host, crate nostd, crate proof
//!
//! brainos-part — every piece of hardware is a body part.
//!
//! A motor is a part. So is a fan, a GPU, a disk, a battery, a radio.
//! `brainos-body` already says how a *device* becomes a region of the
//! body (§8) and what it can be asked to do (`CapabilitySchema`). This
//! crate adds what that model was missing to cover hardware that does
//! not walk:
//!
//! | module | what it adds |
//! |---|---|
//! | `class` | the eight part classes, and which of the five domains each is served by |
//! | `schema` | `PartSchema`: verbs + senses + vitals, and the digest that is the part's identity |
//! | `policy` | `PartPolicy`: stages 4-6 for a part, including refusing a verb that would harm *another* part |
//! | `nerve` | the `Nerve` trait every protocol implements, and `Mandate`, the only thing a nerve will act on |
//! | `homeostat` | Domain 2 regulation (thermal -> fan, charge -> load) with no model call |
//! | `intero` | interoception: which readings are worth remembering |
//! | `pack` | envelope packs: signed limits for parts that state none |
//! | `sim` | a simulated fan and GPU behind the trait, for tests (not hardware) |
//!
//! It extends §8 rather than standing beside it. A part is acquired by
//! the same five steps, gets a `BodyId` the same way, is authorized
//! through the same body map and gated by the same `Kira`.
//!
//! # The five rules
//!
//! 1. **Nothing reaches a part's verb without a `Granted`.** `Nerve::apply`
//!    takes a `Mandate`, and the only way to make one is from a token
//!    the gate minted (`nerve::Mandate::check`).
//! 2. **A reflex is pre-authorized, not unauthorized.** A `Homeostat` is
//!    installed with a `Mandate` and afterwards acts only inside the
//!    range that mandate granted.
//! 3. **A part that declares no limits gets no verbs.** `PartSchema::lowered`
//!    drops them before the entity ever sees them.
//! 4. **A verb must not break another part's vital.** `PartPolicy` hears
//!    every part's readings and refuses at *simulate*.
//! 5. **A nerve holds no memory.** This crate does not depend on
//!    `brainos-memory` at all; a nerve is handed nothing it could write
//!    memory with.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod class;
pub mod homeostat;
pub mod intero;
pub mod nerve;
pub mod pack;
pub mod policy;
pub mod schema;
pub mod sim;

pub use class::{Domain, PartClass};
pub use homeostat::{Homeostat, InstallError, ReflexAct};
pub use intero::{Interoceptor, Noted, NotedKind};
pub use nerve::{Found, Mandate, MandateError, Nerve, NerveFault, Sensed};
pub use pack::{AppliesTo, EnvelopePack, PackError, Source};
pub use policy::{Guard, Meter, PartPolicy};
pub use schema::{PartSchema, Quantity, SchemaFault, SenseSpec, VitalKind, VitalSpec};
