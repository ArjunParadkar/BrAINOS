//! BFS  LEVEL    Operator (Stage 3-4 milestones) - the seat daemon on the Linux body  [crate brainos-operator]
//! BFS  PURPOSE  Tier 3 thinking: streams a turn from the Anthropic API with a tool per available verb, and loops on tool calls until the model is done
//! BFS  DEFINES  ToolCall, TurnResult, tool_definitions, tool_definitions_for, DeliberateTier, kira_target, tool_name_to_verb, verb_to_tool_name
//! BFS  USES     this crate: config, ipc, persona, pool, sse | brainos: miniblur-core | external: anyhow, bytes, futures-util, reqwest, serde-json, std, +1 more
//! BFS  USED BY  -
//!
//! Tier 3: deliberate cognition via Fable API (claude-fable-5, streaming).
//!
//! Stream-based: returns `DelibEvent` items via a channel.
//! The caller (main.rs) handles tool dispatching and returns tool_results.

use anyhow::{bail, Result};
use bytes::Bytes;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::broadcast;

use crate::config::Config;
use crate::ipc::{send, OpMsg};
use crate::persona::build_system;
use crate::sse::{parse_event, parse_raw_events, ContentBlock, Delta, SseEvent};

/// A tool call extracted from a streaming response.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// Result of one streaming turn.
pub enum TurnResult {
    /// Model spoke and stopped (no tool use).
    Done { text: String },
    /// Model requested tool calls; caller dispatches them, then calls again.
    ToolsNeeded {
        assistant_content: Vec<Value>,
        calls: Vec<ToolCall>,
        text_so_far: String,
    },
}

pub fn tool_definitions(available_verbs: &[String]) -> Vec<Value> {
    tool_definitions_for(available_verbs, &[])
}

/// Tool list for the model. `bodies` are the Reach bodies present right now;
/// when there are any, every tool gains an optional `body` argument.
pub fn tool_definitions_for(available_verbs: &[String], bodies: &[String]) -> Vec<Value> {
    all_tool_specs()
        .into_iter()
        .filter(|t| {
            let name = t["name"].as_str().unwrap_or("");
            available_verbs.iter().any(|v| v == name)
        })
        .map(|mut t| {
            if !bodies.is_empty() {
                let desc = format!("Which body to act through: \"seat\" (this machine, default) or one of: {}.", bodies.join(", "));
                if let Some(props) = t["input_schema"]["properties"].as_object_mut() {
                    props.insert("body".into(), json!({"type":"string","description":desc}));
                }
            }
            t
        })
        .map(|mut t| {
            // Anthropic tool names must match ^[a-zA-Z0-9_-]{1,128}$; dots are not allowed.
            // We convert "fs.list" -> "fs_list" and translate back when we receive the call.
            let api_name = t["name"].as_str().unwrap_or("").replace('.', "_");
            t["name"] = serde_json::json!(api_name);
            t
        })
        .collect()
}

