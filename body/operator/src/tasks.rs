//! Stage 4 §6 — standing tasks: watchers and schedules.
//!
//! A standing task is an utterance the entity says to itself on a schedule
//! (`every`) or when a shell predicate comes true (`when`). The utterance runs
//! through the same tiers and the same KIRA gate as if the human had said it,
//! so it benefits from skills: the first run may be deliberate, every later
//! run is a reflex replay. Tasks persist in TASKS.JSON on the key and are
//! re-armed at boot; they only run while a seat is awake.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

use crate::ipc::FaceMsg;

pub const MIN_EVERY_SECS: u64 = 10;
pub const WHEN_POLL_SECS: u64 = 15;
const TICK: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Kind {
    Every { secs: u64 },
    When { cmd: String, contains: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Standing {
    pub id: u64,
    #[serde(flatten)]
    pub kind: Kind,
    pub utterance: String,
    pub active: bool,
    pub runs: u32,
    /// unix seconds
    pub created: u64,
    #[serde(default)]
    pub last_run: u64,
}

pub struct Tasks {
    inner: Mutex<Vec<Standing>>,
    next_id: Mutex<u64>,
    path: PathBuf,
    utter_tx: mpsc::Sender<FaceMsg>,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Tasks {
    pub fn load(brain_dir: &std::path::Path, utter_tx: mpsc::Sender<FaceMsg>) -> Arc<Tasks> {
        let path = brain_dir.join("TASKS.JSON");
        let list: Vec<Standing> = std::fs::read_to_string(&path).ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let next = list.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        if !list.is_empty() { tracing::info!("[tasks] re-armed {} standing task(s)", list.iter().filter(|t| t.active).count()); }
        Arc::new(Tasks { inner: Mutex::new(list), next_id: Mutex::new(next), path, utter_tx })
    }

    fn save(&self) {
        let list = self.inner.lock().unwrap().clone();
        if let Some(p) = self.path.parent() { std::fs::create_dir_all(p).ok(); }
        if let Ok(s) = serde_json::to_string_pretty(&list) {
            if let Err(e) = std::fs::write(&self.path, s) { tracing::warn!("[tasks] save: {e}"); }
        }
    }

    fn alloc(&self) -> u64 {
        let mut g = self.next_id.lock().unwrap();
        let id = *g; *g += 1; id
    }

    pub fn add_every(&self, secs: u64, utterance: &str) -> u64 {
        let id = self.alloc();
        self.inner.lock().unwrap().push(Standing {
            id, kind: Kind::Every { secs: secs.max(MIN_EVERY_SECS) }, utterance: utterance.trim().into(),
            active: true, runs: 0, created: now(), last_run: 0,
        });
        self.save();
        id
    }

    pub fn add_when(&self, cmd: &str, contains: &str, utterance: &str) -> u64 {
        let id = self.alloc();
        self.inner.lock().unwrap().push(Standing {
            id, kind: Kind::When { cmd: cmd.trim().into(), contains: contains.into() }, utterance: utterance.trim().into(),
            active: true, runs: 0, created: now(), last_run: 0,
        });
        self.save();
        id
    }

    pub fn list(&self) -> Vec<Standing> { self.inner.lock().unwrap().clone() }

    pub fn stop(&self, id: u64) -> bool {
        let removed = {
            let mut g = self.inner.lock().unwrap();
            let before = g.len();
            g.retain(|t| t.id != id);
            g.len() != before
        };
        if removed { self.save(); }
        removed
    }

    pub fn describe(&self) -> String {
        let list = self.list();
        if list.is_empty() { return "no standing tasks".into(); }
        list.iter().map(|t| {
            let how = match &t.kind {
                Kind::Every { secs } => format!("every {secs}s"),
                Kind::When { cmd, contains } => format!("when `{}` says {contains:?}", cmd.chars().take(40).collect::<String>()),
            };
            format!("#{} {} -> \"{}\" ({}, {} run{})", t.id, how, t.utterance, if t.active { "armed" } else { "done" }, t.runs, if t.runs == 1 { "" } else { "s" })
        }).collect::<Vec<_>>().join("\n")
    }

    /// The runner: one task, ticks every 5 s, fires utterances into the main loop.
    pub fn spawn_runner(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(TICK).await;
                let due: Vec<(u64, String, Option<(String, String)>)> = {
                    let g = self.inner.lock().unwrap();
                    let t = now();
                    g.iter().filter(|s| s.active).filter_map(|s| match &s.kind {
                        Kind::Every { secs } => (t.saturating_sub(s.last_run) >= *secs).then(|| (s.id, s.utterance.clone(), None)),
                        Kind::When { cmd, contains } => (t.saturating_sub(s.last_run) >= WHEN_POLL_SECS).then(|| (s.id, s.utterance.clone(), Some((cmd.clone(), contains.clone())))),
                    }).collect()
                };
                for (id, utterance, pred) in due {
                    let fire = match pred {
                        None => true,
                        Some((cmd, contains)) => {
                            let out = crate::hands::run_cmd(&["bash", "-c", &cmd], 10).await;
                            out.contains(&contains)
                        }
                    };
                    let mut changed = false;
                    {
                        let mut g = self.inner.lock().unwrap();
                        if let Some(s) = g.iter_mut().find(|s| s.id == id) {
                            s.last_run = now();
                            if fire {
                                s.runs += 1;
                                if matches!(s.kind, Kind::When { .. }) { s.active = false; }
                                changed = true;
                            }
                        }
                    }
                    if fire {
                        tracing::info!("[tasks] #{id} fires: {utterance:?}");
                        let _ = self.utter_tx.send(FaceMsg::Standing { id, text: utterance }).await;
                    }
                    if changed { self.save(); }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_list_stop_persist() {
        let dir = std::env::temp_dir().join(format!("brainos-tasks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, _rx) = mpsc::channel(4);
        let t = Tasks::load(&dir, tx.clone());
        let a = t.add_every(3, "check the build");
        let b = t.add_when("echo done", "done", "open the report");
        assert_eq!(t.list().len(), 2);
        assert_eq!(t.list()[0].kind, Kind::Every { secs: MIN_EVERY_SECS }, "floor applies");
        assert!(t.describe().contains("#1 every 10s"));
        assert!(t.stop(a));
        assert!(!t.stop(a));
        let again = Tasks::load(&dir, tx);
        assert_eq!(again.list().len(), 1);
        assert_eq!(again.list()[0].id, b);
        std::fs::remove_dir_all(&dir).ok();
    }
}
