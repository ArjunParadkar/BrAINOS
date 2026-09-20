//! The archive format.
//!
//! ```text
//!   magic "BRAINEX1" | version u16 | entity id (32) | manifest | sections | root (32) | signature (64)
//! ```
//!
//! Sections are independently framed and independently hashed into a
//! Merkle tree, so a customer can be given a proof about one part of
//! their export — "here is my skill graph, and here is a proof it came
//! from that archive" — without handing over everything.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::codec::{Decode, Decoder, Encode, Encoder};
use brainos_abi::error::Error;
use brainos_abi::ids::EntityId;
use brainos_abi::time::Tick;
use brainos_crypto::domain::Tag;
use brainos_crypto::key::{BrainKey, PublicKey, Signature};
use brainos_crypto::merkle::{MerkleLog, Proof, Root};
use brainos_crypto::sha256::Hash;
use brainos_entity::life::Life;
use brainos_memory::graph::StateGraph;
use brainos_memory::law::MemoryHandle;

pub const MAGIC: [u8; 8] = *b"BRAINEX1";
pub const FORMAT_VERSION: u16 = 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExportError {
    /// the file is not a BrAInOS export
    NotAnArchive,
    /// a newer format version than this build understands
    TooNew { theirs: u16, ours: u16 },
    /// the signature did not verify against the entity it names
    Unsigned,
    /// the Merkle root does not match the sections present
    Tampered,
    /// a section did not decode
    Corrupt(&'static str),
    /// the handle does not belong to the entity in the archive
    WrongEntity,
    /// a required section is missing
    Incomplete(&'static str),
}

/// What kind of thing a section holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
#[repr(u8)]
pub enum SectionKind {
    /// the durable state graph: episodes, semantic memory, beliefs,
    /// skills, calibration, bodies, goals
    Memory = 0,
    /// the life record: age, wakes, bodies ever, spans
    Life = 1,
    /// human-readable notes about the export itself
    Readme = 2,
}

impl SectionKind {
    pub const ALL: [SectionKind; 3] = [
        SectionKind::Memory,
        SectionKind::Life,
        SectionKind::Readme,
    ];

    pub fn from_u8(v: u8) -> Option<SectionKind> {
        SectionKind::ALL.get(v as usize).copied()
    }

    pub const fn name(self) -> &'static str {
        match self {
            SectionKind::Memory => "memory",
            SectionKind::Life => "life",
            SectionKind::Readme => "readme",
        }
    }

    /// Is an archive without this section still an entity?
    pub const fn required(self) -> bool {
        matches!(self, SectionKind::Memory | SectionKind::Life)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub kind: SectionKind,
    pub bytes: Vec<u8>,
}

impl Section {
    pub fn digest(&self) -> Hash {
        let mut h = brainos_crypto::sha256::Sha256::tagged(Tag::Export);
        h.field(&[self.kind as u8]);
        h.field(&self.bytes);
        h.finish()
    }
}

/// What is in the archive, and what deliberately is not.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub entity: EntityId,
    pub exported_at: Tick,
    pub format_version: u16,
    /// what this build calls itself, so a reader knows what wrote it
    pub produced_by: String,
    pub node_count: u32,
    pub skill_count: u32,
    pub body_count: u32,
    pub wakes: u32,
    pub age_ticks: u64,
    /// stated rather than silent
    pub omissions: Vec<String>,
}

impl Encode for Manifest {
    fn encode(&self, e: &mut Encoder) {
        self.entity.encode(e);
        self.exported_at.encode(e);
        e.u16(self.format_version);
        e.str(&self.produced_by);
        e.u32(self.node_count);
        e.u32(self.skill_count);
        e.u32(self.body_count);
        e.u32(self.wakes);
        e.u64(self.age_ticks);
        e.seq(self.omissions.len());
        for o in &self.omissions {
            e.str(o);
        }
    }
}

impl Decode for Manifest {
    fn decode(d: &mut Decoder) -> brainos_abi::error::Result<Manifest> {
        let entity = EntityId::decode(d)?;
        let exported_at = Tick::decode(d)?;
        let format_version = d.u16()?;
        let produced_by = alloc::string::ToString::to_string(d.str()?);
        let node_count = d.u32()?;
        let skill_count = d.u32()?;
        let body_count = d.u32()?;
        let wakes = d.u32()?;
        let age_ticks = d.u64()?;
        let n = d.seq()?;
        let mut omissions = Vec::new();
        for _ in 0..n {
            omissions.push(alloc::string::ToString::to_string(d.str()?));
        }
        Ok(Manifest {
            entity,
            exported_at,
            format_version,
            produced_by,
            node_count,
            skill_count,
            body_count,
            wakes,
            age_ticks,
            omissions,
        })
    }
}

/// A complete export.
#[derive(Clone, Debug, PartialEq)]
pub struct Archive {
    pub manifest: Manifest,
    pub sections: Vec<Section>,
    pub root: Root,
    pub signature: Signature,
}

impl Archive {
    pub fn section(&self, kind: SectionKind) -> Option<&Section> {
        self.sections.iter().find(|s| s.kind == kind)
    }

    /// Recompute the Merkle root over the sections as they are.
    pub fn compute_root(&self) -> Root {
        let mut log = MerkleLog::new();
        for s in &self.sections {
            log.append_hash(s.digest());
        }
        log.root()
    }

    /// What the signature covers: the manifest and the root together.
    /// Signing the root alone would let the manifest be rewritten;
    /// signing the manifest alone would let sections be swapped.
    fn signed_payload(manifest: &Manifest, root: &Root) -> Vec<u8> {
        let mut h = brainos_crypto::sha256::Sha256::tagged(Tag::Export);
        let mut e = Encoder::new();
        manifest.encode(&mut e);
        h.field(&e.finish_bounded());
        h.field(root.as_bytes());
        h.finish().as_bytes().to_vec()
    }

    /// Check the archive against the entity it claims to be.
    pub fn verify(&self, key: &PublicKey) -> Result<(), ExportError> {
        if !self.manifest.entity.ct_eq(&key.entity_id()) {
            return Err(ExportError::WrongEntity);
        }
        if !self.compute_root().ct_eq(&self.root) {
            return Err(ExportError::Tampered);
        }
        if !key.verify(
            Tag::Export,
            &Archive::signed_payload(&self.manifest, &self.root),
            &self.signature,
        ) {
            return Err(ExportError::Unsigned);
        }
        for kind in SectionKind::ALL {
            if kind.required() && self.section(kind).is_none() {
                return Err(ExportError::Incomplete(kind.name()));
            }
        }
        Ok(())
    }

    /// A proof that one section came from this archive. Lets a customer
    /// hand over one part — their skill graph, say — without handing
    /// over everything, and still have it be checkable.
    pub fn prove(&self, kind: SectionKind) -> Option<(Proof, Root)> {
        let index = self.sections.iter().position(|s| s.kind == kind)?;
        let mut log = MerkleLog::new();
        for s in &self.sections {
            log.append_hash(s.digest());
        }
        log.prove(index).map(|p| (p, log.root()))
    }

    pub fn to_bytes(&self) -> brainos_abi::error::Result<Vec<u8>> {
        let mut e = Encoder::new();
        e.raw(&MAGIC);
        e.u16(FORMAT_VERSION);
        self.manifest.encode(&mut e);
        e.seq(self.sections.len());
        for s in &self.sections {
            e.u8(s.kind as u8);
            e.bytes(&s.bytes);
        }
        e.raw(self.root.as_bytes());
        e.raw(self.signature.as_bytes());
        e.finish()
    }

    pub fn from_bytes(raw: &[u8]) -> Result<Archive, ExportError> {
        if raw.len() < 10 || raw[..8] != MAGIC {
            return Err(ExportError::NotAnArchive);
        }
        let version = u16::from_le_bytes([raw[8], raw[9]]);
        if version > FORMAT_VERSION {
            return Err(ExportError::TooNew {
                theirs: version,
                ours: FORMAT_VERSION,
            });
        }

        let mut d = Decoder::new(&raw[10..]);
        let manifest = Manifest::decode(&mut d).map_err(|_| ExportError::Corrupt("manifest"))?;
        let n = d.seq().map_err(|_| ExportError::Corrupt("section count"))?;
        let mut sections = Vec::new();
        for _ in 0..n {
            let k = d.u8().map_err(|_| ExportError::Corrupt("section kind"))?;
            let kind =
                SectionKind::from_u8(k).ok_or(ExportError::Corrupt("unknown section kind"))?;
            let bytes = d
                .bytes()
                .map_err(|_| ExportError::Corrupt("section body"))?
                .to_vec();
            sections.push(Section { kind, bytes });
        }
        let mut root = [0u8; 32];
        d.read_exact(&mut root)
            .map_err(|_| ExportError::Corrupt("root"))?;
        let mut sig = [0u8; 64];
        d.read_exact(&mut sig)
            .map_err(|_| ExportError::Corrupt("signature"))?;
        d.finish().map_err(|_| ExportError::Corrupt("trailing bytes"))?;

        Ok(Archive {
            manifest,
            sections,
            root: Hash(root),
            signature: Signature(sig),
        })
    }
}

/// Produce an export.
pub fn export(
    key: &BrainKey,
    graph: &StateGraph,
    life: &Life,
    skills: usize,
    bodies: usize,
    now: Tick,
) -> Result<Archive, Error> {
    let memory = graph.encode_durable()?;
    let life_bytes = life.to_bytes()?;
    let readme = crate::format::describe_format();

    let sections = alloc::vec![
        Section {
            kind: SectionKind::Memory,
            bytes: memory,
        },
        Section {
            kind: SectionKind::Life,
            bytes: life_bytes,
        },
        Section {
            kind: SectionKind::Readme,
            bytes: readme.into_bytes(),
        },
    ];

    let manifest = Manifest {
        entity: key.entity_id(),
        exported_at: now,
        format_version: FORMAT_VERSION,
        produced_by: alloc::string::ToString::to_string(concat!(
            "brainos-export ",
            env!("CARGO_PKG_VERSION")
        )),
        node_count: graph.len() as u32,
        skill_count: skills as u32,
        body_count: bodies as u32,
        wakes: life.wakes,
        age_ticks: life.age(now),
        omissions: alloc::vec![
            alloc::string::ToString::to_string(
                "the private half of the BrAIn Key: it never leaves the secure element and is not ours to move"
            ),
            alloc::string::ToString::to_string(
                "the KIRA audit log: it belongs to the machine of record, because an entity carrying its own audit log between owners could edit it in transit"
            ),
        ],
    };

    let mut log = MerkleLog::new();
    for s in &sections {
        log.append_hash(s.digest());
    }
    let root = log.root();
    let signature = key.sign(Tag::Export, &Archive::signed_payload(&manifest, &root));

    Ok(Archive {
        manifest,
        sections,
        root,
        signature,
    })
}

/// Read an export back into a graph and a life.
///
/// Verification happens first and completely. A partially-imported
/// entity is a corrupted entity, and importing before checking is how
/// you get one.
pub fn import(
    archive: &Archive,
    key: &PublicKey,
    handle: &MemoryHandle,
    graph: &mut StateGraph,
) -> Result<Life, ExportError> {
    archive.verify(key)?;
    handle
        .authorizes(archive.manifest.entity)
        .map_err(|_| ExportError::WrongEntity)?;

    let memory = archive
        .section(SectionKind::Memory)
        .ok_or(ExportError::Incomplete("memory"))?;
    graph
        .load_durable(handle, &memory.bytes)
        .map_err(|_| ExportError::Corrupt("memory section did not decode"))?;

    let life_section = archive
        .section(SectionKind::Life)
        .ok_or(ExportError::Incomplete("life"))?;
    Life::from_bytes(&life_section.bytes)
        .map_err(|_| ExportError::Corrupt("life section did not decode"))
}
