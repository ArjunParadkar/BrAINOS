//! Entity export v1.0 (Execution Plan §11; Master Document 0.4 "Freedom",
//! bylaw "Entity sovereignty"). The customer owns the entity. Export writes
//! the complete state of an entity — everything that makes it *this* one —
//! in a documented, machine-readable form; import puts it on a key again.
//!
//! ```text
//! <name>.brainos-export/
//!   MANIFEST.json     v, pub, exported_at, counts, sha256 of every file
//!   state.graph       every episode / semantic / belief node (the journal's text form)
//!   skills.skl        the signed skill store, signatures intact (anyone with KEY.PUB can verify)
//!   notes.txt         the private notebook
//!   calibration.json  §8 step-5 records per body, lifted from the semantic nodes
//!   audit.log         audit nodes present in memory (the full trail stays on the machine of record)
//! ```
//!
//! What is never exported: the private seed. The key is the owner's secure
//! element. Importing onto a *different* key is `adopt`: the state graph and
//! notes carry over as they are; skills are re-signed by the adopting key,
//! because a skill that does not verify under the entity's own key never
//! loads (that is what makes a planted skill harmless).

use crate::journal_io::JournalIO;
use crate::skills::SkillStore;
use anyhow::{bail, Context, Result};
use brainos_mind::key::BrainKey;
use brainos_mind::state::{NodeKind, StateGraph};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub v: u32,
    pub brainos_export: String,
    /// the entity's public key, hex
    pub pub_key: String,
    pub exported_at: String,
    pub counts: Counts,
    pub sha256: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Counts {
    pub episodes: usize,
    pub semantic: usize,
    pub beliefs: usize,
    pub audit: usize,
    pub skills: usize,
    pub notes_bytes: usize,
    pub bodies_calibrated: Vec<String>,
}

fn sha_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

fn read_pub(brain_dir: &Path) -> Result<String> {
    let p = brain_dir.join("KEY.PUB");
    let s = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
    let hex = s.lines().next().unwrap_or("").trim().to_string();
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("{} does not hold a 32-byte hex public key", p.display());
    }
    Ok(hex)
}

/// The key the target brain dir can sign with (SEED.HEX + KEY.PUB).
pub fn load_key(brain_dir: &Path) -> Result<BrainKey> {
    let seed = std::fs::read(brain_dir.join("SEED.HEX")).context("SEED.HEX (the private seed) is needed to import")?;
    let pubk = std::fs::read(brain_dir.join("KEY.PUB")).context("KEY.PUB")?;
    let seed_line = seed.split(|b| *b == b'\n').next().unwrap_or(&[]);
    let pub_line = pubk.split(|b| *b == b'\n').next().unwrap_or(&[]);
    BrainKey::load(seed_line, pub_line).map_err(|e| anyhow::anyhow!("brain key: {e}"))
}

/// Calibration records as JSON: `{ "<body>": { "layout": "...", "joints": ["ch6 = L1+L3 swing center ..", ...] } }`.
fn calibration_json(graph: &StateGraph) -> (serde_json::Value, Vec<String>) {
    let mut bodies: BTreeMap<String, (String, BTreeMap<u8, String>)> = BTreeMap::new();
    for nd in graph.nodes().iter().filter(|n| n.kind == NodeKind::Semantic) {
        if let Some((body, rest)) = nd.content.split_once(":layout = ") {
            bodies.entry(body.to_string()).or_default().0 = rest.trim().to_string();
        } else if let Some((body, rest)) = nd.content.split_once(":cal:ch") {
            if let Some((ch, words)) = rest.split_once(" = ") {
                if let Ok(ch) = ch.trim().parse::<u8>() {
                    bodies.entry(body.to_string()).or_default().1.insert(ch, format!("ch{ch} = {}", words.trim()));
                }
            }
        }
    }
    let names: Vec<String> = bodies.keys().cloned().collect();
    let mut out = serde_json::Map::new();
    for (body, (layout, joints)) in bodies {
        out.insert(body, serde_json::json!({"layout": layout, "joints": joints.values().collect::<Vec<_>>()}));
    }
    (serde_json::Value::Object(out), names)
}

