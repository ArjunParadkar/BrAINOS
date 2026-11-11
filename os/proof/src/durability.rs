//! BFS  LEVEL    Stage 6 - property domains (bounded checking, not a proof)  [crate brainos-proof]
//! BFS  PURPOSE  Cuts power at every byte of a journal write on three storage models and checks recovery returns a complete, committed, non-older state
//! BFS  DEFINES  Medium, Violation, DurabilityReport, verify_durability, verify_bit_rot
//! BFS  USES     brainos: abi, crypto, memory
//! BFS  USED BY  host/main, proof/report, proof/tests/durability_suite
//!
//! The fifth domain: losing power at every possible instant.
//!
//! Robots lose power. Not as an edge case — as a normal Tuesday. A
//! battery is pulled, a charger is kicked out, a motor stall browns out
//! the rail, and whatever the entity was in the middle of writing stops
//! being written, at whatever byte it happened to reach.
//!
//! §3.7 says memory lives in the entity's state graph and nowhere else,
//! which makes the journal the single point at which an entity's
//! identity can be destroyed. The design answer is two slots, a CRC, a
//! signature and a generation chain. This domain checks that the answer
//! works *at every instant*, by enumerating them: for a write of `n`
//! bytes there are `n + 1` moments power can be lost, and every one of
//! them is tried.
//!
//! Three media models, because a torn write does not look the same on
//! every device:
//!
//!   zeros   the tail is unwritten and reads as zero — a fresh flash
//!           page, or a file extended and not filled
//!   stale   the tail still holds whatever the slot held before — the
//!           usual case on a block device rewritten in place, and the
//!           nastiest, because the tail is *valid-looking* data
//!   noise   the tail is garbage, which is what a browning-out
//!           controller writes on its way down
//!
//! The property is not "the newest write survives". It is weaker and
//! more useful: **whatever comes back is something that was actually
//! committed, in full, and it is never older than the last write that
//! completed.** An entity may lose the last thing it was doing. It may
//! not come back as a mixture of two entities, and it may not come back
//! as an earlier version of itself.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::ids::EntityId;
use brainos_crypto::key::BrainKey;
use brainos_memory::journal::{plan_write, Journal, OpenFailure, Record, Segment, Slot, SlotState};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Medium {
    /// the unwritten tail reads as zero
    Zeros,
    /// the unwritten tail still holds the slot's previous contents
    Stale,
    /// the unwritten tail is garbage
    Noise,
}

impl Medium {
    pub const ALL: [Medium; 3] = [Medium::Zeros, Medium::Stale, Medium::Noise];
    pub const fn name(self) -> &'static str {
        match self {
            Medium::Zeros => "unwritten tail reads zero",
            Medium::Stale => "tail holds what was there before",
            Medium::Noise => "tail is garbage",
        }
    }
}

/// Apply a write that stopped after `written` bytes.
fn tear(medium: Medium, previous: &[u8], fresh: &[u8], written: usize) -> Vec<u8> {
    let n = written.min(fresh.len());
    // the slot is as long as whichever record is longer: a write in
    // place does not shorten the medium
    let extent = fresh.len().max(previous.len());
    let mut out = Vec::with_capacity(extent);
    out.extend_from_slice(&fresh[..n]);
    match medium {
        Medium::Zeros => out.resize(extent, 0),
        Medium::Noise => {
            // deterministic garbage, so a failure is reproducible
            let mut x: u8 = 0x5A;
            while out.len() < extent {
                x = x.wrapping_mul(31).wrapping_add(17);
                out.push(x);
            }
        }
        Medium::Stale => {
            for i in n..extent {
                out.push(previous.get(i).copied().unwrap_or(0));
            }
        }
    }
    out
}

#[derive(Clone, Debug)]
pub struct Violation {
    pub medium: Medium,
    pub generation: u64,
    pub torn_at: usize,
    pub why: String,
}

