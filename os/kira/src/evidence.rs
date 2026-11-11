//! BFS  LEVEL    Domain 3 KIRA - the eight-stage gate; the only path to action  [crate brainos-kira]
//! BFS  PURPOSE  Packages audit records with inclusion proofs and a signed root so an outsider can verify them with only the entity's public key
//! BFS  DEFINES  EvidenceError, Exhibit, Bundle
//! BFS  USES     this crate: audit | brainos: abi, crypto
//! BFS  USED BY  host/incident, kira/tests/evidence_suite
//!
//! An evidence bundle: what happened, provable to someone who does not
//! trust you.
//!
//! Stage 8 of the gate is audit, and it always runs — a refusal is
//! recorded as carefully as a grant. That is worth very little on its
//! own. An audit log held by the party it might incriminate is a diary,
//! not evidence, and "our logs show" is what every company says after an
//! incident.
//!
//! What makes it evidence is that it is checkable by someone who
//! assumes we are lying. A bundle carries, for each decision it covers:
//! the record's canonical bytes, a Merkle inclusion proof, and the root
//! those proofs recompute to, signed by the entity's own Key. A verifier
//! needs the bundle and thirty-two bytes of public key. Not the log. Not
//! the entity. Not us.
//!
//! Three things follow, and the third is the one people do not expect.
//!
//! 1. A record cannot be *altered*: the proof is over its exact bytes.
//! 2. A record cannot be *removed*: taking a leaf out changes the root
//!    and every root after it, so the signed checkpoint stops matching.
//! 3. A record cannot be *withheld selectively without that being
//!    visible*, because the bundle states how many decisions fall in its
//!    window and how many it contains. A bundle that covers six
//!    decisions out of nine says so. It does not prove what the missing
//!    three were; it removes the option of pretending there were six.
//!
//! **What an audit log is not.** It is a record of *decisions*, not of
//! physics. It can say the gate granted a walk at 0.08 m/s with these
//! arguments, at this tick, having checked these eight things. It cannot
//! say the floor was wet, or where the equipment was standing, or what
//! the robot actually hit. Anyone reading it as a record of events
//! rather than of decisions will overread it, and the bundle says so in
//! its own text rather than leaving it to be assumed.

use crate::audit::{AuditRecord, AuditTrail};
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::ids::EntityId;
use brainos_abi::time::Tick;
use brainos_crypto::domain::Tag;
use brainos_crypto::key::{BrainKey, PublicKey, Signature};
use brainos_crypto::merkle::{Proof, Root};

/// Why a bundle does not stand up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EvidenceError {
    /// the checkpoint is not signed by the entity it names
    UnsignedCheckpoint,
    /// an exhibit's bytes are not what its record encodes to
    RecordDoesNotMatchItsBytes(usize),
    /// an exhibit's inclusion proof does not recompute to the root
    NotInTheLog(usize),
    /// an exhibit is about a different entity
    WrongEntity(usize),
    /// an exhibit falls outside the window the bundle claims to cover
    OutsideTheWindow(usize),
    /// the bundle covers a window but contains nothing
    Empty,
}

/// One decision, with its proof.
#[derive(Clone, Debug)]
pub struct Exhibit {
    /// where in the log
    pub index: usize,
    pub record: AuditRecord,
    /// the exact bytes the leaf was taken over. Carried rather than
    /// recomputed, so that a verifier can check the record *and* the
    /// encoding agree — a bundle whose record re-encodes to something
    /// else is a bundle whose record has been edited.
    pub bytes: Vec<u8>,
    pub proof: Proof,
}

/// What happened between two ticks, and the proof of it.
#[derive(Clone, Debug)]
pub struct Bundle {
    pub entity: EntityId,
    pub from_tick: Tick,
    pub to_tick: Tick,
    /// the log's length when this bundle was sealed
    pub log_len: usize,
    pub root: Root,
    pub signature: Signature,
    pub exhibits: Vec<Exhibit>,
    /// decisions in the window that are not in this bundle because their
    /// records had already been pruned from the working set. Stated, so
    /// that a partial bundle is visibly partial.
    pub beyond_the_window: usize,
}

impl Bundle {
    /// Check it, assuming the producer is lying about everything except
    /// the public key.
    pub fn verify(&self, key: &PublicKey) -> Result<(), EvidenceError> {
        if !key.entity_id().ct_eq(&self.entity) {
            return Err(EvidenceError::WrongEntity(usize::MAX));
        }
        if !key.verify(Tag::AuditNode, self.root.as_bytes(), &self.signature) {
            return Err(EvidenceError::UnsignedCheckpoint);
        }
        if self.exhibits.is_empty() {
            return Err(EvidenceError::Empty);
        }
        for (i, e) in self.exhibits.iter().enumerate() {
            if e.record.canonical() != e.bytes {
                return Err(EvidenceError::RecordDoesNotMatchItsBytes(i));
            }
            if !e.record.entity.ct_eq(&self.entity) {
                return Err(EvidenceError::WrongEntity(i));
            }
            if e.record.tick.0 < self.from_tick.0 || e.record.tick.0 > self.to_tick.0 {
                return Err(EvidenceError::OutsideTheWindow(i));
            }
            if !e.proof.verify_record(&e.bytes, &self.root) {
                return Err(EvidenceError::NotInTheLog(i));
            }
        }
        Ok(())
    }

    pub fn granted(&self) -> usize {
        self.exhibits
            .iter()
            .filter(|e| e.record.decision == crate::audit::Decision::Granted)
            .count()
    }

    pub fn denied(&self) -> usize {
        self.exhibits.len() - self.granted()
    }

