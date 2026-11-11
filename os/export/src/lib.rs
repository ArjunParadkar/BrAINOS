//! BFS  LEVEL    Freedom principle - entity export  [crate brainos-export]
//! BFS  PURPOSE  Crate root for exporting an entity's complete state to a signed, self-documenting file and importing it back, so an owner can leave with everything
//! BFS  DEFINES  archive, format
//! BFS  USES     this crate: archive, format
//! BFS  USED BY  crate bpu, crate host, crate nostd
//!
//! brainos-export — leaving.
//!
//! > *The customer owns the entity. Permanently. The Key is theirs; the
//! > memory is theirs; the body is a subscription they can end. We build
//! > the opposite of lock-in-by-captivity. Our moat is that leaving
//! > means abandoning a life, and the customer is free to do it.*
//!
//! This crate is that principle made mechanical. Export of an entity's
//! complete state is a first-class, documented, tested feature from
//! v1.0 — not a data-request process, not a support ticket, not a JSON
//! dump of whatever happened to be in a database.
//!
//! Four properties, each with a test:
//!
//! 1. **Complete.** Everything durable: memories, skills, beliefs,
//!    calibration, the life record. If it is part of who the entity is,
//!    it is in the file.
//! 2. **Documented.** The format is specified here, versioned, and
//!    readable by something that is not us. `describe_format()` emits
//!    the specification, so a customer's engineer can write a reader
//!    without asking.
//! 3. **Signed and checkable.** The archive carries a signature by the
//!    entity's own Key and a Merkle root over its sections, so a
//!    customer can prove the file is theirs and unaltered — including
//!    proving it years later, to someone who does not trust us.
//! 4. **Round-trippable.** An export imported into a fresh process
//!    produces an entity with the same memories, the same skills and
//!    the same history. The test for that is the last one in the suite,
//!    and it is the one that makes the other three mean anything.
//!
//! **What is deliberately not exported.** The private key half, which
//! never leaves the secure element and is not ours to move. The audit
//! log, which belongs to the machine of record — an entity carrying its
//! own audit log between owners could edit it in transit. Both absences
//! are stated in the manifest rather than being silent.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod archive;
pub mod format;

pub use archive::{adopt, export, import, Archive, ExportError, Manifest, Section, SectionKind};
pub use format::describe_format;
