//! BFS  LEVEL    Domain 2 Autonomic - the BLUR quadruped body (wire, acquisition, supervisor)  [crate brainos-blur]
//! BFS  PURPOSE  Collects each CAN node's chip ID into a census and compares it with the recorded build sheet to catch swapped, missing, duplicate or unknown nodes
//! BFS  DEFINES  NodeIdentity, Conflict, Census, CensusBuilder, BuildSheet, CensusFault, check_census, missing_nodes
//! BFS  USES     this crate: descriptor, wire | brainos: crypto
//! BFS  USED BY  blur/acquire, blur/autonomic, blur/firmware, blur/schema, blur/tests/blur_suite, host/blur_run, host/blur_sil
//!
//! The census: which pieces of silicon are on BLUR's buses right now.
//!
//! The Teensy broadcasts CENSUS on all three buses and every node
//! answers with three IDENTITY pages carrying its 96-bit UID. The
//! census is what the Teensy *saw*; the build sheet is what the entity
//! *recorded* when this body was assembled (or first inhabited). They are
//! compared at the handshake, because a node is part of who this body is.
//!
//! **What this does and does not prove.** CAN has no authentication.
//! A UID is a claim, and a node that copies another's UID passes. What
//! the census catches is a part swapped without telling the entity, a
//! node configured with the wrong id, two nodes answering on one id,
//! and a node the build sheet has never heard of — i.e. assembly
//! mistakes and naive intrusion. A deliberate attacker with a probe on
//! the bus is out of scope for any CAN robot; the mitigation is that the
//! buses are inside the chassis and the Teensy is their only master.

