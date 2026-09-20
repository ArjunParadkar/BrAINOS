//! The crypto suite.
//!
//! SHA-256 and HMAC are checked against published vectors, because "our
//! hash" is only acceptable if it is *the* hash. The rest checks the
//! properties the substrate depends on: domain separation actually
//! separates, the Merkle log actually detects tampering, and one party
//! cannot pass a quorum.

use brainos_crypto::domain::{tagged, Tag};
use brainos_crypto::hkdf::hkdf_sha256;
use brainos_crypto::hmac::HmacSha256;
use brainos_crypto::key::{body_identity, BrainKey, Challenge, ChallengePurpose, PublicKey};
use brainos_crypto::merkle::MerkleLog;
use brainos_crypto::quorum::{Operation, Quorum, QuorumError, Share, Statement};
use brainos_crypto::rng::Drbg;
use brainos_crypto::sha256::{sha256, Sha256};

fn hex(h: &[u8]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

fn key(n: u8) -> BrainKey {
    BrainKey::from_seed(&[n; 32])
}

// ---------- SHA-256 against FIPS / NESSIE vectors ----------

#[test]
fn sha256_matches_the_published_vectors() {
    assert_eq!(
        hex(sha256(b"").as_bytes()),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        hex(sha256(b"abc").as_bytes()),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        hex(sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq").as_bytes()),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
    // the million-a vector, in chunks, to exercise the streaming path
    let mut h = Sha256::new();
    for _ in 0..1000 {
        h.update(&[b'a'; 1000]);
    }
    assert_eq!(
        hex(h.finish().as_bytes()),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}

#[test]
fn streaming_and_one_shot_agree_at_every_block_boundary() {
    let data: Vec<u8> = (0..300u32).map(|i| (i % 251) as u8).collect();
    for split in [0, 1, 55, 63, 64, 65, 127, 128, 129, 200, 299, 300] {
        let mut h = Sha256::new();
        h.update(&data[..split]);
        h.update(&data[split..]);
        assert_eq!(
            h.finish().as_bytes(),
            sha256(&data).as_bytes(),
            "split at {split}"
        );
    }
}

#[test]
fn hmac_sha256_matches_rfc_4231() {
    // RFC 4231 test case 1
    let mac = HmacSha256::mac(&[0x0b; 20], b"Hi There");
    assert_eq!(
        hex(mac.as_bytes()),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    // test case 2
    let mac = HmacSha256::mac(b"Jefe", b"what do ya want for nothing?");
    assert_eq!(
        hex(mac.as_bytes()),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    // test case 3: a key longer than the block, which must be hashed first
    let mac = HmacSha256::mac(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First");
    assert_eq!(
        hex(mac.as_bytes()),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
}

#[test]
fn hmac_verification_is_a_real_check() {
    let tag = HmacSha256::mac(b"k", b"message");
    assert!(HmacSha256::verify(b"k", b"message", &tag));
    assert!(!HmacSha256::verify(b"k", b"messagf", &tag));
    assert!(!HmacSha256::verify(b"j", b"message", &tag));
}

#[test]
fn hkdf_expands_to_the_requested_length_and_is_context_bound() {
    let a = hkdf_sha256(b"salt", b"ikm", b"for-body-A", 48);
    let b = hkdf_sha256(b"salt", b"ikm", b"for-body-B", 48);
    assert_eq!(a.len(), 48);
    assert_ne!(a, b, "different info must give different keys");
    // and it is deterministic
    assert_eq!(a, hkdf_sha256(b"salt", b"ikm", b"for-body-A", 48));
    // longer than one block exercises the counter
    assert_eq!(hkdf_sha256(b"", b"ikm", b"x", 100).len(), 100);
}

// ---------- domain separation ----------

#[test]
fn a_signature_for_one_purpose_does_not_verify_as_another() {
    let k = key(1);
    let payload = b"the same 32 bytes exactly here..";
    let grant_sig = k.sign(Tag::Grant, payload);

    assert!(k.verify(Tag::Grant, payload, &grant_sig));
    // this is the attack domain separation exists to stop
    for other in Tag::ALL {
        if other != Tag::Grant {
            assert!(
                !k.verify(other, payload, &grant_sig),
                "grant signature verified as {other:?}"
            );
        }
    }
}

#[test]
fn every_tag_is_distinct() {
    let mut seen: Vec<&str> = Tag::ALL.iter().map(|t| t.as_str()).collect();
    seen.sort_unstable();
    let before = seen.len();
    seen.dedup();
    assert_eq!(before, seen.len(), "two tags share a string");
}

#[test]
fn tagged_messages_cannot_be_confused_by_shifting_the_boundary() {
    // "ab" + "c" must not hash like "a" + "bc"
    let mut h1 = Sha256::tagged(Tag::Schema);
    h1.field(b"ab");
    h1.field(b"c");
    let mut h2 = Sha256::tagged(Tag::Schema);
    h2.field(b"a");
    h2.field(b"bc");
    assert_ne!(h1.finish().as_bytes(), h2.finish().as_bytes());
}

#[test]
fn tagged_into_refuses_rather_than_truncating() {
    let mut small = [0u8; 4];
    assert_eq!(tagged_into_helper(&mut small), None);
    let mut big = [0u8; 128];
    let n = tagged_into_helper(&mut big).unwrap();
    assert_eq!(&big[..n], &tagged(Tag::Grant, b"hello")[..]);
}

fn tagged_into_helper(buf: &mut [u8]) -> Option<usize> {
    brainos_crypto::domain::tagged_into(Tag::Grant, b"hello", buf)
}

// ---------- the Key ----------

#[test]
fn the_key_is_the_identity() {
    let k = key(7);
    let same = key(7);
    let other = key(8);
    assert_eq!(k.entity_id(), same.entity_id());
    assert_ne!(k.entity_id(), other.entity_id());
}

#[test]
fn a_tampered_key_medium_is_fatal_not_silently_accepted() {
    let k = key(3);
    let good = k.public().to_bytes();
    assert!(BrainKey::load(&[3; 32], &good).is_ok());
    let mut bad = good;
    bad[0] ^= 1;
    assert!(BrainKey::load(&[3; 32], &bad).is_err());
}

#[test]
fn boot_attestation_proves_the_identity_is_self_consistent() {
    let k = key(11);
    let a = k.attest_self(b"brainos-boot");
    assert!(a.verified);
    assert!(!k.verify(Tag::Attestation, b"brainos-boo", &a.signature));
}

#[test]
fn a_challenge_answer_is_bound_to_the_nonce_purpose_and_asker() {
    let device = key(21);
    let entity = key(22);
    let stranger = key(23);

    let c = Challenge::new([9; 32], ChallengePurpose::BodyHandshake, entity.entity_id());
    let answer = device.answer(&c);
    assert!(c.check(&device.public(), &answer));

    // a different answerer
    assert!(!c.check(&stranger.public(), &answer));

    // a replayed answer against a different nonce
    let c2 = Challenge::new([10; 32], ChallengePurpose::BodyHandshake, entity.entity_id());
    assert!(!c2.check(&device.public(), &answer));

    // the same nonce, a different purpose
    let c3 = Challenge::new([9; 32], ChallengePurpose::BrainPlacement, entity.entity_id());
    assert!(!c3.check(&device.public(), &answer));

    // the same nonce and purpose, a different asker
    let c4 = Challenge::new([9; 32], ChallengePurpose::BodyHandshake, stranger.entity_id());
    assert!(!c4.check(&device.public(), &answer));
}

#[test]
fn subkeys_are_per_body_and_never_reveal_the_key() {
    let k = key(5);
    let body_a = brainos_abi::ids::BodyId::from_bytes([1; 32]);
    let body_b = brainos_abi::ids::BodyId::from_bytes([2; 32]);
    let ka = k.derive_subkey(&body_a, 32);
    let kb = k.derive_subkey(&body_b, 32);
    assert_ne!(ka, kb);
    assert_eq!(ka, k.derive_subkey(&body_a, 32));
    // the subkey is not the seed
    assert_ne!(ka.as_slice(), &[5u8; 32]);
}

#[test]
fn a_body_changing_its_capabilities_is_a_different_body() {
    let attest = key(31).public();
    let schema_v1 = sha256(b"walk,turn");
    let schema_v2 = sha256(b"walk,turn,grasp");
    assert_ne!(
        body_identity(&attest, &schema_v1),
        body_identity(&attest, &schema_v2),
        "a body that silently gains a verb must be re-acquired through §8"
    );
    // and the same device with the same schema is stably the same body
    assert_eq!(
        body_identity(&attest, &schema_v1),
        body_identity(&attest, &schema_v1)
    );
}

#[test]
fn a_public_key_round_trips_through_its_bytes() {
    let k = key(41);
    let bytes = k.public().to_bytes();
    let restored = PublicKey::from_bytes(&bytes).unwrap();
    assert_eq!(restored.entity_id(), k.entity_id());
    let sig = k.sign(Tag::Session, b"hello");
    assert!(restored.verify(Tag::Session, b"hello", &sig));
}

// ---------- the Merkle audit log ----------

#[test]
fn an_empty_log_has_a_distinct_root() {
    let log = MerkleLog::new();
    assert_eq!(log.root(), brainos_crypto::sha256::Hash::ZERO);
    assert!(log.is_empty());
}

#[test]
fn every_record_can_be_proven_to_be_in_the_log() {
    for n in 1..=33usize {
        let mut log = MerkleLog::new();
        let records: Vec<Vec<u8>> = (0..n).map(|i| format!("decision {i}").into_bytes()).collect();
        for r in &records {
            log.append(r);
        }
        let root = log.root();
        for (i, r) in records.iter().enumerate() {
            let p = log.prove(i).expect("proof exists");
            assert!(p.verify_record(r, &root), "n={n} i={i}");
        }
        assert!(log.prove(n).is_none());
    }
}

#[test]
fn a_proof_for_one_record_does_not_verify_another() {
    let mut log = MerkleLog::new();
    log.append(b"GRANT legs.walk");
    log.append(b"DENY legs.walk at policy");
    log.append(b"GRANT memory.write");
    let root = log.root();
    let p = log.prove(1).unwrap();
    assert!(p.verify_record(b"DENY legs.walk at policy", &root));
    assert!(!p.verify_record(b"GRANT legs.walk", &root));
}

#[test]
fn removing_a_decision_from_the_middle_changes_every_later_root() {
    let full: Vec<&[u8]> = vec![b"a", b"b", b"c", b"d", b"e"];
    let mut honest = MerkleLog::new();
    for r in &full {
        honest.append(r);
    }
    let mut tampered = MerkleLog::new();
    for (i, r) in full.iter().enumerate() {
        if i != 2 {
            tampered.append(r);
        }
    }
    assert_ne!(honest.root(), tampered.root());

    // and the proof issued before the tampering no longer verifies
    let p = honest.prove(4).unwrap();
    assert!(p.verify(&honest.root()));
    assert!(!p.verify(&tampered.root()));
}

#[test]
fn the_root_changes_on_every_append() {
    let mut log = MerkleLog::new();
    let mut roots = Vec::new();
    for i in 0..20 {
        log.append(format!("r{i}").as_bytes());
        roots.push(log.root());
    }
    let mut sorted: Vec<_> = roots.iter().map(|r| *r.as_bytes()).collect();
    sorted.sort_unstable();
    let before = sorted.len();
    sorted.dedup();
    assert_eq!(before, sorted.len(), "two appends produced the same root");
    assert_eq!(log.roots_through(), roots);
}

#[test]
fn a_leaf_hash_cannot_be_presented_as_an_interior_node() {
    // the second-preimage attack on naive Merkle trees: without domain
    // separation, a two-leaf tree's root equals a one-leaf tree whose
    // leaf is the concatenation of the two hashes
    let leaf = brainos_crypto::merkle::leaf_hash(b"x");
    let node = brainos_crypto::merkle::node_hash(&leaf, &leaf);
    assert_ne!(leaf.as_bytes(), node.as_bytes());
    assert_ne!(
        brainos_crypto::merkle::leaf_hash(&[leaf.as_bytes().as_slice(), leaf.as_bytes()].concat())
            .as_bytes(),
        node.as_bytes()
    );
}

#[test]
fn proof_size_is_logarithmic() {
    let mut log = MerkleLog::new();
    for i in 0..1024 {
        log.append(format!("{i}").as_bytes());
    }
    let p = log.prove(500).unwrap();
    assert!(p.path_len() <= 10, "path was {}", p.path_len());
    assert!(p.verify(&log.root()));
}

// ---------- quorum ----------

fn statement(op: Operation) -> Statement {
    Statement {
        operation: op,
        entity: key(1).entity_id(),
        subject: [4; 32],
        nonce: [5; 32],
    }
}

#[test]
fn one_party_cannot_deactivate_an_entity() {
    let members: Vec<BrainKey> = (60..63).map(key).collect();
    let q = Quorum::new(members.iter().map(|k| k.public()).collect(), 2).unwrap();
    let s = statement(Operation::Deactivate);

    let one = vec![Share {
        member: members[0].public(),
        signature: members[0].sign(Tag::QuorumShare, &s.transcript()),
    }];
    assert_eq!(
        q.authorize(&s, &one),
        Err(QuorumError::NotEnoughShares { have: 1, need: 2 })
    );

    let two: Vec<Share> = members[..2]
        .iter()
        .map(|k| Share {
            member: k.public(),
            signature: k.sign(Tag::QuorumShare, &s.transcript()),
        })
        .collect();
    assert_eq!(q.authorize(&s, &two), Ok(()));
}

#[test]
fn signing_twice_is_not_two_members() {
    let members: Vec<BrainKey> = (70..73).map(key).collect();
    let q = Quorum::new(members.iter().map(|k| k.public()).collect(), 2).unwrap();
    let s = statement(Operation::RevokeBody);
    let share = Share {
        member: members[0].public(),
        signature: members[0].sign(Tag::QuorumShare, &s.transcript()),
    };
    assert_eq!(
        q.authorize(&s, &[share, share]),
        Err(QuorumError::DuplicateMember)
    );
}

#[test]
fn a_non_member_signature_is_refused_distinctly_from_a_short_quorum() {
    let members: Vec<BrainKey> = (80..83).map(key).collect();
    let outsider = key(99);
    let q = Quorum::new(members.iter().map(|k| k.public()).collect(), 2).unwrap();
    let s = statement(Operation::ExportMemory);
    let shares = vec![
        Share {
            member: members[0].public(),
            signature: members[0].sign(Tag::QuorumShare, &s.transcript()),
        },
        Share {
            member: outsider.public(),
            signature: outsider.sign(Tag::QuorumShare, &s.transcript()),
        },
    ];
    assert_eq!(q.authorize(&s, &shares), Err(QuorumError::NotAMember));
}

#[test]
fn a_forged_share_is_caught() {
    let members: Vec<BrainKey> = (90..93).map(key).collect();
    let q = Quorum::new(members.iter().map(|k| k.public()).collect(), 2).unwrap();
    let s = statement(Operation::RotateKey);
    let shares = vec![
        Share {
            member: members[0].public(),
            signature: members[0].sign(Tag::QuorumShare, &s.transcript()),
        },
        Share {
            // member 1's key, but member 2's signature
            member: members[1].public(),
            signature: members[2].sign(Tag::QuorumShare, &s.transcript()),
        },
    ];
    assert_eq!(q.authorize(&s, &shares), Err(QuorumError::BadShare));
}

#[test]
fn shares_collected_for_one_subject_do_not_authorize_another() {
    let members: Vec<BrainKey> = (100..103).map(key).collect();
    let q = Quorum::new(members.iter().map(|k| k.public()).collect(), 2).unwrap();
    let revoke_a = statement(Operation::RevokeBody);
    let mut revoke_b = revoke_a;
    revoke_b.subject = [77; 32];

    let shares: Vec<Share> = members[..2]
        .iter()
        .map(|k| Share {
            member: k.public(),
            signature: k.sign(Tag::QuorumShare, &revoke_a.transcript()),
        })
        .collect();
    assert_eq!(q.authorize(&revoke_a, &shares), Ok(()));
    assert_eq!(q.authorize(&revoke_b, &shares), Err(QuorumError::BadShare));
}

#[test]
fn an_impossible_threshold_is_refused_at_construction() {
    let m = vec![key(1).public(), key(2).public()];
    assert_eq!(
        Quorum::new(m.clone(), 3).err(),
        Some(QuorumError::ImpossibleThreshold)
    );
    assert_eq!(
        Quorum::new(m, 0).err(),
        Some(QuorumError::ImpossibleThreshold)
    );
}

// ---------- the DRBG ----------

#[test]
fn the_drbg_refuses_to_stretch_thin_entropy() {
    assert!(Drbg::from_entropy(&[1; 16]).is_err());
    assert!(Drbg::from_entropy(&[1; 32]).is_ok());
}

#[test]
fn the_drbg_is_deterministic_from_a_seed_and_diverges_from_others() {
    let mut a = Drbg::deterministic(b"test");
    let mut b = Drbg::deterministic(b"test");
    let mut c = Drbg::deterministic(b"other");
    assert_eq!(a.nonce32(), b.nonce32());
    assert_ne!(a.nonce32(), c.nonce32());
}

#[test]
fn successive_nonces_differ() {
    let mut d = Drbg::deterministic(b"nonces");
    let mut seen = Vec::new();
    for _ in 0..200 {
        seen.push(d.nonce32());
    }
    seen.sort_unstable();
    let before = seen.len();
    seen.dedup();
    assert_eq!(before, seen.len());
}

#[test]
fn reseeding_changes_the_stream_without_resetting_it() {
    let mut a = Drbg::deterministic(b"x");
    let mut b = Drbg::deterministic(b"x");
    a.nonce32();
    b.nonce32();
    b.reseed(b"fresh entropy from the peripheral");
    assert_ne!(a.nonce32(), b.nonce32());
    assert_eq!(b.outputs_since_reseed(), 1);
}

#[test]
fn bounded_sampling_stays_in_range_and_covers_it() {
    let mut d = Drbg::deterministic(b"below");
    let mut hits = [0u32; 7];
    for _ in 0..7000 {
        let v = d.below(7);
        assert!(v < 7);
        hits[v as usize] += 1;
    }
    // every bucket reached; a biased or broken sampler misses one
    assert!(hits.iter().all(|&h| h > 700), "{hits:?}");
    assert_eq!(d.below(0), 0);
}

// ---------------------------------------------------------------------
// the incremental root has to agree with the obvious one
// ---------------------------------------------------------------------
//
// `root()` used to rebuild every level, which is O(n) and was fine until
// the limits run asked what it costs for the thing this product is for:
// an entity that accumulates for years. A robot making ten gated
// decisions a second reaches a million in about a day, and a checkpoint
// there cost 0.7 s — and checkpoints are what make the audit log
// evidence rather than a diary, so making them expensive means taking
// fewer of them.
//
// It is now a frontier of perfect-subtree roots, folded right to left.
// That is a clever answer, and a clever answer that disagreed with the
// obvious one would silently invalidate every proof ever issued. So
// they are checked against each other on *every* prefix.

#[test]
fn the_fast_root_and_the_obvious_root_agree_on_every_prefix() {
    let mut log = MerkleLog::new();
    for i in 0..1_500u32 {
        assert_eq!(
            log.root(),
            log.root_the_slow_way(),
            "the two roots disagree at {i} leaves, which would invalidate every proof \
             issued against either"
        );
        log.append(&i.to_le_bytes());
    }
    assert_eq!(log.root(), log.root_the_slow_way());
}

/// And every proof still verifies against the fast root, which is the
/// property that actually matters — a root nothing can be proven against
/// is not a root.
#[test]
fn every_proof_verifies_against_the_fast_root() {
    for n in [1usize, 2, 3, 5, 8, 13, 64, 100, 257] {
        let mut log = MerkleLog::new();
        for i in 0..n {
            log.append(&(i as u32).to_le_bytes());
        }
        let root = log.root();
        for i in 0..n {
            let p = log.prove(i).expect("a proof for every leaf");
            assert!(
                p.verify_record(&(i as u32).to_le_bytes(), &root),
                "leaf {i} of {n} does not verify against the fast root"
            );
        }
    }
}

/// The cost of a root is the population count of the log's length, so it
/// is logarithmic rather than linear.
#[test]
fn the_cost_of_a_root_is_logarithmic() {
    let mut log = MerkleLog::new();
    for i in 0..4_096u32 {
        log.append(&i.to_le_bytes());
    }
    // 4096 is a power of two: exactly one perfect subtree
    assert_eq!(log.frontier_len(), 1);

    log.append(&9_999u32.to_le_bytes());
    assert_eq!(log.frontier_len(), 2, "4097 = 4096 + 1");

    let mut odd = MerkleLog::new();
    for i in 0..4_095u32 {
        odd.append(&i.to_le_bytes());
    }
    // 4095 is twelve ones in binary, and that is the whole cost
    assert_eq!(odd.frontier_len(), 12);
}

/// Forgetting old leaves must not move the root.
///
/// This is what makes an entity affordable on a board. A populated
/// entity at embedded bounds leaves about two thousand decisions' worth
/// of headroom, which at ten gated decisions a second is a few minutes,
/// and the log grows 32 bytes per decision forever. Keeping every leaf
/// in SRAM was a workstation decision being made silently on the board.
///
/// The frontier is what makes it safe: the root comes from O(log n)
/// subtree hashes, not from the leaves. Every checkpoint an entity ever
/// signed stays correct.
#[test]
fn forgetting_old_leaves_does_not_move_the_root() {
    let mut log = MerkleLog::new();
    for i in 0..1_000u32 {
        log.append(&i.to_le_bytes());
    }
    let before = log.root();
    let len_before = log.len();

    let dropped = log.forget_leaves_before(64);
    assert_eq!(dropped, 936);
    assert_eq!(log.root(), before, "forgetting leaves moved the root");
    assert_eq!(log.len(), len_before, "the log forgot how long it is");
    assert_eq!(log.provable(), 64);
    assert_eq!(log.forgotten(), 936);

    // and it keeps growing correctly afterwards
    for i in 1_000..1_100u32 {
        log.append(&i.to_le_bytes());
    }
    let mut fresh = MerkleLog::new();
    for i in 0..1_100u32 {
        fresh.append(&i.to_le_bytes());
    }
    assert_eq!(
        log.root(),
        fresh.root(),
        "a log that forgot and then grew disagrees with one that never forgot"
    );
}

/// What is actually lost is the ability to *produce* a proof for a
/// forgotten decision. A proof issued before it was forgotten still
/// verifies, because a proof is checked against a root rather than
/// against the log.
#[test]
fn a_proof_issued_before_forgetting_still_verifies_afterwards() {
    let mut log = MerkleLog::new();
    for i in 0..500u32 {
        log.append(&i.to_le_bytes());
    }
    let proof = log.prove(7).expect("a proof while the leaf is held");
    let root = log.root();

    log.forget_leaves_before(16);
    assert!(
        log.prove(7).is_none(),
        "a proof was produced for a leaf the log no longer holds"
    );
    assert!(
        proof.verify_record(&7u32.to_le_bytes(), &root),
        "a proof issued earlier stopped verifying, which would make forgetting \
         retroactively destroy evidence"
    );
    assert_eq!(log.root(), root);
}