    /// Decisions the bundle skips over: indices between its lowest and
    /// highest exhibit that it does not contain.
    ///
    /// This is what makes selective omission *visible* rather than
    /// merely wrong. A bundle cannot be forged — the proofs are over
    /// exact bytes against a signed root — but it can be curated, and
    /// curation is the obvious move for a party choosing which of its
    /// own decisions to show. It cannot be prevented by the bundle
    /// alone; the log's owner decides what to put in it. What *can* be
    /// done is to make the holes obvious: the log is append-only and its
    /// indices are consecutive, so a bundle running from #70 to #110
    /// with #75 absent is a bundle with a hole in it, and a verifier can
    /// say "produce #75" without knowing anything about what it says.
    ///
    /// An honest bundle over a contiguous window has none.
    pub fn gaps(&self) -> Vec<usize> {
        let mut present: Vec<usize> = self.exhibits.iter().map(|e| e.index).collect();
        present.sort_unstable();
        let (Some(&lo), Some(&hi)) = (present.first(), present.last()) else {
            return Vec::new();
        };
        (lo..=hi).filter(|i| !present.contains(i)).collect()
    }

    /// Is this the whole story for the window it covers?
    pub fn is_complete(&self) -> bool {
        self.beyond_the_window == 0 && self.gaps().is_empty()
    }

    /// The bundle, written out for a human — and for the reader who will
    /// otherwise treat it as a record of events.
    pub fn render(&self) -> String {
        let mut s = String::new();
        s.push_str("BrAInOS — evidence bundle\n");
        s.push_str("entity ");
        s.push_str(self.entity.fingerprint().as_str());
        s.push_str("\nticks ");
        push_u64(&mut s, self.from_tick.0);
        s.push_str(" to ");
        push_u64(&mut s, self.to_tick.0);
        s.push_str("\ndecisions in this bundle: ");
        push_u64(&mut s, self.exhibits.len() as u64);
        s.push_str(" (");
        push_u64(&mut s, self.granted() as u64);
        s.push_str(" granted, ");
        push_u64(&mut s, self.denied() as u64);
        s.push_str(" refused)\n");
        s.push_str("log length when sealed: ");
        push_u64(&mut s, self.log_len as u64);
        s.push('\n');
        if self.beyond_the_window > 0 {
            s.push_str("INCOMPLETE: ");
            push_u64(&mut s, self.beyond_the_window as u64);
            s.push_str(" decision(s) in this window are no longer in the working set\n");
        }
        let gaps = self.gaps();
        if !gaps.is_empty() {
            s.push_str("CURATED: this bundle skips ");
            push_u64(&mut s, gaps.len() as u64);
            s.push_str(" decision(s) that fall between its own first and last —");
            for g in gaps.iter().take(12) {
                s.push_str(" #");
                push_u64(&mut s, *g as u64);
            }
            if gaps.len() > 12 {
                s.push_str(" …");
            }
            s.push_str("\n          Ask for them. The log is append-only and its indices are\n");
            s.push_str("          consecutive, so a hole is a hole whatever it contains.\n");
        }
        s.push_str("\n");
        for e in &self.exhibits {
            s.push_str("  #");
            push_u64(&mut s, e.index as u64);
            s.push_str("  ");
            s.push_str(&e.record.render());
            s.push_str("\n       proof: ");
            push_u64(&mut s, e.proof.path_len() as u64);
            s.push_str(" hashes to the signed root\n");
        }
        s.push_str(
            "\nWhat this is: a record of decisions. It says what was asked, what the
gate did about it, and at which of the eight stages it stopped. Every
line is provable against the signed root above using nothing but this
file and the entity's 32-byte public key.

What this cannot do by itself: stop its producer from being selective.
A bundle is as complete as whoever sealed it chose to make it. What it
can do is make a gap visible — the indices are consecutive, so a
missing one is a missing one — which turns a request to be trusted into
a request to produce #75.

What this is not: a record of events. It cannot say whether the floor
was wet, where anything was standing, or what the body physically did.
Reading it as an account of what happened rather than of what was
decided will overread it.\n",
        );
        s
    }
}

impl AuditTrail {
    /// Where the working window starts in the log.
    pub fn window_base(&self) -> usize {
        self.len().saturating_sub(self.recent(usize::MAX).len())
    }

    /// Seal a bundle covering every decision between two ticks.
    ///
    /// Sealing takes a fresh checkpoint, because a proof is against the
    /// root of the log as it stands. That is the honest ordering: the
    /// bundle is a statement about the log at the moment it was
    /// produced, and if the log is later edited the bundle is what
    /// disagrees with it.
    pub fn bundle(&mut self, key: &BrainKey, from: Tick, to: Tick) -> Bundle {
        let base = self.window_base();
        let mut exhibits = Vec::new();
        let mut indices = Vec::new();

        for (pos, r) in self.recent(usize::MAX).iter().enumerate() {
            if r.tick.0 >= from.0 && r.tick.0 <= to.0 {
                indices.push((base + pos, r.clone()));
            }
        }

        // how many decisions in this window are no longer held. The log
        // knows its own length; the window may not reach back that far.
        let beyond_the_window = if base == 0 { 0 } else { base };

        let (log_len, root, signature) = self.checkpoint(key);

        for (index, record) in indices {
            let bytes = record.canonical();
            match self.prove(index) {
                Some(proof) => exhibits.push(Exhibit {
                    index,
                    record,
                    bytes,
                    proof,
                }),
                None => continue,
            }
        }

        Bundle {
            entity: key.entity_id(),
            from_tick: from,
            to_tick: to,
            log_len,
            root,
            signature,
            exhibits,
            beyond_the_window,
        }
    }
}

fn push_u64(s: &mut String, mut v: u64) {
    if v == 0 {
        s.push('0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut n = 0;
    while v > 0 {
        buf[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        s.push(buf[n] as char);
    }
}
