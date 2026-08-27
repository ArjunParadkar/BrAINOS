//! brainos-operator — Stage 3 production body.
//!
//! Socket path: $BRAINOS_SOCK > config > $XDG_RUNTIME_DIR/brainos/operator.sock

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
mod history;
mod ipc;
mod journal_io;
mod key_reader;
mod kira_gate;
mod persona;
mod reflex;
mod sse;

use config::Config;
use ipc::{FaceMsg, IpcServer, OpMsg};
use kira_gate::SharedInstance;

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

    let brain_dir = config.brain_dir();
    let brain_key = load_brain_key(&brain_dir);
    tracing::info!("[operator] identity: {}", brain_key.fingerprint());

    let (body_map, available_verbs) = body_registry::build(&brain_dir);
    tracing::info!("[operator] {} verbs registered", available_verbs.len());

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
    let shared: SharedInstance = Arc::new(Mutex::new(instance));

    // socket path: BRAINOS_SOCK > config > XDG default
    let sock_path: PathBuf = std::env::var("BRAINOS_SOCK")
        .map(PathBuf::from)
        .unwrap_or_else(|_| config.socket_path());

    let server = IpcServer::bind(&sock_path)?;
    let ipc_tx = server.tx.clone();
    let (face_tx, mut face_rx) = mpsc::channel::<FaceMsg>(256);
    tokio::spawn(server.run(face_tx));

    let fast_tier = Arc::new(fast::FastTier::new((*config).clone()));
    let deliberate_tier = Arc::new(deliberate::DeliberateTier::new((*config).clone()));
    {
        // warm the local model's cached prefix in the background
        let ft = fast_tier.clone();
        let verbs: Vec<String> = available_verbs.clone();
        tokio::spawn(async move { ft.warmup(&verbs).await; });
    }
    let history = Arc::new(Mutex::new(history::History::new()));
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let cancel_tx = Arc::new(cancel_tx);

    let scratch_dir = PathBuf::from(&config.task.scratchpad);
    std::fs::create_dir_all(&scratch_dir).ok();

    let hand_ctx = Arc::new(hands::HandCtx {
        config: config.clone(),
        instance: shared.clone(),
        ipc_tx: ipc_tx.clone(),
        brain_dir: brain_dir.clone(),
        scratch_dir: scratch_dir.clone(),
    });

    // SIGTERM: save journal
    {
        let shared2 = shared.clone();
        let bd2 = brain_dir.clone();
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            tracing::info!("[operator] shutting down — saving journal");
            let j = journal_io::JournalIO::new(&bd2);
            let g = shared2.lock().unwrap();
            let _ = j.save(&g.state);
            drop(g);
            std::process::exit(0);
        });
    }

    ipc::send_state(&ipc_tx, "idle", "reflex");
    tracing::info!("[operator] ready on {}", sock_path.display());

    let mut say_id: u64 = 1;
    let mut last_save = std::time::Instant::now();

    loop {
        // periodic journal flush
        if last_save.elapsed().as_secs() > 300 {
            let g = shared.lock().unwrap();
            let _ = journal_io::JournalIO::new(&brain_dir).save(&g.state);
            drop(g);
            last_save = std::time::Instant::now();
        }

        let Some(msg) = face_rx.recv().await else { break; };

        match msg {
            FaceMsg::Utter { text } => {
                let text = text.trim().to_string();
                if text.is_empty() { continue; }
                tracing::info!("[utter] {:?}", text);
                say_id += 1;

                // reset cancel
                let _ = cancel_tx.send(false);

                handle_utter(
                    &text,
                    say_id,
                    &config,
                    &api_key,
                    &available_verbs,
                    &shared,
                    &history,
                    &fast_tier,
                    &deliberate_tier,
                    &hand_ctx,
                    &ipc_tx,
                    &cancel_rx,
                )
                .await;

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
            }

            FaceMsg::Ptt { on } => {
                if on {
                    ipc::send_state(&ipc_tx, "listening", "reflex");
                } else {
                    ipc::send_state(&ipc_tx, "idle", "reflex");
                }
            }

            FaceMsg::Cancel => {
                tracing::info!("[operator] cancel");
                let _ = cancel_tx.send(true);
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
    }

    Ok(())
}

async fn handle_utter(
    text: &str,
    say_id: u64,
    config: &Arc<Config>,
    api_key: &Option<String>,
    available_verbs: &[String],
    instance: &SharedInstance,
    history: &Arc<Mutex<history::History>>,
    fast_tier: &Arc<fast::FastTier>,
    deliberate_tier: &Arc<deliberate::DeliberateTier>,
    hand_ctx: &Arc<hands::HandCtx>,
    ipc_tx: &broadcast::Sender<String>,
    cancel_rx: &watch::Receiver<bool>,
) {
    let t0 = std::time::Instant::now();

    // ── Tier 1: REFLEX ──────────────────────────────────────────────────────
    ipc::send_state(ipc_tx, "thinking", "reflex");

    if let Some(intent) = reflex::match_intent(text) {
        if reflex::needs_confirm(&intent, text) {
            let what = match intent.target.as_str() { "sleep" => "sleep", "shutdown" => "shut down", _ => "reboot" };
            ipc::send(ipc_tx, &OpMsg::Say {
                id: say_id,
                text: format!("confirm: {what}? say '{what} now' to confirm."),
                delta: false, done: true,
            });
            ipc::send_state(ipc_tx, "idle", "reflex");
            return;
        }

        let verb = intent.verb.clone();
        let target = intent.target.clone();

        match kira_gate::gate(instance, &verb, &target) {
            Ok(_) => {
                ipc::send_state(ipc_tx, "acting", "reflex");
                let input = verb_to_input(&verb, &target);
                let result = hands::dispatch(&verb, &input, hand_ctx).await;
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                tracing::info!("[tier=reflex {ms:.1}ms] {text} -> {verb} {target} ok");
                ipc::send(ipc_tx, &OpMsg::Act {
                    verb: verb.clone(), target: target.clone(),
                    status: "ok".into(),
                    digest: result.chars().take(100).collect(),
                });
                let reply = format_reply(&verb, &target, &result);
                ipc::send(ipc_tx, &OpMsg::Say { id: say_id, text: reply, delta: false, done: true });
            }
            Err(denial) => {
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                tracing::warn!("[tier=reflex {ms:.1}ms] KIRA denied: {denial}");
                ipc::send(ipc_tx, &OpMsg::Act {
                    verb: verb.clone(), target: target.clone(),
                    status: "deny".into(), digest: denial.to_string(),
                });
                ipc::send(ipc_tx, &OpMsg::Say {
                    id: say_id, text: format!("i can't do that: {denial}"),
                    delta: false, done: true,
                });
            }
        }
        ipc::send_state(ipc_tx, "idle", "reflex");
        return;
    }

    // ── Tier 2: FAST ────────────────────────────────────────────────────────
    let want_deliberate = api_key.is_some() && persona::should_escalate(text);

    if !want_deliberate {
        ipc::send_state(ipc_tx, "thinking", "fast");
        let hist_json = history.lock().unwrap().as_json();
        // Speed contract: if the local model takes longer than a beat, say so
        // immediately rather than leaving the human staring at a silent bar.
        let ask = fast_tier.ask(text, available_verbs, &hist_json);
        tokio::pin!(ask);
        let fast_result = tokio::select! {
            r = &mut ask => r,
            _ = tokio::time::sleep(std::time::Duration::from_millis(600)) => {
                ipc::send(ipc_tx, &OpMsg::Say { id: say_id, text: "one sec.".into(), delta: false, done: true });
                ask.await
            }
        };
        match fast_result {
            // an "act" with no usable verb is the small model guessing: never
            // speak a guess as if it were an action — escalate instead.
            Some(resp) if resp.intent != "escalate"
                && !(resp.intent == "act" && resp.verb.as_deref().map(|v| v.is_empty() || !available_verbs.iter().any(|a| a == v)).unwrap_or(true)) => {
                let ms = t0.elapsed().as_millis();
                tracing::info!("[tier=fast {ms}ms] intent={} verb={:?}", resp.intent, resp.verb);
                ipc::send(ipc_tx, &OpMsg::Say {
                    id: say_id, text: resp.reply.clone(), delta: false, done: true,
                });
                if resp.intent == "act" {
                    if let (Some(verb), Some(target)) = (&resp.verb, &resp.target) {
                        if kira_gate::gate(instance, verb, target).is_ok() {
                            let input = verb_to_input(verb, target);
                            let result = hands::dispatch(verb, &input, hand_ctx).await;
                            ipc::send(ipc_tx, &OpMsg::Act {
                                verb: verb.clone(), target: target.clone(),
                                status: "ok".into(),
                                digest: result.chars().take(100).collect(),
                            });
                        }
                    }
                }
                {
                    let mut h = history.lock().unwrap();
                    h.push(history::Turn::user(text));
                    h.push(history::Turn::assistant(&resp.reply));
                }
                ipc::send_state(ipc_tx, "idle", "fast");
                return;
            }
            _ => tracing::info!("[tier=fast] timeout/escalate → deliberate"),
        }
    }

    // ── Tier 3: DELIBERATE ──────────────────────────────────────────────────
    let key = match api_key {
        Some(k) => k.clone(),
        None => {
            ipc::send(ipc_tx, &OpMsg::Say {
                id: say_id,
                text: "i need an API key for that — the reflex and fast tiers are still here.".into(),
                delta: false, done: true,
            });
            ipc::send_state(ipc_tx, "idle", "reflex");
            return;
        }
    };

    ipc::send_state(ipc_tx, "thinking", "deliberate");

    // filler cue
    {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        text.hash(&mut h);
        let cue = persona::FILLER_CUES[(h.finish() as usize) % persona::FILLER_CUES.len()];
        ipc::send(ipc_tx, &OpMsg::Say { id: say_id, text: cue.into(), delta: false, done: true });
    }

    let memories = {
        let g = instance.lock().unwrap();
        g.state.durable_digest(8, 4)
    };

    // Build a tool executor that lives long enough.
    // We use a simple closure that captures Arc clones.
    let instance2 = instance.clone();
    let hctx2 = hand_ctx.clone();
    let itx2 = ipc_tx.clone();
    let sid2 = say_id;

    let mut tool_exec = |name: &str, input: &Value| -> std::pin::Pin<Box<dyn std::future::Future<Output = (String, bool)> + Send>> {
        let name = name.to_string();
        let input = input.clone();
        let inst = instance2.clone();
        let ctx = hctx2.clone();
        let tx = itx2.clone();
        let target_str = deliberate::kira_target(&name, &input);

        Box::pin(async move {
            match kira_gate::gate(&inst, &name, &target_str) {
                Ok(_) => {
                    let result = hands::dispatch(&name, &input, &ctx).await;
                    ipc::send(&tx, &OpMsg::Act {
                        verb: name.clone(), target: target_str,
                        status: "ok".into(),
                        digest: result.chars().take(100).collect(),
                    });
                    (result, false)
                }
                Err(denial) => {
                    let msg = format!("denied by KIRA: {denial}");
                    ipc::send(&tx, &OpMsg::Act {
                        verb: name.clone(), target: target_str,
                        status: "deny".into(), digest: msg.clone(),
                    });
                    (msg, true)
                }
            }
        })
    };

    {
        let mut hist = history.lock().unwrap();
        match deliberate_tier
            .run_loop(
                text,
                &key,
                available_verbs,
                &memories,
                &mut *hist,
                ipc_tx,
                say_id,
                &mut tool_exec,
                cancel_rx,
            )
            .await
        {
            Ok(reply) => {
                let ms = t0.elapsed().as_millis();
                tracing::info!("[tier=deliberate {ms}ms] {} chars", reply.len());
                let mut g = instance.lock().unwrap();
                let tick = g.tick;
                g.state.learn_skill("deliberate", text, tick);
            }
            Err(e) if e.to_string() != "cancelled" => {
                tracing::warn!("[tier=deliberate] error: {e}");
                ipc::send(ipc_tx, &OpMsg::Say {
                    id: say_id,
                    text: format!("something went wrong: {e}"),
                    delta: false, done: true,
                });
            }
            _ => {}
        }
    }

    ipc::send_state(ipc_tx, "idle", "deliberate");
}

fn verb_to_input(verb: &str, target: &str) -> serde_json::Value {
    match verb {
        "app.open" | "app.close" | "window.focus" => serde_json::json!({"name": target}),
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
        "notes.read" => result.chars().take(200).collect(),
        "screen.look" => "screenshot taken.".into(),
        "sys.lock" => "screen locked.".into(),
        _ => if result.is_empty() { "done.".into() } else { result.chars().take(80).collect() },
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
