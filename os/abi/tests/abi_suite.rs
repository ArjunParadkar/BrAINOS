//! The ABI suite. These tests exist to defend three properties the rest
//! of the system assumes without checking: encodings are canonical,
//! refusals are correctly labelled, and verbs cannot be widened.

use brainos_abi::codec::{Decode, Decoder, Encode, Encoder, MAX_LEN};
use brainos_abi::error::{Denied, DenyCode, Error, Stage};
use brainos_abi::ids::{BodyId, EntityId, Id32, NodeId};
use brainos_abi::text::Text;
use brainos_abi::time::{Duration, Instant, Tick};
use brainos_abi::verb::{Namespace, Verb};
use brainos_abi::version::{AbiVersion, ABI};

// ---------- ids ----------

#[test]
fn id_hex_round_trips_and_rejects_anything_else() {
    let id = Id32([0xAB; 32]);
    let mut hex = [0u8; 64];
    id.write_hex(&mut hex);
    let s = core::str::from_utf8(&hex).unwrap();
    assert_eq!(s, "ab".repeat(32));
    assert_eq!(Id32::parse_hex(s).unwrap(), id);

    // a short id is an error, never a prefix match
    assert!(Id32::parse_hex("abab").is_err());
    assert!(Id32::parse_hex(&"ab".repeat(33)).is_err());
    assert!(Id32::parse_hex(&format!("{}zz", "ab".repeat(31))).is_err());
}

#[test]
fn constant_time_equality_agrees_with_ordinary_equality() {
    let a = Id32([1; 32]);
    let b = Id32([1; 32]);
    let mut c = [1u8; 32];
    c[31] = 2;
    let c = Id32(c);
    assert!(a.ct_eq(&b));
    assert!(!a.ct_eq(&c));
    assert_eq!(a == b, a.ct_eq(&b));
    assert_eq!(a == c, a.ct_eq(&c));
}

#[test]
fn fingerprints_are_display_only_and_distinct_types() {
    let id = EntityId::parse_hex(&"1234abcd".repeat(8)).unwrap();
    let f = id.fingerprint();
    assert_eq!(f.as_str().len(), 14);
    assert!(f.as_str().contains(".."));
    // the fingerprint is the first 8 and last 4 hex characters
    assert!(f.as_str().starts_with("1234abcd"));
    assert!(f.as_str().ends_with("abcd"));
}

#[test]
fn id_newtypes_do_not_interconvert() {
    // This is really a compile-time property; the test documents it and
    // checks the bytes are still reachable when deliberately converted.
    let e = EntityId::from_bytes([7; 32]);
    let b = BodyId::from_bytes(*e.as_bytes());
    assert_eq!(e.as_bytes(), b.as_bytes());
}

// ---------- codec ----------

#[test]
fn primitives_round_trip() {
    let mut e = Encoder::new();
    e.u8(0xFE);
    e.u16(0xBEEF);
    e.u32(0xDEAD_BEEF);
    e.u64(0x0123_4567_89AB_CDEF);
    e.i64(-42);
    e.bool(true);
    e.bool(false);
    e.str("hello");
    e.bytes(&[1, 2, 3]);
    let bytes = e.finish().unwrap();

    let mut d = Decoder::new(&bytes);
    assert_eq!(d.u8().unwrap(), 0xFE);
    assert_eq!(d.u16().unwrap(), 0xBEEF);
    assert_eq!(d.u32().unwrap(), 0xDEAD_BEEF);
    assert_eq!(d.u64().unwrap(), 0x0123_4567_89AB_CDEF);
    assert_eq!(d.i64().unwrap(), -42);
    assert!(d.bool().unwrap());
    assert!(!d.bool().unwrap());
    assert_eq!(d.str().unwrap(), "hello");
    assert_eq!(d.bytes().unwrap(), &[1, 2, 3]);
    d.finish().unwrap();
}

#[test]
fn encoding_is_canonical_for_floats() {
    // every NaN encodes identically, and -0.0 encodes as +0.0 — otherwise
    // a signed structure would have a covert channel in its float fields
    let nan_a = f32::NAN;
    let nan_b = f32::from_bits(0x7FC0_1234);
    assert!(nan_b.is_nan());
    let mut ea = Encoder::new();
    ea.f32(nan_a);
    let mut eb = Encoder::new();
    eb.f32(nan_b);
    assert_eq!(ea.finish().unwrap(), eb.finish().unwrap());

    let mut ep = Encoder::new();
    ep.f64(0.0);
    let mut en = Encoder::new();
    en.f64(-0.0);
    assert_eq!(ep.finish().unwrap(), en.finish().unwrap());
}

#[test]
fn a_bool_is_only_zero_or_one() {
    let mut d = Decoder::new(&[2]);
    assert!(matches!(d.bool(), Err(Error::Malformed(_))));
}

