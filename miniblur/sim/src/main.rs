//! miniblur-sim — a Mini-Blur that exists as numbers (Execution Plan §4.1
//! WP0.5). It speaks the Reach wire protocol to the seat exactly as the
//! ESP32-S3 firmware will, backed by `miniblur-core`'s model, so the whole
//! entity side of Phase 0 — §8 acquisition, the hexapod KIRA policy, two
//! bodies one memory — is VERIFIED here before a servo ever moves. It is
//! also the regression harness: `bench` runs every gait through the model.
//!
//!   miniblur-sim serve --seat 127.0.0.1:7777 --name miniblur --pub-file KEY.PUB
//!   miniblur-sim bench
//!   miniblur-sim selftest
//!
//! Like the board, it never remembers (§13.2): no history, no cache. The
//! only file it writes is a tamper log for refused handshakes.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use miniblur_core::gait::{Dir, GaitParams, Plan, Walk};
use miniblur_core::runtime::{Body, ServoBus};
use miniblur_core::schema::{self, Model};
use miniblur_core::stability;
use miniblur_core::wire::{self, FromSeat};
use miniblur_core::MAX_SERVOS;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Servos as ten floats.
#[derive(Default)]
struct SimBus {
    deg: [f32; MAX_SERVOS],
    writes: u64,
}

impl ServoBus for SimBus {
    fn set_deg(&mut self, ch: u8, deg: f32) {
        if (ch as usize) < MAX_SERVOS {
            self.deg[ch as usize] = deg;
            self.writes += 1;
        }
    }
}

struct Opts {
    seat: String,
    name: String,
    pubkey: Option<[u8; 32]>,
    dir: PathBuf,
    vbat: f32,
    realtime: bool,
    silent_after: Option<u64>,
    tippy: bool,
    weak_rail: bool,
    once: bool,
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  miniblur-sim serve --seat HOST:PORT --name NAME (--pub HEX | --pub-file PATH) [--vbat V] [--realtime] [--silent-after N] [--tippy] [--weak-rail] [--once] [--dir DIR]\n  miniblur-sim bench\n  miniblur-sim selftest\n  miniblur-sim hands\n  miniblur-sim schema"
    );
    std::process::exit(2)
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    let v = wire::hex_decode(s.trim())?;
    if v.len() != 32 {
        return None;
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    Some(out)
}

fn parse_opts(args: &[String]) -> Opts {
    let mut o = Opts {
        seat: String::new(),
        name: "miniblur".into(),
        pubkey: None,
        dir: std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".miniblur-sim")).unwrap_or_else(|| PathBuf::from(".miniblur-sim")),
        vbat: 7.4,
        realtime: false,
        silent_after: None,
        tippy: false,
        weak_rail: false,
        once: false,
    };
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let mut val = || {
            i += 1;
            args.get(i).cloned().unwrap_or_else(|| usage())
        };
        match a {
            "--seat" => o.seat = val(),
            "--name" => o.name = val(),
            "--pub" => o.pubkey = Some(parse_hex32(&val()).unwrap_or_else(|| usage())),
            "--pub-file" => {
                let p = val();
                let s = std::fs::read_to_string(&p).unwrap_or_else(|e| {
                    eprintln!("cannot read {p}: {e}");
                    std::process::exit(2)
                });
                o.pubkey = Some(parse_hex32(s.lines().next().unwrap_or("")).unwrap_or_else(|| usage()));
            }
            "--dir" => o.dir = PathBuf::from(val()),
            "--vbat" => o.vbat = val().parse().unwrap_or_else(|_| usage()),
            "--silent-after" => o.silent_after = Some(val().parse().unwrap_or_else(|_| usage())),
            "--realtime" => o.realtime = true,
            "--tippy" => o.tippy = true,
            "--weak-rail" => o.weak_rail = true,
            "--once" => o.once = true,
            _ => usage(),
        }
        i += 1;
    }
    o
}

fn model_for(o: &Opts) -> Model {
    let mut m = Model::default();
    if o.tippy {
        // a battery hung far off the back: the forward model refuses every walk
        m.geom.com = miniblur_core::geometry::Vec2::new(-60.0, 0.0);
    }
    if o.weak_rail {
        m.limits.power.rail_amps = 1.0;
    }
    m
}

fn nonce_hex() -> String {
    let mut b = [0u8; 32];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut b);
    } else {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        b[..16].copy_from_slice(&t.to_le_bytes());
    }
    wire::hex_encode(&b)
}

