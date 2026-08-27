use serde_json::Value;
use super::HandCtx;

pub async fn run(input: &Value, _ctx: &HandCtx) -> String {
    let cmd = input["cmd"].as_str().unwrap_or("").trim();
    if cmd.is_empty() { return "error: no command".into(); }
    let timeout_s = input["timeout_s"].as_u64().unwrap_or(30).min(120);
    // run via bash -c for shell features, but KIRA already gated it
    super::run_cmd(&["bash", "-c", cmd], timeout_s).await
}
