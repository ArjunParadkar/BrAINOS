//! BFS  LEVEL    Operator (Stage 3-4 milestones) - the seat daemon on the Linux body  [crate brainos-operator]
//! BFS  PURPOSE  Saves the exact steps of a successful turn as a key-signed skill tied to the utterance, so the same request later replays without a model
//! BFS  DEFINES  NEVER_COMPILE, Step, Skill, SkillStore, Match, normalize, describe
//! BFS  USES     this crate: reflex | brainos: mind | external: serde, serde-json, std
//! BFS  USED BY  operator/export, operator/hands/entity_hand, operator/hands/mod
//!
//! Stage 4 §3 — procedural compilation.
//!
//! A deliberate (or fast) turn whose every hand succeeded is compiled into a
//! Skill: the normalized utterance plus the exact sequence of (verb, input,
//! body) steps. The next time the human says the same thing, the skill
//! replays in the reflex tier with no model in the path. Every step is
//! KIRA-gated again at replay — the store is memory, not authority.
//!
//! Persistence: skills live in their own two-slot journal (SKL_A / SKL_B) on
//! the key, and EVERY record is signed by the BrAIn Key. The mind crate
//! deliberately refuses to rehydrate `skill` nodes from the episodic journal
//! (a tampered journal could plant a pathway); here a record whose signature
//! does not verify against the entity's own key is dropped at load, so a
//! planted skill is not the entity's and never replays.