#[derive(Clone, Debug)]
pub struct DurabilityReport {
    /// how many distinct power-loss instants were enumerated
    pub instants: usize,
    /// of those, how many came back with the newest write intact
    pub kept_newest: usize,
    /// how many fell back to the previous good state, which is a
    /// success, not a failure — it is what two slots are for
    pub fell_back: usize,
    /// how many came back with nothing although a complete earlier
    /// record existed — the only outcome here that is a failure
    pub lost_everything: usize,
    /// how many came back with nothing because there was nothing: an
    /// entity whose very first journal write tore has no memory yet,
    /// which is correct and is not corruption
    pub started_fresh: usize,
    pub violations: Vec<Violation>,
}

impl DurabilityReport {
    pub fn holds(&self) -> bool {
        self.violations.is_empty() && self.lost_everything == 0 && self.instants > 0
    }

    /// Did the domain actually exercise both outcomes? If every instant
    /// kept the newest write, the tears were not where the data is.
    pub fn is_meaningful(&self) -> bool {
        self.kept_newest > 0 && (self.fell_back > 0 || self.started_fresh > 0)
    }
}

fn payload_for(generation: u64) -> Vec<u8> {
    // Distinguishable per generation, so a mixture is detectable rather
    // than merely suspected — and of *varying length*, which matters:
    // a short record written over a longer one leaves the old tail
    // behind, and that tail is valid-looking data sitting immediately
    // after a valid header. Fixed-size payloads never produce that case
    // and it is the one worth producing.
    let len = 24 + (generation as usize % 5) * 40;
    let mut p = Vec::with_capacity(len);
    for i in 0..len as u64 {
        p.push((generation.wrapping_mul(37).wrapping_add(i)) as u8);
    }
    p
}

/// Write `generations` records, tearing the last one at every possible
/// byte, on every medium.
pub fn verify_durability(generations: u64) -> DurabilityReport {
    let key = BrainKey::from_seed(&[0xD0; 32]);
    let entity: EntityId = key.entity_id();
    let mut report = DurabilityReport {
        instants: 0,
        kept_newest: 0,
        fell_back: 0,
        lost_everything: 0,
        started_fresh: 0,
        violations: Vec::new(),
    };

    for medium in Medium::ALL {
        // --- build a healthy history first ---
        let mut j = Journal::new(entity);
        let mut slot_a: Vec<u8> = Vec::new();
        let mut slot_b: Vec<u8> = Vec::new();
        let mut committed: Vec<(u64, Vec<u8>)> = Vec::new();

        for g in 1..=generations {
            let state = slot_state(&slot_a, &slot_b);
            let (slot, record) = j.commit(&key, Segment::Episodes, payload_for(g), state);
            let bytes = record.to_slot_bytes().expect("a record encodes");
            match slot {
                Slot::A => slot_a = bytes,
                Slot::B => slot_b = bytes,
            }
            committed.push((record.generation, payload_for(g)));
        }

        // --- now tear the next write at every instant ---
        let next_gen = generations + 1;
        let state = slot_state(&slot_a, &slot_b);
        let (target, planned_gen) = plan_write(state);
        let mut writer = clone_journal(&j);
        let (_, record) = writer.commit(&key, Segment::Episodes, payload_for(next_gen), state);
        let fresh = record.to_slot_bytes().expect("a record encodes");
        let previous = match target {
            Slot::A => slot_a.clone(),
            Slot::B => slot_b.clone(),
        };

        for torn_at in 0..=fresh.len() {
            report.instants += 1;
            let damaged = tear(medium, &previous, &fresh, torn_at);
            let (a, b) = match target {
                Slot::A => (damaged, slot_b.clone()),
                Slot::B => (slot_a.clone(), damaged),
            };

            // the entity comes back cold: it has its Key and the medium,
            // and no memory of what it last wrote
            let mut recovering = Journal::new(entity);
            match recovering.open_cold(&key.public(), Segment::Episodes, &a, &b) {
                Ok(r) => {
                    let expected_new = payload_for(next_gen);
                    if r.generation == planned_gen && r.payload == expected_new {
                        report.kept_newest += 1;
                    } else if committed
                        .iter()
                        .any(|(g, p)| *g == r.generation && p == &r.payload)
                    {
                        report.fell_back += 1;
                    } else {
                        report.violations.push(Violation {
                            medium,
                            generation: r.generation,
                            torn_at,
                            why: alloc::format!(
                                "recovered generation {} with a payload that was never committed \
                                 — the entity came back as something it never was",
                                r.generation
                            ),
                        });
                        continue;
                    }
                    if !r.verify(&key.public()) {
                        report.violations.push(Violation {
                            medium,
                            generation: r.generation,
                            torn_at,
                            why: String::from("the recovered record does not verify"),
                        });
                    }
                    if r.segment != Segment::Episodes {
                        report.violations.push(Violation {
                            medium,
                            generation: r.generation,
                            torn_at,
                            why: String::from("recovered a record from the wrong segment"),
                        });
                    }
                }
                Err(OpenFailure::Empty) => {
                    if committed.is_empty() {
                        // the entity's very first write tore. There is
                        // nothing to come back to, and coming back with
                        // nothing is the right answer rather than a
                        // failure — it is a newborn, not a casualty.
                        report.started_fresh += 1;
                    } else {
                        report.lost_everything += 1;
                        report.violations.push(Violation {
                            medium,
                            generation: 0,
                            torn_at,
                            why: String::from(
                                "nothing came back, although a complete earlier record was \
                                 sitting in the other slot",
                            ),
                        });
                    }
                }
                Err(OpenFailure::NotOurs) => {
                    report.violations.push(Violation {
                        medium,
                        generation: 0,
                        torn_at,
                        why: String::from(
                            "a torn write was reported as another entity's data, which \
                             calls for a completely different reaction",
                        ),
                    });
                }
                Err(OpenFailure::RolledBack { .. }) => {
                    report.violations.push(Violation {
                        medium,
                        generation: 0,
                        torn_at,
                        why: String::from(
                            "a cold open reported a rollback, although a cold open has \
                             nothing to chain to",
                        ),
                    });
                }
            }
        }
    }

    report
}

