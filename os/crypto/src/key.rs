//! BFS  LEVEL    Domain 1 Silicon - the BrAIn Key, hashes, Merkle log, quorum  [crate brainos-crypto]
//! BFS  PURPOSE  The entity's ed25519 keypair: signs and verifies labelled messages, answers attestation challenges, derives per-body subkeys, hides the seed
//! BFS  DEFINES  SIGNATURE_LEN, SEED_LEN, Signature, PublicKey, BrainKey, Attestation, Challenge, ChallengePurpose, body_identity
//! BFS  USES     this crate: domain, hkdf, sha256 | brainos: abi | external: ed25519-dalek
//! BFS  USED BY  blur/acquire, blur/firmware, blur/session, blur/tests/blur_suite, body/acquire, body/tests/body_suite, bpu/nerve, bpu/tests/seam, +62 more
//!
//! The BrAIn Key — identity as cryptographic root (Architecture §5).
//!
//! The entity *is* this keypair. Not "has an account represented by";
//! is. Everything else — memory, bodies, history — is content addressed
//! to it or signed by it. Swap the board, the model, the whole robot:
//! same key, same entity. Lose the key and the entity is gone, which is
//! why the seed never leaves the secure element and why `BrainKey` does
//! not expose it even to code inside this crate.
//!
//! Five anchors, per §3.2 of the Technical Execution Report:
//! identity, authority, portability, attestation, deactivation. The first
//! four are in this module. Deactivation is in `quorum`, because a
//! single party should not be able to end an entity.
//!
//! **Subkeys.** A body does not get the Key. It gets a subkey derived
//! from `HKDF(Key ‖ body-id)`, so authority flows downward without the
//! root ever being handed to something that has a motor attached to it.

use crate::domain::{tagged, Tag};
use crate::hkdf::hkdf_sha256;
use crate::sha256::{Hash, Sha256};
use alloc::vec::Vec;
use brainos_abi::error::{Error, Result};
use brainos_abi::ids::EntityId;
use core::fmt;
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};

pub const SIGNATURE_LEN: usize = 64;
pub const SEED_LEN: usize = 32;

/// A detached ed25519 signature.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Signature(pub [u8; SIGNATURE_LEN]);

impl Signature {
    pub fn as_bytes(&self) -> &[u8; SIGNATURE_LEN] {
        &self.0
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // first four bytes only: a signature is not a secret, but a full
        // one in a log line is noise that hides the line's actual point
        write!(
            f,
            "sig:{:02x}{:02x}{:02x}{:02x}..",
            self.0[0], self.0[1], self.0[2], self.0[3]
        )
    }
}

impl Default for Signature {
    fn default() -> Signature {
        Signature([0u8; SIGNATURE_LEN])
    }
}

/// The public half — what a peer needs to check that the entity is who
/// it claims to be, and nothing more.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PublicKey {
    inner: VerifyingKey,
}

impl PublicKey {
    pub fn from_bytes(b: &[u8; 32]) -> Result<PublicKey> {
        VerifyingKey::from_bytes(b)
            .map(|inner| PublicKey { inner })
            .map_err(|_| Error::Crypto("not a valid ed25519 public key"))
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.inner.to_bytes()
    }

    pub fn entity_id(&self) -> EntityId {
        EntityId::from_bytes(self.inner.to_bytes())
    }

    /// Verify a signature over a tagged message. There is deliberately no
    /// way to verify an *untagged* message through this type: every
    /// signature in the system names what kind of thing it signs.
    pub fn verify(&self, tag: Tag, payload: &[u8], sig: &Signature) -> bool {
        let msg = tagged(tag, payload);
        match ed25519_dalek::Signature::from_slice(&sig.0) {
            Ok(s) => self.inner.verify(&msg, &s).is_ok(),
            Err(_) => false,
        }
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.entity_id().fingerprint())
    }
}

/// The Key. Holds the private half; never yields it.
pub struct BrainKey {
    signing: SigningKey,
    public: PublicKey,
}

impl BrainKey {
    /// Mint from a 32-byte seed. On a development board the seed comes
    /// from a file; on real hardware this constructor is replaced by one
    /// that never sees the seed at all because the secure element signs.
    /// Everything above this module is written against `sign`/`verify`
    /// and does not change when that swap happens.
    pub fn from_seed(seed: &[u8; SEED_LEN]) -> BrainKey {
        let signing = SigningKey::from_bytes(seed);
        let public = PublicKey {
            inner: signing.verifying_key(),
        };
        BrainKey { signing, public }
    }