fn all_tool_specs() -> Vec<Value> {
    vec![
        json!({"name":"shell.run","description":"Run a shell command (30s timeout, bounded output).","input_schema":{"type":"object","properties":{"cmd":{"type":"string"},"timeout_s":{"type":"integer"}},"required":["cmd"]}}),
        json!({"name":"app.open","description":"Open a desktop application.","input_schema":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}}),
        json!({"name":"app.list","description":"List available desktop applications.","input_schema":{"type":"object","properties":{}}}),
        json!({"name":"app.close","description":"Close an application.","input_schema":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}}),
        json!({"name":"window.list","description":"List open windows.","input_schema":{"type":"object","properties":{}}}),
        json!({"name":"window.focus","description":"Focus a window.","input_schema":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}}),
        json!({"name":"window.workspace","description":"Switch workspace: 'next','prev', or number.","input_schema":{"type":"object","properties":{"target":{"type":"string"}},"required":["target"]}}),
        json!({"name":"window.fullscreen","description":"Toggle fullscreen.","input_schema":{"type":"object","properties":{}}}),
        json!({"name":"input.type","description":"Type text.","input_schema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}),
        json!({"name":"input.key","description":"Press a key combo.","input_schema":{"type":"object","properties":{"combo":{"type":"string"}},"required":["combo"]}}),
        json!({"name":"screen.look","description":"Take a screenshot and describe the screen.","input_schema":{"type":"object","properties":{"question":{"type":"string"}}}}),
        json!({"name":"web.search","description":"Search the web via DuckDuckGo.","input_schema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}),
        json!({"name":"web.open","description":"Navigate to a URL.","input_schema":{"type":"object","properties":{"url":{"type":"string"}},"required":["url"]}}),
        json!({"name":"web.read","description":"Read the current browser page as text.","input_schema":{"type":"object","properties":{}}}),
        json!({"name":"fs.list","description":"List files in a directory.","input_schema":{"type":"object","properties":{"path":{"type":"string"}}}}),
        json!({"name":"fs.read","description":"Read a file.","input_schema":{"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer"}},"required":["path"]}}),
        json!({"name":"fs.write","description":"Write a file.","input_schema":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}}),
        json!({"name":"fs.mkdir","description":"Create a directory.","input_schema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}),
        json!({"name":"fs.delete","description":"Delete a file or empty directory.","input_schema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}),
        json!({"name":"fs.stat","description":"Get file metadata.","input_schema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}),
        json!({"name":"fs.search","description":"Search files by name or content.","input_schema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}),
        json!({"name":"sys.volume","description":"Control volume: up/down/mute/N%.","input_schema":{"type":"object","properties":{"action":{"type":"string"}},"required":["action"]}}),
        json!({"name":"sys.brightness","description":"Control brightness: up/down/N%.","input_schema":{"type":"object","properties":{"action":{"type":"string"}},"required":["action"]}}),
        json!({"name":"sys.wifi","description":"WiFi control: on/off/status.","input_schema":{"type":"object","properties":{"action":{"type":"string"}},"required":["action"]}}),
        json!({"name":"sys.battery","description":"Get battery status.","input_schema":{"type":"object","properties":{}}}),
        json!({"name":"sys.lock","description":"Lock the screen.","input_schema":{"type":"object","properties":{}}}),
        json!({"name":"sys.notify","description":"Show a desktop notification.","input_schema":{"type":"object","properties":{"title":{"type":"string"},"body":{"type":"string"}},"required":["title","body"]}}),
        json!({"name":"ui.build","description":"Build a named face surface with HTML.","input_schema":{"type":"object","properties":{"name":{"type":"string"},"html":{"type":"string"}},"required":["name","html"]}}),
        json!({"name":"ui.say","description":"Speak text via TTS.","input_schema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}),
        json!({"name":"notes.write","description":"Write a note to the private notebook.","input_schema":{"type":"object","properties":{"content":{"type":"string"}},"required":["content"]}}),
        json!({"name":"notes.read","description":"Read the private notebook.","input_schema":{"type":"object","properties":{"topic":{"type":"string"}}}}),
        json!({"name":"task.plan","description":"Start a multi-step task.","input_schema":{"type":"object","properties":{"goal":{"type":"string"}},"required":["goal"]}}),
        json!({"name":"code.run","description":"Run code (python3/bash/node).","input_schema":{"type":"object","properties":{"lang":{"type":"string"},"src":{"type":"string"}},"required":["lang","src"]}}),
        json!({"name":"voice.listen","description":"Record and transcribe speech.","input_schema":{"type":"object","properties":{"seconds":{"type":"integer"}}}}),
        json!({"name":"voice.speak","description":"Speak text via piper TTS.","input_schema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}),
        // Stage 4: the entity's own proprioception and standing orders
        json!({"name":"body.list","description":"List the bodies you are in right now (this seat plus any Reach devices) and their hands.","input_schema":{"type":"object","properties":{}}}),
        json!({"name":"body.forget","description":"Release a Reach body; its hands leave your body map.","input_schema":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}}),
        json!({"name":"skill.list","description":"List the plans you have compiled into muscle memory.","input_schema":{"type":"object","properties":{}}}),
        json!({"name":"skill.forget","description":"Drop a compiled skill: the last one used, or by its utterance.","input_schema":{"type":"object","properties":{"utterance":{"type":"string"}}}}),
        json!({"name":"task.every","description":"Standing order: say `utterance` to yourself every N seconds (min 10). Use for 'every morning', 'every 20 minutes', 'keep checking'.","input_schema":{"type":"object","properties":{"seconds":{"type":"integer"},"utterance":{"type":"string"}},"required":["seconds","utterance"]}}),
        json!({"name":"task.when","description":"Watcher: run shell `cmd` every 15 s; when its output contains `contains`, do `utterance` once. Use for 'when the build finishes', 'tell me when X'.","input_schema":{"type":"object","properties":{"cmd":{"type":"string"},"contains":{"type":"string"},"utterance":{"type":"string"}},"required":["cmd","contains","utterance"]}}),
        json!({"name":"task.list","description":"List standing tasks and watchers.","input_schema":{"type":"object","properties":{}}}),
        json!({"name":"task.stop","description":"Stop a standing task by id.","input_schema":{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}}),
        json!({"name":"pool.status","description":"Which models (brains) you have right now and whether they are up.","input_schema":{"type":"object","properties":{}}}),
        // Phase 0 — a walking body (Mini-Blur). Only offered when such a body is reached; every call is KIRA-gated
        // against the body's own envelope (angle, tip-over, rail, link, battery) before it is sent.
        json!({"name":"gait.stand","description":"Stand up on all six legs (a walking body).","input_schema":{"type":"object","properties":{"body":{"type":"string"}}}}),
        json!({"name":"gait.sit","description":"Sit down and unload the servos (a walking body).","input_schema":{"type":"object","properties":{"body":{"type":"string"}}}}),
        json!({"name":"gait.stop","description":"Stop any motion at once and stand (a walking body). Always allowed.","input_schema":{"type":"object","properties":{"body":{"type":"string"}}}}),
        json!({"name":"gait.walk","description":"Walk a number of steps in a direction. dir: fwd|back|left|right (left/right turn in place). steps 1-20. speed 0.1-1.0 (default 0.5). The body refuses steps its envelope, balance, power or link cannot carry.","input_schema":{"type":"object","properties":{"dir":{"type":"string"},"steps":{"type":"integer"},"speed":{"type":"number"},"body":{"type":"string"}},"required":["dir","steps"]}}),
        json!({"name":"body.telemetry","description":"Battery, rail current, mode, link age and pose of a walking body.","input_schema":{"type":"object","properties":{"body":{"type":"string"}}}}),
        json!({"name":"body.schema","description":"The typed capability schema a walking body published (joints, envelope, limits).","input_schema":{"type":"object","properties":{"body":{"type":"string"}}}}),
        json!({"name":"body.calibrate","description":"Calibration of a walking body: mode 'begin' allows raw servo moves and refuses gaits; mode 'end' returns to standing; or a channel record {ch, center, min, max, dir} which is remembered in your own memory, never on the body.","input_schema":{"type":"object","properties":{"mode":{"type":"string"},"ch":{"type":"integer"},"center":{"type":"number"},"min":{"type":"number"},"max":{"type":"number"},"dir":{"type":"integer"},"body":{"type":"string"}}}}),
        json!({"name":"servo.set","description":"Move one servo channel to an absolute angle (calibration mode only; refused while a gait runs).","input_schema":{"type":"object","properties":{"ch":{"type":"integer"},"deg":{"type":"number"},"body":{"type":"string"}},"required":["ch","deg"]}}),
    ]
}

pub struct DeliberateTier {
    client: reqwest::Client,
    config: Config,
}

impl DeliberateTier {
    pub fn new(config: Config) -> DeliberateTier {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(300))
            .build()
            .expect("build deliberate client");
        DeliberateTier { client, config }
    }

    /// Stream one API turn. Emits text deltas as `say` IPC messages.
    /// Returns Done or ToolsNeeded.
    pub async fn stream_turn(
        &self,
        system: &str,
        messages: &[Value],
        tools: &[Value],
        api_key: &str,
        ipc_tx: &broadcast::Sender<String>,
        say_id: u64,
        model: &str,
        release: &mut Option<tokio::sync::watch::Receiver<bool>>,
    ) -> Result<TurnResult> {
        // Stage 4 §4: while `release` is Some(false) this turn is SPECULATIVE —
        // text is buffered instead of shown, so a fast-tier answer can still
        // win without the human ever seeing two voices.
        let mut held: String = String::new();
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_str(api_key)?);
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert("anthropic-beta", HeaderValue::from_static("server-side-fallback-2026-07-01"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let body = json!({
            "model": if model.is_empty() { self.config.anthropic.model.as_str() } else { model },
            "max_tokens": self.config.anthropic.max_tokens,
            "fallbacks": "default",
            "stream": true,
            "output_config": {"effort": self.config.anthropic.effort},
            "system": [{"type":"text","text":system,"cache_control":{"type":"ephemeral"}}],
            "tools": tools,
            "messages": messages,
        });

        let resp = self.client
            .post("https://api.anthropic.com/v1/messages")
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("connect: {e}"))?;

        let status = resp.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            tokio::time::sleep(Duration::from_secs(2)).await;
            bail!("API {status} — retry");
        }
        if !status.is_success() {
            let b = resp.text().await.unwrap_or_default();
            bail!("API {status}: {b}");
        }

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        let mut text_blocks: HashMap<usize, String> = HashMap::new();
        let mut tool_blocks: HashMap<usize, (String, String, String)> = HashMap::new();
        let mut assistant_blocks: Vec<Value> = Vec::new();
        let mut stop_reason: Option<String> = None;

        while let Some(chunk) = stream.next().await {
            let chunk: Bytes = chunk.map_err(|e| anyhow::anyhow!("stream: {e}"))?;
            buf.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buf.find("\n\n") {
                let ev = buf[..pos + 2].to_string();
                buf.drain(..pos + 2);

                for (etype, data) in parse_raw_events(&ev) {
                    match parse_event(&etype, &data) {
                        Ok(SseEvent::ContentBlockStart { index, content_block }) => {
                            match &content_block {
                                ContentBlock::Text { .. } => {
                                    text_blocks.insert(index, String::new());
                                    assistant_blocks.push(json!({"type":"text","text":""}));
                                }
                                ContentBlock::ToolUse { id, name } => {
                                    tool_blocks.insert(index, (id.clone(), name.clone(), String::new()));
                                    assistant_blocks.push(json!({"type":"tool_use","id":id,"name":name,"input":{}}));
                                }
                            }
                        }
                        Ok(SseEvent::ContentBlockDelta { index, delta }) => {
                            match delta {
                                Delta::TextDelta { text: t } => {
                                    if let Some(acc) = text_blocks.get_mut(&index) { acc.push_str(&t); }
                                    if let Some(rx) = release.as_ref() {
                                        if !*rx.borrow() { held.push_str(&t); continue; }
                                        if !held.is_empty() { send(ipc_tx, &OpMsg::Say { id: say_id, text: std::mem::take(&mut held), delta: true, done: false }); }
                                    }
                                    send(ipc_tx, &OpMsg::Say { id: say_id, text: t, delta: true, done: false });
                                }
                                Delta::InputJsonDelta { partial_json } => {
                                    if let Some((_, _, pj)) = tool_blocks.get_mut(&index) { pj.push_str(&partial_json); }
                                }
                            }
                        }
                        Ok(SseEvent::ContentBlockStop { index }) => {
                            if let Some((id, _, pj)) = tool_blocks.get(&index) {
                                let input: Value = serde_json::from_str(pj).unwrap_or(json!({}));
                                for b in assistant_blocks.iter_mut() {
                                    if b["type"] == "tool_use" && b["id"] == id.as_str() {
                                        b["input"] = input.clone();
                                    }
                                }
                            }
                        }
                        Ok(SseEvent::MessageDelta { delta, .. }) => { stop_reason = delta.stop_reason; }
                        Ok(SseEvent::Error { error }) => bail!("API error: {error}"),
                        _ => {}
                    }
                }
            }
        }

        // a held turn waits here for the verdict; released text flushes in order
        if let Some(rx) = release.as_mut() {
            if !*rx.borrow() {
                let _ = rx.wait_for(|v| *v).await;
            }
            if !held.is_empty() { send(ipc_tx, &OpMsg::Say { id: say_id, text: std::mem::take(&mut held), delta: true, done: false }); }
            *release = None; // later iterations of this turn are live
        }
        // emit done
        send(ipc_tx, &OpMsg::Say { id: say_id, text: String::new(), delta: false, done: true });

        let full_text: String = text_blocks.values().cloned().collect::<Vec<_>>().join("");

        if stop_reason.as_deref() == Some("tool_use") && !tool_blocks.is_empty() {
            // update text in assistant_blocks
            for b in assistant_blocks.iter_mut() {
                if b["type"] == "text" {
                    b["text"] = json!(&full_text);
                }
            }
            let calls: Vec<ToolCall> = tool_blocks.values()
                .map(|(id, name, pj)| ToolCall {
                    id: id.clone(),
                    name: tool_name_to_verb(name),  // "fs_list" -> "fs.list"
                    input: serde_json::from_str(pj).unwrap_or(json!({})),
                })
                .collect();
            Ok(TurnResult::ToolsNeeded {
                assistant_content: assistant_blocks,
                calls,
                text_so_far: full_text,
            })
        } else {
            Ok(TurnResult::Done { text: full_text })
        }
    }

    /// One deliberate turn over a snapshot of the conversation. Returns the
    /// reply text and the list of hands that ran (verb, input, is_error), in
    /// order — the raw material for skill compilation.
    pub async fn run_loop(
        &self,
        user_text: &str,
        api_key: &str,
        available_verbs: &[String],
        bodies: &[String],
        memories: &[String],
        extra_system: &str,
        prior: Vec<Value>,
        ipc_tx: &broadcast::Sender<String>,
        say_id: u64,
        pool: &crate::pool::Pool,
        tool_exec: &mut dyn FnMut(&str, &Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = (String, bool)> + Send>>,
        cancel_rx: &tokio::sync::watch::Receiver<bool>,
        mut release: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> Result<(String, Vec<(String, Value, bool)>)> {
        let mut system = build_system(available_verbs, memories);
        if !bodies.is_empty() {
            system.push_str(&format!("\nYou are present in more than one body right now. Besides this seat you can act through: {}. Pass body=\"<name>\" on any tool to use that device's hand; omit it for this machine. Use body.list if unsure.\n", bodies.join(", ")));
        }
        if !extra_system.is_empty() { system.push_str(extra_system); }
        let tools = tool_definitions_for(available_verbs, bodies);

        let mut messages = prior;
        messages.push(json!({"role":"user","content":user_text}));
        let mut full_text = String::new();
        let mut ran: Vec<(String, Value, bool)> = Vec::new();

        for _iteration in 0..10 {
            if *cancel_rx.borrow() { bail!("cancelled"); }

            // Stage 4 §5: first healthy deliberate model; fail over on a request error.
            let mut turn: Option<TurnResult> = None;
            let mut last_err: Option<anyhow::Error> = None;
            let cands = pool.candidates("deliberate");
            if cands.is_empty() { bail!("no deliberate model in the pool"); }
            for (idx, spec) in cands {
                let t0 = std::time::Instant::now();
                match self.stream_turn(&system, &messages, &tools, api_key, ipc_tx, say_id, &spec.model, &mut release).await {
                    Ok(t) => { pool.mark_ok(idx, t0.elapsed().as_millis()); turn = Some(t); break; }
                    Err(e) => {
                        let msg = e.to_string();
                        if msg == "cancelled" { return Err(e); }
                        tracing::warn!("[deliberate] {} failed: {msg} — trying next model", spec.model);
                        pool.mark_failed(idx);
                        last_err = Some(e);
                    }
                }
            }
            let Some(turn) = turn else { return Err(last_err.unwrap_or_else(|| anyhow::anyhow!("all deliberate models failed"))); };

            match turn {
                TurnResult::Done { text } => {
                    full_text = text;
                    break;
                }
                TurnResult::ToolsNeeded { assistant_content, calls, text_so_far } => {
                    full_text = text_so_far;
                    messages.push(json!({"role":"assistant","content":assistant_content}));

                    let mut results = Vec::new();
                    for call in &calls {
                        if *cancel_rx.borrow() { bail!("cancelled"); }
                        let fut = tool_exec(&call.name, &call.input);
                        let (result, is_error) = fut.await;
                        ran.push((call.name.clone(), call.input.clone(), is_error));
                        results.push(json!({
                            "type": "tool_result",
                            "tool_use_id": call.id,
                            "content": result,
                            "is_error": is_error,
                        }));
                    }
                    messages.push(json!({"role":"user","content":results}));
                }
            }
        }

        Ok((full_text, ran))
    }
}

/// Extract the primary target string from a tool call input (for IPC act message).
pub fn kira_target(name: &str, input: &Value) -> String {
    // walking-body verbs use the string form the body re-checks on its side
    if name == "gait.walk" || name == "servo.set" {
        return miniblur_core::runtime::target_of(name, &input.to_string());
    }
    if name.starts_with("gait.") || name.starts_with("body.") {
        return String::new();
    }
    match name {
        "shell.run" | "code.run" => input.get("cmd").or(input.get("src")),
        "fs.list" | "fs.read" | "fs.write" | "fs.mkdir" | "fs.delete" | "fs.stat" => input.get("path"),
        "web.search" | "fs.search" => input.get("query"),
        "web.open" => input.get("url"),
        _ => input.as_object().and_then(|m| m.values().find(|v| v.is_string())),
    }
    .and_then(|v| v.as_str())
    .unwrap_or("")
    .to_string()
}

/// Convert API tool name (underscores) back to verb (dots): "fs_list" → "fs.list".
/// Replaces the first underscore with a dot — works for all brainos verb namespaces.
pub fn tool_name_to_verb(api_name: &str) -> String {
    if let Some(pos) = api_name.find('_') {
        let mut s = api_name.to_string();
        s.replace_range(pos..pos+1, ".");
        s
    } else {
        api_name.to_string()
    }
}

/// Convert verb (dots) to API tool name (underscores): "fs.list" → "fs_list".
pub fn verb_to_tool_name(verb: &str) -> String {
    verb.replace('.', "_")
}