fn tamper(dir: &PathBuf, seat: &str, reason: &str) {
    let _ = std::fs::create_dir_all(dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("tamper.log")) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "{t} refused handshake from {seat}: {reason}");
    }
    eprintln!("[miniblur] TAMPER: refused handshake from {seat}: {reason}");
}

/// One connection: hello → attest → ready → serve until the seat goes away.
fn serve_once(o: &Opts, pubkey: &[u8; 32]) -> Result<(), String> {
    let stream = TcpStream::connect(&o.seat).map_err(|e| format!("connect {}: {e}", o.seat))?;
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(Duration::from_millis(20))).ok();
    let mut wr = stream.try_clone().map_err(|e| e.to_string())?;
    let mut rd = BufReader::new(stream);

    let model = model_for(o);
    let nonce = nonce_hex();
    wr.write_all(wire::hello(&o.name, &nonce, "esp32s3-sim", schema::HANDS, &model.schema_json()).as_bytes())
        .map_err(|e| e.to_string())?;

    // attest: the only authority is the BrAIn Key this body was paired with
    let mut line = String::new();
    rd.get_mut().set_read_timeout(Some(Duration::from_secs(10))).ok();
    rd.read_line(&mut line).map_err(|e| format!("waiting for attest: {e}"))?;
    let (pub_hex, sig_hex) = match wire::parse(&line) {
        FromSeat::Attest { pub_hex, sig_hex } => (pub_hex, sig_hex),
        other => {
            tamper(&o.dir, &o.seat, &format!("expected attest, got {other:?}"));
            let _ = wr.write_all(wire::refused("expected attest").as_bytes());
            return Err("no attest".into());
        }
    };
    let claimed = parse_hex32(&pub_hex);
    if claimed.as_ref() != Some(pubkey) {
        tamper(&o.dir, &o.seat, "that is not my entity (public key differs from the pairing)");
        let _ = wr.write_all(wire::refused("that is not my entity").as_bytes());
        return Err("wrong key".into());
    }
    let vk = VerifyingKey::from_bytes(pubkey).map_err(|e| e.to_string())?;
    let sig_bytes = wire::hex_decode(&sig_hex).unwrap_or_default();
    let sig = match <[u8; 64]>::try_from(sig_bytes.as_slice()) {
        Ok(b) => Signature::from_bytes(&b),
        Err(_) => {
            tamper(&o.dir, &o.seat, "malformed attestation");
            let _ = wr.write_all(wire::refused("malformed attestation").as_bytes());
            return Err("bad sig".into());
        }
    };
    if vk.verify(&wire::challenge_bytes(&nonce, &o.name), &sig).is_err() {
        tamper(&o.dir, &o.seat, "bad signature over challenge");
        let _ = wr.write_all(wire::refused("signature did not verify").as_bytes());
        return Err("bad sig".into());
    }
    wr.write_all(wire::ready().as_bytes()).map_err(|e| e.to_string())?;
    eprintln!("[miniblur] '{}' is now a body of {}.. ({} hands)", o.name, &pub_hex[..8], schema::HANDS.len());

    // serve
    let t0 = Instant::now();
    let now = || t0.elapsed().as_millis() as u64;
    let mut body = Body::new(model, SimBus::default(), now());
    body.state.vbat = o.vbat;
    rd.get_mut().set_read_timeout(Some(Duration::from_millis(20))).ok();
    let mut cmds: u64 = 0;
    let mut silent = false;
    loop {
        line.clear();
        match rd.read_line(&mut line) {
            Ok(0) => return Ok(()), // seat closed
            Ok(_) => {
                if silent {
                    continue; // the body has gone dark on purpose (a lost link, from the seat's side)
                }
                match wire::parse(&line) {
                    FromSeat::Ping => {
                        body.heard(now());
                        let _ = wr.write_all(wire::pong().as_bytes());
                    }
                    FromSeat::Pong => body.heard(now()),
                    FromSeat::Cmd { id, verb, input } => {
                        cmds += 1;
                        let (digest, ok) = body.handle(&verb, &input, now(), !o.realtime);
                        eprintln!("[miniblur] {verb} {} → {}", if ok { "ok" } else { "REFUSED" }, digest.chars().take(120).collect::<String>());
                        let _ = wr.write_all(wire::result(id, ok, &digest).as_bytes());
                        if o.silent_after.map(|n| cmds >= n).unwrap_or(false) {
                            eprintln!("[miniblur] going silent after {cmds} command(s) (simulating a lost link)");
                            silent = true;
                        }
                    }
                    FromSeat::Bye { reason } => {
                        eprintln!("[miniblur] seat said bye: {reason}");
                        return Ok(());
                    }
                    FromSeat::Unknown(_) => {}
                    FromSeat::Attest { .. } => {}
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(format!("read: {e}")),
        }
        if let Some(msg) = body.tick(now()) {
            eprintln!("[miniblur] reflex: {msg}");
        }
    }
}

fn cmd_serve(args: &[String]) {
    let o = parse_opts(args);
    if o.seat.is_empty() {
        usage();
    }
    let Some(pubkey) = o.pubkey else {
        eprintln!("this body is not paired: pass --pub HEX or --pub-file KEY.PUB");
        std::process::exit(2);
    };
    loop {
        match serve_once(&o, &pubkey) {
            Ok(()) => eprintln!("[miniblur] seat went away"),
            Err(e) => eprintln!("[miniblur] {e}"),
        }
        if o.once {
            return;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// The regression harness: every direction at two speeds through the
/// default model; the worst CoM margin and the rail peak. Any walk the
/// envelope refuses is a failure — a gait change must not ship if the
/// body it is meant for cannot do it.
fn cmd_bench() -> i32 {
    let m = Model::default();
    println!("miniblur-core bench — {}", m.describe());
    println!("servo table (hypothesis until the wiring doc): {}", m.layout_fingerprint());
    println!("{:<8} {:>6} {:>10} {:>9} {:>8} {:>7}", "dir", "speed", "margin_mm", "peak_A", "moving", "verdict");
    let mut failed = 0;
    for dir in [Dir::Fwd, Dir::Back, Dir::Left, Dir::Right] {
        for speed in [0.3f32, 1.0] {
            let w = Walk { dir, steps: 2, speed };
            let plan = Plan::new(GaitParams::default(), w);
            let margin = stability::plan_min_margin_mm(&m.geom, &plan);
            let (peak, n) = m.limits.power.plan_peak_amps(&m.table, &plan).unwrap_or((f32::NAN, 0));
            let verdict = miniblur_core::policy::validate(&m, "gait.walk", &w.target())
                .and_then(|_| miniblur_core::policy::simulate(&m, "gait.walk", &w.target()).map(|_| ()));
            let ok = verdict.is_ok();
            if !ok {
                failed += 1;
            }
            println!(
                "{:<8} {:>6.1} {:>10.1} {:>9.2} {:>8} {:>7}{}",
                dir.name(), speed, margin, peak, n, if ok { "ok" } else { "REFUSED" },
                verdict.err().map(|e| format!("  {e}")).unwrap_or_default()
            );
        }
    }
    // the things that must be refused, refused
    let mut tippy = m;
    tippy.geom.com = miniblur_core::geometry::Vec2::new(-60.0, 0.0);
    let t = miniblur_core::policy::simulate(&tippy, "gait.walk", "fwd 1 0.5");
    println!("tippy body (CoM 60 mm aft): {}", if t.is_err() { "refused at simulate — correct" } else { "GRANTED — WRONG" });
    if t.is_ok() {
        failed += 1;
    }
    let v = miniblur_core::policy::validate(&m, "gait.walk", "fwd 40");
    println!("40 steps: {}", if v.is_err() { "refused at validate — correct" } else { "GRANTED — WRONG" });
    if v.is_ok() {
        failed += 1;
    }
    println!("{}", if failed == 0 { "bench: all gaits inside the envelope" } else { "bench: FAILED" });
    if failed == 0 { 0 } else { 1 }
}

/// RFC 8032 test vector 1 through the same verifier the handshake uses.
fn cmd_selftest() -> i32 {
    let pk = parse_hex32("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a").unwrap();
    let sig = wire::hex_decode("e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b").unwrap();
    let vk = VerifyingKey::from_bytes(&pk).unwrap();
    let s = Signature::from_bytes(<&[u8; 64]>::try_from(sig.as_slice()).unwrap());
    let ok = vk.verify(b"", &s).is_ok();
    let bad = vk.verify(b"x", &s).is_err();
    println!("ed25519 selftest: {}", if ok && bad { "ok" } else { "FAILED" });
    if ok && bad { 0 } else { 1 }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("serve") => cmd_serve(&args[1..]),
        Some("bench") => std::process::exit(cmd_bench()),
        Some("selftest") => std::process::exit(cmd_selftest()),
        Some("hands") => println!("{}", schema::HANDS.join("\n")),
        Some("schema") => println!("{}", Model::default().schema_json()),
        _ => usage(),
    }
}
