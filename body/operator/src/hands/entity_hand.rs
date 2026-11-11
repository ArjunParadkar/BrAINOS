//! BFS  LEVEL    Operator (Stage 3-4 milestones) - the seat daemon on the Linux body  [crate brainos-operator]
//! BFS  PURPOSE  Verbs for inspecting the entity itself: list/forget remote bodies and learned skills, add/list/stop standing tasks, show model pool status
//! BFS  DEFINES  body_list, body_forget, skill_list, skill_forget, task_every, task_when, task_list, task_stop, pool_status
//! BFS  USES     this crate: HandCtx, skills, tasks | external: serde-json
//! BFS  USED BY  -
//!
//! Stage 4 — proprioception and control of the entity itself:
//! its bodies (Reach), its learned skills, its standing tasks, its brains.

use serde_json::Value;
use super::HandCtx;
use crate::skills;
use crate::tasks;

pub async fn body_list(ctx: &HandCtx) -> String {
    let bodies = ctx.reach.list();
    let bound: Vec<(String, String)> = {
        let g = ctx.instance.lock().unwrap();
        g.kira.bound_regions().into_iter().map(|(id, d)| (id.to_string(), d)).collect()
    };
    let mut lines = vec![format!("seat: this machine ({} hands)", ctx.local_verbs.len())];
    for b in bodies {
        lines.push(format!("{}: {} device, {} hands [{}], {} ms, here for {}s",
            b.name, b.os, b.hands.len(), b.hands.join(" "), b.rtt_ms, b.since.elapsed().as_secs()));
        if let Some((_, d)) = bound.iter().find(|(id, _)| *id == format!("reach/{}", b.name)) {
            lines.push(format!("  bound by: {d}"));
        }
    }
    lines.join("\n")
}

pub async fn body_forget(input: &Value, ctx: &HandCtx) -> String {
    let name = input["name"].as_str().or(input.as_str()).unwrap_or("").trim();
    if name.is_empty() { return "error: which body?".into(); }
    if ctx.reach.forget(name) { format!("released '{name}' — its hands are gone from my body") } else { format!("no body named '{name}'") }
}

pub async fn skill_list(ctx: &HandCtx) -> String {
    let st = ctx.skills.lock().unwrap();
    if st.is_empty() { return "no compiled skills yet — the first time i think something through and it works, i keep the plan".into(); }
    st.all().iter().map(skills::describe).collect::<Vec<_>>().join("\n")
}

pub async fn skill_forget(input: &Value, ctx: &HandCtx) -> String {
    let what = input["utterance"].as_str().or(input.as_str()).unwrap_or("").trim().to_string();
    let dropped = {
        let mut st = ctx.skills.lock().unwrap();
        if what.is_empty() || what == "that" || what == "last" { st.forget_last() } else { st.forget(&what) }
    };
    match dropped {
        Some(s) => {
            let g = ctx.instance.lock().unwrap();
            let _ = ctx.skills.lock().unwrap().save(&ctx.brain_dir, &g.key);
            format!("forgotten: \"{}\" — i'll think it through next time", s.key)
        }
        None => "nothing to forget".into(),
    }
}

pub async fn task_every(input: &Value, ctx: &HandCtx) -> String {
    let secs = input["seconds"].as_u64().or(input["secs"].as_u64()).unwrap_or(0);
    let utt = input["utterance"].as_str().unwrap_or("").trim();
    if secs == 0 || utt.is_empty() { return "error: need seconds and an utterance".into(); }
    let id = ctx.tasks.add_every(secs, utt);
    format!("standing task #{id}: every {}s i'll do \"{utt}\"", secs.max(tasks::MIN_EVERY_SECS))
}

pub async fn task_when(input: &Value, ctx: &HandCtx) -> String {
    let cmd = input["cmd"].as_str().unwrap_or("").trim();
    let contains = input["contains"].as_str().unwrap_or("").trim();
    let utt = input["utterance"].as_str().unwrap_or("").trim();
    if cmd.is_empty() || contains.is_empty() || utt.is_empty() { return "error: need cmd, contains and an utterance".into(); }
    let id = ctx.tasks.add_when(cmd, contains, utt);
    format!("watcher #{id}: every {}s i'll run `{cmd}`; when it says {contains:?} i'll do \"{utt}\" once", tasks::WHEN_POLL_SECS)
}

pub async fn task_list(ctx: &HandCtx) -> String { ctx.tasks.describe() }

pub async fn task_stop(input: &Value, ctx: &HandCtx) -> String {
    let id = input["id"].as_u64().or_else(|| input.as_str().and_then(|s| s.trim_start_matches('#').parse().ok())).unwrap_or(0);
    if ctx.tasks.stop(id) { format!("stopped task #{id}") } else { format!("no task #{id}") }
}

pub async fn pool_status(ctx: &HandCtx) -> String { ctx.pool.describe() }
