//! IPC contract between operator and face (§4).
//!
//! Unix socket at $XDG_RUNTIME_DIR/brainos/operator.sock.
//! Newline-delimited JSON in both directions.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

// ---- operator → face -------------------------------------------------------

/// All messages the operator sends to face clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum OpMsg {
    State { mode: String, tier: String },
    Say { id: u64, text: String, delta: bool, done: bool },
    Act { verb: String, target: String, status: String, digest: String,
          #[serde(skip_serializing_if = "Option::is_none")] tier: Option<String>,
          #[serde(skip_serializing_if = "Option::is_none")] body: Option<String> },
    /// Stage 4: a Reach body joined or left the entity.
    Body { name: String, hands: Vec<String>, os: String, joined: bool, rtt_ms: u64,
           /// "legs" for a walking body with a bound policy, "device" otherwise
           #[serde(default = "default_body_class")] class: String },
    /// Stage 4: the model pool's health.
    Pool { models: Value },
    Surface { name: String, #[serde(skip_serializing_if = "Option::is_none")] html: Option<String>, #[serde(skip_serializing_if = "Option::is_none")] close: Option<bool> },
    Teach { #[serde(skip_serializing_if = "Option::is_none")] x: Option<i32>, #[serde(skip_serializing_if = "Option::is_none")] y: Option<i32>, #[serde(skip_serializing_if = "Option::is_none")] w: Option<i32>, #[serde(skip_serializing_if = "Option::is_none")] h: Option<i32>, #[serde(skip_serializing_if = "Option::is_none")] caption: Option<String>, #[serde(skip_serializing_if = "Option::is_none")] clear: Option<bool> },
    Task { id: u64, title: String, steps: Vec<TaskStep>, done: bool },
    /// What the ears made of the last push-to-talk (the human's own line).
    Heard { text: String },
    NeedKey,
}

fn default_body_class() -> String { "device".into() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStep {
    pub title: String,
    pub status: String, // "pending" | "running" | "done" | "error"
}

// ---- face → operator -------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum FaceMsg {
    Utter { text: String },
    Ptt { on: bool },
    Cancel,
    SurfaceEvent { name: String, event: String, data: Value },
    SetKey { key: String },
    /// Stage 4: a standing task fires its utterance (internal; a face may send it too).
    Standing { id: u64, text: String },
}

// ---- server ----------------------------------------------------------------

pub struct IpcServer {
    listener: UnixListener,
    /// broadcast channel: operator → all connected faces
    pub tx: broadcast::Sender<String>,
}

impl IpcServer {
    pub fn bind(sock_path: &Path) -> Result<IpcServer> {
        if sock_path.exists() {
            let _ = std::fs::remove_file(sock_path);
        }
        if let Some(parent) = sock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let listener = UnixListener::bind(sock_path)?;
        let (tx, _) = broadcast::channel(256);
        tracing::info!("[ipc] listening on {}", sock_path.display());
        Ok(IpcServer { listener, tx })
    }

    /// Accept connections forever, spawning a task per client.
    pub async fn run(
        self,
        face_tx: tokio::sync::mpsc::Sender<FaceMsg>,
    ) {
        let listener = self.listener;
        let broadcast_tx = self.tx;
        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let btx = broadcast_tx.clone();
                    let ftx = face_tx.clone();
                    tokio::spawn(handle_client(stream, btx, ftx));
                }
                Err(e) => {
                    tracing::error!("[ipc] accept error: {e}");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    }
}

async fn handle_client(
    stream: UnixStream,
    broadcast_tx: broadcast::Sender<String>,
    face_tx: tokio::sync::mpsc::Sender<FaceMsg>,
) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let mut rx = broadcast_tx.subscribe();

    // pump incoming lines to face_tx
    let face_tx2 = face_tx.clone();
    let read_task = tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim().to_string();
            if line.is_empty() { continue; }
            match serde_json::from_str::<FaceMsg>(&line) {
                Ok(msg) => {
                    let _ = face_tx2.send(msg).await;
                }
                Err(e) => {
                    tracing::warn!("[ipc] bad face message: {e}: {line}");
                }
            }
        }
    });

    // pump broadcast channel to writer
    let write_task = tokio::spawn(async move {
        while let Ok(msg) = rx.recv().await {
            let line = msg + "\n";
            if writer.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    });

    let _ = tokio::join!(read_task, write_task);
    tracing::debug!("[ipc] client disconnected");
}

/// Convenience: serialize and broadcast an operator message.
pub fn send(tx: &broadcast::Sender<String>, msg: &OpMsg) {
    match serde_json::to_string(msg) {
        Ok(s) => { let _ = tx.send(s); }
        Err(e) => tracing::error!("[ipc] serialize error: {e}"),
    }
}

/// Convenience: broadcast a state update.
pub fn send_state(tx: &broadcast::Sender<String>, mode: &str, tier: &str) {
    send(tx, &OpMsg::State { mode: mode.into(), tier: tier.into() });
}
