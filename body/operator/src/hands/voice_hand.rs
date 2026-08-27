//! Voice hand — the model-callable face of `crate::voice` (record+transcribe, speak).

use serde_json::Value;
use super::HandCtx;
use crate::voice::Listen;

pub async fn listen(input: &Value, ctx: &HandCtx) -> String {
    let secs = input["seconds"].as_u64().unwrap_or(ctx.config.voice.record_seconds as u64).clamp(1, 30);
    if ctx.voice.is_recording().await {
        return "already listening (push-to-talk is held)".into();
    }
    if let Err(e) = ctx.voice.start_recording().await { return format!("error: {e}"); }
    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
    match ctx.voice.stop_and_transcribe().await {
        Listen::Heard(t) => t,
        Listen::Silence => "(heard nothing)".into(),
        Listen::Failed(e) => format!("error: {e}"),
    }
}

pub async fn speak(input: &Value, ctx: &HandCtx) -> String {
    let text = input["text"].as_str().unwrap_or("").trim();
    if text.is_empty() { return "error: no text".into(); }
    match ctx.voice.speak(text).await {
        Ok(_) => { ctx.voice.wait_spoken().await; format!("spoke: {text}") }
        Err(e) => format!("error: {e}"),
    }
}
