//! BFS  LEVEL    Domain 3 KIRA - the eight-stage gate; the only path to action  [crate brainos-kira]
//! BFS  PURPOSE  Writes every gate decision, granted or denied, into a Merkle log so any past decision can be proven and none can be quietly edited
//! BFS  DEFINES  Decision, AuditRecord, AuditTrail
//! BFS  USES     this crate: action, capability, stages | brainos: abi, crypto
//! BFS  USED BY  host/incident, kira/evidence, kira/gate, kira/lib, kira/tests/evidence_suite, kira/tests/gate_suite, proof/adversary
//!
//! The audit trail.
//!
//! Stage 8 runs whatever happened, and what it writes goes into a Merkle
//! log. The consequence is worth stating because it is the difference
//! between a log and a record: anyone holding the 32-byte root can be
//! given a proof that a specific decision is in the log, and if a
//! decision is removed or altered, every root after it changes. The
//! entity cannot quietly edit its own history, and neither can we.
//!
//! This is also the EU Machinery Regulation answer (risk R1). "Show the
//! KIRA log" for an AI-controlled machine stops being a request for a
//! file and becomes a verifiable claim.

use crate::action::Request;
use crate::capability::Grant;
use crate::stages::Trace;
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::codec::{Encode, Encoder};
use brainos_abi::error::{Denied, Stage};
use brainos_abi::ids::{BodyId, EntityId};
use brainos_abi::time::Tick;
use brainos_abi::verb::Verb;
use brainos_crypto::key::{BrainKey, PublicKey, Signature};
use brainos_crypto::merkle::{MerkleLog, Proof, Root};
use brainos_crypto::sha256::Hash;
use brainos_crypto::domain::Tag;

/// What the gate decided.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decision {
    Granted,
    Denied { stage: Stage },
}

/// One decision, as it goes into the log.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditRecord {
    pub entity: EntityId,
    pub verb: Verb,
    pub body: Option<BodyId>,
    pub tick: Tick,
    pub decision: Decision,
    /// the digest of the request, so the record is linked to exactly
    /// what was asked rather than to a description of it
    pub request: Hash,
    /// present only when granted
    pub grant: Option<Grant>,
    /// the refusal, when denied
    pub denial: Option<Denied>,
    /// the eight stage notes, in order
    pub notes: Vec<String>,
    /// what the proposer said it was for. Narration, never consulted.
    pub reason: String,
}

impl AuditRecord {
    pub fn build(
        entity: EntityId,
        request: &Request,
        trace: &Trace,
        grant: Option<Grant>,
    ) -> AuditRecord {
        let refusal = trace.refusal();
        AuditRecord {
            entity,
            verb: request.verb,
            body: request.body,
            tick: request.tick,
            decision: match refusal {
                None => Decision::Granted,
                Some((stage, _)) => Decision::Denied { stage },
            },
            request: request.digest(),
            grant,
            denial: refusal.map(|(_, d)| d),
            notes: trace
                .entries()
                .iter()
                .map(|e| alloc::string::ToString::to_string(e.outcome.note()))
                .collect(),
            reason: request.reason.clone(),
        }
    }

    /// Canonical bytes. This is what the Merkle leaf is taken over, so
    /// the encoding must be stable across versions in the same way a
    /// signature's must.
    pub fn canonical(&self) -> Vec<u8> {
        let mut e = Encoder::new();
        self.entity.encode(&mut e);
        self.verb.encode(&mut e);
        e.opt(&self.body);
        self.tick.encode(&mut e);
        match self.decision {
            Decision::Granted => e.u8(0),
            Decision::Denied { stage } => {
                e.u8(1);
                e.u8(stage as u8);
            }
        }
        e.raw(self.request.as_bytes());
        match &self.grant {
            Some(g) => {
                e.bool(true);
                g.id.encode(&mut e);
                e.u64(g.ttl_ticks);
            }
            None => e.bool(false),
        }
        match &self.denial {
            Some(d) => {
                e.bool(true);
                e.u8(d.stage as u8);
                e.u16(d.code as u16);
            }
            None => e.bool(false),
        }
        e.seq(self.notes.len());
        for n in &self.notes {
            e.str(n);
        }
        e.str(&self.reason);
        e.finish_bounded()
    }

