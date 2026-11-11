//! BFS  LEVEL    Foundation - the wire (frames, sessions, links)  [crate brainos-transport]
//! BFS  PURPOSE  Tests for transport: frames round-trip, wrong keys and corruption are refused, replayed commands are refused, link health is reported
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, crypto, transport
//! BFS  USED BY  (entry point or test)
//!
//! The wire suite.
//!
//! The property that matters most here is the one that is easiest to
//! get wrong and hardest to notice: a replayed "step forward" is how a
//! robot walks off a table.

use brainos_abi::ids::{BodyId, EntityId};
use brainos_abi::time::{Duration, Instant};
use brainos_crypto::key::{BrainKey, PublicKey};
use brainos_crypto::rng::Drbg;
use brainos_transport::frame::{Frame, FrameError, Kind, MAX_PAYLOAD, OVERHEAD};
use brainos_transport::link::{Health, Link};
use brainos_transport::loopback::{Faults, Loopback};
use brainos_transport::session::{Peer, Wire, WireError};

fn key() -> [u8; 32] {
    [7; 32]
}

// ---------- framing ----------

#[test]
fn a_frame_round_trips() {
    let f = Frame::new(Kind::Command, 42, b"walk 0.3").unwrap();
    let raw = f.seal(&key());
    let (back, used) = Frame::open(&raw, &key()).unwrap();
    assert_eq!(back, f);
    assert_eq!(used, raw.len());
    assert_eq!(used, OVERHEAD + 8);
}

#[test]
fn a_frame_from_the_wrong_key_does_not_arrive() {
    let f = Frame::new(Kind::Command, 1, b"walk").unwrap();
    let raw = f.seal(&key());
    assert_eq!(Frame::open(&raw, &[8; 32]), Err(FrameError::Forged));
}

#[test]
fn one_flipped_bit_anywhere_is_caught() {
    let f = Frame::new(Kind::Command, 1, b"walk 0.3 heading 0").unwrap();
    let raw = f.seal(&key());
    for i in 0..raw.len() {
        for bit in 0..8 {
            let mut bad = raw.clone();
            bad[i] ^= 1 << bit;
            if bad == raw {
                continue;
            }
            assert!(
                Frame::open(&bad, &key()).is_err(),
                "byte {i} bit {bit} got through"
            );
        }
    }
}

#[test]
fn a_hostile_length_field_cannot_ask_for_the_world() {
    let f = Frame::new(Kind::Command, 1, b"x").unwrap();
    let mut raw = f.seal(&key());
    raw[12] = 0xFF;
    raw[13] = 0xFF;
    assert_eq!(Frame::open(&raw, &key()), Err(FrameError::BadLength));
}

#[test]
fn an_oversized_payload_is_refused_at_construction() {
    let big = vec![0u8; MAX_PAYLOAD + 1];
    assert_eq!(Frame::new(Kind::Command, 1, &big), Err(FrameError::TooLarge));
    assert!(Frame::new(Kind::Command, 1, &big[..MAX_PAYLOAD]).is_ok());
}

#[test]
fn garbage_is_recognized_as_garbage_rather_than_parsed() {
    assert_eq!(Frame::open(b"", &key()), Err(FrameError::Short));
    assert_eq!(Frame::open(&[0u8; 64], &key()), Err(FrameError::NotAFrame));
    let mut wrong_kind = Frame::new(Kind::Ping, 1, b"").unwrap().seal(&key());
    wrong_kind[2] = 200;
    assert_eq!(
        Frame::open(&wrong_kind, &key()),
        Err(FrameError::UnknownKind)
    );
}

#[test]
fn a_stream_that_lost_sync_can_find_the_next_frame() {
    let f = Frame::new(Kind::Telemetry, 9, b"tilt 1.0").unwrap();
    let raw = f.seal(&key());
    let mut noisy = vec![0xAAu8, 0x55, 0x00, 0xFF];
    noisy.extend_from_slice(&raw);
    let at = Frame::resync(&noisy).unwrap();
    assert_eq!(at, 4);
    let (back, _) = Frame::open(&noisy[at..], &key()).unwrap();
    assert_eq!(back.counter, 9);
}

#[test]
fn only_the_handshake_may_arrive_before_attestation() {
    assert!(Kind::Hello.pre_attestation());
    assert!(Kind::Attest.pre_attestation());
    for k in [
        Kind::Command,
        Kind::Telemetry,
        Kind::Schema,
        Kind::Calibration,
        Kind::Ping,
        Kind::Stopped,
    ] {
        assert!(!k.pre_attestation(), "{k:?}");
    }
}

