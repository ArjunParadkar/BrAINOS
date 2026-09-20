//! Hostile input.
//!
//! Every decoder in this workspace reads bytes somebody else produced:
//! a body's telemetry, a manufacturer's device description, a web page,
//! a journal slot on storage the owner of the robot can physically
//! reach. Three things must be true of all of them, and none is
//! guaranteed by writing careful code:
//!
//! 1. **Never panic.** A panic in a decoder is a denial of service, and
//!    on a body it is a fall. There is no input for which the right
//!    answer is `unwrap`.
//! 2. **Never half-decode.** Either a whole valid value comes out or an
//!    error does. A partially-populated structure that a caller then
//!    acts on is worse than a refusal.
//! 3. **Canonical.** Anything that decodes must re-encode to exactly the
//!    bytes it came from. This is the property the audit Merkle log, the
//!    export signature and the body-identity digest all rest on: if a
//!    value could arrive two ways, a signature over it proves nothing
//!    about the value.
//!
//! Property 3 is the interesting one and the one a hand-written test
//! suite never checks, because writing a non-canonical encoding by hand
//! requires already knowing where the hole is.
//!
//! Two generators. **Random** bytes find the crashes in length and index
//! handling. **Mutation** — take a valid encoding and flip one bit —
//! finds the ones in the fields, because random bytes almost never get
//! past a magic number and a length check, while a mutated valid frame
//! gets all the way to the logic.

use alloc::vec::Vec;
use brainos_abi::codec::{Decode, Encode};
use brainos_crypto::rng::Drbg;

/// One decoder under test.
pub struct Target {
    pub name: &'static str,
    /// what it reads, in one line
    pub reads: &'static str,
    /// true when the format carries its own integrity check — a MAC, a
    /// signature, a CRC. For these the *expectation is inverted*:
    /// mutation must never get through, and a target where it did would
    /// mean the check is not doing its job.
    pub integrity_protected: bool,
    /// decode; `true` when the bytes produced a value
    pub decode: fn(&[u8]) -> bool,
    /// decode, re-encode, and report whether the bytes are identical.
    /// `None` when the input did not decode at all.
    pub round_trip: fn(&[u8]) -> Option<bool>,
    /// a valid encoding, for the mutation generator to start from
    pub seed_value: fn() -> Vec<u8>,
}

// --- the abi codec ---

fn verb_decode(b: &[u8]) -> bool {
    brainos_abi::verb::Verb::from_bytes(b).is_ok()
}

fn verb_round_trip(b: &[u8]) -> Option<bool> {
    let v = brainos_abi::verb::Verb::from_bytes(b).ok()?;
    Some(v.to_bytes().ok()? == b)
}

fn verb_seed() -> Vec<u8> {
    brainos_abi::verb::Verb::literal("legs.walk")
        .to_bytes()
        .expect("a verb encodes")
}

fn namespace_decode(b: &[u8]) -> bool {
    brainos_abi::verb::Namespace::from_bytes(b).is_ok()
}

fn namespace_round_trip(b: &[u8]) -> Option<bool> {
    let n = brainos_abi::verb::Namespace::from_bytes(b).ok()?;
    Some(n.to_bytes().ok()? == b)
}

fn namespace_seed() -> Vec<u8> {
    brainos_abi::verb::Namespace::only("legs")
        .expect("a namespace")
        .to_bytes()
        .expect("it encodes")
}

fn text_decode(b: &[u8]) -> bool {
    brainos_abi::text::Text::<32>::from_bytes(b).is_ok()
}

fn text_round_trip(b: &[u8]) -> Option<bool> {
    let t = brainos_abi::text::Text::<32>::from_bytes(b).ok()?;
    Some(t.to_bytes().ok()? == b)
}

fn text_seed() -> Vec<u8> {
    brainos_abi::text::Text::<32>::from_str_exact("stop-on-lost-link")
        .expect("it fits")
        .to_bytes()
        .expect("it encodes")
}

// --- the transport ---

const WIRE_KEY: [u8; 32] = [0x5A; 32];

