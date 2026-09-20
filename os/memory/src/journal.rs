//! The journal — memory that survives a power cut.
//!
//! Stage 1 shipped a two-slot CRC-sealed journal and it is the right
//! design: a write always targets the slot *not* holding the newest valid
//! record, so a power loss mid-write can only tear the copy that was
//! already redundant. Robots lose power constantly; laptops do not; this
//! is one of the primitives that turned out to be a robotics primitive
//! tested on a laptop first.
//!
//! This is that design with three things added, each for a reason the
//! two-slot version ran into:
//!
//! * **Segments.** A state graph is larger than a servo's worth of
//!   bytes, and rewriting all of it on every commit is both slow and a
//!   larger window to be interrupted in. Memory is split into segments —
//!   episodes, semantic, skills, bodies — each with its own two slots, so
//!   a gait's worth of new episodes does not rewrite the skill graph.
//!
//! * **A signature, not only a CRC.** A CRC proves the bytes were not
//!   *corrupted*. It proves nothing about whether they were *replaced*.
//!   A body's storage is physically accessible to whoever holds the
//!   robot, so each sealed record is also signed by the Key, and a record
//!   that decodes cleanly but was not signed by this entity is refused
//!   exactly as loudly as a torn one.
//!
//! * **A generation chain.** Each record names the digest of the record
//!   it replaced. Rolling a segment back to an older valid state — which
//!   a CRC and a signature both permit, since the old record was
//!   genuinely written by this entity — breaks the chain, and the entity
//!   notices it has been rolled back rather than silently resuming an
//!   earlier life.

use alloc::vec::Vec;
use brainos_abi::codec::{Decode, Decoder, Encode, Encoder};
use brainos_abi::error::{Error, Result};
use brainos_abi::ids::EntityId;
use brainos_crypto::domain::Tag;
use brainos_crypto::key::{BrainKey, PublicKey, Signature};
use brainos_crypto::sha256::{Hash, Sha256};

pub const MAGIC: [u8; 8] = *b"BRNJRNL2";

/// Which part of memory a record holds. Each has its own slot pair.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
#[repr(u8)]
pub enum Segment {
    /// episodic memory
    Episodes = 0,
    /// semantic memory and beliefs
    Knowledge = 1,
    /// the skill graph
    Skills = 2,
    /// bodies and their calibration
    Bodies = 3,
    /// goals and the entity's own configuration
    Self_ = 4,
}

impl Segment {
    pub const ALL: [Segment; 5] = [
        Segment::Episodes,
        Segment::Knowledge,
        Segment::Skills,
        Segment::Bodies,
        Segment::Self_,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Segment::Episodes => "episodes",
            Segment::Knowledge => "knowledge",
            Segment::Skills => "skills",
            Segment::Bodies => "bodies",
            Segment::Self_ => "self",
        }
    }

    pub fn from_u8(v: u8) -> Option<Segment> {
        Segment::ALL.get(v as usize).copied()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    pub fn other(self) -> Slot {
        match self {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }
}

/// One sealed record.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub entity: EntityId,
    pub segment: Segment,
    pub generation: u64,
    /// the digest of the record this one replaced; zero for the first
    pub previous: Hash,
    pub payload: Vec<u8>,
    pub signature: Signature,
}

impl Record {
    /// What gets signed and what the digest is taken over. Everything
    /// that makes this record *this* record, and nothing else.
    fn transcript(
        entity: &EntityId,
        segment: Segment,
        generation: u64,
        previous: &Hash,
        payload: &[u8],
    ) -> Hash {
        let mut h = Sha256::tagged(Tag::Journal);
        h.field(entity.as_bytes());
        h.field(&[segment as u8]);
        h.field(&generation.to_le_bytes());
        h.field(previous.as_bytes());
        h.field(payload);
        h.finish()
    }

    pub fn digest(&self) -> Hash {
        Record::transcript(
            &self.entity,
            self.segment,
            self.generation,
            &self.previous,
            &self.payload,
        )
    }

