//! BFS  LEVEL    Operator (Stage 3-4 milestones) - the seat daemon on the Linux body  [crate brainos-operator]
//! BFS  PURPOSE  Tier 2 thinking: asks a local model (llama-server or ollama) for a JSON act/chat/escalate answer within a short timeout, else hands off to tier 3
//! BFS  DEFINES  FastResponse, FastTier, build_fast_system
//! BFS  USES     this crate: config, persona, pool | external: anyhow, serde, serde-json, std
//! BFS  USED BY  -
//!
//! Tier 2: fast local model (llama-server or ollama).
//!
//! POST to llama.cpp /completion with json_schema grammar, or to
//! ollama /api/generate with format:json.  Timeout 1.5 s.
//! On timeout or absent server → escalate (return None).
//!
//! Output shape: {"intent":"act|chat|escalate","verb":"...","target":"...","reply":"..."}

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

use crate::config::Config;
use crate::persona::build_system;

/// Parsed fast-tier response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FastResponse {
    pub intent: String,     // "act" | "chat" | "escalate"
    pub verb: Option<String>,
    pub target: Option<String>,
    pub reply: String,
}

/// JSON schema for the grammar-constrained output.
const OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "intent": {"type": "string", "enum": ["act", "chat", "escalate"]},
        "verb": {"type": "string"},
        "target": {"type": "string"},
        "reply": {"type": "string"}
    },
    "required": ["intent", "reply"]
}"#;

pub struct FastTier {
    client: reqwest::Client,
    config: Config,
    pool: std::sync::Arc<crate::pool::Pool>,
}

impl FastTier {
    pub fn new(config: Config, pool: std::sync::Arc<crate::pool::Pool>) -> FastTier {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.llama.timeout_ms))
            .build()
            .expect("build reqwest client");
        FastTier { client, config, pool }
    }

    /// Pre-evaluate the cached system prefix so the first real turn is fast.
    pub async fn warmup(&self, available_verbs: &[String]) {
        if self.config.llama.backend.as_str() == "ollama" { return; }
        let system = build_fast_system(available_verbs);
        let prompt = format!("<|im_start|>system\n{system}<|im_end|>\n");
        let url = format!("{}/completion", self.config.llama.url);
        let client = reqwest::Client::builder().timeout(Duration::from_secs(60)).build().unwrap();
        let t0 = std::time::Instant::now();
        let ok = client.post(&url)
            .json(&json!({"prompt": prompt, "n_predict": 1, "cache_prompt": true, "id_slot": 0}))
            .send().await.map(|r| r.status().is_success()).unwrap_or(false);
        tracing::info!("[fast] prefix warm-up {} in {}ms", if ok {"ok"} else {"unavailable"}, t0.elapsed().as_millis());
    }

    /// Ask the fast model. Returns None to signal escalation to deliberate.
    pub async fn ask(
        &self,
        text: &str,
        available_verbs: &[String],
        history: &[serde_json::Value],
    ) -> Option<FastResponse> {
        let system = build_fast_system(available_verbs);
        // Stage 4 §5: walk the pool's fast candidates in order; a model that
        // fails is marked so the next turn starts from the healthy one.
        for (idx, spec) in self.pool.candidates("fast") {
            let t0 = std::time::Instant::now();
            let r = match spec.kind.as_str() {
                "ollama" => self.ask_ollama_at(text, &system, &spec.url, &spec.model).await,
                _ => self.ask_llama_at(text, &system, history, &spec.url).await,
            };
            match r {
                Some(resp) => { self.pool.mark_ok(idx, t0.elapsed().as_millis()); return Some(resp); }
                None => { self.pool.mark_failed(idx); }
            }
        }
        None
    }

    async fn ask_llama_at(&self, text: &str, system: &str, history: &[serde_json::Value], base: &str) -> Option<FastResponse> {
        let mut cfg = self.config.clone();
        cfg.llama.url = base.to_string();
        FastTier { client: self.client.clone(), config: cfg, pool: self.pool.clone() }.ask_llama(text, system, history).await
    }

    async fn ask_ollama_at(&self, text: &str, system: &str, base: &str, model: &str) -> Option<FastResponse> {
        let mut cfg = self.config.clone();
        cfg.llama.ollama_url = base.to_string();
        if !model.is_empty() { cfg.llama.ollama_model = model.to_string(); }
        FastTier { client: self.client.clone(), config: cfg, pool: self.pool.clone() }.ask_ollama(text, system, &[]).await
    }

    async fn ask_llama(
        &self,
        text: &str,
        system: &str,
        history: &[serde_json::Value],
    ) -> Option<FastResponse> {
        let prompt = build_llama_prompt(system, history, text);
        let url = format!("{}/completion", self.config.llama.url);
        let schema: serde_json::Value = serde_json::from_str(OUTPUT_SCHEMA).unwrap();
        let body = json!({
            "prompt": prompt,
            "n_predict": 64,
            "temperature": 0.1,
            "cache_prompt": true,
            "id_slot": 0,
            "json_schema": schema,
        });

        let t0 = std::time::Instant::now();
        let resp = match self.client.post(&url).json(&body).send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::info!("[fast] llama unavailable/timeout after {}ms: {e}", t0.elapsed().as_millis());
                return None;
            }
        };

        if !resp.status().is_success() {
            tracing::warn!("[fast] llama returned {}", resp.status());
            return None; // escalate
        }

        let v: serde_json::Value = resp.json().await.ok()?;
        let content = v["content"].as_str().unwrap_or("").trim().to_string();
        parse_fast_response(&content)
    }

    async fn ask_ollama(
        &self,
        text: &str,
        system: &str,
        _history: &[serde_json::Value],
    ) -> Option<FastResponse> {
        let url = format!("{}/api/generate", self.config.llama.ollama_url);
        let prompt = format!("{system}\n\nUser: {text}\nRespond ONLY with JSON matching the schema above.");
        let schema: serde_json::Value = serde_json::from_str(OUTPUT_SCHEMA).unwrap();
        let body = json!({
            "model": self.config.llama.ollama_model,
            "prompt": prompt,
            "format": schema,
            "stream": false,
            "options": {
                "temperature": 0.1,
                "num_predict": 256,
            }
        });

        let t0 = std::time::Instant::now();
        let resp = match self.client.post(&url).json(&body).send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::info!("[fast] llama unavailable/timeout after {}ms: {e}", t0.elapsed().as_millis());
                return None;
            }
        };

        if !resp.status().is_success() {
            tracing::warn!("[fast] ollama returned {}", resp.status());
            return None;
        }

        let v: serde_json::Value = resp.json().await.ok()?;
        let content = v["response"].as_str().unwrap_or("").trim().to_string();
        parse_fast_response(&content)
    }

    /// Check if the fast backend is reachable (used for health diagnostics).
    pub async fn ping(&self) -> bool {
        let url = match self.config.llama.backend.as_str() {
            "ollama" => format!("{}/api/tags", self.config.llama.ollama_url),
            _ => format!("{}/health", self.config.llama.url),
        };
        self.client.get(&url).send().await.map(|r| r.status().is_success()).unwrap_or(false)
    }
}

