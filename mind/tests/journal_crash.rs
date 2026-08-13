//! Stage 1.4 — crash consistency: the two-slot journal must survive a
//! torn write at EVERY byte boundary, and corruption anywhere in a
//! record must invalidate it entirely — never half-load it.

use brainos_mind::journal::{newest, open, plan_write, seal, Slot, MAX_PAYLOAD};

#[test]
fn seal_open_roundtrip() {
    for payload in [&b""[..], b"x", b"episode|1|90|met the human\n"] {
        let rec = seal(42, payload);
        let (gen, got) = open(&rec).expect("sealed record must open");
        assert_eq!(gen, 42);
        assert_eq!(got, payload);
    }
}

#[test]
fn torn_write_at_every_byte_is_invalid() {
    let rec = seal(7, b"the whole self lives in this payload");
    for cut in 0..rec.len() {
        assert!(
            open(&rec[..cut]).is_none(),
            "a record torn at byte {cut} must not open"
        );
    }
    assert!(open(&rec).is_some());
}

#[test]
fn any_flipped_bit_invalidates_the_record() {
    let rec = seal(9, b"memory integrity is the point");
    for i in 0..rec.len() {
        let mut bad = rec.clone();
        bad[i] ^= 0x40;
        if let Some((gen, payload)) = open(&bad) {
            panic!(
                "bit flip at byte {i} still opened (gen {gen}, {} bytes) — corruption went undetected",
                payload.len()
            );
        }
    }
}

#[test]
fn oversize_and_lying_lengths_are_refused() {
    // a record claiming more payload than exists
    let mut rec = seal(1, b"short");
    rec[16..20].copy_from_slice(&(1000u32).to_le_bytes());
    assert!(open(&rec).is_none());
    // a record claiming more than the global bound
    let mut rec = seal(1, b"short");
    rec[16..20].copy_from_slice(&((MAX_PAYLOAD as u32) + 1).to_le_bytes());
    assert!(open(&rec).is_none());
    // trailing bytes after a valid record are tolerated (FAT slack)
    let mut rec = seal(1, b"short");
    rec.extend_from_slice(b"slack");
    assert!(open(&rec).is_some());
}

#[test]
fn writes_always_target_the_older_slot() {
    assert_eq!(plan_write(None, None), (Slot::A, 1));
    assert_eq!(plan_write(Some(3), None), (Slot::B, 4));
    assert_eq!(plan_write(None, Some(3)), (Slot::A, 4));
    assert_eq!(plan_write(Some(5), Some(4)), (Slot::B, 6));
    assert_eq!(plan_write(Some(4), Some(5)), (Slot::A, 6));
    // tie (should not happen, but must not deadlock the plan)
    assert_eq!(plan_write(Some(4), Some(4)), (Slot::B, 5));
}

#[test]
fn newest_prefers_valid_over_recent() {
    let old = seal(3, b"yesterday's self");
    let new = seal(4, b"today's self");
    // both valid: newest generation wins
    assert_eq!(newest(&old, &new).unwrap().1, b"today's self");
    assert_eq!(newest(&new, &old).unwrap().1, b"today's self");
    // the newer record torn: the valid older one wins
    let torn = &new[..new.len() - 3];
    assert_eq!(newest(&old, torn).unwrap().1, b"yesterday's self");
    // both torn: honestly nothing
    assert!(newest(&old[..5], torn).is_none());
}

/// The property the whole design exists for: simulate power loss at every
/// byte of every write across many generations — after any crash, the pair
/// of slots always yields either the previous self or the new one. Never
/// garbage, never nothing.
#[test]
fn power_loss_mid_write_never_loses_the_self() {
    let mut slot_a: Vec<u8> = Vec::new();
    let mut slot_b: Vec<u8> = Vec::new();

    // first, one committed generation to protect
    let rec = seal(1, b"self@1");
    slot_a = rec.clone();
    let mut committed: Vec<u8> = b"self@1".to_vec();

    for gen in 2u64..12 {
        let new_payload = format!("self@{gen}").into_bytes();
        let ga = open(&slot_a).map(|(g, _)| g);
        let gb = open(&slot_b).map(|(g, _)| g);
        let (target, next_gen) = plan_write(ga, gb);
        let rec = seal(next_gen, &new_payload);

        // crash at every possible point of this write
        for cut in 0..=rec.len() {
            let (mut ta, mut tb) = (slot_a.clone(), slot_b.clone());
            let torn = rec[..cut].to_vec();
            match target {
                Slot::A => ta = torn,
                Slot::B => tb = torn,
            }
            let survivor = newest(&ta, &tb).expect("a committed self must always survive");
            assert!(
                survivor.1 == committed.as_slice() || survivor.1 == new_payload.as_slice(),
                "after a crash at byte {cut} of gen {next_gen}, woke as neither the \
                 previous self nor the new one"
            );
        }

        // no crash: the write completes and becomes the committed self
        match target {
            Slot::A => slot_a = rec,
            Slot::B => slot_b = rec,
        }
        committed = new_payload;
        assert_eq!(newest(&slot_a, &slot_b).unwrap().1, committed.as_slice());
    }
}
