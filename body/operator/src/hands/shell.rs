//! BFS  LEVEL    Operator (Stage 3-4 milestones) - the seat daemon on the Linux body  [crate brainos-operator]
//! BFS  PURPOSE  shell.run verb: runs a command through bash -c with a timeout (default 30 s, max 120 s); KIRA has already approved it by this point
//! BFS  DEFINES  run
//! BFS  USES     this crate: HandCtx, run_cmd | external: serde-json
//! BFS  USED BY  -
//!
use serde_json::Value;
use super::HandCtx;

pub async fn run(input: &Value, _ctx: &HandCtx) -> String {
    let cmd = input["cmd"].as_str().unwrap_or("").trim();
    if cmd.is_empty() { return "error: no command".into(); }
    let timeout_s = input["timeout_s"].as_u64().unwrap_or(30).min(120);
    // run via bash -c for shell features, but KIRA already gated it
    super::run_cmd(&["bash", "-c", cmd], timeout_s).await
}