// ---------- sessions ----------

fn pair() -> (Wire, Wire, BrainKey) {
    let entity = BrainKey::from_seed(&[1; 32]);
    let body = BodyId::from_bytes([2; 32]);
    let mut rng = Drbg::deterministic(b"session");
    let (mut a, nonce) = Wire::initiate(&entity, body, &mut rng, Instant(0));
    let subkey = entity.derive_subkey(&body, 32);
    let mut b = Wire::accept(&subkey, body, nonce, Instant(0));

    let peer = Peer {
        entity: Some(entity.entity_id()),
        body: Some(body),
        key: entity.public(),
    };
    a.attested(peer);
    b.attested(peer);
    (a, b, entity)
}

#[test]
fn both_sides_derive_the_same_key_without_it_crossing_the_wire() {
    let (mut a, mut b, _) = pair();
    let raw = a.send(Kind::Command, b"walk 0.3").unwrap();
    let got = b.receive(&raw).unwrap();
    assert_eq!(got.payload, b"walk 0.3");
    assert_eq!(got.kind, Kind::Command);
}

#[test]
fn a_replayed_command_is_refused() {
    let (mut a, mut b, _) = pair();
    let raw = a.send(Kind::Command, b"step forward").unwrap();
    assert!(b.receive(&raw).is_ok());
    // the same bytes again — this is how a robot walks off a table
    let err = b.receive(&raw).unwrap_err();
    assert!(matches!(
        err,
        WireError::Frame(FrameError::Replayed { .. })
    ));
    assert_eq!(b.replays_refused, 1);
}

#[test]
fn an_old_frame_delivered_late_is_refused() {
    let (mut a, mut b, _) = pair();
    let first = a.send(Kind::Command, b"one").unwrap();
    let second = a.send(Kind::Command, b"two").unwrap();
    // the second overtakes the first on a reordering link
    assert!(b.receive(&second).is_ok());
    assert!(
        b.receive(&first).is_err(),
        "a stale command is worse than a dropped one"
    );
}

#[test]
fn nothing_but_the_handshake_is_accepted_before_attestation() {
    let entity = BrainKey::from_seed(&[1; 32]);
    let body = BodyId::from_bytes([2; 32]);
    let mut rng = Drbg::deterministic(b"x");
    let (mut a, nonce) = Wire::initiate(&entity, body, &mut rng, Instant(0));
    let subkey = entity.derive_subkey(&body, 32);
    let mut b = Wire::accept(&subkey, body, nonce, Instant(0));

    // neither side is attested yet
    assert!(!a.is_attested());
    assert_eq!(a.send(Kind::Command, b"walk"), Err(WireError::NotAttested));

    // the handshake itself goes through
    let hello = a.send(Kind::Hello, b"hello").unwrap();
    assert!(b.receive(&hello).is_ok());

    // and a command sent by a peer that *is* attested still does not get
    // in on a receiver that is not — a fresh receiver, so the refusal is
    // about attestation rather than about the replay counter
    let mut a2 = Wire::accept(&subkey, body, nonce, Instant(0));
    a2.attested(Peer {
        entity: Some(entity.entity_id()),
        body: Some(body),
        key: entity.public(),
    });
    let sneaky = a2.send(Kind::Command, b"walk").unwrap();
    let mut fresh = Wire::accept(&subkey, body, nonce, Instant(0));
    assert!(!fresh.is_attested());
    assert_eq!(fresh.receive(&sneaky), Err(WireError::NotAttested));
}

#[test]
fn a_recorded_handshake_replayed_later_produces_a_useless_key() {
    let entity = BrainKey::from_seed(&[1; 32]);
    let body = BodyId::from_bytes([2; 32]);
    let mut rng = Drbg::deterministic(b"replay");

    let (mut first, nonce_a) = Wire::initiate(&entity, body, &mut rng, Instant(0));
    let (_second, nonce_b) = Wire::initiate(&entity, body, &mut rng, Instant(1));
    assert_ne!(nonce_a, nonce_b, "a fresh nonce per session");

    first.attested(Peer {
        entity: Some(entity.entity_id()),
        body: Some(body),
        key: entity.public(),
    });
    let raw = first.send(Kind::Command, b"walk").unwrap();

    // a body that started the *second* session cannot open the first
    // session's frames
    let subkey = entity.derive_subkey(&body, 32);
    let mut wrong_session = Wire::accept(&subkey, body, nonce_b, Instant(1));
    wrong_session.attested(Peer {
        entity: Some(entity.entity_id()),
        body: Some(body),
        key: entity.public(),
    });
    assert!(matches!(
        wrong_session.receive(&raw),
        Err(WireError::Frame(FrameError::Forged))
    ));
}