fn frame_decode(b: &[u8]) -> bool {
    brainos_transport::frame::Frame::open(b, &WIRE_KEY).is_ok()
}

fn frame_round_trip(b: &[u8]) -> Option<bool> {
    let (f, used) = brainos_transport::frame::Frame::open(b, &WIRE_KEY).ok()?;
    Some(f.seal(&WIRE_KEY) == b[..used])
}

fn frame_seed() -> Vec<u8> {
    brainos_transport::frame::Frame::new(
        brainos_transport::frame::Kind::Command,
        7,
        b"legs.walk 0.08",
    )
    .expect("a frame")
    .seal(&WIRE_KEY)
}

// --- the body protocol ---

fn command_decode(b: &[u8]) -> bool {
    brainos_bodyd::protocol::Wire::decode_command(b).is_ok()
}

fn command_round_trip(b: &[u8]) -> Option<bool> {
    let c = brainos_bodyd::protocol::Wire::decode_command(b).ok()?;
    Some(brainos_bodyd::protocol::Wire::encode_command(&c).ok()? == b)
}

fn command_seed() -> Vec<u8> {
    brainos_bodyd::protocol::Wire::encode_command(
        &brainos_bodyd::protocol::Command::new(brainos_abi::verb::Verb::literal("legs.walk"))
            .arg("speed", 0.08)
            .arg("heading", 0.0),
    )
    .expect("a command encodes")
}

fn report_decode(b: &[u8]) -> bool {
    brainos_bodyd::protocol::Wire::decode_report(b).is_ok()
}

fn report_round_trip(b: &[u8]) -> Option<bool> {
    let r = brainos_bodyd::protocol::Wire::decode_report(b).ok()?;
    Some(brainos_bodyd::protocol::Wire::encode_report(&r).ok()? == b)
}

fn report_seed() -> Vec<u8> {
    brainos_bodyd::protocol::Wire::encode_report(&brainos_bodyd::protocol::Report::Telemetry {
        rail_v: 7.4,
        current_a: 0.42,
        tilt_deg: 1.2,
        hottest_c: 38.0,
        margin_mm: 9.5,
        stops: 0,
        overruns: 0,
    })
    .expect("a report encodes")
}

// --- the journal ---

fn journal_decode(b: &[u8]) -> bool {
    brainos_memory::journal::Record::from_slot_bytes(b).is_ok()
}

fn journal_round_trip(b: &[u8]) -> Option<bool> {
    let r = brainos_memory::journal::Record::from_slot_bytes(b).ok()?;
    Some(r.to_slot_bytes().ok()? == b)
}

fn journal_seed() -> Vec<u8> {
    let key = brainos_crypto::key::BrainKey::from_seed(&[3; 32]);
    let mut j = brainos_memory::journal::Journal::new(key.entity_id());
    let (_, record) = j.commit(
        &key,
        brainos_memory::journal::Segment::graph(),
        b"a memory".to_vec(),
        brainos_memory::journal::SlotState::default(),
    );
    record.to_slot_bytes().expect("a record encodes")
}

// --- the adapters ---

fn json_decode(b: &[u8]) -> bool {
    match core::str::from_utf8(b) {
        Ok(s) => brainos_adapters::json::Json::parse(s).is_ok(),
        Err(_) => false,
    }
}

fn json_round_trip(_: &[u8]) -> Option<bool> {
    // JSON is not a canonical format and this crate does not claim it
    // is: whitespace, key order and number spelling all survive a parse
    // in a way the value does not remember. The canonical thing derived
    // *from* a parsed description is the CapabilitySchema digest, and
    // that is checked in the adapters' own suite.
    None
}

fn json_seed() -> Vec<u8> {
    br#"{"name":"arm","device_type":"arm","tools":[{"name":"move","input_schema":{"properties":{"angle":{"type":"number","minimum":-10,"maximum":10}}}}]}"#.to_vec()
}

fn mhs_decode(b: &[u8]) -> bool {
    match core::str::from_utf8(b) {
        Ok(s) => brainos_adapters::mhs::from_mhs(s).is_ok(),
        Err(_) => false,
    }
}

