//! Stage 4 §2 — Reach: the entity in every device it can reach.
//!
//! A device that cannot boot the key runs `reach/brainos-reach.py` and OFFERS
//! its hands over TCP. This module is the operator side:
//!
//!   device → `{"t":"hello","name":..,"nonce":<hex>,"hands":[..],"os":..}`
//!   seat   → `{"t":"attest","pub":<hex>,"sig":<hex ed25519(nonce|name)>}`
//!   device → `{"t":"ready"}`                       (or `{"t":"refused",..}`)
//!   seat   → `{"t":"cmd","id":N,"verb":..,"input":{..}}`
//!   device → `{"t":"result","id":N,"ok":bool,"digest":".."}`
//!   either → `{"t":"ping"}` / `{"t":"pong"}`
//!
//! Each accepted body is incorporated into the ONE body map as region
//! `reach/<name>` with exactly the verbs it offered, so KIRA's authz sees it
//! and the deliberate tier advertises it. When the socket drops, the region
//! is released and the verbs are gone from the next proposal on.
//!
//! The device never remembers (§13.2): every result comes back as a bounded
//! digest and becomes memory here, in the journal on the key.

use anyhow::{bail, Result};
use brainos_mind::body::{Region, RegionClass};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::ipc::{self, OpMsg};
use crate::kira_gate::SharedInstance;

/// Verbs a Reach body may offer. Anything else in its `hands` is ignored —
/// the device does not get to invent verbs the operator cannot dispatch.
pub const OFFERABLE: &[&str] = &[
    "shell.run", "code.run",
    "fs.list", "fs.read", "fs.write", "fs.mkdir", "fs.delete", "fs.move", "fs.stat", "fs.search",
    "sys.notify", "sys.time", "sys.date", "sys.battery", "sys.volume", "sys.brightness",
    "sys.lock", "sys.power", "sys.clipboard",
    "app.open", "app.list",
    "screen.look",
    "input.type", "input.key",
];

pub const SEAT: &str = "seat";
const CALL_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_DIGEST: usize = 4096;

#[derive(Debug, Clone)]
pub struct BodyInfo {
    pub name: String,
    pub hands: Vec<String>,
    pub os: String,
    pub rtt_ms: u128,
    pub since: Instant,
}

struct Cmd {
    verb: String,
    input: Value,
    reply: oneshot::Sender<(String, bool)>,
}

#[derive(Default)]
pub struct ReachHub {
    bodies: Mutex<HashMap<String, (BodyInfo, mpsc::Sender<Cmd>)>>,
}

impl ReachHub {
    pub fn new() -> Arc<ReachHub> { Arc::new(ReachHub::default()) }

    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.bodies.lock().unwrap().keys().cloned().collect();
        v.sort();
        v
    }

    pub fn list(&self) -> Vec<BodyInfo> {
        let mut v: Vec<BodyInfo> = self.bodies.lock().unwrap().values().map(|(b, _)| b.clone()).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn has(&self, name: &str) -> bool {
        self.bodies.lock().unwrap().contains_key(name)
    }

    /// Verbs currently offered by any Reach body.
    pub fn remote_verbs(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for (b, _) in self.bodies.lock().unwrap().values() {
            for h in &b.hands { if !v.contains(h) { v.push(h.clone()); } }
        }
        v
    }

    /// Run a verb on a named body. Returns (digest, is_error).
    pub async fn call(&self, body: &str, verb: &str, input: &Value) -> (String, bool) {
        let tx = match self.bodies.lock().unwrap().get(body) {
            Some((_, tx)) => tx.clone(),
            None => return (format!("no body named '{body}' is reachable right now"), true),
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        if tx.send(Cmd { verb: verb.into(), input: input.clone(), reply: reply_tx }).await.is_err() {
            return (format!("body '{body}' just went away"), true);
        }
        match tokio::time::timeout(CALL_TIMEOUT, reply_rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => (format!("body '{body}' dropped the connection mid-command"), true),
            Err(_) => (format!("body '{body}' did not answer within {}s", CALL_TIMEOUT.as_secs()), true),
        }
    }

    /// Drop a body on purpose. The connection task notices its channel closed
    /// and releases the region.
    pub fn forget(&self, name: &str) -> bool {
        self.bodies.lock().unwrap().remove(name).is_some()
    }

    fn insert(&self, info: BodyInfo, tx: mpsc::Sender<Cmd>) -> bool {
        let mut g = self.bodies.lock().unwrap();
        if g.contains_key(&info.name) { return false; }
        g.insert(info.name.clone(), (info, tx));
        true
    }

    fn remove(&self, name: &str) {
        self.bodies.lock().unwrap().remove(name);
    }

    fn set_rtt(&self, name: &str, ms: u128) {
        if let Some((b, _)) = self.bodies.lock().unwrap().get_mut(name) { b.rtt_ms = ms; }
    }
}

/// Body names are addressable in speech ("on the pi") — keep them plain.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 32
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && name != SEAT && name != "local" && name != "here"
}