/// Export the entity in `brain_dir` to the directory `out` (created).
pub fn export(brain_dir: &Path, out: &Path) -> Result<Manifest> {
    let pub_key = read_pub(brain_dir)?;
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let (graph, _) = JournalIO::new(brain_dir).load();
    let skills_raw = SkillStore::raw_lines(brain_dir);
    let notes = JournalIO::new(brain_dir).load_notes();
    let (cal, bodies) = calibration_json(&graph);
    let audit: String = graph
        .nodes()
        .iter()
        .filter(|n| n.kind == NodeKind::Audit)
        .map(|n| format!("{}|{}\n", n.tick, n.content))
        .collect();

    let files: Vec<(&str, Vec<u8>)> = vec![
        ("state.graph", graph.serialize().into_bytes()),
        ("skills.skl", skills_raw.clone().into_bytes()),
        ("notes.txt", notes.clone().into_bytes()),
        ("calibration.json", serde_json::to_vec_pretty(&cal)?),
        ("audit.log", format!("# audit nodes held in memory at export; the immutable trail stays on the machine of record\n{audit}").into_bytes()),
    ];
    let mut sha = BTreeMap::new();
    for (name, bytes) in &files {
        std::fs::write(out.join(name), bytes)?;
        sha.insert(name.to_string(), sha_hex(bytes));
    }
    let manifest = Manifest {
        v: FORMAT_VERSION,
        brainos_export: "the complete state of one entity; the private key is never included".into(),
        pub_key,
        exported_at: chrono::Utc::now().to_rfc3339(),
        counts: Counts {
            episodes: graph.count(NodeKind::Episode),
            semantic: graph.count(NodeKind::Semantic),
            beliefs: graph.count(NodeKind::Belief),
            audit: graph.count(NodeKind::Audit),
            skills: skills_raw.lines().filter(|l| !l.trim().is_empty()).count(),
            notes_bytes: notes.len(),
            bodies_calibrated: bodies,
        },
        sha256: sha,
    };
    std::fs::write(out.join("MANIFEST.json"), serde_json::to_vec_pretty(&manifest)?)?;
    Ok(manifest)
}

/// Read and check an export directory: manifest, format version, hashes.
pub fn verify(dir: &Path) -> Result<Manifest> {
    let m: Manifest = serde_json::from_slice(&std::fs::read(dir.join("MANIFEST.json")).context("MANIFEST.json")?)?;
    if m.v != FORMAT_VERSION {
        bail!("export format v{} is not v{FORMAT_VERSION}", m.v);
    }
    for (name, want) in &m.sha256 {
        let bytes = std::fs::read(dir.join(name)).with_context(|| format!("{name} named in the manifest is missing"))?;
        let got = sha_hex(&bytes);
        if &got != want {
            bail!("{name}: sha256 mismatch (export was altered)");
        }
    }
    Ok(m)
}

#[derive(Debug, Default, PartialEq)]
pub struct Imported {
    pub counts: Counts,
    pub skills_resigned: bool,
    pub skills_rejected: usize,
}