fn mhs_round_trip(b: &[u8]) -> Option<bool> {
    // the schema it produces must itself round-trip through the codec,
    // because that is what the body-identity digest is taken over
    let s = core::str::from_utf8(b).ok()?;
    let schema = brainos_adapters::mhs::from_mhs(s).ok()?;
    let bytes = schema.to_bytes().ok()?;
    let back = brainos_body::schema::CapabilitySchema::from_bytes(&bytes).ok()?;
    Some(back.digest() == schema.digest() && back.to_bytes().ok()? == bytes)
}

/// Every decoder under test.
pub fn targets() -> Vec<Target> {
    alloc::vec![
        Target {
            name: "abi::Verb",
            integrity_protected: false,
            reads: "a verb off the wire or out of a schema",
            decode: verb_decode,
            round_trip: verb_round_trip,
            seed_value: verb_seed,
        },
        Target {
            name: "abi::Namespace",
            integrity_protected: false,
            reads: "the scope of a delegated capability",
            decode: namespace_decode,
            round_trip: namespace_round_trip,
            seed_value: namespace_seed,
        },
        Target {
            name: "abi::Text",
            integrity_protected: false,
            reads: "a fixed-capacity string from anywhere",
            decode: text_decode,
            round_trip: text_round_trip,
            seed_value: text_seed,
        },
        Target {
            name: "transport::Frame",
            integrity_protected: true,
            reads: "bytes off a serial line next to brushed motors",
            decode: frame_decode,
            round_trip: frame_round_trip,
            seed_value: frame_seed,
        },
        Target {
            name: "bodyd::Command",
            integrity_protected: false,
            reads: "what the seat told the body to do",
            decode: command_decode,
            round_trip: command_round_trip,
            seed_value: command_seed,
        },
        Target {
            name: "bodyd::Report",
            integrity_protected: false,
            reads: "what the body told the seat",
            decode: report_decode,
            round_trip: report_round_trip,
            seed_value: report_seed,
        },
        Target {
            name: "memory::journal::Record",
            integrity_protected: true,
            reads: "a slot on storage the robot's owner can physically reach",
            decode: journal_decode,
            round_trip: journal_round_trip,
            seed_value: journal_seed,
        },
        Target {
            name: "adapters::Json",
            integrity_protected: false,
            reads: "a device description from a manufacturer",
            decode: json_decode,
            round_trip: json_round_trip,
            seed_value: json_seed,
        },
        Target {
            name: "adapters::mhs",
            integrity_protected: false,
            reads: "an MHS device description, all the way to a body identity",
            decode: mhs_decode,
            round_trip: mhs_round_trip,
            seed_value: json_seed,
        },
    ]
}

/// What one target's fuzzing found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetReport {
    pub name: &'static str,
    pub integrity_protected: bool,
    /// how many inputs were tried
    pub tried: usize,
    /// how many produced a value rather than an error
    pub accepted: usize,
    /// how many accepted values failed to re-encode to their own bytes
    pub not_canonical: usize,
    /// how many accepted values came from mutated-valid input rather
    /// than from random bytes — the number that says whether the
    /// mutation generator is reaching past the magic and the length
    pub accepted_from_mutation: usize,
}

impl TargetReport {
    pub fn is_canonical(&self) -> bool {
        self.not_canonical == 0
    }

    /// Did the fuzzing get anywhere?
    ///
    /// For a plain format, reaching the logic means mutated-valid input
    /// was accepted; a target where nothing ever was has had its length
    /// check tested and nothing else.
    ///
    /// For an integrity-protected one the answer is inverted and the
    /// test is elsewhere: mutation getting through would mean the MAC or
    /// the CRC is not doing its job, so "nothing accepted" is the
    /// result, not a gap.
    pub fn reached_the_logic(&self) -> bool {
        if self.integrity_protected {
            self.tried > 0
        } else {
            self.accepted_from_mutation > 0
        }
    }

