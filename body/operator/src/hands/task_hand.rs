//! BFS  LEVEL    Operator (Stage 3-4 milestones) - the seat daemon on the Linux body  [crate brainos-operator]
//! BFS  PURPOSE  Placeholder task.plan/status/cancel verbs: they only acknowledge, because the deliberate tier does the real multi-step planning itself
//! BFS  DEFINES  plan, status, cancel
//! BFS  USES     this crate: HandCtx | external: serde-json
//! BFS  USED BY  -
//!
use serde_json::Value;
use super::HandCtx;

pub async fn plan(input: &Value, _ctx: &HandCtx) -> String {
    let goal = input["goal"].as_str().unwrap_or("").trim();
    if goal.is_empty() { return "error: no goal".into(); }
    // The deliberate tier handles actual task planning inline.
    // task.plan called as a tool just acknowledges and the model then drives the loop.
    format!("task started: {goal}")
}

pub async fn status(_ctx: &HandCtx) -> String {
    "no active task".into()
}

pub async fn cancel(_ctx: &HandCtx) -> String {
    "cancel requested".into()
}