    /// One line, for a human reading the log.
    pub fn render(&self) -> String {
        let mut s = String::new();
        match self.decision {
            Decision::Granted => s.push_str("GRANT "),
            Decision::Denied { stage } => {
                s.push_str("DENY[");
                s.push_str(stage.name());
                s.push_str("] ");
            }
        }
        s.push_str(self.verb.as_str());
        if let Some(b) = &self.body {
            s.push_str(" via ");
            s.push_str(b.fingerprint().as_str());
        }
        s.push_str(" @t");
        push_u64(&mut s, self.tick.0);
        if let Some(d) = &self.denial {
            s.push_str(" — ");
            s.push_str(d.code.name());
            s.push_str(": ");
            s.push_str(d.detail);
        }
        s
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

/// The log, plus the working window of records.
///
/// Records are kept in a bounded window; the Merkle log keeps every leaf
/// hash forever, which is 32 bytes per decision. A robot making ten
/// gated decisions a second accumulates about 25 MB a year of leaf
/// hashes — acceptable, and the records themselves are what gets pruned.
/// Pruned records stay provable: their leaves and every root that
/// included them are unchanged.
pub struct AuditTrail {
    log: MerkleLog,
    window: Vec<AuditRecord>,
    window_cap: usize,
    /// roots signed by the entity at checkpoints, so a verifier needs
    /// only these to check any proof
    checkpoints: Vec<(usize, Root, Signature)>,
    granted: u64,
    denied: u64,
    denied_by_stage: [u64; 8],
}

impl Default for AuditTrail {
    fn default() -> AuditTrail {
        AuditTrail::new(4_096)
    }
}

impl AuditTrail {
    /// The working window a board can afford.
    ///
    /// A measurement, not a preference. A populated entity at
    /// `Bounds::EMBEDDED` making 256 gated decisions carried 158 KiB of
    /// audit trail on an ESP32-S3 that has about 192 KiB for the whole
    /// entity — and almost all of it was the *record window*, not the
    /// leaves, because a record keeps its eight stage notes as eight
    /// separate strings. The records' real home is the machine of
    /// record; what a board needs is the leaves, for the root, and
    /// enough recent records to tell a human why it just refused
    /// something.
    pub fn embedded() -> AuditTrail {
        AuditTrail::new(32)
    }

    /// How many records the working window holds.
    pub fn window_cap(&self) -> usize {
        self.window_cap
    }

    /// Resize the working window, dropping the oldest if it shrinks.
    pub fn set_window_cap(&mut self, cap: usize) {
        self.window_cap = cap;
        while self.window.len() > self.window_cap {
            self.window.remove(0);
        }
    }

    pub fn new(window_cap: usize) -> AuditTrail {
        AuditTrail {
            log: MerkleLog::new(),
            window: Vec::new(),
            window_cap,
            checkpoints: Vec::new(),
            granted: 0,
            denied: 0,
            denied_by_stage: [0; 8],
        }
    }

    pub fn len(&self) -> usize {
        self.log.len()
    }

    pub fn is_empty(&self) -> bool {
        self.log.is_empty()
    }

    pub fn root(&self) -> Root {
        self.log.root()
    }

    pub fn granted_count(&self) -> u64 {
        self.granted
    }

    pub fn denied_count(&self) -> u64 {
        self.denied
    }

    pub fn denied_at(&self, stage: Stage) -> u64 {
        self.denied_by_stage[stage.index()]
    }

    /// Stage 8. Returns the record's index, which is what a proof refers
    /// to later.
    pub fn record(&mut self, record: AuditRecord) -> usize {
        let index = self.log.append(&record.canonical());
        match record.decision {
            Decision::Granted => self.granted += 1,
            Decision::Denied { stage } => {
                self.denied += 1;
                self.denied_by_stage[stage.index()] += 1;
            }
        }
        self.window.push(record);
        while self.window.len() > self.window_cap {
            self.window.remove(0);
        }
        index
    }

    /// Roughly what the trail occupies in RAM, live: the working window
    /// of records plus 32 bytes for every leaf hash ever appended.
    ///
    /// The second term is the one that grows without bound. It is 32
    /// bytes per gated decision and it is kept forever on purpose —
    /// pruning the records while keeping the leaves is what lets a
    /// proof issued last year still verify — but on a board it is the
    /// term that decides how long the robot can run before something
    /// has to be offloaded.
    pub fn approximate_bytes(&self) -> usize {
        let records: usize = self
            .window
            .iter()
            .map(|r| {
                core::mem::size_of::<AuditRecord>()
                    + r.reason.capacity()
                    + r.notes.iter().map(|n| n.capacity()).sum::<usize>()
            })
            .sum();
        records + self.log.len() * 32 + core::mem::size_of::<AuditTrail>()
    }

    pub fn recent(&self, n: usize) -> &[AuditRecord] {
        let start = self.window.len().saturating_sub(n);
        &self.window[start..]
    }

    /// Sign the current root. A verifier who has a signed checkpoint can
    /// check any proof against it without trusting whoever supplied the
    /// proof.
    pub fn checkpoint(&mut self, key: &BrainKey) -> (usize, Root, Signature) {
        let root = self.log.root();
        let sig = key.sign(Tag::AuditNode, root.as_bytes());
        let cp = (self.log.len(), root, sig);
        self.checkpoints.push(cp);
        cp
    }

    pub fn checkpoints(&self) -> &[(usize, Root, Signature)] {
        &self.checkpoints
    }

    pub fn verify_checkpoint(&self, cp: &(usize, Root, Signature), key: &PublicKey) -> bool {
        key.verify(Tag::AuditNode, cp.1.as_bytes(), &cp.2)
    }

    /// Prove that a record is in the log.
    pub fn prove(&self, index: usize) -> Option<Proof> {
        self.log.prove(index)
    }

    /// Every denial at one stage, from the working window. What a human
    /// asks for after an incident: "show me everything the gate refused
    /// at policy".
    pub fn denials_at(&self, stage: Stage) -> Vec<&AuditRecord> {
        self.window
            .iter()
            .filter(|r| matches!(r.decision, Decision::Denied { stage: s } if s == stage))
            .collect()
    }

    /// The refusals worth telling the human about.
    pub fn salient(&self, n: usize) -> Vec<&AuditRecord> {
        self.window
            .iter()
            .rev()
            .filter(|r| r.denial.map(|d| d.is_salient()).unwrap_or(false))
            .take(n)
            .collect()
    }
}