#[test]
fn trailing_bytes_are_rejected() {
    let mut e = Encoder::new();
    e.u32(7);
    let mut bytes = e.finish().unwrap();
    bytes.push(0);
    assert!(matches!(u32::from_bytes(&bytes), Err(Error::Malformed(_))));
}

#[test]
fn a_fabricated_length_cannot_make_the_decoder_reserve_the_world() {
    // a header claiming a megabyte, followed by nothing
    let mut e = Encoder::new();
    e.u32(1_000_000);
    let bytes = e.finish().unwrap();
    let mut d = Decoder::new(&bytes);
    assert!(matches!(d.len_prefix(), Err(Error::Truncated)));

    // and one claiming more than MAX_LEN is rejected on the cap first
    let mut e = Encoder::new();
    e.u32(MAX_LEN as u32 + 1);
    let bytes = e.finish().unwrap();
    let mut d = Decoder::new(&bytes);
    assert!(matches!(d.len_prefix(), Err(Error::Capacity(_))));
}

#[test]
fn truncated_input_is_an_error_not_a_partial_value() {
    let mut e = Encoder::new();
    e.u64(1);
    let bytes = e.finish().unwrap();
    let mut d = Decoder::new(&bytes[..4]);
    assert!(matches!(d.u64(), Err(Error::Truncated)));
}

#[test]
fn vectors_round_trip_through_the_traits() {
    let v: Vec<u64> = vec![1, 2, 3, 4, 5];
    let bytes = v.to_bytes().unwrap();
    assert_eq!(Vec::<u64>::from_bytes(&bytes).unwrap(), v);
}

#[test]
fn strings_must_be_utf8() {
    let mut e = Encoder::new();
    e.bytes(&[0xFF, 0xFE]);
    let bytes = e.finish().unwrap();
    let mut d = Decoder::new(&bytes);
    assert!(matches!(d.str(), Err(Error::Malformed(_))));
}

// ---------- errors and refusals ----------

#[test]
fn every_deny_code_is_labelled_with_its_own_stage() {
    // the proof harness leans on this; check it here too, cheaply
    for code in DenyCode::ALL {
        let d = Denied::new(code, "");
        assert_eq!(
            d.stage,
            code.stage(),
            "{code} is labelled with the wrong stage"
        );
    }
}

#[test]
fn a_refusal_cannot_be_built_with_the_wrong_stage() {
    // `Denied::new` derives the stage from the code, so this is the only
    // stage a DriveViolation can ever carry
    let d = Denied::new(DenyCode::DriveViolation, "preserve memory integrity");
    assert_eq!(d.stage, Stage::Policy);
    assert!(d.is_salient());
}

#[test]
fn stages_run_in_a_fixed_order_ending_at_audit() {
    assert_eq!(Stage::ALL.len(), 8);
    for (i, s) in Stage::ALL.iter().enumerate() {
        assert_eq!(s.index(), i);
        assert_eq!(Stage::from_index(i), Some(*s));
    }
    assert_eq!(Stage::Parse.next(), Some(Stage::Authenticate));
    assert_eq!(Stage::Commit.next(), Some(Stage::Audit));
    assert_eq!(Stage::Audit.next(), None);
    // the order is the argument: policy is before commit, always
    assert!(Stage::Policy < Stage::Commit);
    assert!(Stage::Authorize < Stage::Simulate);
}

#[test]
fn routine_refusals_do_not_wake_the_human() {
    assert!(!DenyCode::StaleRequest.is_salient());
    assert!(!DenyCode::OutOfBounds.is_salient());
    assert!(DenyCode::DriveViolation.is_salient());
    assert!(DenyCode::PredictedInvariantBreak.is_salient());
}

// ---------- text ----------

#[test]
fn text_truncates_on_a_character_boundary_and_says_so() {
    // 'é' is two bytes; a capacity of 4 must drop it whole, not halve it
    let mut t: Text<4> = Text::new();
    t.push_str("abcé");
    assert_eq!(t.as_str(), "abc");
    assert!(t.was_truncated());
    assert!(core::str::from_utf8(t.as_bytes()).is_ok());
}

#[test]
fn exact_text_refuses_rather_than_silently_shortening() {
    assert!(Text::<4>::from_str_exact("abcde").is_err());
    assert_eq!(Text::<8>::from_str_exact("abcde").unwrap().as_str(), "abcde");
}

#[test]
fn text_formats_numbers_without_an_allocator() {
    let mut t: Text<32> = Text::new();
    t.push_str("servo ");
    t.push_u64(7);
    t.push_str(" at ");
    t.push_i64(-45);
    t.push_str("deg");
    assert_eq!(t.as_str(), "servo 7 at -45deg");

    let mut z: Text<8> = Text::new();
    z.push_u64(0);
    assert_eq!(z.as_str(), "0");
}