/// Message the seat signs: `<nonce hex>|<name>` as ASCII. Mirrored in
/// brainos-reach.py; a change here is a protocol change.
pub fn challenge_bytes(nonce: &str, name: &str) -> Vec<u8> {
    format!("{nonce}|{name}").into_bytes()
}

/// Listen for Reach bodies.
pub async fn listen(
    addr: String,
    hub: Arc<ReachHub>,
    instance: SharedInstance,
    ipc_tx: broadcast::Sender<String>,
) -> Result<()> {
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!("[reach] listening on {addr}");
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => { tracing::warn!("[reach] accept: {e}"); tokio::time::sleep(Duration::from_millis(200)).await; continue; }
        };
        let hub = hub.clone();
        let inst = instance.clone();
        let tx = ipc_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = serve(stream, hub, inst, tx).await {
                tracing::info!("[reach] {peer}: {e}");
            }
        });
    }
}

async fn serve(
    stream: TcpStream,
    hub: Arc<ReachHub>,
    instance: SharedInstance,
    ipc_tx: broadcast::Sender<String>,
) -> Result<()> {
    stream.set_nodelay(true).ok();
    let (rd, mut wr) = stream.into_split();
    let mut lines = BufReader::new(rd).lines();

    // 1. hello
    let hello: Value = match tokio::time::timeout(Duration::from_secs(10), lines.next_line()).await {
        Ok(Ok(Some(l))) => serde_json::from_str(&l)?,
        _ => bail!("no hello"),
    };
    if hello["t"] != "hello" { bail!("expected hello, got {}", hello["t"]); }
    let name = hello["name"].as_str().unwrap_or("").to_string();
    if !valid_name(&name) { bail!("bad body name {name:?}"); }
    let nonce = hello["nonce"].as_str().unwrap_or("").to_string();
    if nonce.len() < 32 || !nonce.chars().all(|c| c.is_ascii_hexdigit()) { bail!("bad nonce"); }
    let os = hello["os"].as_str().unwrap_or("unknown").to_string();
    let offered: Vec<String> = hello["hands"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    let hands: Vec<String> = offered.into_iter().filter(|h| OFFERABLE.contains(&h.as_str())).collect();
    if hands.is_empty() { bail!("body {name} offered no usable hands"); }

    // 2. attest — the only authority is the BrAIn Key
    let (sig, pubk) = {
        let g = instance.lock().unwrap();
        (g.key.sign(&challenge_bytes(&nonce, &name)), g.key.public_hex.clone())
    };
    let attest = json!({"t":"attest","pub":pubk,"sig":brainos_mind::key::to_hex(&sig)});
    wr.write_all(format!("{attest}\n").as_bytes()).await?;

    // 3. ready
    let t_rtt = Instant::now();
    let ready: Value = match tokio::time::timeout(Duration::from_secs(10), lines.next_line()).await {
        Ok(Ok(Some(l))) => serde_json::from_str(&l)?,
        _ => bail!("no ready from {name}"),
    };
    if ready["t"] != "ready" {
        bail!("body {name} refused the handshake: {}", ready["reason"].as_str().unwrap_or("?"));
    }
    let rtt = t_rtt.elapsed().as_millis();

    // 4. incorporate
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<Cmd>(32);
    let info = BodyInfo { name: name.clone(), hands: hands.clone(), os: os.clone(), rtt_ms: rtt, since: Instant::now() };
    if !hub.insert(info, cmd_tx) {
        let _ = wr.write_all(b"{\"t\":\"bye\",\"reason\":\"a body with that name is already here\"}\n").await;
        bail!("duplicate body name {name}");
    }
    {
        let mut g = instance.lock().unwrap();
        let id = format!("reach/{name}");
        if !g.body.knows(&id) {
            g.body.incorporate(Region {
                id,
                class: RegionClass::HostTool,
                capabilities: hands.clone(),
                proprioception: format!("{os} device '{name}' reached over the network ({} hands, {rtt} ms)", hands.len()),
            });
        }
    }
    ipc::send(&ipc_tx, &OpMsg::Body { name: name.clone(), hands: hands.clone(), os: os.clone(), joined: true, rtt_ms: rtt as u64 });
    tracing::info!("[reach] incorporated '{name}' ({os}, {} hands, {rtt} ms)", hands.len());

    // 5. serve until either side goes away
    let mut pending: HashMap<u64, oneshot::Sender<(String, bool)>> = HashMap::new();
    let mut next_id: u64 = 1;
    let mut ping_at = Instant::now();
    let mut last_ping_sent: Option<Instant> = None;
    let outcome: Result<()> = loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { break Ok(()) }; // hub.forget() or shutdown
                let id = next_id; next_id += 1;
                let line = json!({"t":"cmd","id":id,"verb":cmd.verb,"input":cmd.input});
                if wr.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                    let _ = cmd.reply.send(("connection to body lost".into(), true));
                    break Err(anyhow::anyhow!("write failed"));
                }
                pending.insert(id, cmd.reply);
            }
            line = lines.next_line() => {
                let Ok(Some(line)) = line else { break Err(anyhow::anyhow!("connection closed by {name}")) };
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                match v["t"].as_str().unwrap_or("") {
                    "result" => {
                        let id = v["id"].as_u64().unwrap_or(0);
                        let ok = v["ok"].as_bool().unwrap_or(false);
                        let mut digest = v["digest"].as_str().unwrap_or("").to_string();
                        if digest.len() > MAX_DIGEST { digest.truncate(MAX_DIGEST); digest.push_str("…[truncated]"); }
                        if let Some(r) = pending.remove(&id) { let _ = r.send((digest, !ok)); }
                    }
                    "ping" => { let _ = wr.write_all(b"{\"t\":\"pong\"}\n").await; }
                    "pong" => {
                        if let Some(t) = last_ping_sent.take() { hub.set_rtt(&name, t.elapsed().as_millis()); }
                    }
                    _ => {}
                }
            }
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(ping_at + Duration::from_secs(20))) => {
                ping_at = Instant::now();
                last_ping_sent = Some(ping_at);
                if wr.write_all(b"{\"t\":\"ping\"}\n").await.is_err() { break Err(anyhow::anyhow!("ping write failed")); }
            }
        }
    };

    // 6. release — the hand is gone from the body map immediately
    hub.remove(&name);
    for (_, r) in pending.drain() { let _ = r.send((format!("body '{name}' went away"), true)); }
    {
        let mut g = instance.lock().unwrap();
        g.body.release(&format!("reach/{name}"));
    }
    ipc::send(&ipc_tx, &OpMsg::Body { name: name.clone(), hands: vec![], os, joined: false, rtt_ms: 0 });
    tracing::info!("[reach] released '{name}'");
    outcome
}

