//! Stage 4 §5 — the model pool: an array of models thinking as one mind.
//!
//! Models are stateless muscles. The operator owns the ONE history and ONE
//! state graph; a tier asks the pool for the first healthy model of its
//! role, fails over on error, and the human can always ask "which brains do
//! you have right now?" (`pool.status`).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ModelSpec {
    /// "fast" | "deliberate" | "vision"
    pub role: String,
    /// "llama" | "ollama" | "anthropic"
    pub kind: String,
    pub url: String,
    pub model: String,
}

impl Default for ModelSpec {
    fn default() -> Self {
        ModelSpec { role: "fast".into(), kind: "llama".into(), url: String::new(), model: String::new() }
    }
}

#[derive(Debug, Clone)]
struct Health {
    ok: bool,
    ms: u128,
    probed: bool,
    /// tripped by a request error this session; cleared by the next successful probe
    failed: bool,
    last: Option<Instant>,
}

pub struct Pool {
    specs: Vec<ModelSpec>,
    health: Mutex<Vec<Health>>,
    client: reqwest::Client,
}

impl Pool {
    pub fn new(specs: Vec<ModelSpec>) -> Pool {
        let n = specs.len();
        Pool {
            specs,
            health: Mutex::new(vec![Health { ok: true, ms: 0, probed: false, failed: false, last: None }; n]),
            client: reqwest::Client::builder().timeout(Duration::from_secs(3)).build().expect("pool client"),
        }
    }

    pub fn specs(&self) -> &[ModelSpec] { &self.specs }

    /// Candidates for a role, healthiest first, in config order.
    pub fn candidates(&self, role: &str) -> Vec<(usize, ModelSpec)> {
        let h = self.health.lock().unwrap();
        let mut good: Vec<(usize, ModelSpec)> = Vec::new();
        let mut bad: Vec<(usize, ModelSpec)> = Vec::new();
        for (i, s) in self.specs.iter().enumerate() {
            if s.role != role { continue; }
            if h[i].ok && !h[i].failed { good.push((i, s.clone())); } else { bad.push((i, s.clone())); }
        }
        good.extend(bad); // a "bad" model is still the last resort
        good
    }

    pub fn pick(&self, role: &str) -> Option<(usize, ModelSpec)> {
        self.candidates(role).into_iter().next()
    }

    pub fn mark_failed(&self, idx: usize) {
        if let Some(h) = self.health.lock().unwrap().get_mut(idx) { h.failed = true; }
    }

    pub fn mark_ok(&self, idx: usize, ms: u128) {
        if let Some(h) = self.health.lock().unwrap().get_mut(idx) { h.failed = false; h.ok = true; h.ms = ms; h.last = Some(Instant::now()); }
    }

    /// Probe every model once. Local servers get an HTTP health call; the
    /// API gets a TCP reach test (no tokens spent).
    pub async fn probe_all(&self) {
        for (i, s) in self.specs.iter().enumerate() {
            let t0 = Instant::now();
            let ok = match s.kind.as_str() {
                "llama" => self.client.get(format!("{}/health", s.url)).send().await.map(|r| r.status().is_success()).unwrap_or(false),
                "ollama" => self.client.get(format!("{}/api/tags", s.url)).send().await.map(|r| r.status().is_success()).unwrap_or(false),
                "anthropic" => tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect("api.anthropic.com:443")).await.map(|r| r.is_ok()).unwrap_or(false),
                _ => false,
            };
            let ms = t0.elapsed().as_millis();
            let mut h = self.health.lock().unwrap();
            h[i] = Health { ok, ms, probed: true, failed: if ok { false } else { h[i].failed }, last: Some(Instant::now()) };
            tracing::info!("[pool] {} {} {} → {} ({ms} ms)", s.role, s.kind, if s.model.is_empty() { s.url.as_str() } else { s.model.as_str() }, if ok { "ok" } else { "down" });
        }
    }

    pub fn status(&self) -> Value {
        let h = self.health.lock().unwrap();
        json!({
            "models": self.specs.iter().enumerate().map(|(i, s)| json!({
                "role": s.role, "kind": s.kind, "model": s.model, "url": s.url,
                "ok": h[i].ok && !h[i].failed, "probed": h[i].probed, "ms": h[i].ms as u64,
            })).collect::<Vec<_>>()
        })
    }

    /// One line for speech: "fast: qwen (ok) · deliberate: claude-fable-5 (ok)".
    pub fn describe(&self) -> String {
        let h = self.health.lock().unwrap();
        let mut parts = Vec::new();
        for (i, s) in self.specs.iter().enumerate() {
            let name = if s.model.is_empty() { s.kind.clone() } else { s.model.clone() };
            let state = if !h[i].probed { "unprobed" } else if h[i].ok && !h[i].failed { "ok" } else { "down" };
            parts.push(format!("{}: {name} ({state})", s.role));
        }
        if parts.is_empty() { "no models configured".into() } else { parts.join(" · ") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(role: &str, model: &str) -> ModelSpec {
        ModelSpec { role: role.into(), kind: "anthropic".into(), url: String::new(), model: model.into() }
    }

    #[test]
    fn failover_order_and_recovery() {
        let p = Pool::new(vec![spec("deliberate", "a"), spec("deliberate", "b"), spec("fast", "q")]);
        assert_eq!(p.pick("deliberate").unwrap().1.model, "a");
        p.mark_failed(0);
        assert_eq!(p.pick("deliberate").unwrap().1.model, "b");
        // both failed → still returns something (last resort), in config order
        p.mark_failed(1);
        assert_eq!(p.pick("deliberate").unwrap().1.model, "a");
        p.mark_ok(1, 5);
        assert_eq!(p.pick("deliberate").unwrap().1.model, "b");
        assert_eq!(p.pick("fast").unwrap().1.model, "q");
        assert!(p.pick("vision").is_none());
        assert!(p.describe().contains("deliberate: a (unprobed)"));
    }
}