    /// Load from the key medium: the seed and the recorded public key.
    /// A mismatch means the medium was tampered with, and that is fatal —
    /// an entity that boots on a seed that does not derive its recorded
    /// identity is a different entity wearing the name.
    pub fn load(seed: &[u8; SEED_LEN], recorded_public: &[u8; 32]) -> Result<BrainKey> {
        let k = BrainKey::from_seed(seed);
        if k.public.to_bytes() != *recorded_public {
            return Err(Error::Crypto("seed does not derive the recorded identity"));
        }
        Ok(k)
    }

    pub fn public(&self) -> PublicKey {
        self.public
    }

    pub fn entity_id(&self) -> EntityId {
        self.public.entity_id()
    }

    /// Sign a tagged message.
    pub fn sign(&self, tag: Tag, payload: &[u8]) -> Signature {
        let msg = tagged(tag, payload);
        Signature(self.signing.sign(&msg).to_bytes())
    }

    pub fn verify(&self, tag: Tag, payload: &[u8], sig: &Signature) -> bool {
        self.public.verify(tag, payload, sig)
    }

    /// Derive a per-body subkey. The body gets this; it never gets the
    /// Key. Binding the body id into `info` is what makes the derivation
    /// one-way in the direction that matters: holding a subkey tells you
    /// nothing about the Key or about any sibling body's subkey.
    pub fn derive_subkey(&self, body_id: &brainos_abi::ids::BodyId, len: usize) -> Vec<u8> {
        let mut info = Sha256::tagged(Tag::Subkey);
        info.field(body_id.as_bytes());
        let info = info.finish();
        // the seed is the IKM; `SigningKey::to_bytes` returns it, and this
        // is the one place in the crate that touches it
        hkdf_sha256(
            self.public.to_bytes().as_slice(),
            &self.signing.to_bytes(),
            info.as_bytes(),
            len,
        )
    }

    /// Boot attestation: sign a context string and immediately verify it.
    /// Proof that Domain 1 holds a working, self-consistent identity
    /// before anything above it is allowed to run.
    pub fn attest_self(&self, context: &[u8]) -> Attestation {
        let signature = self.sign(Tag::Attestation, context);
        Attestation {
            signature,
            verified: self.verify(Tag::Attestation, context, &signature),
        }
    }

    /// Answer someone else's challenge.
    pub fn answer(&self, c: &Challenge) -> Signature {
        self.sign(Tag::Attestation, &c.transcript())
    }
}

impl fmt::Debug for BrainKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // never print anything derived from the seed
        write!(f, "BrainKey({})", self.entity_id().fingerprint())
    }
}

pub struct Attestation {
    pub signature: Signature,
    pub verified: bool,
}

/// A challenge in an attestation exchange.
///
/// The nonce is the anti-replay element and the `purpose` is the
/// anti-confusion element: an answer to "prove you are you, for session
/// 7, in order to be incorporated as a body" does not verify as an answer
/// to any other question.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Challenge {
    pub nonce: [u8; 32],
    pub purpose: ChallengePurpose,
    /// who is asking — binds the answer to one verifier
    pub asker: EntityId,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ChallengePurpose {
    /// §8 step 2: a device proving it is the body it claims to be
    BodyHandshake = 1,
    /// a peer entity proving its identity before a memory exchange
    PeerAttestation = 2,
    /// a hosting node proving it is entitled to run this entity's brain
    BrainPlacement = 3,
    /// the human's own hardware token, for quorum operations
    QuorumMember = 4,
}

impl Challenge {
    pub fn new(nonce: [u8; 32], purpose: ChallengePurpose, asker: EntityId) -> Challenge {
        Challenge {
            nonce,
            purpose,
            asker,
        }
    }

    /// What gets signed. Everything that makes this challenge *this*
    /// challenge goes in; nothing else does.
    pub fn transcript(&self) -> Vec<u8> {
        let mut h = Sha256::tagged(Tag::Attestation);
        h.field(&self.nonce);
        h.field(&[self.purpose as u8]);
        h.field(self.asker.as_bytes());
        h.finish().as_bytes().to_vec()
    }

    /// Check an answer. Note that this takes the *answerer's* public key:
    /// the verifier decides who it expects to hear from, rather than
    /// letting the answer name its own signer.
    pub fn check(&self, answerer: &PublicKey, sig: &Signature) -> bool {
        answerer.verify(Tag::Attestation, &self.transcript(), sig)
    }
}

/// Hash a body's identity: attestation key and schema digest together.
/// Two devices with the same friendly name are different bodies; the same
/// device advertising different capabilities is also a different body,
/// and must be acquired again through §8.
pub fn body_identity(attestation_key: &PublicKey, schema_digest: &Hash) -> brainos_abi::ids::BodyId {
    let mut h = Sha256::tagged(Tag::BodyIdentity);
    h.field(&attestation_key.to_bytes());
    h.field(schema_digest.as_bytes());
    brainos_abi::ids::BodyId(h.finish().to_id())
}