/// UDP beacon so a device can find the seat by name on the LAN:
/// `BRAINOS?` → `BRAINOS <seat-name> <tcp-port>`.
pub async fn beacon(port: u16, seat_name: String, tcp_port: u16) {
    let sock = match UdpSocket::bind(("0.0.0.0", port)).await {
        Ok(s) => s,
        Err(e) => { tracing::warn!("[reach] beacon bind {port}: {e}"); return; }
    };
    tracing::info!("[reach] beacon on udp/{port}");
    let mut buf = [0u8; 64];
    loop {
        let Ok((n, peer)) = sock.recv_from(&mut buf).await else { continue };
        if &buf[..n] == b"BRAINOS?" {
            let _ = sock.send_to(format!("BRAINOS {seat_name} {tcp_port}").as_bytes(), peer).await;
        }
    }
}

/// Spoken routing: "on the pi, restart cups" / "on pi: ..." / "restart cups on the pi".
/// Only a currently reachable body name (or "here"/"seat") is recognised, so
/// ordinary sentences containing "on" are untouched. Returns (utterance, body).
pub fn split_body(text: &str, names: &[String]) -> (String, Option<String>) {
    let t = text.trim();
    let low = t.to_ascii_lowercase();
    let strip_article = |s: &str| s.trim().trim_start_matches("the ").trim_start_matches("my ").trim().to_string();
    let resolve = |cand: &str| -> Option<String> {
        let c = strip_article(cand);
        let c = c.trim_end_matches(|ch: char| matches!(ch, ',' | ':' | '.' | '!'));
        if c == SEAT || c == "here" || c == "local" { return Some(SEAT.into()); }
        names.iter().find(|n| n.as_str() == c).cloned()
    };
    // prefix form
    if let Some(rest) = low.strip_prefix("on ") {
        if let Some(pos) = rest.find(|c: char| matches!(c, ',' | ':')) {
            if let Some(b) = resolve(&rest[..pos]) {
                return (t[3 + pos + 1..].trim().to_string(), Some(b));
            }
        }
        // "on pi run echo hi" — first one or two words
        let words: Vec<&str> = rest.split_whitespace().collect();
        for take in [2usize, 1] {
            if words.len() > take {
                if let Some(b) = resolve(&words[..take].join(" ")) {
                    let consumed = 3 + words[..take].iter().map(|w| w.len()).sum::<usize>() + (take - 1);
                    return (t[consumed..].trim().to_string(), Some(b));
                }
            }
        }
    }
    // suffix form: "... on the pi"
    if let Some(pos) = low.rfind(" on ") {
        if let Some(b) = resolve(&low[pos + 4..]) {
            return (t[..pos].trim().to_string(), Some(b));
        }
    }
    (t.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_plain() {
        assert!(valid_name("pi"));
        assert!(valid_name("lab-2"));
        assert!(!valid_name("seat"));
        assert!(!valid_name("Pi"));
        assert!(!valid_name("a b"));
        assert!(!valid_name(""));
    }

    #[test]
    fn split_body_forms() {
        let names = vec!["pi".to_string(), "phone".to_string()];
        assert_eq!(split_body("on the pi, restart cups", &names), ("restart cups".into(), Some("pi".into())));
        assert_eq!(split_body("on pi: run echo hi", &names), ("run echo hi".into(), Some("pi".into())));
        assert_eq!(split_body("on phone run echo hi", &names), ("run echo hi".into(), Some("phone".into())));
        assert_eq!(split_body("list the files on the pi", &names), ("list the files".into(), Some("pi".into())));
        assert_eq!(split_body("on here, open firefox", &names), ("open firefox".into(), Some(SEAT.into())));
        // unknown body → untouched
        assert_eq!(split_body("on the moon, dance", &names), ("on the moon, dance".into(), None));
        assert_eq!(split_body("turn on wifi", &names), ("turn on wifi".into(), None));
        assert_eq!(split_body("what's on the screen", &names), ("what's on the screen".into(), None));
    }

    #[test]
    fn challenge_bytes_are_stable() {
        assert_eq!(challenge_bytes("ab12", "pi"), b"ab12|pi".to_vec());
    }
}