    /// For an integrity-protected format: did the check catch every
    /// single mutation?
    pub fn integrity_held(&self) -> bool {
        !self.integrity_protected || self.accepted_from_mutation == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostileReport {
    pub targets: Vec<TargetReport>,
    pub total_inputs: usize,
}

impl HostileReport {
    pub fn holds(&self) -> bool {
        self.targets
            .iter()
            .all(|t| t.is_canonical() && t.integrity_held())
    }

    pub fn shallow(&self) -> Vec<&'static str> {
        self.targets
            .iter()
            .filter(|t| !t.reached_the_logic())
            .map(|t| t.name)
            .collect()
    }
}

/// Feed every decoder random and mutated bytes.
///
/// `rounds` inputs per target from each generator. Deterministic: the
/// same count gives the same inputs, so a failure can be reproduced
/// rather than re-rolled.
pub fn verify_hostile(rounds: usize) -> HostileReport {
    let mut reports = Vec::new();
    let mut total = 0usize;

    for target in targets() {
        let mut rng = Drbg::deterministic(target.name.as_bytes());
        let mut r = TargetReport {
            name: target.name,
            integrity_protected: target.integrity_protected,
            tried: 0,
            accepted: 0,
            not_canonical: 0,
            accepted_from_mutation: 0,
        };

        // --- random bytes ---
        for _ in 0..rounds {
            let len = (rng.below(300) + 1) as usize;
            let mut buf = alloc::vec![0u8; len];
            rng.fill(&mut buf);
            r.tried += 1;
            total += 1;
            if (target.decode)(&buf) {
                r.accepted += 1;
                if let Some(false) = (target.round_trip)(&buf) {
                    r.not_canonical += 1;
                }
            }
        }

        // --- mutated valid input ---
        //
        // Random bytes almost never get past a magic number and a length
        // check. A valid encoding with one bit flipped gets all the way
        // to the logic, which is where the interesting failures are.
        let seed = (target.seed_value)();
        for _ in 0..rounds {
            let mut buf = seed.clone();
            // Between one and four bit flips, at *distinct* positions.
            //
            // The distinctness matters and the harness found out why:
            // picking positions independently lets two flips land on the
            // same bit and cancel, which produces an unmutated input that
            // then decodes perfectly — and looks, in the report, exactly
            // like an integrity check having failed. One frame in six
            // thousand, which is about what the arithmetic predicts.
            // A generator that can emit its own seed is a generator that
            // reports false positives.
            let flips = rng.below(4) + 1;
            let mut flipped: Vec<usize> = Vec::new();
            for _ in 0..flips {
                if buf.is_empty() {
                    break;
                }
                let mut position;
                let mut attempts = 0;
                loop {
                    let i = rng.below(buf.len() as u64) as usize;
                    let bit = rng.below(8) as u64;
                    position = i * 8 + bit as usize;
                    attempts += 1;
                    if !flipped.contains(&position) || attempts > 16 {
                        break;
                    }
                }
                if flipped.contains(&position) {
                    continue;
                }
                flipped.push(position);
                buf[position / 8] ^= 1 << (position % 8);
            }
            debug_assert!(
                buf != seed || seed.is_empty(),
                "a mutation that changed nothing"
            );
            // sometimes truncate or extend, because length handling is
            // where the index bugs are
            match rng.below(4) {
                0 if buf.len() > 1 => {
                    let keep = rng.below(buf.len() as u64) as usize;
                    buf.truncate(keep);
                }
                1 => {
                    let extra = (rng.below(32) + 1) as usize;
                    let mut tail = alloc::vec![0u8; extra];
                    rng.fill(&mut tail);
                    buf.extend_from_slice(&tail);
                }
                _ => {}
            }

            r.tried += 1;
            total += 1;
            if (target.decode)(&buf) {
                r.accepted += 1;
                r.accepted_from_mutation += 1;
                if let Some(false) = (target.round_trip)(&buf) {
                    r.not_canonical += 1;
                }
            }
        }

        reports.push(r);
    }

    HostileReport {
        targets: reports,
        total_inputs: total,
    }
}
