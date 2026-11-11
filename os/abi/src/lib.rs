//! BFS  LEVEL    Foundation - shared vocabulary (ids, time, verbs, codec, errors)  [crate brainos-abi]
//! BFS  PURPOSE  Crate root: the shared types every other crate agrees on (ids, time, verbs, byte encoding, errors, version), with no dependencies
//! BFS  DEFINES  codec, error, ids, text, time, verb, version
//! BFS  USES     this crate: codec, error, ids, text, time, verb, version
//! BFS  USED BY  crate adapters, crate blur, crate body, crate bodyd, crate bpu, crate cognition, crate crypto, crate entity, +11 more
//!
//! brainos-abi — the vocabulary.
//!
//! Five domains have to agree on what a body is, what a tick is, what a
//! verb looks like, and what it means when something is refused. If they
//! agree by convention, the agreement rots. This crate is the agreement,
//! written down once, with no dependencies at all — not even on `alloc`
//! for the parts that must run inside a servo loop.
//!
//! Three rules govern everything here:
//!
//! 1. **Identity is bytes, not strings.** An `EntityId` is a public key.
//!    A `BodyId` is a hash. Display forms exist for humans; comparison,
//!    hashing, and the wire always use the bytes. Nothing anywhere
//!    decides authority by comparing a name.
//!
//! 2. **Encoding is canonical.** One value has exactly one encoding.
//!    This is not an aesthetic preference: the audit log is a Merkle
//!    tree and the export format is signed, so "the same state encodes
//!    two ways" would mean "the same state has two hashes", which would
//!    mean the signature proves nothing.
//!
//! 3. **Refusal is typed.** `Denied` carries the stage and a reason
//!    code, never a formatted sentence that some caller will later parse.
//!    A refusal the machine cannot reason about is a refusal that gets
//!    routed around.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod codec;
pub mod error;
pub mod ids;
pub mod text;
pub mod time;
pub mod verb;
pub mod version;

pub use codec::{Decode, Decoder, Encode, Encoder};
pub use error::{Denied, DenyCode, Error, Result, Stage};
pub use ids::{BodyId, CapabilityId, EntityId, Fingerprint, NodeId, RegionId, SessionId};
pub use text::Text;
pub use time::{Duration, Instant, Micros, Tick};
pub use verb::{Namespace, Verb};
pub use version::{AbiVersion, ABI};
