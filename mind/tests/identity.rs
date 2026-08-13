//! Stage 1.1 — identity and attestation: the BrAIn Key is real
//! cryptography, and tampered key material is refused at load.

mod common;

use brainos_mind::key::{to_hex, BrainKey};
use common::key;

fn seed_hex(n: u8) -> String {
    to_hex(&[n; 32])
}

fn pub_hex_of(n: u8) -> String {
    key(n).public_hex.clone()
}

#[test]
fn load_accepts_matching_seed_and_public_key() {
    let k = BrainKey::load(seed_hex(3).as_bytes(), pub_hex_of(3).as_bytes()).unwrap();
    assert_eq!(k.public_hex, pub_hex_of(3));
}

#[test]
fn load_refuses_tampered_key_material() {
    // seed from one identity, recorded public key from another
    let err = BrainKey::load(seed_hex(3).as_bytes(), pub_hex_of(4).as_bytes());
    assert!(matches!(err, Err("seed does not derive the recorded identity")));

    // unreadable seed / public key
    assert!(BrainKey::load(b"not hex at all", pub_hex_of(3).as_bytes()).is_err());
    assert!(BrainKey::load(seed_hex(3).as_bytes(), b"zz").is_err());
    assert!(BrainKey::load(b"", b"").is_err());
}

#[test]
fn attestation_is_self_consistent_and_forgery_resistant() {
    let k = key(5);
    let att = k.attest(b"boot-attest|2026-07-27T12:00:00");
    assert!(att.verified);

    // the signature does not verify for a different context
    assert!(!k.verify(b"boot-attest|different", &att.signature));

    // a bit-flipped signature does not verify
    let mut bad = att.signature;
    bad[10] ^= 0x01;
    assert!(!k.verify(b"boot-attest|2026-07-27T12:00:00", &bad));

    // another identity cannot verify this entity's attestation as its own
    assert!(!key(6).verify(b"boot-attest|2026-07-27T12:00:00", &att.signature));
}

#[test]
fn signatures_bind_to_exactly_one_identity() {
    let a = key(1);
    let b = key(2);
    let sig = a.sign(b"who am i");
    assert!(a.verify(b"who am i", &sig));
    assert!(!b.verify(b"who am i", &sig));
}

#[test]
fn fingerprint_is_a_stable_short_form() {
    let k = key(8);
    let fp = k.fingerprint();
    assert_eq!(fp.len(), 8 + 2 + 4);
    assert!(k.public_hex.starts_with(&fp[..8]));
    assert!(k.public_hex.ends_with(&fp[10..]));
}
