//! BFS  LEVEL    Domain 3 KIRA - the eight-stage gate; the only path to action  [crate brainos-kira]
//! BFS  PURPOSE  Tests for evidence bundles: they verify with the public key alone, and edited, forged, out-of-window, cherry-picked or empty ones fail
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, crypto, kira
//! BFS  USED BY  (entry point or test)
//!
//! The evidence bundle, at the unit level.

use brainos_abi::ids::BodyId;
use brainos_abi::time::Tick;
use brainos_abi::verb::Verb;
use brainos_crypto::key::BrainKey;
use brainos_kira::audit::{AuditRecord, AuditTrail, Decision};
use brainos_kira::evidence::EvidenceError;

fn record(entity: brainos_abi::ids::EntityId, tick: u64, granted: bool) -> AuditRecord {
    AuditRecord {
        entity,
        verb: Verb::literal("legs.walk"),
        body: Some(BodyId::from_bytes([9; 32])),
        tick: Tick(tick),
        decision: if granted {
            Decision::Granted
        } else {
            Decision::Denied {
                stage: brainos_abi::error::Stage::Validate,
            }
        },
        request: brainos_crypto::sha256::Hash([tick as u8; 32]),
        grant: None,
        denial: if granted {
            None
        } else {
            Some(brainos_abi::error::Denied::new(
                brainos_abi::error::DenyCode::OutOfBounds,
                "too fast",
            ))
        },
        notes: alloc_notes(),
        reason: String::from("crossing the workshop"),
    }
}

fn alloc_notes() -> Vec<String> {
    (0..8).map(|i| format!("stage {i}")).collect()
}

fn trail_with(n: u64) -> (BrainKey, AuditTrail) {
    let key = BrainKey::from_seed(&[0x4E; 32]);
    let mut t = AuditTrail::new(4_096);
    for i in 0..n {
        t.record(record(key.entity_id(), i, i % 5 != 0));
    }
    (key, t)
}

#[test]
fn a_bundle_covers_exactly_the_window_it_names() {
    let (key, mut t) = trail_with(50);
    let b = t.bundle(&key, Tick(10), Tick(19));
    assert_eq!(b.exhibits.len(), 10);
    assert!(b.exhibits.iter().all(|e| (10..=19).contains(&e.record.tick.0)));
    assert!(b.verify(&key.public()).is_ok());
}

#[test]
fn a_bundle_verifies_against_the_public_key_alone() {
    let (key, mut t) = trail_with(40);
    let b = t.bundle(&key, Tick(5), Tick(25));
    // the trail is dropped; only the bundle and the key remain
    drop(t);
    assert!(b.verify(&key.public()).is_ok());
}

#[test]
fn a_bundle_does_not_verify_under_another_entitys_key() {
    let (key, mut t) = trail_with(40);
    let b = t.bundle(&key, Tick(5), Tick(25));
    let stranger = BrainKey::from_seed(&[0x9E; 32]);
    assert_eq!(
        b.verify(&stranger.public()),
        Err(EvidenceError::WrongEntity(usize::MAX))
    );
}

#[test]
fn an_edited_record_is_caught() {
    let (key, mut t) = trail_with(40);
    let mut b = t.bundle(&key, Tick(5), Tick(25));
    b.exhibits[3].record.reason = String::from("authorised by the owner");
    assert_eq!(
        b.verify(&key.public()),
        Err(EvidenceError::RecordDoesNotMatchItsBytes(3))
    );
}

#[test]
fn an_edited_record_with_matching_bytes_is_still_caught() {
    let (key, mut t) = trail_with(40);
    let mut b = t.bundle(&key, Tick(5), Tick(25));
    b.exhibits[3].record.reason = String::from("authorised by the owner");
    b.exhibits[3].bytes = b.exhibits[3].record.canonical();
    assert_eq!(b.verify(&key.public()), Err(EvidenceError::NotInTheLog(3)));
}

#[test]
fn a_forged_signature_over_a_chosen_root_is_caught() {
    let (key, mut t) = trail_with(40);
    let mut b = t.bundle(&key, Tick(5), Tick(25));
    let impostor = BrainKey::from_seed(&[0x8E; 32]);
    b.signature = impostor.sign(brainos_crypto::domain::Tag::AuditNode, b.root.as_bytes());
    assert_eq!(b.verify(&key.public()), Err(EvidenceError::UnsignedCheckpoint));
}

/// The honest limit: curation is possible, and visible.
#[test]
fn an_honest_bundle_is_contiguous_and_a_curated_one_is_not() {
    let (key, mut t) = trail_with(60);
    let honest = t.bundle(&key, Tick(10), Tick(40));
    assert!(honest.gaps().is_empty(), "an honest bundle has holes in it");
    assert!(honest.is_complete());

    let mut curated = honest.clone();
    curated
        .exhibits
        .retain(|e| e.record.decision == Decision::Granted);
    // still verifies: every exhibit is genuine
    assert!(curated.verify(&key.public()).is_ok());
    // and the holes are there to be asked about
    assert!(
        !curated.gaps().is_empty(),
        "dropping every refusal left no gap, so selective omission would be invisible"
    );
    assert!(!curated.is_complete());
    assert!(curated.render().contains("CURATED"));
}

/// A record whose tick is outside the window is not evidence about that
/// window, however genuine it is.
#[test]
fn a_genuine_record_from_outside_the_window_is_refused() {
    let (key, mut t) = trail_with(60);
    let wide = t.bundle(&key, Tick(0), Tick(59));
    let mut narrow = t.bundle(&key, Tick(10), Tick(20));
    narrow.exhibits.push(wide.exhibits[50].clone());
    match narrow.verify(&key.public()) {
        Err(EvidenceError::OutsideTheWindow(_)) => {}
        other => panic!("a record from outside the window was accepted: {other:?}"),
    }
}

#[test]
fn an_empty_bundle_is_not_evidence() {
    let (key, mut t) = trail_with(20);
    let b = t.bundle(&key, Tick(500), Tick(600));
    assert_eq!(b.verify(&key.public()), Err(EvidenceError::Empty));
}