use brainos_mind::{journal, key::BrainKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Verbs that are never compiled: destructive, interactive, or self-referential.
pub const NEVER_COMPILE: &[&str] = &[
    "sys.power", "fs.delete", "fs.move", "voice.listen", "voice.speak",
    "task.plan", "task.status", "task.cancel", "task.every", "task.when",
    "task.list", "task.stop", "skill.list", "skill.forget", "body.forget",
    "ui.build", "ui.update", "ui.close", "ui.teach", "ui.point", "ui.clear",
    "screen.look", "web.read",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Step {
    pub verb: String,
    pub input: Value,
    /// "seat" = the machine the operator runs on; otherwise a Reach body name
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Skill {
    /// normalized utterance (see `normalize`)
    pub key: String,
    pub steps: Vec<Step>,
    pub confidence: u8,
    pub tick: u64,
    pub uses: u32,
    /// which tier produced it ("deliberate" | "fast")
    pub origin: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SignedRecord {
    skill: Skill,
    /// hex ed25519 signature over the canonical JSON of `skill`
    sig: String,
}

#[derive(Debug, Default)]
pub struct SkillStore {
    skills: Vec<Skill>,
    /// index of the skill most recently replayed or learned — "forget that"
    last: Option<usize>,
}

/// How the store matched an utterance.
#[derive(Debug, Clone, PartialEq)]
pub enum Match {
    Exact(usize),
    /// token-set overlap above threshold with identical slot tokens
    Near(usize, f32),
}

impl Match {
    pub fn index(&self) -> usize {
        match self { Match::Exact(i) | Match::Near(i, _) => *i }
    }
}

const NEAR_THRESHOLD: f32 = 0.80;
const CONFIDENCE_NEW: u8 = 60;
const CONFIDENCE_DROP: u8 = 20;

/// Lowercase, whitespace-collapsed, trailing punctuation stripped, "please" removed.
pub fn normalize(text: &str) -> String {
    crate::reflex::normalize_utterance(text).to_lowercase()
}

/// A slot token carries a specific referent (number, path, URL, quoted text,
/// dotted name). Two utterances with different slots are different plans.
fn is_slot(tok: &str) -> bool {
    tok.chars().any(|c| c.is_ascii_digit())
        || tok.contains('/')
        || tok.contains('.')
        || tok.contains(':')
        || tok.contains('@')
        || tok.starts_with('"')
        || tok.starts_with('\'')
}

fn tokens(s: &str) -> BTreeSet<String> {
    s.split_whitespace().map(|t| t.trim_matches(|c: char| matches!(c, ',' | ';' | '!' | '?')).to_string()).filter(|t| !t.is_empty()).collect()
}

fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f32 {
    if a.is_empty() && b.is_empty() { return 1.0; }
    let inter = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    inter / union
}

impl SkillStore {
    pub fn new() -> SkillStore { SkillStore::default() }

    pub fn len(&self) -> usize { self.skills.len() }
    pub fn is_empty(&self) -> bool { self.skills.is_empty() }
    pub fn get(&self, i: usize) -> Option<&Skill> { self.skills.get(i) }
    pub fn all(&self) -> &[Skill] { &self.skills }

    /// Can this step sequence be compiled at all?
    pub fn compilable(steps: &[Step]) -> bool {
        !steps.is_empty() && steps.iter().all(|s| !NEVER_COMPILE.contains(&s.verb.as_str()))
    }

    /// Learn (or refresh) a skill for `utterance`. Returns true if new.
    pub fn learn(&mut self, utterance: &str, steps: Vec<Step>, tick: u64, origin: &str) -> bool {
        let key = normalize(utterance);
        if key.is_empty() || !Self::compilable(&steps) { return false; }
        if let Some(i) = self.skills.iter().position(|s| s.key == key) {
            let s = &mut self.skills[i];
            s.steps = steps;
            s.tick = tick;
            s.confidence = s.confidence.saturating_add(5).min(100);
            self.last = Some(i);
            return false;
        }
        self.skills.push(Skill { key, steps, confidence: CONFIDENCE_NEW, tick, uses: 0, origin: origin.into() });
        self.last = Some(self.skills.len() - 1);
        true
    }

    /// Find a skill for the utterance: exact key first, then a near match
    /// whose slot tokens are identical.
    pub fn find(&self, utterance: &str) -> Option<Match> {
        let key = normalize(utterance);
        if key.is_empty() { return None; }
        if let Some(i) = self.skills.iter().position(|s| s.key == key) {
            return Some(Match::Exact(i));
        }
        let qt = tokens(&key);
        let q_slots: BTreeSet<&String> = qt.iter().filter(|t| is_slot(t)).collect();
        let mut best: Option<(usize, f32)> = None;
        for (i, s) in self.skills.iter().enumerate() {
            let st = tokens(&s.key);
            let s_slots: BTreeSet<&String> = st.iter().filter(|t| is_slot(t)).collect();
            if q_slots != s_slots { continue; }
            let j = jaccard(&qt, &st);
            if j >= NEAR_THRESHOLD && best.map_or(true, |(_, b)| j > b) {
                best = Some((i, j));
            }
        }
        best.map(|(i, j)| Match::Near(i, j))
    }

    /// A replay succeeded end to end.
    pub fn reinforce(&mut self, i: usize) {
        if let Some(s) = self.skills.get_mut(i) {
            s.confidence = s.confidence.saturating_add(5).min(100);
            s.uses += 1;
            self.last = Some(i);
        }
    }

    /// A replay step failed or was denied. Returns true if the skill was dropped.
    pub fn penalize(&mut self, i: usize) -> bool {
        let drop = match self.skills.get_mut(i) {
            Some(s) => { s.confidence = s.confidence.saturating_sub(20); s.confidence <= CONFIDENCE_DROP }
            None => false,
        };
        if drop { self.skills.remove(i); self.last = None; }
        drop
    }

    /// "forget that": drop the most recently used/learned skill.
    pub fn forget_last(&mut self) -> Option<Skill> {
        let i = self.last.take()?;
        if i < self.skills.len() { Some(self.skills.remove(i)) } else { None }
    }

    /// "forget how to <utterance>"
    pub fn forget(&mut self, utterance: &str) -> Option<Skill> {
        let m = self.find(utterance)?;
        let i = m.index();
        self.last = None;
        Some(self.skills.remove(i))
    }

    pub fn last_index(&self) -> Option<usize> { self.last }
    pub fn set_last(&mut self, i: usize) { self.last = Some(i); }

    // ---- persistence: signed, two-slot, on the key ----

    fn slot_paths(dir: &Path) -> (PathBuf, PathBuf) {
        (dir.join("SKL_A"), dir.join("SKL_B"))
    }

    fn canonical(skill: &Skill) -> Vec<u8> {
        // serde_json preserves struct field order → deterministic bytes
        serde_json::to_vec(skill).unwrap_or_default()
    }

    /// Serialize every skill as a signed JSON line.
    pub fn to_signed_lines(&self, key: &BrainKey) -> String {
        let mut out = String::new();
        for s in &self.skills {
            let sig = key.sign(&Self::canonical(s));
            let rec = SignedRecord { skill: s.clone(), sig: brainos_mind::key::to_hex(&sig) };
            if let Ok(line) = serde_json::to_string(&rec) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        out
    }

    /// Load signed lines; records that fail verification are dropped and counted.
    /// Returns (loaded, rejected).
    pub fn from_signed_lines(text: &str, key: &BrainKey) -> (SkillStore, usize) {
        let mut store = SkillStore::new();
        let mut rejected = 0usize;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() { continue; }
            let Ok(rec) = serde_json::from_str::<SignedRecord>(line) else { rejected += 1; continue; };
            let Some(sig) = parse_sig(&rec.sig) else { rejected += 1; continue; };
            if !key.verify(&Self::canonical(&rec.skill), &sig) { rejected += 1; continue; }
            if !Self::compilable(&rec.skill.steps) { rejected += 1; continue; }
            store.skills.push(rec.skill);
        }
        (store, rejected)
    }

    /// Import only (entity export v1.0, `adopt`): read signed lines WITHOUT
    /// verifying them, so skills signed by a previous key can be re-signed
    /// under the key that adopts them. Never used at boot — a skill that
    /// does not verify under the entity's own key does not load.
    pub fn from_lines_unverified(text: &str) -> (SkillStore, usize) {
        let mut store = SkillStore::new();
        let mut rejected = 0usize;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() { continue; }
            let Ok(rec) = serde_json::from_str::<SignedRecord>(line) else { rejected += 1; continue; };
            if !Self::compilable(&rec.skill.steps) { rejected += 1; continue; }
            store.skills.push(rec.skill);
        }
        (store, rejected)
    }

    /// The newest signed-lines payload on disk, verbatim (for export).
    pub fn raw_lines(dir: &Path) -> String {
        let (pa, pb) = Self::slot_paths(dir);
        let raw_a = std::fs::read(&pa).unwrap_or_default();
        let raw_b = std::fs::read(&pb).unwrap_or_default();
        match journal::newest(&raw_a, &raw_b) {
            Some((_, payload)) => String::from_utf8_lossy(payload).into_owned(),
            None => String::new(),
        }
    }

    pub fn save(&self, dir: &Path, key: &BrainKey) -> anyhow::Result<()> {
        std::fs::create_dir_all(dir)?;
        let (pa, pb) = Self::slot_paths(dir);
        let raw_a = std::fs::read(&pa).unwrap_or_default();
        let raw_b = std::fs::read(&pb).unwrap_or_default();
        let gen_a = journal::open(&raw_a).map(|(g, _)| g);
        let gen_b = journal::open(&raw_b).map(|(g, _)| g);
        let (slot, gen) = journal::plan_write(gen_a, gen_b);
        let payload = self.to_signed_lines(key);
        let record = journal::seal(gen, payload.as_bytes());
        let path = match slot { journal::Slot::A => pa, journal::Slot::B => pb };
        std::fs::write(&path, &record)?;
        tracing::debug!("[skills] saved {} skills gen={gen} to {}", self.skills.len(), path.display());
        Ok(())
    }

    pub fn load(dir: &Path, key: &BrainKey) -> SkillStore {
        let (pa, pb) = Self::slot_paths(dir);
        let raw_a = std::fs::read(&pa).unwrap_or_default();
        let raw_b = std::fs::read(&pb).unwrap_or_default();
        match journal::newest(&raw_a, &raw_b) {
            Some((gen, payload)) => {
                let text = String::from_utf8_lossy(payload);
                let (store, rejected) = Self::from_signed_lines(&text, key);
                if rejected > 0 {
                    tracing::warn!("[skills] {rejected} skill record(s) failed signature/shape check and were dropped");
                }
                tracing::info!("[skills] loaded {} skills (gen={gen})", store.len());
                store
            }
            None => SkillStore::new(),
        }
    }
}

fn parse_sig(hex: &str) -> Option<[u8; 64]> {
    if hex.len() != 128 { return None; }
    let mut out = [0u8; 64];
    for i in 0..64 {
        out[i] = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// Short human-readable description of a skill for the face / `skill.list`.
pub fn describe(s: &Skill) -> String {
    let verbs: Vec<String> = s.steps.iter().map(|st| {
        if st.body == "seat" { st.verb.clone() } else { format!("{}@{}", st.verb, st.body) }
    }).collect();
    format!("\"{}\" -> {} (confidence {}, used {}x)", s.key, verbs.join(" > "), s.confidence, s.uses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn step(verb: &str, path: &str) -> Step {
        Step { verb: verb.into(), input: json!({"path": path}), body: "seat".into() }
    }
    fn key() -> BrainKey { BrainKey::from_seed([7u8; 32]) }

    #[test]
    fn exact_and_near_match_with_slot_guard() {
        let mut st = SkillStore::new();
        assert!(st.learn("list the files in /tmp/x", vec![step("fs.list", "/tmp/x")], 1, "deliberate"));
        assert_eq!(st.find("List the files in /tmp/x."), Some(Match::Exact(0)));
        // near: one extra function word, same slot
        match st.find("list all the files in /tmp/x") { Some(Match::Near(0, j)) => assert!(j >= 0.80), m => panic!("{m:?}") }
        // different slot → never replays the other plan
        assert_eq!(st.find("list the files in /tmp/y"), None);
        // number slots count too
        assert!(st.learn("set volume to 40", vec![Step { verb: "sys.volume".into(), input: json!({"action":"40"}), body: "seat".into() }], 2, "fast"));
        assert_eq!(st.find("set volume to 60"), None);
        assert_eq!(st.find("set volume to 40"), Some(Match::Exact(1)));
    }

    #[test]
    fn never_compile_and_confidence_lifecycle() {
        let mut st = SkillStore::new();
        assert!(!st.learn("shut down", vec![Step { verb: "sys.power".into(), input: json!({}), body: "seat".into() }], 1, "fast"));
        assert!(st.is_empty());
        assert!(st.learn("do x", vec![step("fs.list", "/a")], 1, "deliberate"));
        assert_eq!(st.get(0).unwrap().confidence, 60);
        st.reinforce(0);
        assert_eq!(st.get(0).unwrap().confidence, 65);
        assert_eq!(st.get(0).unwrap().uses, 1);
        assert!(!st.penalize(0)); // 45
        assert!(!st.penalize(0)); // 25
        assert!(st.penalize(0), "≤20 drops the skill"); // 5
        assert!(st.is_empty());
    }

    #[test]
    fn forget_last_and_by_utterance() {
        let mut st = SkillStore::new();
        st.learn("a b c", vec![step("fs.list", "/a")], 1, "deliberate");
        st.learn("d e f", vec![step("fs.list", "/d")], 2, "deliberate");
        assert_eq!(st.forget_last().unwrap().key, "d e f");
        assert!(st.forget("zzz").is_none());
        assert_eq!(st.forget("a b c").unwrap().key, "a b c");
        assert!(st.is_empty());
    }

    #[test]
    fn signed_roundtrip_rejects_tampered_and_foreign_records() {
        let mut st = SkillStore::new();
        st.learn("list /tmp/x", vec![step("fs.list", "/tmp/x")], 1, "deliberate");
        let k = key();
        let text = st.to_signed_lines(&k);
        let (back, rej) = SkillStore::from_signed_lines(&text, &k);
        assert_eq!(rej, 0);
        assert_eq!(back.all(), st.all());

        // tampered: change the verb inside the signed payload
        let tampered = text.replace("fs.list", "shell.run");
        let (back, rej) = SkillStore::from_signed_lines(&tampered, &k);
        assert_eq!(rej, 1);
        assert!(back.is_empty());

        // foreign: signed by another entity's key
        let other = BrainKey::from_seed([9u8; 32]);
        let foreign = st.to_signed_lines(&other);
        let (back, rej) = SkillStore::from_signed_lines(&foreign, &k);
        assert_eq!(rej, 1);
        assert!(back.is_empty());
    }

    #[test]
    fn two_slot_persistence_on_disk() {
        let dir = std::env::temp_dir().join(format!("brainos-skills-{}", std::process::id()));
        let k = key();
        let mut st = SkillStore::new();
        st.learn("list /tmp/x", vec![step("fs.list", "/tmp/x")], 1, "deliberate");
        st.save(&dir, &k).unwrap();
        st.learn("list /tmp/z", vec![step("fs.list", "/tmp/z")], 2, "deliberate");
        st.save(&dir, &k).unwrap();
        let back = SkillStore::load(&dir, &k);
        assert_eq!(back.len(), 2, "newest generation wins");
        std::fs::remove_dir_all(&dir).ok();
    }
}