fn slot_state(a: &[u8], b: &[u8]) -> SlotState {
    SlotState {
        generation_a: Record::from_slot_bytes(a).ok().map(|r| r.generation),
        generation_b: Record::from_slot_bytes(b).ok().map(|r| r.generation),
    }
}

/// `Journal` holds only counters and digests; this is enough of a copy
/// for the harness to branch a history without the type needing to be
/// `Clone` in shipping code.
fn clone_journal(j: &Journal) -> Journal {
    let mut c = Journal::new(j.entity());
    c.adopt_from(j);
    c
}

/// A separate, smaller question: does a single flipped bit anywhere in a
/// *complete* record get caught?
///
/// The CRC is there for the medium's own errors, and the signature for
/// everything else. Between them nothing should get through, and a
/// record that does get through must be byte-identical to what was
/// written.
pub fn verify_bit_rot(samples: usize) -> (usize, usize) {
    let key = BrainKey::from_seed(&[0xD1; 32]);
    let mut j = Journal::new(key.entity_id());
    let (_, record) = j.commit(
        &key,
        Segment::Knowledge,
        payload_for(7),
        SlotState::default(),
    );
    let good = record.to_slot_bytes().expect("a record encodes");

    let mut caught = 0;
    let mut slipped = 0;
    let total_bits = good.len() * 8;
    let step = (total_bits / samples.max(1)).max(1);

    let mut bit = 0;
    while bit < total_bits {
        let mut bad = good.clone();
        bad[bit / 8] ^= 1 << (bit % 8);
        match Record::from_slot_bytes(&bad) {
            Err(_) => caught += 1,
            Ok(r) => {
                if r.verify(&key.public()) && r.to_slot_bytes().as_deref() == Ok(&bad[..]) {
                    // decoded, verified, and re-encodes to exactly these
                    // bytes: it is the same record, so the flip was in a
                    // byte that is not part of the record
                    slipped += 1;
                } else {
                    caught += 1;
                }
            }
        }
        bit += step;
    }
    (caught, slipped)
}
