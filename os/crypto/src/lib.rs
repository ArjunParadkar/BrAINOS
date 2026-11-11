//! BFS  LEVEL    Domain 1 Silicon - the BrAIn Key, hashes, Merkle log, quorum  [crate brainos-crypto]
//! BFS  PURPOSE  Crate root: the entity's signing key plus hand-written SHA-256, HMAC, HKDF, random generator, Merkle audit log and k-of-n sign-off
//! BFS  DEFINES  domain, hkdf, hmac, key, merkle, quorum, rng, sha256
//! BFS  USES     this crate: domain, hkdf, hmac, key, merkle, quorum, rng, sha256
//! BFS  USED BY  crate blur, crate body, crate bodyd, crate cognition, crate entity, crate export, crate fleet, crate host, +6 more
//!
//! brainos-crypto — identity, integrity, and the audit anchor.
//!
//! Domain 1 of the trust hierarchy is a hardware root of trust, and
//! everything above it inherits authority by signature. This crate is the
//! arithmetic of that inheritance.
//!
//! **What is borrowed and what is ours.** Ed25519 is `ed25519-dalek`,
//! because hand-rolling a signature scheme is how you get a signature
//! scheme with a bug in it, and this is the one place where the
//! build-it-ourselves principle loses to the don't-be-clever-with-crypto
//! principle. Everything else — SHA-256, HMAC, HKDF, the Merkle tree,
//! the DRBG — is ours, written from the specification, because each is a
//! well-defined function with published test vectors that we can be held
//! to, and because a `no_std` core with a dozen transitive dependencies
//! is not a core we understand at author level.
//!
//! **Domain separation everywhere.** Every signature and every hash in
//! BrAInOS is made over a domain-separated message: a fixed tag, then a
//! length, then the payload. A signature minted for a KIRA grant must not
//! verify as an attestation response, and the only reliable way to get
//! that is to never hash two different kinds of thing the same way.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod domain;
pub mod hkdf;
pub mod hmac;
pub mod key;
pub mod merkle;
pub mod quorum;
pub mod rng;
pub mod sha256;

pub use domain::{tagged, tagged_into, Tag};
pub use hkdf::hkdf_sha256;
pub use hmac::HmacSha256;
pub use key::{Attestation, BrainKey, Challenge, PublicKey, Signature, SIGNATURE_LEN};
pub use merkle::{MerkleLog, Proof as MerkleProof, Root as MerkleRoot};
pub use quorum::{Quorum, QuorumError, Share};
pub use rng::Drbg;
pub use sha256::{Hash, Sha256, HASH_LEN};