#[test]
fn a_different_body_derives_a_different_key() {
    let entity = BrainKey::from_seed(&[1; 32]);
    let body_a = BodyId::from_bytes([2; 32]);
    let body_b = BodyId::from_bytes([3; 32]);
    let mut rng = Drbg::deterministic(b"bodies");
    let (mut wire_a, nonce) = Wire::initiate(&entity, body_a, &mut rng, Instant(0));
    wire_a.attested(Peer {
        entity: Some(entity.entity_id()),
        body: Some(body_a),
        key: entity.public(),
    });
    let raw = wire_a.send(Kind::Command, b"walk").unwrap();

    // body B, same entity, same nonce — a compromised body cannot
    // impersonate its sibling
    let subkey_b = entity.derive_subkey(&body_b, 32);
    let mut wire_b = Wire::accept(&subkey_b, body_b, nonce, Instant(0));
    wire_b.attested(Peer {
        entity: Some(entity.entity_id()),
        body: Some(body_b),
        key: entity.public(),
    });
    assert!(wire_b.receive(&raw).is_err());
}

#[test]
fn a_closed_session_accepts_nothing() {
    let (mut a, mut b, _) = pair();
    let raw = a.send(Kind::Ping, b"").unwrap();
    b.close();
    assert_eq!(b.receive(&raw), Err(WireError::Closed));
    assert_eq!(b.send(Kind::Pong, b""), Err(WireError::Closed));
}

#[test]
fn a_challenge_answer_is_checked_against_the_key_we_expect() {
    let entity = BrainKey::from_seed(&[1; 32]);
    let device = BrainKey::from_seed(&[2; 32]);
    let impostor = BrainKey::from_seed(&[3; 32]);
    let mut rng = Drbg::deterministic(b"c");
    let body = BodyId::from_bytes([2; 32]);
    let (w, _) = Wire::initiate(&entity, body, &mut rng, Instant(0));

    let c = w.challenge(entity.entity_id(), &mut rng);
    let good = device.answer(&c);
    assert!(Wire::check_answer(&c, &device.public(), &good));
    assert!(!Wire::check_answer(&c, &impostor.public(), &good));
}

// ---------- link health ----------

#[test]
fn a_link_reports_health_and_never_decides_anything() {
    let mut l = Link::for_a_body_that_can_fall(Instant(0));
    assert_eq!(l.health(Instant(10_000)), Health::Good);
    assert_eq!(l.health(Instant(100_000)), Health::Quiet);
    assert_eq!(l.health(Instant(300_000)), Health::Lost);
    l.heard(Instant(300_000));
    assert_eq!(l.health(Instant(310_000)), Health::Good);
    l.close();
    assert_eq!(l.health(Instant(310_000)), Health::Closed);
}

#[test]
fn silence_is_the_number_the_reflex_layer_actually_needs() {
    let l = Link::new(Instant(1_000));
    assert_eq!(l.silence(Instant(1_500)), Duration::from_micros(500));
    // a clock that appears to go backwards yields zero, never an age
    assert_eq!(l.silence(Instant(500)), Duration::ZERO);
}

#[test]
fn a_busy_link_does_not_need_keepalives() {
    let mut l = Link::for_a_body_that_can_fall(Instant(0));
    l.sent(Instant(0));
    assert!(!l.should_ping(Instant(10_000)));
    assert!(l.should_ping(Instant(30_000)));
}

#[test]
fn round_trips_are_measured_and_the_worst_one_is_kept() {
    let mut l = Link::new(Instant(0));
    l.ping_sent(1, Instant(0));
    assert_eq!(l.pong(1, Instant(5_000)), Some(Duration::from_millis(5)));
    l.ping_sent(2, Instant(10_000));
    assert_eq!(l.pong(2, Instant(12_000)), Some(Duration::from_millis(2)));
    assert_eq!(l.stats.rtt, Duration::from_millis(2));
    assert_eq!(l.stats.worst_rtt, Duration::from_millis(5));
}

