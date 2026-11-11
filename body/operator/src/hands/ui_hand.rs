//! BFS  LEVEL    Operator (Stage 3-4 milestones) - the seat daemon on the Linux body  [crate brainos-operator]
//! BFS  PURPOSE  UI verbs: message the face (on-screen UI process) to show or close an HTML panel, draw or clear a pointing overlay, or display a line of text
//! BFS  DEFINES  build, update, close_surface, teach, point, clear, say
//! BFS  USES     this crate: HandCtx, ipc | external: serde-json, std
//! BFS  USED BY  -
//!
//! UI hands — emit IPC surface/teach messages to the face.

use crate::ipc::{send, OpMsg};
use serde_json::Value;
use super::HandCtx;

pub async fn build(input: &Value, ctx: &HandCtx) -> String {
    let name = input["name"].as_str().unwrap_or("default");
    let html = input["html"].as_str().unwrap_or("");
    send(&ctx.ipc_tx, &OpMsg::Surface {
        name: name.to_string(),
        html: Some(html.to_string()),
        close: None,
    });
    format!("surface '{name}' built")
}

pub async fn update(input: &Value, ctx: &HandCtx) -> String {
    build(input, ctx).await
}

pub async fn close_surface(input: &Value, ctx: &HandCtx) -> String {
    let name = input["name"].as_str().unwrap_or("default");
    send(&ctx.ipc_tx, &OpMsg::Surface {
        name: name.to_string(),
        html: None,
        close: Some(true),
    });
    format!("surface '{name}' closed")
}

pub async fn teach(input: &Value, ctx: &HandCtx) -> String {
    let x = input["x"].as_i64().map(|v| v as i32);
    let y = input["y"].as_i64().map(|v| v as i32);
    let w = input["w"].as_i64().map(|v| v as i32);
    let h = input["h"].as_i64().map(|v| v as i32);
    let caption = input["caption"].as_str().map(|s| s.to_string());
    send(&ctx.ipc_tx, &OpMsg::Teach { x, y, w, h, caption, clear: None });
    "teach overlay set".into()
}

pub async fn point(input: &Value, ctx: &HandCtx) -> String {
    teach(input, ctx).await
}

pub async fn clear(ctx: &HandCtx) -> String {
    send(&ctx.ipc_tx, &OpMsg::Teach {
        x: None, y: None, w: None, h: None, caption: None, clear: Some(true),
    });
    "overlay cleared".into()
}

pub async fn say(input: &Value, ctx: &HandCtx) -> String {
    let text = input["text"].as_str().unwrap_or("").to_string();
    use std::sync::atomic::{AtomicU64, Ordering};
    static SAY_ID: AtomicU64 = AtomicU64::new(1000);
    let id = SAY_ID.fetch_add(1, Ordering::Relaxed);
    send(&ctx.ipc_tx, &OpMsg::Say { id, text: text.clone(), delta: false, done: true });
    format!("said: {text}")
}
