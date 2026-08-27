//! Read and write the entity's episodic journal (EPI_A / EPI_B).
//! Uses brainos_mind::journal for the two-slot CRC format.

use anyhow::{Context, Result};
use brainos_mind::{journal, state::StateGraph};
use std::path::{Path, PathBuf};

pub struct JournalIO {
    brain_dir: PathBuf,
}

impl JournalIO {
    pub fn new(brain_dir: &Path) -> JournalIO {
        JournalIO { brain_dir: brain_dir.to_path_buf() }
    }

    fn slot_path(&self, slot: journal::Slot) -> PathBuf {
        let name = match slot {
            journal::Slot::A => "EPI_A",
            journal::Slot::B => "EPI_B",
        };
        self.brain_dir.join(name)
    }

    /// Load episodic memories into a StateGraph.
    /// Returns the number of inherited episodes.
    pub fn load(&self) -> (StateGraph, usize) {
        let mut graph = StateGraph::new();

        let raw_a = self.read_slot(journal::Slot::A);
        let raw_b = self.read_slot(journal::Slot::B);

        match journal::newest(&raw_a, &raw_b) {
            Some((gen, payload)) => {
                tracing::info!("[journal] loaded gen={gen} ({} bytes)", payload.len());
                graph.load(payload);
            }
            None => {
                tracing::info!("[journal] no valid journal slot; starting fresh");
                // try legacy flat-file fallback (EPISODES.LOG)
                let legacy = self.brain_dir.join("EPISODES.LOG");
                if legacy.exists() {
                    if let Ok(data) = std::fs::read(&legacy) {
                        tracing::info!("[journal] migrating from EPISODES.LOG");
                        graph.load(&data);
                    }
                }
            }
        }

        let n = graph.inherited_episodes;
        tracing::info!("[journal] {} inherited episodes, {} total nodes", n, graph.node_count());
        (graph, n)
    }

    /// Persist the state graph using the two-slot journal.
    pub fn save(&self, graph: &StateGraph) -> Result<()> {
        std::fs::create_dir_all(&self.brain_dir)
            .with_context(|| format!("creating brain dir {}", self.brain_dir.display()))?;

        let payload = graph.serialize();
        let payload_bytes = payload.as_bytes();

        let raw_a = self.read_slot(journal::Slot::A);
        let raw_b = self.read_slot(journal::Slot::B);

        let gen_a = journal::open(&raw_a).map(|(g, _)| g);
        let gen_b = journal::open(&raw_b).map(|(g, _)| g);
        let (target_slot, generation) = journal::plan_write(gen_a, gen_b);

        let record = journal::seal(generation, payload_bytes);
        let path = self.slot_path(target_slot);

        std::fs::write(&path, &record)
            .with_context(|| format!("writing journal slot {:?}", target_slot))?;

        tracing::debug!(
            "[journal] saved gen={generation} slot={:?} ({} bytes, {} nodes)",
            target_slot, record.len(), graph.node_count()
        );
        Ok(())
    }

    fn read_slot(&self, slot: journal::Slot) -> Vec<u8> {
        let path = self.slot_path(slot);
        std::fs::read(&path).unwrap_or_default()
    }

    /// Load the NOTE_A/NOTE_B notebook using the same two-slot format.
    pub fn load_notes(&self) -> String {
        let raw_a = std::fs::read(self.brain_dir.join("NOTE_A")).unwrap_or_default();
        let raw_b = std::fs::read(self.brain_dir.join("NOTE_B")).unwrap_or_default();
        match journal::newest(&raw_a, &raw_b) {
            Some((_, payload)) => String::from_utf8_lossy(payload).into_owned(),
            None => {
                // legacy fallback
                let legacy = self.brain_dir.join("NOTES.TXT");
                std::fs::read_to_string(&legacy).unwrap_or_default()
            }
        }
    }

    /// Save the notebook using NOTE_A / NOTE_B.
    pub fn save_notes(&self, content: &str) -> Result<()> {
        std::fs::create_dir_all(&self.brain_dir)?;
        let raw_a = std::fs::read(self.brain_dir.join("NOTE_A")).unwrap_or_default();
        let raw_b = std::fs::read(self.brain_dir.join("NOTE_B")).unwrap_or_default();
        let gen_a = journal::open(&raw_a).map(|(g, _)| g);
        let gen_b = journal::open(&raw_b).map(|(g, _)| g);
        let (slot, gen) = journal::plan_write(gen_a, gen_b);
        let record = journal::seal(gen, content.as_bytes());
        let name = match slot { journal::Slot::A => "NOTE_A", journal::Slot::B => "NOTE_B" };
        std::fs::write(self.brain_dir.join(name), &record)?;
        Ok(())
    }
}