/// Import an export into `brain_dir` (which must hold SEED.HEX + KEY.PUB).
/// Refuses a non-empty target journal unless `force`. Refuses a different
/// key unless `adopt`, in which case skills are re-signed by the target key.
pub fn import(dir: &Path, brain_dir: &Path, adopt: bool, force: bool) -> Result<Imported> {
    let m = verify(dir)?;
    let key = load_key(brain_dir)?;
    let same_key = key.public_hex == m.pub_key;
    if !same_key && !adopt {
        bail!(
            "this export belongs to {}.. and the key here is {}..; pass --adopt to take it on under this key",
            &m.pub_key[..8],
            &key.public_hex[..8]
        );
    }
    let jio = JournalIO::new(brain_dir);
    let (existing, _) = jio.load();
    if existing.node_count() > 0 && !force {
        bail!("{} already holds {} memories; pass --force to replace them", brain_dir.display(), existing.node_count());
    }

    // state graph
    let mut graph = StateGraph::new();
    graph.load(&std::fs::read(dir.join("state.graph"))?);
    jio.save(&graph)?;

    // notes
    let notes = std::fs::read_to_string(dir.join("notes.txt")).unwrap_or_default();
    jio.save_notes(&notes)?;

    // skills: verified as-is under the same key; re-signed under an adopting key
    let text = std::fs::read_to_string(dir.join("skills.skl")).unwrap_or_default();
    let (store, rejected) = if same_key {
        SkillStore::from_signed_lines(&text, &key)
    } else {
        SkillStore::from_lines_unverified(&text)
    };
    store.save(brain_dir, &key)?;

    let (cal, bodies) = calibration_json(&graph);
    let _ = cal;
    Ok(Imported {
        counts: Counts {
            episodes: graph.count(NodeKind::Episode),
            semantic: graph.count(NodeKind::Semantic),
            beliefs: graph.count(NodeKind::Belief),
            audit: 0,
            skills: store.len(),
            notes_bytes: notes.len(),
            bodies_calibrated: bodies,
        },
        skills_resigned: !same_key,
        skills_rejected: rejected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::Step;
    use brainos_mind::key::to_hex;

    fn brain(seed: u8) -> (tempdir::Dir, BrainKey) {
        let d = tempdir::Dir::new(&format!("export-{seed}"));
        let key = BrainKey::from_seed([seed; 32]);
        std::fs::write(d.path().join("SEED.HEX"), format!("{}\n", to_hex(&[seed; 32]))).unwrap();
        std::fs::write(d.path().join("KEY.PUB"), format!("{}\n", key.public_hex)).unwrap();
        (d, key)
    }

    // a tiny scratch-dir helper so the tests need no extra dependency
    mod tempdir {
        pub struct Dir(std::path::PathBuf);
        impl Dir {
            pub fn new(tag: &str) -> Dir {
                let p = std::env::temp_dir().join(format!("brainos-{tag}-{}-{}", std::process::id(), nanos()));
                std::fs::create_dir_all(&p).unwrap();
                Dir(p)
            }
            pub fn path(&self) -> &std::path::Path {
                &self.0
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        fn nanos() -> u128 {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        }
    }

    fn populate(brain: &Path, key: &BrainKey) {
        let mut g = StateGraph::new();
        g.add(NodeKind::Episode, "walked 3 steps on miniblur".into(), 90, 1);
        g.add(NodeKind::Episode, "wrote a note about the dance".into(), 90, 2);
        g.add(NodeKind::Semantic, "miniblur:cal:ch6 = L1+L3 swing center 92.5 min 30.0 max 150.0 dir 1".into(), 95, 3);
        g.add(NodeKind::Semantic, "miniblur:layout = 0:L1lift;6:L1swing+L3;".into(), 95, 3);
        g.add(NodeKind::Belief, "the room is quiet".into(), 60, 4);
        JournalIO::new(brain).save(&g).unwrap();
        JournalIO::new(brain).save_notes("the greeting dance is three steps forward, then two back\n").unwrap();
        let mut st = SkillStore::new();
        assert!(st.learn(
            "do the greeting dance",
            vec![
                Step { verb: "gait.walk".into(), input: serde_json::json!({"dir":"fwd","steps":3}), body: "miniblur".into() },
                Step { verb: "gait.walk".into(), input: serde_json::json!({"dir":"back","steps":2}), body: "miniblur".into() },
            ],
            5,
            "test",
        ));
        st.save(brain, key).unwrap();
    }

    #[test]
    fn export_then_import_on_the_same_key_round_trips() {
        let (b, key) = brain(1);
        populate(b.path(), &key);
        let out = tempdir::Dir::new("out");
        let m = export(b.path(), out.path()).unwrap();
        assert_eq!(m.v, 1);
        assert_eq!(m.pub_key, key.public_hex);
        assert_eq!(m.counts.episodes, 2);
        assert_eq!(m.counts.semantic, 2);
        assert_eq!(m.counts.skills, 1);
        assert_eq!(m.counts.bodies_calibrated, vec!["miniblur".to_string()]);
        assert!(!std::fs::read_to_string(out.path().join("state.graph")).unwrap().contains(&to_hex(&[1u8; 32])), "no seed material in the export");
        assert!(!out.path().join("SEED.HEX").exists());
        verify(out.path()).unwrap();

        let cal: serde_json::Value = serde_json::from_slice(&std::fs::read(out.path().join("calibration.json")).unwrap()).unwrap();
        assert!(cal["miniblur"]["joints"][0].as_str().unwrap().starts_with("ch6 = L1+L3"));

        // a fresh key medium with the SAME key
        let (b2, _) = brain(1);
        let r = import(out.path(), b2.path(), false, false).unwrap();
        assert_eq!(r.counts.episodes, 2);
        assert_eq!(r.counts.skills, 1);
        assert!(!r.skills_resigned);
        let (g2, _) = JournalIO::new(b2.path()).load();
        assert_eq!(g2.count(NodeKind::Episode), 2);
        assert!(g2.nodes().iter().any(|n| n.content.starts_with("miniblur:cal:ch6")));
        assert_eq!(JournalIO::new(b2.path()).load_notes(), "the greeting dance is three steps forward, then two back\n");
        let st2 = SkillStore::load(b2.path(), &key);
        assert_eq!(st2.len(), 1);
        assert!(st2.find("do the greeting dance").is_some());

        // importing again onto a populated key needs --force
        assert!(import(out.path(), b2.path(), false, false).is_err());
        assert!(import(out.path(), b2.path(), false, true).is_ok());
    }

    #[test]
    fn a_different_key_needs_adopt_and_then_owns_the_skills() {
        let (b, key) = brain(2);
        populate(b.path(), &key);
        let out = tempdir::Dir::new("out2");
        export(b.path(), out.path()).unwrap();
        let (b3, key3) = brain(3);
        let e = import(out.path(), b3.path(), false, false).unwrap_err().to_string();
        assert!(e.contains("--adopt"), "{e}");
        let r = import(out.path(), b3.path(), true, false).unwrap();
        assert!(r.skills_resigned);
        assert_eq!(r.counts.skills, 1);
        // the skills now verify under the adopting key, and not under the old one
        assert_eq!(SkillStore::load(b3.path(), &key3).len(), 1);
        assert_eq!(SkillStore::load(b3.path(), &key).len(), 0);
    }

    #[test]
    fn a_tampered_export_is_refused() {
        let (b, key) = brain(4);
        populate(b.path(), &key);
        let out = tempdir::Dir::new("out4");
        export(b.path(), out.path()).unwrap();
        let mut s = std::fs::read_to_string(out.path().join("skills.skl")).unwrap();
        s.push_str("{\"planted\":true}\n");
        std::fs::write(out.path().join("skills.skl"), s).unwrap();
        assert!(verify(out.path()).unwrap_err().to_string().contains("sha256"));
    }
}
