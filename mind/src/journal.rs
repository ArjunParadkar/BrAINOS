//! Crash-consistent persistence for the entity's memory (Stage 1.4).
//!
//! The key medium is FAT under a firmware driver: there is no rename, no
//! fsync contract, no journal beneath us — a delete-then-rewrite of
//! EPISODES.LOG has a window where power loss destroys the self. The
//! Memory-Integrity Law (§13.2) makes that unacceptable, so memory is
//! written as a two-slot journal instead:
//!
//!   slot A and slot B each hold one sealed record:
//!     magic(8) | generation u64 | payload_len u32 | crc32 u32 | payload
//!
//! A write always targets the slot that does NOT hold the newest valid
//! record, with generation = newest + 1. Power loss mid-write can only
//! tear the slot being written; the other slot still decodes, and the
//! entity wakes with its previous memories instead of none. The CRC is
//! what turns "partially written" into "provably invalid" — a torn or
//! bit-flipped record is rejected, never half-loaded.
//!
//! This module is pure: sealing, validation and slot choice live here
//! (and under test); only reading/writing the two files is firmware work.

use alloc::vec::Vec;

pub const MAGIC: [u8; 8] = *b"BRNJRNL1";
pub const HEADER_LEN: usize = 8 + 8 + 4 + 4;
/// One record never exceeds this — matches the bounded serialize() side.
pub const MAX_PAYLOAD: usize = 64 * 1024;

/// CRC-32 (IEEE 802.3, reflected). Bitwise — memory writes are rare and
/// bounded, so simplicity beats a table here.
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

/// The CRC seals generation and length along with the payload, so a
/// corrupted header can never silently reorder history — any flipped bit
/// anywhere in the record invalidates the whole slot.
fn record_crc(generation: u64, payload: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    let gen = generation.to_le_bytes();
    let len = (payload.len() as u32).to_le_bytes();
    for &b in gen.iter().chain(len.iter()).chain(payload.iter()) {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Seal a payload into one slot record.
pub fn seal(generation: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&generation.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&record_crc(generation, payload).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Validate one slot's raw bytes. Anything malformed — short, wrong
/// magic, length beyond the data, CRC mismatch — is `None`, never a
/// partial result.
pub fn open(raw: &[u8]) -> Option<(u64, &[u8])> {
    if raw.len() < HEADER_LEN || raw[..8] != MAGIC {
        return None;
    }
    let generation = u64::from_le_bytes(raw[8..16].try_into().ok()?);
    let len = u32::from_le_bytes(raw[16..20].try_into().ok()?) as usize;
    if len > MAX_PAYLOAD || raw.len() < HEADER_LEN + len {
        return None;
    }
    let recorded_crc = u32::from_le_bytes(raw[20..24].try_into().ok()?);
    let payload = &raw[HEADER_LEN..HEADER_LEN + len];
    if record_crc(generation, payload) != recorded_crc {
        return None;
    }
    Some((generation, payload))
}

/// Which slot to write next, and with what generation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    A,
    B,
}

/// Given the generations of whatever currently decodes in each slot,
/// choose the write target: always the slot NOT holding the newest valid
/// record, so the newest survivable state is never the one at risk.
pub fn plan_write(gen_a: Option<u64>, gen_b: Option<u64>) -> (Slot, u64) {
    match (gen_a, gen_b) {
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

/// Given both slots' raw bytes, the payload to wake up with: the newest
/// valid record wins; a valid old record beats an invalid new one.
pub fn newest<'a>(raw_a: &'a [u8], raw_b: &'a [u8]) -> Option<(u64, &'a [u8])> {
    match (open(raw_a), open(raw_b)) {
        (Some(a), Some(b)) => Some(if a.0 >= b.0 { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}
