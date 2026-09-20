//! A file-backed journal store.
//!
//! Two files per segment — `episodes.a`, `episodes.b`, and so on — which
//! is the two-slot journal's shape rendered onto a filesystem that has
//! no transactions worth trusting. The write discipline is the same as
//! it is on bare FAT under a firmware driver, and for the same reason:
//! a rename is not atomic in any sense we can rely on across the
//! platforms this has to run on, so we do not use one.
//!
//! The one thing this layer adds beyond writing bytes is the `fsync`.
//! Without it, "the journal survived a power cut" is a claim about the
//! page cache rather than about the disk.

use brainos_abi::ids::EntityId;
use brainos_crypto::key::{BrainKey, PublicKey};
use brainos_memory::journal::{Journal, OpenFailure, Record, Segment, Slot, SlotState};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    Open(OpenFailure),
    Encode(brainos_abi::error::Error),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "io: {e}"),
            StoreError::Open(o) => write!(f, "journal: {o:?}"),
            StoreError::Encode(e) => write!(f, "encode: {e}"),
        }
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> StoreError {
        StoreError::Io(e)
    }
}

pub struct FileStore {
    root: PathBuf,
    journal: Journal,
}

impl FileStore {
    pub fn open(root: impl AsRef<Path>, entity: EntityId) -> Result<FileStore, StoreError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        Ok(FileStore {
            root,
            journal: Journal::new(entity),
        })
    }

    fn path(&self, seg: Segment, slot: Slot) -> PathBuf {
        self.root.join(format!(
            "{}.{}",
            seg.name(),
            match slot {
                Slot::A => "a",
                Slot::B => "b",
            }
        ))
    }

    fn read_slot(&self, seg: Segment, slot: Slot) -> Vec<u8> {
        fs::read(self.path(seg, slot)).unwrap_or_default()
    }

    fn slot_state(&self, seg: Segment) -> SlotState {
        let gen_of = |slot| {
            Record::from_slot_bytes(&self.read_slot(seg, slot))
                .ok()
                .map(|r| r.generation)
        };
        SlotState {
            generation_a: gen_of(Slot::A),
            generation_b: gen_of(Slot::B),
        }
    }

    /// Seal and write one segment.
    pub fn commit(
        &mut self,
        key: &BrainKey,
        seg: Segment,
        payload: Vec<u8>,
    ) -> Result<(), StoreError> {
        let state = self.slot_state(seg);
        let (slot, record) = self.journal.commit(key, seg, payload, state);
        let bytes = record.to_slot_bytes().map_err(StoreError::Encode)?;

        let path = self.path(seg, slot);
        let mut f = fs::File::create(&path)?;
        f.write_all(&bytes)?;
        // without this, "the journal survived a power cut" is a claim
        // about the page cache
        f.sync_all()?;
        Ok(())
    }

    /// Read a segment back, cold.
    pub fn open_segment(
        &mut self,
        key: &PublicKey,
        seg: Segment,
    ) -> Result<Vec<u8>, StoreError> {
        let a = self.read_slot(seg, Slot::A);
        let b = self.read_slot(seg, Slot::B);
        self.journal
            .open_cold(key, seg, &a, &b)
            .map(|r| r.payload)
            .map_err(StoreError::Open)
    }

    /// Read a segment, checking that it chains from what we last wrote.
    pub fn open_segment_checked(
        &mut self,
        key: &PublicKey,
        seg: Segment,
    ) -> Result<Vec<u8>, StoreError> {
        let a = self.read_slot(seg, Slot::A);
        let b = self.read_slot(seg, Slot::B);
        self.journal
            .open(key, seg, &a, &b)
            .map(|r| r.payload)
            .map_err(StoreError::Open)
    }

    pub fn exists(&self, seg: Segment) -> bool {
        self.path(seg, Slot::A).exists() || self.path(seg, Slot::B).exists()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Simulate a power cut during a write: truncate whichever slot was
    /// written most recently. Used by the crash tests, and kept here
    /// rather than in a test file because it needs to know the layout.
    pub fn tear_newest(&mut self, seg: Segment) -> Result<(), StoreError> {
        let state = self.slot_state(seg);
        let newest = match (state.generation_a, state.generation_b) {
            (Some(a), Some(b)) if a >= b => Slot::A,
            (Some(_), Some(_)) => Slot::B,
            (Some(_), None) => Slot::A,
            (None, Some(_)) => Slot::B,
            (None, None) => return Ok(()),
        };
        let path = self.path(seg, newest);
        let bytes = fs::read(&path)?;
        let keep = bytes.len() / 2;
        fs::write(&path, &bytes[..keep])?;
        Ok(())
    }
}
