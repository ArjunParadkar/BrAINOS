//! BFS  LEVEL    Domain 2 Autonomic - the BLUR quadruped body (wire, acquisition, supervisor)  [crate brainos-blur]
//! BFS  PURPOSE  The body's half of the handshake: holds BLUR's signing key, answers the entity's challenge, checks the entity is an owner, and signs the census
//! BFS  DEFINES  absorb, census_payload, Firmware
//! BFS  USES     this crate: census | brainos: body, crypto
//! BFS  USED BY  blur/acquire, blur/tests/blur_suite, host/blur_run, host/blur_sil
//!
//! The body's side of §8: who BLUR is, and whom it lets in.
//!
//! BLUR's attestation key lives on the Teensy (the motor nodes cannot
//! sign anything). It answers the entity's challenge, checks the
//! entity's own signature against its owner list before it will be
//! inhabited, and signs its census so the entity can tell "the body
//! reports these parts" from "something on the link claims the body
//! reports these parts".

use crate::census::Census;
use alloc::vec::Vec;
use brainos_body::schema::CapabilitySchema;
use brainos_crypto::domain::Tag;
use brainos_crypto::key::{BrainKey, Challenge, PublicKey, Signature};
use brainos_crypto::sha256::Sha256;

/// The transcript step §8's handshake performs; replicated so the body
/// can verify the entity's signature over the same bytes the entity
/// signed. `tests::transcript_matches_acquisition` holds the two equal.
pub fn absorb(transcript: &[u8; 32], label: &str, data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::tagged(Tag::Session);
    h.field(transcript);
    h.field(label.as_bytes());
    h.field(data);
    *h.finish().as_bytes()
}

pub fn census_payload(transcript: &[u8; 32], c: &Census) -> Vec<u8> {
    let mut p = Vec::with_capacity(64);
    p.extend_from_slice(b"blur.census");
    p.extend_from_slice(transcript);
    p.extend_from_slice(c.digest().as_bytes());
    p
}

pub struct Firmware {
    key: BrainKey,
    pub schema: CapabilitySchema,
    /// entities this body accepts as its inhabitant (set at assembly)
    pub owners: Vec<PublicKey>,
}

impl Firmware {
    pub fn new(seed: &[u8; 32], schema: CapabilitySchema, owners: Vec<PublicKey>) -> Firmware {
        Firmware {
            key: BrainKey::from_seed(seed),
            schema,
            owners,
        }
    }

    pub fn public(&self) -> PublicKey {
        self.key.public()
    }

    pub fn answer(&self, c: &Challenge) -> Signature {
        self.key.answer(c)
    }

    /// The device side of mutual attestation: an owner, signing the
    /// transcript as it stood after the device proved itself.
    pub fn accepts(&self, entity: &PublicKey, sig: &Signature, transcript_after_device: &[u8; 32]) -> bool {
        self.owners.iter().any(|o| o.to_bytes() == entity.to_bytes())
            && entity.verify(Tag::Attestation, transcript_after_device, sig)
    }

    pub fn sign_census(&self, transcript: &[u8; 32], c: &Census) -> Signature {
        self.key.sign(Tag::Attestation, &census_payload(transcript, c))
    }
}