use crate::descriptor::{node as node_spec, Bus, NodeKind, NODES};
use crate::wire::{CanFrame, IdentityPage, KIND_ESC1, KIND_ESP32S3};
use alloc::vec::Vec;
use brainos_crypto::domain::Tag;
use brainos_crypto::sha256::{Hash, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeIdentity {
    pub bus: Bus,
    pub id: u8,
    pub kind: u8,
    pub slots: u8,
    pub fw: u8,
    pub uid: [u8; 12],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// two different identities answered on one id
    TwoAnswers { bus: Bus, id: u8 },
    /// the same id answered on two buses
    TwoBuses { id: u8 },
    /// a node sent some identity pages and not all three
    Incomplete { bus: Bus, id: u8 },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Census {
    pub nodes: Vec<NodeIdentity>,
    pub conflicts: Vec<Conflict>,
    pub imu_present: bool,
}

impl Census {
    pub fn get(&self, id: u8) -> Option<&NodeIdentity> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Canonical bytes — what the Teensy signs and the entity verifies.
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(self.imu_present as u8);
        out.push(self.nodes.len() as u8);
        for n in &self.nodes {
            out.extend_from_slice(&[n.bus as u8, n.id, n.kind, n.slots, n.fw]);
            out.extend_from_slice(&n.uid);
        }
        out.push(self.conflicts.len() as u8);
        for c in &self.conflicts {
            match *c {
                Conflict::TwoAnswers { bus, id } => out.extend_from_slice(&[1, bus as u8, id]),
                Conflict::TwoBuses { id } => out.extend_from_slice(&[2, 0, id]),
                Conflict::Incomplete { bus, id } => out.extend_from_slice(&[3, bus as u8, id]),
            }
        }
        out
    }

    pub fn digest(&self) -> Hash {
        let mut h = Sha256::tagged(Tag::Attestation);
        h.field(b"blur.census.v1");
        h.field(&self.bytes());
        h.finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slot {
    bus: Bus,
    id: u8,
    pages: [Option<IdentityPage>; 3],
}

/// Collects IDENTITY pages for one census window.
#[derive(Clone, Debug)]
pub struct CensusBuilder {
    started: bool,
    slots: Vec<Slot>,
    conflicts: Vec<Conflict>,
}

impl Default for CensusBuilder {
    fn default() -> Self {
        CensusBuilder::new()
    }
}

impl CensusBuilder {
    pub fn new() -> CensusBuilder {
        CensusBuilder {
            started: false,
            slots: Vec::new(),
            conflicts: Vec::new(),
        }
    }
    pub fn is_fresh(&self) -> bool {
        !self.started
    }
    pub fn started(&mut self) {
        self.started = true;
    }

    fn conflict(&mut self, c: Conflict) {
        if !self.conflicts.contains(&c) {
            self.conflicts.push(c);
        }
    }

    pub fn page(&mut self, f: &CanFrame) {
        let Some(p) = IdentityPage::decode(f) else {
            return;
        };
        let id = f.node();
        if self.slots.iter().any(|s| s.id == id && s.bus != f.bus) {
            self.conflict(Conflict::TwoBuses { id });
        }
        let pos = match self.slots.iter().position(|s| s.id == id && s.bus == f.bus) {
            Some(i) => i,
            None => {
                self.slots.push(Slot {
                    bus: f.bus,
                    id,
                    pages: [None; 3],
                });
                self.slots.len() - 1
            }
        };
        let k = p.page as usize;
        match self.slots[pos].pages[k] {
            Some(old) if old != p => {
                let bus = f.bus;
                self.conflict(Conflict::TwoAnswers { bus, id });
            }
            _ => self.slots[pos].pages[k] = Some(p),
        }
    }

    pub fn finish(mut self, imu_present: bool) -> Census {
        let mut nodes = Vec::new();
        let slots = core::mem::take(&mut self.slots);
        for s in slots {
            match s.pages {
                [Some(a), Some(b), Some(c)] if a.kind == b.kind && b.kind == c.kind => {
                    let mut uid = [0u8; 12];
                    uid[0..4].copy_from_slice(&a.uid);
                    uid[4..8].copy_from_slice(&b.uid);
                    uid[8..12].copy_from_slice(&c.uid);
                    nodes.push(NodeIdentity {
                        bus: s.bus,
                        id: s.id,
                        kind: a.kind,
                        slots: a.slots,
                        fw: a.fw,
                        uid,
                    });
                }
                _ => self.conflict(Conflict::Incomplete { bus: s.bus, id: s.id }),
            }
        }
        nodes.sort_by_key(|n| (n.id, n.bus as u8));
        self.conflicts.sort_by_key(|c| match *c {
            Conflict::TwoAnswers { bus, id } => (1u8, id, bus as u8),
            Conflict::TwoBuses { id } => (2, id, 0),
            Conflict::Incomplete { bus, id } => (3, id, bus as u8),
        });
        Census {
            nodes,
            conflicts: self.conflicts,
            imu_present,
        }
    }
}

/// The entity's record of which silicon this body is made of. Written to
/// the state graph when the body is assembled (or on first inhabitation,
/// with the human's confirmation); compared against every census.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BuildSheet {
    pub uids: Vec<(u8, [u8; 12])>,
}

impl BuildSheet {
    pub fn from_census(c: &Census) -> BuildSheet {
        BuildSheet {
            uids: c.nodes.iter().map(|n| (n.id, n.uid)).collect(),
        }
    }
    pub fn uid(&self, id: u8) -> Option<[u8; 12]> {
        self.uids.iter().find(|(i, _)| *i == id).map(|(_, u)| *u)
    }
}

/// Why a census does not describe the body the entity expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CensusFault {
    /// two nodes on one id, or one id on two buses
    Conflict(Conflict),
    /// a node the descriptor has no place for
    Stranger { bus: Bus, id: u8 },
    /// a node on the wrong bus, of the wrong kind, or with the wrong slot count
    WrongPart { id: u8 },
    /// the silicon differs from the build sheet
    Swapped { id: u8 },
}

/// The handshake-stage check: is every node that answered a node this
/// body is supposed to have, made of the silicon the entity recorded?
/// A *missing* node is not a handshake fault — the body may legitimately
/// say it lacks that part — and is caught at the schema stage instead.
pub fn check_census(c: &Census, sheet: Option<&BuildSheet>) -> Result<(), CensusFault> {
    if let Some(&x) = c.conflicts.iter().find(|x| !matches!(x, Conflict::Incomplete { .. })) {
        return Err(CensusFault::Conflict(x));
    }
    if let Some(&x) = c.conflicts.first() {
        return Err(CensusFault::Conflict(x));
    }
    for n in &c.nodes {
        let Some(spec) = node_spec(n.id) else {
            return Err(CensusFault::Stranger { bus: n.bus, id: n.id });
        };
        let kind = match spec.kind {
            NodeKind::Esc1 => KIND_ESC1,
            NodeKind::Esp32S3 => KIND_ESP32S3,
        };
        if spec.bus != n.bus || kind != n.kind || spec.slots != n.slots {
            return Err(CensusFault::WrongPart { id: n.id });
        }
        if let Some(sheet) = sheet {
            match sheet.uid(n.id) {
                Some(u) if u == n.uid => {}
                _ => return Err(CensusFault::Swapped { id: n.id }),
            }
        }
    }
    Ok(())
}

/// Nodes the descriptor declares that did not answer.
pub fn missing_nodes(c: &Census) -> Vec<u8> {
    NODES
        .iter()
        .filter(|s| c.get(s.id).is_none())
        .map(|s| s.id)
        .collect()
}