    /// Seal a payload into a record.
    pub fn seal(
        key: &BrainKey,
        segment: Segment,
        generation: u64,
        previous: Hash,
        payload: Vec<u8>,
    ) -> Record {
        let entity = key.entity_id();
        let t = Record::transcript(&entity, segment, generation, &previous, &payload);
        let signature = key.sign(Tag::Journal, t.as_bytes());
        Record {
            entity,
            segment,
            generation,
            previous,
            payload,
            signature,
        }
    }

    /// Does this record genuinely belong to this entity?
    pub fn verify(&self, public: &PublicKey) -> bool {
        if !self.entity.ct_eq(&public.entity_id()) {
            return false;
        }
        public.verify(Tag::Journal, self.digest().as_bytes(), &self.signature)
    }

    /// Serialize with a magic header and a CRC, so a torn write is
    /// rejected before any signature arithmetic is attempted. The CRC is
    /// the cheap check; the signature is the real one.
    pub fn to_slot_bytes(&self) -> Result<Vec<u8>> {
        let mut body = Encoder::new();
        self.entity.encode(&mut body);
        body.u8(self.segment as u8);
        body.u64(self.generation);
        body.raw(self.previous.as_bytes());
        body.bytes(&self.payload);
        body.raw(self.signature.as_bytes());
        let body = body.finish()?;

        let mut out = Encoder::new();
        out.raw(&MAGIC);
        out.u32(body.len() as u32);
        out.u32(crc32(&body));
        out.raw(&body);
        out.finish()
    }

    pub fn from_slot_bytes(raw: &[u8]) -> Result<Record> {
        if raw.len() < 16 || raw[..8] != MAGIC {
            return Err(Error::Storage("slot has no journal magic"));
        }
        let len = u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]) as usize;
        let crc = u32::from_le_bytes([raw[12], raw[13], raw[14], raw[15]]);
        if raw.len() < 16 + len {
            return Err(Error::Storage("slot is short of its declared length"));
        }
        let body = &raw[16..16 + len];
        if crc32(body) != crc {
            return Err(Error::Storage("slot failed its crc"));
        }

        let mut d = Decoder::new(body);
        let entity = EntityId::decode(&mut d)?;
        let segment =
            Segment::from_u8(d.u8()?).ok_or(Error::Malformed("unknown journal segment"))?;
        let generation = d.u64()?;
        let mut prev = [0u8; 32];
        d.read_exact(&mut prev)?;
        let payload = d.bytes()?.to_vec();
        let mut sig = [0u8; 64];
        d.read_exact(&mut sig)?;
        d.finish()?;

        Ok(Record {
            entity,
            segment,
            generation,
            previous: Hash(prev),
            payload,
            signature: Signature(sig),
        })
    }
}

/// CRC-32 (IEEE 802.3, reflected), bitwise.
///
/// Table-free on purpose: journal writes are rare and bounded, a 1 KiB
/// table on a microcontroller is a real cost, and the bitwise loop is
/// obviously the algorithm it claims to be.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// What a segment's two slots currently hold.
#[derive(Clone, Copy, Debug, Default)]
pub struct SlotState {
    pub generation_a: Option<u64>,
    pub generation_b: Option<u64>,
}

/// Where the next write goes, and with what generation.
///
/// Always the slot not holding the newest valid record. When both slots
/// are valid the older one is overwritten; when one is torn it is the
/// obvious target; when neither decodes the segment starts fresh at
/// generation 1.
pub fn plan_write(state: SlotState) -> (Slot, u64) {
    match (state.generation_a, state.generation_b) {
        (None, None) => (Slot::A, 1),
        (Some(a), None) => (Slot::B, a + 1),
        (None, Some(b)) => (Slot::A, b + 1),
        (Some(a), Some(b)) => {
            if a >= b {
                (Slot::B, a + 1)
            } else {
                (Slot::A, b + 1)
            }
        }
    }
}