#[test]
fn text_round_trips_and_refuses_oversize_on_decode() {
    let t = Text::<16>::from_str_exact("walk").unwrap();
    let bytes = t.to_bytes().unwrap();
    assert_eq!(Text::<16>::from_bytes(&bytes).unwrap(), t);
    // the same bytes will not fit in a smaller Text
    assert!(matches!(
        Text::<3>::from_bytes(&bytes),
        Err(Error::Capacity(_))
    ));
}

// ---------- time ----------

#[test]
fn ticks_saturate_rather_than_wrapping() {
    let t = Tick(u64::MAX);
    assert_eq!(t.next(), Tick(u64::MAX));
    assert_eq!(Tick(5).since(Tick(9)), 0);
    assert_eq!(Tick(9).since(Tick(5)), 4);
}

#[test]
fn a_clock_that_goes_backwards_yields_zero_not_an_age() {
    let early = Instant(1_000);
    let late = Instant(500);
    assert_eq!(late.since(early), Duration::ZERO);
    assert_eq!(late.checked_since(early), None);
    assert_eq!(early.since(late), Duration::from_micros(500));
}

#[test]
fn durations_convert_and_display() {
    assert_eq!(Duration::from_millis(1).as_micros(), 1_000);
    assert_eq!(Duration::from_hz(100), Duration::from_micros(10_000));
    assert_eq!(Duration::from_hz(0), Duration::FOREVER);
    assert_eq!(format!("{}", Duration::from_micros(250)), "250us");
    assert_eq!(format!("{}", Duration::from_micros(2_500)), "2.500ms");
    assert_eq!(format!("{}", Duration::from_micros(2_500_000)), "2.500s");
}

#[test]
fn deadlines_are_points_not_spans() {
    let start = Instant(0);
    let deadline = start + Duration::from_millis(10);
    assert!(!Instant(9_999).has_passed(deadline));
    assert!(Instant(10_000).has_passed(deadline));
}

// ---------- verbs ----------

#[test]
fn verbs_parse_only_the_grammar() {
    let v = Verb::parse("legs.walk").unwrap();
    assert_eq!(v.namespace(), "legs");
    assert_eq!(v.action(), "walk");
    assert_eq!(v.as_str(), "legs.walk");

    for bad in [
        "walk",           // no namespace
        "legs.",          // empty action
        ".walk",          // empty namespace
        "legs.sub.walk",  // two dots
        "Legs.walk",      // uppercase
        "legs walk",      // space
        "legs.wälk",      // non-ascii
        "legs.*",         // wildcards live in patterns, not verbs
        "",
    ] {
        assert!(Verb::parse(bad).is_err(), "{bad:?} should not parse");
    }
}

#[test]
fn verb_length_is_bounded() {
    let long_ns = "a".repeat(24);
    assert!(Verb::parse(&format!("{long_ns}.walk")).is_err());
    let ok_ns = "a".repeat(23);
    assert!(Verb::parse(&format!("{ok_ns}.walk")).is_ok());
}

#[test]
fn namespaces_never_widen() {
    let any = Namespace::Any;
    let legs = Namespace::only("legs").unwrap();
    let walk = Namespace::Exact(Verb::literal("legs.walk"));
    let arm = Namespace::only("arm").unwrap();

    // narrowing is allowed in every direction down the lattice
    assert!(walk.is_within(&legs));
    assert!(walk.is_within(&any));
    assert!(legs.is_within(&any));

    // widening never is
    assert!(!legs.is_within(&walk));
    assert!(!any.is_within(&legs));
    assert!(!legs.is_within(&arm));
    assert!(!walk.is_within(&arm));

    // and the width rank agrees with the relation
    assert!(any.width() < legs.width());
    assert!(legs.width() < walk.width());
}

#[test]
fn a_verb_matches_only_patterns_that_cover_it() {
    let walk = Verb::literal("legs.walk");
    assert!(walk.matches(&Namespace::Any));
    assert!(walk.matches(&Namespace::only("legs").unwrap()));
    assert!(!walk.matches(&Namespace::only("arm").unwrap()));
    assert!(walk.matches(&Namespace::Exact(walk)));
    assert!(!walk.matches(&Namespace::Exact(Verb::literal("legs.turn"))));
}

#[test]
fn verbs_and_namespaces_round_trip() {
    let v = Verb::literal("fs.read");
    assert_eq!(Verb::from_bytes(&v.to_bytes().unwrap()).unwrap(), v);
    for ns in [
        Namespace::Any,
        Namespace::only("legs").unwrap(),
        Namespace::Exact(v),
    ] {
        let bytes = ns.to_bytes().unwrap();
        assert_eq!(Namespace::from_bytes(&bytes).unwrap(), ns);
    }
}

