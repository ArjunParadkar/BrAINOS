//! brainos-operator — Stage 3 production body, Stage 4 entity.
//!
//! Socket path: $BRAINOS_SOCK > config > $XDG_RUNTIME_DIR/brainos/operator.sock
//!
//! One utterance, in order:
//!   skill replay (compiled plan, no model)  →  reflex (regex, no model)
//!   →  fast (local model) ∥ deliberate (API, HELD)  →  whichever wins
//! and a deliberate turn whose every hand succeeded is compiled into a skill.

use anyhow::Result;
use serde_json::Value;
use brainos_mind::{body::Region, instance::Instance, key::BrainKey, state::NodeKind};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc, watch};
use tracing_subscriber::EnvFilter;

mod body_registry;
mod config;
mod deliberate;
mod fast;
mod hands;
mod hexapod;
mod history;
mod ipc;
mod journal_io;
mod key_reader;
mod kira_gate;
mod persona;
mod pool;
mod reach;
mod reflex;
mod skills;
mod sse;
mod tasks;
mod voice;

use config::Config;
use ipc::{FaceMsg, IpcServer, OpMsg};
use kira_gate::SharedInstance;
use skills::{SkillStore, Step};

/// Everything a turn needs. Built once; cheap to clone (all Arcs).
#[derive(Clone)]
struct Ctx {
    config: Arc<Config>,
    api_key: Option<String>,
    instance: SharedInstance,
    history: Arc<Mutex<history::History>>,
    fast: Arc<fast::FastTier>,
    deliberate: Arc<deliberate::DeliberateTier>,
    hands: Arc<hands::HandCtx>,
    ipc_tx: broadcast::Sender<String>,
    cancel_rx: watch::Receiver<bool>,
    reach: Arc<reach::ReachHub>,
    skills: Arc<Mutex<SkillStore>>,
    pool: Arc<pool::Pool>,
    brain_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("brainos_operator=info".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<String> = std::env::args().collect();
    let config_path = args.windows(2)
        .find(|w| w[0] == "--config")
        .map(|w| w[1].as_str());

    let config = Arc::new(Config::load(config_path)?);
    tracing::info!("[operator] starting (model={})", config.anthropic.model);

    let api_key: Option<String> = key_reader::load_api_key(
        &config.anthropic.key_device,
        &config.anthropic.key_file,
    );
    if api_key.is_none() {
        tracing::warn!("[operator] no API key — reflex + fast tiers only");
    }

    let brain_dir = config.brain_dir();
    tracing::info!("[operator] brain_dir={}", brain_dir.display());

    let brain_key = load_brain_key(&brain_dir);
    tracing::info!("[key] fingerprint {}", brain_key.fingerprint());

    let (body_map, local_verbs) = body_registry::build(&brain_dir);
    tracing::info!("[operator] {} verbs registered", local_verbs.len());

    let journal = journal_io::JournalIO::new(&brain_dir);
    let (graph, inherited) = journal.load();
    tracing::info!("[operator] {} inherited episodes", inherited);

    let mut instance = Instance::new(brain_key);
    instance.state = graph;
    for r in body_map.regions() {
        instance.body.incorporate(Region {
            id: r.id.clone(),
            class: r.class,
            capabilities: r.capabilities.clone(),
            proprioception: r.proprioception.clone(),
        });
    }
    // Stage 4 §3: compiled skills, signed by this key — foreign or tampered records are dropped
    let skill_store = SkillStore::load(&brain_dir, &instance.key);
    let skills_shared = Arc::new(Mutex::new(skill_store));
    let shared: SharedInstance = Arc::new(Mutex::new(instance));

    // socket path: BRAINOS_SOCK > config > XDG default
    let sock_path: PathBuf = std::env::var("BRAINOS_SOCK")
        .map(PathBuf::from)
        .unwrap_or_else(|_| config.socket_path());

    let server = IpcServer::bind(&sock_path)?;
    let ipc_tx = server.tx.clone();
    let (face_tx, mut face_rx) = mpsc::channel::<FaceMsg>(256);
    tokio::spawn(server.run(face_tx.clone()));

    // Stage 4 §5: the model pool
    let pool = Arc::new(pool::Pool::new(config.pool_specs()));
    {
        let p = pool.clone();
        let tx = ipc_tx.clone();
        tokio::spawn(async move {
            loop {
                p.probe_all().await;
                ipc::send(&tx, &OpMsg::Pool { models: p.status()["models"].clone() });
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        });
    }

    let fast_tier = Arc::new(fast::FastTier::new((*config).clone(), pool.clone()));
    let deliberate_tier = Arc::new(deliberate::DeliberateTier::new((*config).clone()));
    {
        // warm the local model's cached prefix in the background
        let ft = fast_tier.clone();
        let verbs: Vec<String> = local_verbs.clone();
        tokio::spawn(async move { ft.warmup(&verbs).await; });
    }
    let history = Arc::new(Mutex::new(history::History::new()));
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let cancel_tx = Arc::new(cancel_tx);

    let scratch_dir = PathBuf::from(&config.task.scratchpad);
    std::fs::create_dir_all(&scratch_dir).ok();

    let voice = Arc::new(voice::Voice::new(config.clone()));
    tracing::info!("[voice] ears={} mouth={}", voice.has_ears(), voice.has_mouth());

    // Stage 4 §2: Reach — other bodies
    let reach_hub = reach::ReachHub::new();
    let reach_listen = std::env::var("BRAINOS_REACH").ok().unwrap_or_else(|| config.reach.listen.clone());
    if !reach_listen.is_empty() {
        let hub = reach_hub.clone();
        let inst = shared.clone();
        let tx = ipc_tx.clone();
        let addr = reach_listen.clone();
        tokio::spawn(async move {
            if let Err(e) = reach::listen(addr, hub, inst, tx).await { tracing::warn!("[reach] listener: {e}"); }
        });
        if config.reach.beacon_port > 0 {
            let port: u16 = reach_listen.rsplit(':').next().and_then(|p| p.parse().ok()).unwrap_or(7777);
            tokio::spawn(reach::beacon(config.reach.beacon_port, config.reach.seat_name.clone(), port));
        }
    } else {
        tracing::info!("[reach] off (set [reach] listen or BRAINOS_REACH=0.0.0.0:7777)");
    }

    // Stage 4 §6: standing tasks
    let tasks = tasks::Tasks::load(&brain_dir, face_tx.clone());
    tasks.clone().spawn_runner();

    let hand_ctx = Arc::new(hands::HandCtx {
        config: config.clone(),
        instance: shared.clone(),
        ipc_tx: ipc_tx.clone(),
        brain_dir: brain_dir.clone(),
        scratch_dir: scratch_dir.clone(),
        voice: voice.clone(),
        reach: reach_hub.clone(),
        skills: skills_shared.clone(),
        tasks: tasks.clone(),
        pool: pool.clone(),
        local_verbs: local_verbs.clone(),
    });

    // SIGTERM: save journal + skills
    {
        let shared2 = shared.clone();
        let skills2 = skills_shared.clone();
        let bd2 = brain_dir.clone();
        tokio::spawn(async move {
            let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            tracing::info!("[operator] shutting down — saving journal");
            let j = journal_io::JournalIO::new(&bd2);
            let g = shared2.lock().unwrap();
            let _ = j.save(&g.state);
            let _ = skills2.lock().unwrap().save(&bd2, &g.key);
            drop(g);
            std::process::exit(0);
        });
    }

    let ctx = Ctx {
        config: config.clone(),
        api_key,
        instance: shared.clone(),
        history,
        fast: fast_tier,
        deliberate: deliberate_tier,
        hands: hand_ctx,
        ipc_tx: ipc_tx.clone(),
        cancel_rx,
        reach: reach_hub,
        skills: skills_shared,
        pool,
        brain_dir: brain_dir.clone(),
    };

    ipc::send_state(&ipc_tx, "idle", "reflex");
    tracing::info!("[operator] ready on {}", sock_path.display());

    let mut say_id: u64 = 1;
    let mut last_save = std::time::Instant::now();

    loop {
        // periodic journal flush
        if last_save.elapsed().as_secs() > 300 {
            let g = shared.lock().unwrap();
            let _ = journal_io::JournalIO::new(&brain_dir).save(&g.state);
            let _ = ctx.skills.lock().unwrap().save(&brain_dir, &g.key);
            drop(g);
            last_save = std::time::Instant::now();
        }

        let Some(msg) = face_rx.recv().await else { break; };

        // (text, spoken): what to run through the tiers this iteration, and whether
        // it arrived by voice (then the reply goes back out by voice too).
        let mut turn: Option<(String, bool)> = None;

        match msg {
            FaceMsg::Utter { text } => {
                let text = text.trim().to_string();
                if text.is_empty() { continue; }
                turn = Some((text, false));
            }

            FaceMsg::Standing { id, text } => {
                let text = text.trim().to_string();
                if text.is_empty() { continue; }
                tracing::info!("[tasks] #{id} → {text:?}");
                ipc::send(&ipc_tx, &OpMsg::Heard { text: format!("[standing task #{id}] {text}") });
                turn = Some((text, false));
            }

            FaceMsg::Ptt { on: true } => {
                if !voice.has_ears() {
                    ipc::send(&ipc_tx, &OpMsg::Say {
                        id: say_id, text: "I have no ears on this body (no recorder or whisper model).".into(),
                        delta: false, done: true,
                    });
                    continue;
                }
                voice.stop_speaking().await;
                match voice.start_recording().await {
                    Ok(()) => ipc::send_state(&ipc_tx, "listening", "reflex"),
                    Err(e) => {
                        tracing::warn!("[voice] {e}");
                        ipc::send(&ipc_tx, &OpMsg::Say { id: say_id, text: format!("my ears failed: {e}"), delta: false, done: true });
                    }
                }
            }

            FaceMsg::Ptt { on: false } => {
                if !voice.is_recording().await {
                    ipc::send_state(&ipc_tx, "idle", "reflex");
                    continue;
                }
                ipc::send_state(&ipc_tx, "thinking", "reflex");
                let t0 = std::time::Instant::now();
                match voice.stop_and_transcribe().await {
                    voice::Listen::Heard(text) => {
                        tracing::info!("[voice] heard {:?} in {} ms", text, t0.elapsed().as_millis());
                        ipc::send(&ipc_tx, &OpMsg::Heard { text: text.clone() });
                        turn = Some((text, true));
                    }
                    voice::Listen::Silence => {
                        tracing::info!("[voice] silence ({} ms)", t0.elapsed().as_millis());
                        ipc::send_state(&ipc_tx, "idle", "reflex");
                    }
                    voice::Listen::Failed(e) => {
                        tracing::warn!("[voice] {e}");
                        ipc::send(&ipc_tx, &OpMsg::Say { id: say_id, text: format!("my ears failed: {e}"), delta: false, done: true });
                        ipc::send_state(&ipc_tx, "idle", "reflex");
                    }
                }
            }

            FaceMsg::Cancel => {
                tracing::info!("[operator] cancel");
                let _ = cancel_tx.send(true);
                voice.abort_recording().await;
                voice.stop_speaking().await;
                ipc::send_state(&ipc_tx, "idle", "reflex");
            }

            FaceMsg::SetKey { key } => {
                let device = &config.anthropic.key_device;
                if !device.is_empty() && std::path::Path::new(device).exists() {
                    match key_reader::write_api_key_to_p3(std::path::Path::new(device), &key) {
                        Ok(_) => {
                            ipc::send(&ipc_tx, &OpMsg::Say {
                                id: say_id,
                                text: "key saved.".into(),
                                delta: false,
                                done: true,
                            });
                        }
                        Err(e) => tracing::warn!("[operator] set_key failed: {e}"),
                    }
                }
            }

            FaceMsg::SurfaceEvent { name, event, data } => {
                tracing::debug!("[ipc] surface_event {name}/{event}: {data}");
            }
        }

        if let Some((text, spoken)) = turn {
            tracing::info!("[utter] {:?}{}", text, if spoken { " (voice)" } else { "" });
            say_id += 1;

            // voice in -> voice out: gather this turn's `say` off the broadcast as it
            // streams, so the mouth gets the whole reply once the tiers are done.
            let capture = if spoken && config.voice.speak_replies && voice.has_mouth() {
                let buf = Arc::new(Mutex::new(String::new()));
                Some((buf.clone(), tokio::spawn(collect_say(ipc_tx.subscribe(), say_id, buf))))
            } else { None };

            // reset cancel
            let _ = cancel_tx.send(false);

            handle_utter(&text, say_id, &ctx).await;

            // record episode
            {
                let mut g = shared.lock().unwrap();
                let tick = g.tick;
                g.state.add(
                    NodeKind::Episode,
                    format!("utterance: {}", text.chars().take(80).collect::<String>()),
                    80,
                    tick,
                );
            }

            if let Some((buf, task)) = capture {
                // let the final `say` land on the broadcast, then stop collecting
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                task.abort();
                let reply = buf.lock().unwrap().clone();
                if !reply.trim().is_empty() && !*ctx.cancel_rx.borrow() {
                    match voice.speak(&reply).await {
                        Ok(_) => tracing::info!("[voice] speaking {} chars", reply.len()),
                        Err(e) => tracing::warn!("[voice] mouth failed: {e}"),
                    }
                }
            }
        }
    }

    Ok(())
}

/// Accumulate the text of every `say` for `say_id` into `buf` until aborted.
/// Deltas append; whole messages (reflex/fast send one non-delta `say`) are
/// joined with a space.
async fn collect_say(mut rx: broadcast::Receiver<String>, say_id: u64, buf: Arc<Mutex<String>>) {
    loop {
        match rx.recv().await {
            Ok(line) => {
                if let Ok(OpMsg::Say { id, text: t, delta, .. }) = serde_json::from_str::<OpMsg>(&line) {
                    if id != say_id || t.is_empty() { continue; }
                    let mut b = buf.lock().unwrap();
                    if !delta && !b.is_empty() { b.push(' '); }
                    b.push_str(&t);
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// The verbs the whole entity has right now: the seat's plus every Reach body's.
fn current_verbs(instance: &SharedInstance) -> Vec<String> {
    let g = instance.lock().unwrap();
    let mut v: Vec<String> = Vec::new();
    for r in g.body.regions() {
        for c in &r.capabilities { if !v.contains(c) { v.push(c.clone()); } }
    }
    v
}

fn say(ctx: &Ctx, say_id: u64, text: impl Into<String>) {
    ipc::send(&ctx.ipc_tx, &OpMsg::Say { id: say_id, text: text.into(), delta: false, done: true });
}

fn act(ctx: &Ctx, verb: &str, target: &str, status: &str, digest: &str, tier: &str, body: &str) {
    ipc::send(&ctx.ipc_tx, &OpMsg::Act {
        verb: verb.into(), target: target.into(), status: status.into(),
        digest: digest.chars().take(100).collect(),
        tier: Some(tier.into()),
        body: if body == reach::SEAT { None } else { Some(body.into()) },
    });
}

fn looks_failed(result: &str) -> bool {
    let r = result.trim_start().to_ascii_lowercase();
    r.starts_with("error") || r.starts_with("exec error") || r.starts_with("timeout") || r.starts_with("unknown verb") || r.starts_with("denied")
}

fn with_body(mut input: Value, body: &str) -> Value {
    if body != reach::SEAT {
        if let Some(m) = input.as_object_mut() { m.insert("body".into(), Value::String(body.into())); }
        else { input = serde_json::json!({"target": input, "body": body}); }
    }
    input
}

fn save_skills(ctx: &Ctx) {
    let g = ctx.instance.lock().unwrap();
    if let Err(e) = ctx.skills.lock().unwrap().save(&ctx.brain_dir, &g.key) { tracing::warn!("[skills] save: {e}"); }
}

/// Gate + run one hand. Returns (result, ok).
async fn run_hand(ctx: &Ctx, verb: &str, input: &Value, tier: &str) -> (String, bool) {
    let body = hands::body_of(input);
    let target = deliberate::kira_target(verb, input);
    match kira_gate::gate_via(&ctx.instance, &body, verb, &target) {
        Ok(_) => {
            let result = hands::dispatch(verb, input, &ctx.hands).await;
            let ok = !looks_failed(&result);
            act(ctx, verb, &target, if ok { "ok" } else { "err" }, &result, tier, &body);
            (result, ok)
        }
        Err(denial) => {
            let msg = format!("denied by KIRA: {denial}");
            act(ctx, verb, &target, "deny", &msg, tier, &body);
            (msg, false)
        }
    }
}

/// Stage 4 §3.4 — replay a compiled skill. Every step is gated again.
async fn replay_skill(ctx: &Ctx, text: &str, m: skills::Match, say_id: u64, t0: std::time::Instant) {
    let (steps, key) = {
        let st = ctx.skills.lock().unwrap();
        let s = st.get(m.index()).unwrap();
        (s.steps.clone(), s.key.clone())
    };
    ipc::send_state(&ctx.ipc_tx, "acting", "skill");
    let n = steps.len();
    let mut last = String::new();
    for (i, step) in steps.iter().enumerate() {
        if *ctx.cancel_rx.borrow() { break; }
        let input = with_body(step.input.clone(), &step.body);
        let (result, ok) = run_hand(ctx, &step.verb, &input, "skill").await;
        if !ok {
            let dropped = { let mut st = ctx.skills.lock().unwrap(); st.penalize(m.index()) };
            save_skills(ctx);
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            tracing::warn!("[tier=skill {ms:.1}ms] step {}/{n} {} failed: {}", i + 1, step.verb, result.chars().take(80).collect::<String>());
            say(ctx, say_id, format!(
                "that stopped working at step {} of {} ({}): {}{}",
                i + 1, n, step.verb, result.chars().take(120).collect::<String>(),
                if dropped { " — i've dropped that skill and will think it through next time." } else { "" }
            ));
            ipc::send_state(&ctx.ipc_tx, "idle", "skill");
            return;
        }
        last = result;
    }
    { let mut st = ctx.skills.lock().unwrap(); st.reinforce(m.index()); }
    save_skills(ctx);
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let how = match m { skills::Match::Exact(_) => "exact".to_string(), skills::Match::Near(_, j) => format!("near {j:.2}") };
    tracing::info!("[tier=skill {ms:.1}ms] {text:?} ~ {key:?} ({how}), {n} step(s) ok");
    let reply = if n == 1 {
        format_reply(&steps[0].verb, &deliberate::kira_target(&steps[0].verb, &steps[0].input), &last)
    } else {
        format!("done — {n} steps. {}", last.chars().take(160).collect::<String>())
    };
    say(ctx, say_id, reply);
    ipc::send_state(&ctx.ipc_tx, "idle", "skill");
}

async fn handle_utter(raw_text: &str, say_id: u64, ctx: &Ctx) {
    let t0 = std::time::Instant::now();
    let ipc_tx = &ctx.ipc_tx;

    // ── Tier 0: SKILL REPLAY (compiled plan, no model) ─────────────────────
    ipc::send_state(ipc_tx, "thinking", "reflex");
    let hit = { ctx.skills.lock().unwrap().find(raw_text) };
    if let Some(m) = hit {
        replay_skill(ctx, raw_text, m, say_id, t0).await;
        return;
    }

    // Stage 4 §2.4: "on the pi, ..." routes the whole turn to a body
    let names = ctx.reach.names();
    let (text, route) = reach::split_body(raw_text, &names);
    let text = text.as_str();
    let route_body = route.clone().unwrap_or_else(|| reach::SEAT.to_string());

    // ── Tier 1: REFLEX ──────────────────────────────────────────────────────
    if let Some(intent) = reflex::match_intent(text) {
        if reflex::needs_confirm(&intent, text) {
            let what = match intent.target.as_str() { "sleep" => "sleep", "shutdown" => "shut down", _ => "reboot" };
            say(ctx, say_id, format!("confirm: {what}? say '{what} now' to confirm."));
            ipc::send_state(ipc_tx, "idle", "reflex");
            return;
        }
        let verb = intent.verb.clone();
        let target = intent.target.clone();
        let input = with_body(verb_to_input(&verb, &target), &route_body);
        ipc::send_state(ipc_tx, "acting", "reflex");
        let (result, ok) = run_hand(ctx, &verb, &input, "reflex").await;
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        if ok {
            tracing::info!("[tier=reflex {ms:.1}ms] {text} -> {verb} {target} @{route_body} ok");
            say(ctx, say_id, format_reply(&verb, &target, &result));
        } else {
            tracing::warn!("[tier=reflex {ms:.1}ms] {verb} failed: {result}");
            say(ctx, say_id, if result.starts_with("denied") { format!("i can't do that: {result}") } else { result.chars().take(200).collect() });
        }
        ipc::send_state(ipc_tx, "idle", "reflex");
        return;
    }

    let available_verbs = current_verbs(&ctx.instance);
    let bodies = names.clone();
    let want_deliberate = ctx.api_key.is_some() && persona::should_escalate(text);
    let speculate = ctx.config.tiers.speculate && ctx.api_key.is_some() && !want_deliberate;

    // routing hint for the deliberate tier
    let extra_system = match &route {
        Some(b) if b != reach::SEAT => format!("\nThe human addressed the body '{b}' for this request: pass body=\"{b}\" on your tool calls.\n"),
        _ => String::new(),
    };

    // ── Tier 2: FAST (∥ HELD DELIBERATE when speculating) ──────────────────
    // A deliberate future that is started now but shows nothing and runs no
    // hand until released. If the fast tier wins, it is simply dropped.
    let (rel_tx, rel_rx) = watch::channel(false);
    let memories = { ctx.instance.lock().unwrap().state.durable_digest(8, 4) };
    let prior = ctx.history.lock().unwrap().as_json();
    let ctx_d = ctx.clone();
    let text_d = text.to_string();
    let verbs_d = available_verbs.clone();
    let bodies_d = bodies.clone();
    let extra_d = extra_system.clone();
    let key_d = ctx.api_key.clone().unwrap_or_default();
    let delib = async move {
        let mut tool_exec = |name: &str, input: &Value| -> std::pin::Pin<Box<dyn std::future::Future<Output = (String, bool)> + Send>> {
            let name = name.to_string();
            let input = input.clone();
            let c = ctx_d.clone();
            Box::pin(async move {
                let (r, ok) = run_hand(&c, &name, &input, "deliberate").await;
                (r, !ok)
            })
        };
        let release = if speculate { Some(rel_rx) } else { None };
        ctx_d.deliberate.run_loop(
            &text_d, &key_d, &verbs_d, &bodies_d, &memories, &extra_d, prior,
            &ctx_d.ipc_tx, say_id, &ctx_d.pool, &mut tool_exec, &ctx_d.cancel_rx, release,
        ).await
    };
    tokio::pin!(delib);
    let mut delib_result: Option<anyhow::Result<(String, Vec<(String, Value, bool)>)>> = None;

    if !want_deliberate {
        ipc::send_state(ipc_tx, "thinking", "fast");
        let hist_json = ctx.history.lock().unwrap().as_json();
        let ask = ctx.fast.ask(text, &available_verbs, &hist_json);
        tokio::pin!(ask);
        let budget = tokio::time::sleep(std::time::Duration::from_millis(ctx.config.tiers.fast_budget_ms.max(100)));
        tokio::pin!(budget);
        let fast_result = loop {
            tokio::select! {
                r = &mut ask => break r,
                r = &mut delib, if speculate && delib_result.is_none() => { delib_result = Some(r); }
                _ = &mut budget => {
                    if speculate {
                        // the fast tier missed its budget: the held stream takes over
                        tracing::info!("[tier=fast] over budget ({} ms) → releasing deliberate", ctx.config.tiers.fast_budget_ms);
                        break None;
                    } else {
                        say(ctx, say_id, "one sec.");
                        break ask.await;
                    }
                }
            }
        };
        match fast_result {
            // an "act" with no usable verb is the small model guessing: never
            // speak a guess as if it were an action — escalate instead.
            Some(resp) if resp.intent != "escalate"
                && !(resp.intent == "act" && resp.verb.as_deref().map(|v| v.is_empty() || !available_verbs.iter().any(|a| a == v)).unwrap_or(true)) => {
                let ms = t0.elapsed().as_millis();
                tracing::info!("[tier=fast {ms}ms] intent={} verb={:?}{}", resp.intent, resp.verb, if speculate { " (deliberate dropped)" } else { "" });
                say(ctx, say_id, resp.reply.clone());
                if resp.intent == "act" {
                    if let (Some(verb), Some(target)) = (&resp.verb, &resp.target) {
                        let input = with_body(verb_to_input(verb, target), &route_body);
                        let (_r, ok) = run_hand(ctx, verb, &input, "fast").await;
                        if ok {
                            let step = Step { verb: verb.clone(), input: verb_to_input(verb, target), body: route_body.clone() };
                            let tick = ctx.instance.lock().unwrap().tick;
                            let new = ctx.skills.lock().unwrap().learn(raw_text, vec![step], tick, "fast");
                            if new { tracing::info!("[skills] compiled from fast: {raw_text:?}"); save_skills(ctx); }
                        }
                    }
                }
                {
                    let mut h = ctx.history.lock().unwrap();
                    h.push(history::Turn::user(text));
                    h.push(history::Turn::assistant(&resp.reply));
                }
                ipc::send_state(ipc_tx, "idle", "fast");
                return; // `delib` is dropped here → the speculative request is cancelled
            }
            _ => tracing::info!("[tier=fast] timeout/escalate → deliberate"),
        }
    }

    // ── Tier 3: DELIBERATE ──────────────────────────────────────────────────
    if ctx.api_key.is_none() {
        say(ctx, say_id, "i need an API key for that — the reflex and fast tiers are still here.");
        ipc::send_state(ipc_tx, "idle", "reflex");
        return;
    }
    ipc::send_state(ipc_tx, "thinking", "deliberate");

    if speculate {
        let _ = rel_tx.send(true); // flush what was held, let the hands run
    } else {
        // filler cue — only when nothing is already in flight
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        text.hash(&mut h);
        let cue = persona::FILLER_CUES[(h.finish() as usize) % persona::FILLER_CUES.len()];
        say(ctx, say_id, cue);
    }

    let result = match delib_result { Some(r) => r, None => delib.await };
    match result {
        Ok((reply, ran)) => {
            let ms = t0.elapsed().as_millis();
            tracing::info!("[tier=deliberate {ms}ms] {} chars, {} hand(s){}", reply.len(), ran.len(), if speculate { " (speculative)" } else { "" });
            {
                let mut h = ctx.history.lock().unwrap();
                h.push(history::Turn::user(text));
                h.push(history::Turn::assistant(&reply));
            }
            let tick = {
                let mut g = ctx.instance.lock().unwrap();
                let tick = g.tick;
                g.state.learn_skill("deliberate", text, tick);
                tick
            };
            // Stage 4 §3.1: every hand succeeded → compile the plan
            if !ran.is_empty() && ran.iter().all(|(_, _, err)| !err) {
                let steps: Vec<Step> = ran.iter().map(|(verb, input, _)| {
                    let body = hands::body_of(input);
                    let mut inp = input.clone();
                    if let Some(m) = inp.as_object_mut() { m.remove("body"); }
                    Step { verb: verb.clone(), input: inp, body }
                }).collect();
                if SkillStore::compilable(&steps) {
                    let new = ctx.skills.lock().unwrap().learn(raw_text, steps, tick, "deliberate");
                    tracing::info!("[skills] {} {:?} ({} step(s))", if new { "compiled" } else { "refreshed" }, raw_text, ran.len());
                    save_skills(ctx);
                }
            }
        }
        Err(e) if e.to_string() != "cancelled" => {
            tracing::warn!("[tier=deliberate] error: {e}");
            say(ctx, say_id, format!("something went wrong: {e}"));
        }
        _ => {}
    }
    ipc::send_state(ipc_tx, "idle", "deliberate");
}

fn verb_to_input(verb: &str, target: &str) -> serde_json::Value {
    match verb {
        "app.open" | "app.close" | "window.focus" | "body.forget" => serde_json::json!({"name": target}),
        "sys.volume" | "sys.brightness" | "sys.wifi" | "sys.bluetooth" | "sys.power"
            => serde_json::json!({"action": target}),
        "web.search" | "fs.search" => serde_json::json!({"query": target}),
        "web.open" => serde_json::json!({"url": target}),
        "window.workspace" => serde_json::json!({"target": target}),
        "input.type" => serde_json::json!({"text": target}),
        "input.key" => serde_json::json!({"combo": target}),
        "notes.write" => serde_json::json!({"content": target}),
        "notes.read" => serde_json::json!({"topic": target}),
        "screen.look" => serde_json::json!({"_screenshot_only": target == "screenshot"}),
        "skill.forget" => serde_json::json!({"utterance": target}),
        "task.stop" => serde_json::json!({"id": target.parse::<u64>().unwrap_or(0)}),
        "shell.run" => serde_json::json!({"cmd": target}),
        // walking body: "forward 3 steps" / "left" → {dir, steps, speed}
        "gait.walk" => {
            let words: Vec<&str> = target.split_whitespace().collect();
            let dir = match words.first().copied().unwrap_or("forward") {
                "back" | "backward" | "backwards" | "reverse" => "back",
                "left" => "left",
                "right" => "right",
                _ => "fwd",
            };
            let steps = words.iter().find_map(|w| w.parse::<u32>().ok()).unwrap_or(1);
            serde_json::json!({"dir": dir, "steps": steps, "speed": 0.5})
        }
        "body.calibrate" => {
            // "begin" | "end" | "6 center 92.5 min 30 max 150 dir 1"
            let t = target.trim();
            if t == "begin" || t == "end" || t == "start" || t == "finish" {
                serde_json::json!({"mode": if t == "start" { "begin" } else if t == "finish" { "end" } else { t }})
            } else {
                let w: Vec<&str> = t.split_whitespace().collect();
                let mut o = serde_json::Map::new();
                if let Some(ch) = w.first().and_then(|c| c.parse::<u64>().ok()) { o.insert("ch".into(), ch.into()); }
                let mut i = 1;
                while i + 1 < w.len() {
                    if let Ok(v) = w[i + 1].parse::<f64>() {
                        if matches!(w[i], "center" | "min" | "max" | "dir") { o.insert(w[i].into(), v.into()); }
                    }
                    i += 2;
                }
                serde_json::Value::Object(o)
            }
        }
        "servo.set" => {
            let w: Vec<&str> = target.split_whitespace().collect();
            serde_json::json!({"ch": w.first().and_then(|c| c.parse::<u64>().ok()).unwrap_or(255), "deg": w.get(1).and_then(|d| d.parse::<f64>().ok()).unwrap_or(-1.0)})
        }
        "fs.list" | "fs.read" | "fs.stat" | "fs.mkdir" => serde_json::json!({"path": target}),
        "sys.time" | "sys.date" | "sys.battery" | "sys.lock" | "skill.list" | "task.list" | "body.list" | "pool.status"
            | "app.list" | "window.list" | "window.fullscreen" | "window.close"
            | "gait.stand" | "gait.sit" | "gait.stop" | "body.telemetry" | "body.schema" => serde_json::json!({}),
        _ => serde_json::json!(target),
    }
}

fn format_reply(verb: &str, target: &str, result: &str) -> String {
    match verb {
        "sys.time" => format!("it's {result}"),
        "sys.date" => format!("today is {result}"),
        "sys.battery" => {
            let pct = result.lines()
                .find(|l| l.contains("percentage"))
                .and_then(|l| l.split(':').nth(1))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| result.chars().take(30).collect());
            format!("battery is {pct}")
        }
        "notes.write" => "noted.".into(),
        "gait.stand" | "gait.sit" | "gait.stop" | "gait.walk" | "body.telemetry" | "body.calibrate" | "servo.set"
            => result.chars().take(200).collect(),
        "body.schema" => "here is the body's schema.".into(),
        "notes.read" | "skill.list" | "task.list" | "body.list" | "pool.status" | "skill.forget" | "task.stop" | "body.forget"
            => result.chars().take(400).collect(),
        "screen.look" => "screenshot taken.".into(),
        "sys.lock" => "screen locked.".into(),
        "shell.run" | "code.run" => if result.trim().is_empty() { "done.".into() } else { result.trim().chars().take(200).collect() },
        _ => { let _ = target; if result.is_empty() { "done.".into() } else { result.chars().take(80).collect() } },
    }
}

fn load_brain_key(brain_dir: &PathBuf) -> BrainKey {
    let seed_path = brain_dir.join("SEED.HEX");
    let pub_path = brain_dir.join("KEY.PUB");

    if seed_path.exists() && pub_path.exists() {
        let seed = std::fs::read(&seed_path).unwrap_or_default();
        let pubk = std::fs::read(&pub_path).unwrap_or_default();
        if let Ok(k) = BrainKey::load(seed.trim_ascii(), pubk.trim_ascii()) {
            tracing::info!("[key] loaded from brain_dir");
            return k;
        }
    }

    // try workspace/key/brain_key.json
    let json_path = brain_dir
        .parent().and_then(|p| p.parent())
        .map(|p| p.join("key").join("brain_key.json"))
        .unwrap_or_default();

    if json_path.exists() {
        if let Ok(txt) = std::fs::read_to_string(&json_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                let seed_s = v["private_seed"].as_str().unwrap_or("");
                let pub_s = v["public_key"].as_str().unwrap_or("");
                if !seed_s.is_empty() {
                    if let Ok(k) = BrainKey::load(seed_s.as_bytes(), pub_s.as_bytes()) {
                        tracing::info!("[key] loaded from brain_key.json");
                        return k;
                    }
                }
            }
        }
    }

    tracing::warn!("[key] ephemeral dev key");
    BrainKey::from_seed([0x42u8; 32])
}