/// Why a segment could not be opened.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpenFailure {
    /// neither slot holds anything that decodes
    Empty,
    /// a record decoded but was not signed by this entity
    NotOurs,
    /// the newest record does not chain from the one we last wrote —
    /// the segment was rolled back to an earlier valid state
    RolledBack { expected: Hash, found: Hash },
}

/// The journal: what to write, what to trust when waking.
///
/// This type holds no storage. Reading and writing the eight slot files
/// is firmware work and differs per platform; sealing, validation, slot
/// choice and rollback detection live here, under test, identical
/// everywhere. That split is what made the Stage 1 journal testable and
/// it is kept.
pub struct Journal {
    entity: EntityId,
    /// the digest we last wrote per segment, for the chain check
    last_digest: [Hash; 5],
    generation: [u64; 5],
}

impl Journal {
    pub fn new(entity: EntityId) -> Journal {
        Journal {
            entity,
            last_digest: [Hash::ZERO; 5],
            generation: [0; 5],
        }
    }

    pub fn entity(&self) -> EntityId {
        self.entity
    }

    pub fn generation(&self, seg: Segment) -> u64 {
        self.generation[seg as usize]
    }

    pub fn last_digest(&self, seg: Segment) -> Hash {
        self.last_digest[seg as usize]
    }

    /// Prepare the next record for a segment.
    pub fn commit(
        &mut self,
        key: &BrainKey,
        segment: Segment,
        payload: Vec<u8>,
        state: SlotState,
    ) -> (Slot, Record) {
        let (slot, generation) = plan_write(state);
        let previous = self.last_digest[segment as usize];
        let record = Record::seal(key, segment, generation, previous, payload);
        self.last_digest[segment as usize] = record.digest();
        self.generation[segment as usize] = generation;
        (slot, record)
    }

    /// Choose what to wake up with, given both slots' raw bytes.
    ///
    /// The newest *valid and ours* record wins. A valid old record beats
    /// an invalid new one — that is the whole point of two slots. A
    /// record that decodes but is not ours is a different failure and is
    /// reported as one, because "someone put another entity's memory on
    /// this medium" and "the write tore" call for very different
    /// reactions.
    pub fn open(
        &mut self,
        public: &PublicKey,
        segment: Segment,
        raw_a: &[u8],
        raw_b: &[u8],
    ) -> core::result::Result<Record, OpenFailure> {
        let a = Record::from_slot_bytes(raw_a).ok();
        let b = Record::from_slot_bytes(raw_b).ok();

        let mut candidates: Vec<Record> = Vec::new();
        let mut saw_foreign = false;
        for r in [a, b].into_iter().flatten() {
            if r.segment != segment {
                continue;
            }
            if r.verify(public) {
                candidates.push(r);
            } else {
                saw_foreign = true;
            }
        }

        if candidates.is_empty() {
            return Err(if saw_foreign {
                OpenFailure::NotOurs
            } else {
                OpenFailure::Empty
            });
        }

        candidates.sort_by_key(|r| r.generation);
        let newest = candidates.pop().expect("non-empty");

        // rollback check: if we have written before, the record we find
        // must chain from what we last wrote
        let expected = self.last_digest[segment as usize];
        if !expected.ct_eq(&Hash::ZERO) && !newest.previous.ct_eq(&expected) {
            return Err(OpenFailure::RolledBack {
                expected,
                found: newest.previous,
            });
        }

        self.generation[segment as usize] = newest.generation;
        self.last_digest[segment as usize] = newest.digest();
        Ok(newest)
    }

    /// Wake with no prior state: accept the newest valid record without a
    /// chain check, because there is nothing to chain to yet. Used on a
    /// cold boot, where the entity has just loaded its Key and has no
    /// memory of what it last wrote.
    pub fn open_cold(
        &mut self,
        public: &PublicKey,
        segment: Segment,
        raw_a: &[u8],
        raw_b: &[u8],
    ) -> core::result::Result<Record, OpenFailure> {
        self.last_digest[segment as usize] = Hash::ZERO;
        self.open(public, segment, raw_a, raw_b)
    }
}