#[test]
fn an_unanswered_ping_is_counted() {
    let mut l = Link::new(Instant(0));
    l.ping_sent(1, Instant(0));
    l.ping_sent(2, Instant(1_000));
    assert_eq!(l.stats.unanswered, 1);
}

#[test]
fn an_unmatched_pong_is_still_evidence_of_life() {
    let mut l = Link::new(Instant(0));
    assert_eq!(l.pong(99, Instant(5_000)), None);
    assert_eq!(l.silence(Instant(5_000)), Duration::ZERO);
}

// ---------- the wire under a bad link ----------

#[test]
fn a_perfect_link_delivers_everything_in_order() {
    let (mut a, mut b, _) = pair();
    let mut wire = Loopback::perfect();
    for i in 0..100 {
        wire.a_sends(a.send(Kind::Command, &[i as u8]).unwrap());
    }
    let mut got = 0;
    while let Some(raw) = wire.b_receives() {
        assert!(b.receive(&raw).is_ok());
        got += 1;
    }
    assert_eq!(got, 100);
    assert_eq!(b.frames_refused, 0);
}

#[test]
fn a_noisy_link_loses_frames_and_never_delivers_a_wrong_one() {
    let (mut a, mut b, _) = pair();
    let mut wire = Loopback::new(Faults::NOISY, b"noisy");

    let sent = 500;
    for i in 0..sent {
        wire.a_sends(a.send(Kind::Command, &(i as u32).to_le_bytes()).unwrap());
    }
    wire.settle();

    let mut delivered = Vec::new();
    while let Some(raw) = wire.b_receives() {
        if let Ok(f) = b.receive(&raw) {
            delivered.push(f);
        }
    }

    // some were lost; that is the link's business
    assert!(delivered.len() < sent, "the fault model did nothing");
    assert!(delivered.len() > sent / 2, "the fault model ate everything");

    // but every frame that arrived is genuine, whole, and in order
    let mut last = 0u64;
    for f in &delivered {
        assert!(f.counter > last, "out of order or replayed");
        last = f.counter;
        assert_eq!(f.payload.len(), 4);
    }
    assert!(
        b.frames_refused > 0,
        "the corruption and duplication should have been caught"
    );
}

#[test]
fn a_duplicating_link_cannot_make_a_body_act_twice() {
    let (mut a, mut b, _) = pair();
    let mut wire = Loopback::new(
        Faults {
            duplicate_per_mille: 1000, // everything twice
            ..Faults::PERFECT
        },
        b"dup",
    );
    for i in 0..50 {
        wire.a_sends(a.send(Kind::Command, &[i as u8]).unwrap());
    }
    wire.settle();

    let mut accepted = 0;
    let mut refused = 0;
    while let Some(raw) = wire.b_receives() {
        match b.receive(&raw) {
            Ok(_) => accepted += 1,
            Err(_) => refused += 1,
        }
    }
    assert_eq!(accepted, 50, "each command exactly once");
    assert_eq!(refused, 50, "and each duplicate refused");
    assert_eq!(b.replays_refused, 50);
}

#[test]
fn the_fault_model_is_deterministic() {
    let run = || {
        let (mut a, mut b, _) = pair();
        let mut wire = Loopback::new(Faults::NOISY, b"seed");
        for i in 0..200 {
            wire.a_sends(a.send(Kind::Telemetry, &[i as u8]).unwrap());
        }
        wire.settle();
        let mut n = 0;
        while let Some(raw) = wire.b_receives() {
            if b.receive(&raw).is_ok() {
                n += 1;
            }
        }
        (n, wire.dropped, wire.corrupted, wire.duplicated)
    };
    assert_eq!(run(), run());
}

#[test]
fn a_peer_carries_who_it_is_rather_than_what_it_claimed() {
    let entity = BrainKey::from_seed(&[1; 32]);
    let p = Peer {
        entity: Some(entity.entity_id()),
        body: Some(BodyId::from_bytes([2; 32])),
        key: entity.public(),
    };
    assert_eq!(p.key.entity_id(), entity.entity_id());
    // the public key and the claimed id must agree
    assert_eq!(p.entity, Some(p.key.entity_id()));
    let _: PublicKey = p.key;
    let _: Option<EntityId> = p.entity;
}