// ---------- versioning ----------

#[test]
fn a_newer_peer_is_refused_rather_than_partially_understood() {
    let us = AbiVersion::new(1, 2, 0);
    assert!(us.accepts(&AbiVersion::new(1, 2, 9)));
    assert!(us.accepts(&AbiVersion::new(1, 0, 0)));
    // newer minor: we would ignore fields we do not know, and on a body
    // an ignored field may be an envelope limit
    assert!(!us.accepts(&AbiVersion::new(1, 3, 0)));
    assert!(!us.accepts(&AbiVersion::new(2, 0, 0)));
    assert!(us.why_not(&AbiVersion::new(2, 0, 0)).is_some());
    assert!(us.why_not(&AbiVersion::new(1, 1, 0)).is_none());
}

#[test]
fn the_shipped_abi_accepts_itself() {
    assert!(ABI.accepts(&ABI));
    assert_eq!(
        AbiVersion::from_bytes(&ABI.to_bytes().unwrap()).unwrap(),
        ABI
    );
}

#[test]
fn node_ids_encode_and_zero_means_none() {
    assert!(NodeId::NONE.is_none());
    assert!(!NodeId(1).is_none());
    let n = NodeId(4242);
    assert_eq!(NodeId::from_bytes(&n.to_bytes().unwrap()).unwrap(), n);
}

#[test]
fn decoding_a_float_rejects_anything_the_encoder_would_not_have_written() {
    // Decoding must be injective, or a signature over the bytes is not a
    // signature over the value. The encoder flattens every NaN to one
    // quiet NaN and −0.0 to +0.0, so the decoder has to refuse the
    // un-normalized forms rather than silently re-normalizing them.
    //
    // Found by the hostile-input harness: three of four hundred mutated
    // commands decoded to a value that re-encoded to different bytes,
    // and every one of them was a float whose sign or NaN payload the
    // encoder would have flattened.

    // −0.0
    let mut e = Encoder::new();
    e.u32(0x8000_0000);
    let bytes = e.finish().unwrap();
    assert!(matches!(
        Decoder::new(&bytes).f32(),
        Err(Error::Malformed(_))
    ));

    // a NaN with a payload
    let mut e = Encoder::new();
    e.u32(0x7FC0_1234);
    let bytes = e.finish().unwrap();
    assert!(matches!(
        Decoder::new(&bytes).f32(),
        Err(Error::Malformed(_))
    ));

    // a signalling NaN
    let mut e = Encoder::new();
    e.u32(0x7F80_0001);
    let bytes = e.finish().unwrap();
    assert!(matches!(
        Decoder::new(&bytes).f32(),
        Err(Error::Malformed(_))
    ));

    // and the canonical forms decode fine
    for v in [0.0f32, 1.0, -1.0, f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
        let mut e = Encoder::new();
        e.f32(v);
        let bytes = e.finish().unwrap();
        let back = Decoder::new(&bytes).f32().expect("a canonical float decodes");
        if v.is_nan() {
            assert!(back.is_nan());
        } else {
            assert_eq!(back, v);
        }
    }
}

#[test]
fn the_same_rejection_applies_to_doubles() {
    let mut e = Encoder::new();
    e.u64(0x8000_0000_0000_0000);
    let bytes = e.finish().unwrap();
    assert!(matches!(
        Decoder::new(&bytes).f64(),
        Err(Error::Malformed(_))
    ));

    let mut e = Encoder::new();
    e.u64(0x7FF8_0000_0000_1234);
    let bytes = e.finish().unwrap();
    assert!(matches!(
        Decoder::new(&bytes).f64(),
        Err(Error::Malformed(_))
    ));

    let mut e = Encoder::new();
    e.f64(-0.0);
    let bytes = e.finish().unwrap();
    assert_eq!(
        Decoder::new(&bytes).f64().unwrap().to_bits(),
        0,
        "the encoder flattened it, so the decoder sees the canonical form"
    );
}

#[test]
fn every_float_that_decodes_re_encodes_to_its_own_bytes() {
    // the property stated directly, over the whole 32-bit space sampled
    // coarsely enough to run in a test
    let mut checked = 0u32;
    for high in 0..=0xFFu32 {
        for low in [0x0000u32, 0x0001, 0x1234, 0x7FFF, 0x8000, 0xFFFF] {
            let bits = (high << 24) | (low << 8) | (low & 0xFF);
            let mut e = Encoder::new();
            e.u32(bits);
            let bytes = e.finish().unwrap();
            if let Ok(v) = Decoder::new(&bytes).f32() {
                let mut re = Encoder::new();
                re.f32(v);
                assert_eq!(
                    re.finish().unwrap(),
                    bytes,
                    "bits {bits:#010x} decoded and re-encoded differently"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 1_000, "only {checked} values decoded");
}