/// The fast tier's system prompt: deliberately SHORT. It is the cached
/// prefix llama-server keeps in its KV cache (`cache_prompt`), so every turn
/// only evaluates the few tokens of history + the utterance.
pub fn build_fast_system(available_verbs: &[String]) -> String {
    let verbs = available_verbs.join(", ");
    format!(
        "You are Blur, the operator of this machine (BrAInOS). Classify the human's \
         utterance. intent=act when exactly one of your verbs does the whole job \
         (fill verb + target); intent=chat for small talk or a quick factual \
         answer you are sure of (reply in one short sentence); intent=escalate \
         for anything multi-step, creative, about the screen, teaching, building \
         UI, code, or unclear. Never invent verbs. Verbs: {verbs}"
    )
}

fn build_llama_prompt(system: &str, history: &[serde_json::Value], text: &str) -> String {
    // Qwen2.5 chat template (ChatML). Keep the system block byte-stable so
    // llama-server's prompt cache hits on every turn.
    let mut p = format!("<|im_start|>system\n{system}<|im_end|>\n");
    for turn in history.iter().rev().take(4).rev() {
        let role = turn["role"].as_str().unwrap_or("user");
        let content = turn["content"].as_str().unwrap_or("");
        let content: String = content.chars().take(300).collect();
        p.push_str(&format!("<|im_start|>{role}\n{content}<|im_end|>\n"));
    }
    p.push_str(&format!(
        "<|im_start|>user\n{text}\nRespond with JSON only: {{\"intent\":\"act|chat|escalate\",\"verb\":\"verb.name or empty\",\"target\":\"...\",\"reply\":\"at most twelve words\"}}<|im_end|>\n<|im_start|>assistant\n"
    ));
    p
}

fn parse_fast_response(content: &str) -> Option<FastResponse> {
    // strip markdown code fences if present
    let stripped = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    // try direct parse
    if let Ok(r) = serde_json::from_str::<FastResponse>(stripped) {
        return Some(r);
    }

    // try to find a JSON object in the response
    if let Some(start) = stripped.find('{') {
        if let Some(end) = stripped.rfind('}') {
            if let Ok(r) = serde_json::from_str::<FastResponse>(&stripped[start..=end]) {
                return Some(r);
            }
        }
    }

    tracing::warn!("[fast] unparseable response: {content:?}");
    None // escalate
}
