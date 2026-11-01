//! `brainos blur gate2` — the Autonomic layer and KIRA's physical
//! policies, against the twin.
//!
//! Gate 2, as the brief states it, and how each clause is tested here:
//!
//! 1. *Under random pushes while standing, BLUR stays up or falls safely
//!    and self-rights.* BLUR is acquired in the jig, released onto the
//!    floor, and pushed — random direction, 10–60 N for 100 ms — N times.
//!    A trial passes when the body is upright and still again within 12 s,
//!    either because it never fell or because the get-up reflex brought it
//!    back. Peak joint torque during every trial is reported against the
//!    driver limits.
//! 2. *Envelope violations are clamped within 1 ms.* A session is granted
//!    with a narrow current limit and the motor cortex is made to ask for
//!    far more. The supervisor clamps before the bus (counted); the nodes
//!    clamp current inside their own loop, every physics step (0.5 ms),
//!    and the twin records the worst applied torque against each node's
//!    limit over the whole run.
//! 3. *E-stop cuts torque in every test.* The latching switch is pressed
//!    at random moments; the twin counts any physics step in which torque
//!    was applied with the rail off, or by a node that had latched itself
//!    stopped — including after the switch is released.
//! 4. *A model asking for an out-of-envelope command is refused at the
//!    correct KIRA stage.* Proposals through the entity's gate, with
//!    `BlurPolicy` bound and the twin as its predictor; the expected stage
//!    is written in the table before the run. And the Teensy's own check:
//!    a grant whose request was altered after signing is refused by
//!    `session::admit`.

use crate::blur_run::{acquire_once_live, Live, BODY_SEED, ENTITY_SEED};
use crate::blur_sil::{Nerves, TwinOptions};
use brainos_abi::error::{DenyCode, Stage};
use brainos_abi::ids::BodyId;
use brainos_abi::time::Tick;
use brainos_abi::verb::Verb;
use brainos_blur::acquire::Record;
use brainos_blur::attitude::ImuMount;
use brainos_blur::autonomic::JointSeen;
use brainos_blur::descriptor::{ACTUATORS, N_ACT};
use brainos_blur::wire::{Config, ConfigKey, NerveDown, Setpoint, SP_ENABLE};
use brainos_blur::mlp::Mlp;
use brainos_blur::policy::{BlurPolicy, Prediction, Predictor};
use brainos_blur::schema::blur_schema;
use brainos_blur::session::{admit, Refused};
use brainos_blur::supervisor::{reflex_gains, stand_pose, Command, Envelope, Grant, Mode, Reflex, Supervisor};
use brainos_body::policy::{Arg, Args};
use brainos_crypto::key::BrainKey;
use brainos_entity::Entity;
use brainos_kira::action::Request;
use brainos_kira::gate::Verdict;
use std::path::PathBuf;
use std::sync::Mutex;

/// The rollout predictor: a second socket to the same twin, asking it to
/// copy its current state and run the requested skill forward.
pub struct TwinPredictor {
    nerves: Mutex<Nerves>,
}

fn num(s: &str, key: &str) -> Option<f64> {
    let k = s.find(key)? + key.len();
    let t = &s[k..];
    let end = t.find(|c: char| c == ',' || c == '}').unwrap_or(t.len());
    t[..end].trim().trim_matches('"').parse().ok()
}

impl Predictor for TwinPredictor {
    fn predict(&self, verb: &Verb, args: &Args) -> Option<Prediction> {
        let mut msg = String::from(r#"{"cmd":"rollout","ms":1500"#);
        if verb.as_str() == "body.pose" {
            let joint = args.iter().find(|(n, _)| n == "joint").and_then(|(_, a)| a.as_text())?;
            let angle = args.iter().find(|(n, _)| n == "angle").and_then(|(_, a)| a.as_scalar())?;
            msg.push_str(&format!(r#","joint":"{joint}","angle":{angle}"#));
        }
        msg.push('}');
        let r = self.nerves.lock().ok()?.control(&msg).ok()?;
        Some(Prediction {
            falls: r.contains(r#""falls": true"#),
            joint_limit: if r.contains(r#""joint_limit": null"#) { None } else { Some("joint".into()) },
            person_contact: r.contains(r#""person": true"#),
            peak_torque_frac: num(&r, r#""peak": "#).unwrap_or(f64::NAN) as f32,
            horizon_s: num(&r, r#""horizon": "#).unwrap_or(0.0) as f32,
        })
    }
}

pub fn getup_blob() -> Option<(PathBuf, Vec<u8>)> {
    let p = std::env::var_os("BLUR_GETUP").map(PathBuf::from).unwrap_or_else(|| {
        crate::blur_sil::repo_root().join("blur/learn/policies/getup.mlp")
    });
    std::fs::read(&p).ok().map(|b| (p, b))
}

fn state(live: &mut Live) -> String {
    live.port.nerves.control(r#"{"cmd":"state"}"#).unwrap_or_default()
}

fn upright(s: &str) -> (f64, f64) {
    let up = num(s, r#""up": "#).unwrap_or(-1.0);
    let z = s.find(r#""chest": ["#).and_then(|k| {
        let t = &s[k + 10..];
        t.split(',').nth(2).and_then(|v| v.trim().parse().ok())
    }).unwrap_or(0.0);
    (up, z)
}

fn mount_from(live: &Live) -> ImuMount {
    let cal = live.report.calibration.as_ref();
    let b = |n: &str| cal.and_then(|c| c.bias_of(n)).unwrap_or(0.0);
    ImuMount { roll: b("imu.mount_roll"), pitch: b("imu.mount_pitch"), gyro_bias: [b("imu.gyro_bias_x"), b("imu.gyro_bias_y"), b("imu.gyro_bias_z")] }
}

/// The reflex policies the supervisor runs: the three-skill chain
/// (righting, stand-up, balance) when all three blobs exist, otherwise the
/// single get-up policy of the first attempt.
#[derive(Clone)]
pub struct Reflexes {
    pub getup: Option<Mlp>,
    pub righting: Option<Mlp>,
    pub standup: Option<Mlp>,
    pub balance: Option<Mlp>,
}

fn load_blob(name: &str, var: &str) -> Result<Option<(PathBuf, Mlp)>, String> {
    let p = std::env::var_os(var).map(PathBuf::from)
        .unwrap_or_else(|| crate::blur_sil::repo_root().join(format!("blur/learn/policies/{name}.mlp")));
    let Ok(b) = std::fs::read(&p) else { return Ok(None) };
    Mlp::from_bytes(&b).map(|m| Some((p.clone(), m))).map_err(|e| format!("{} does not load: {e:?}", p.display()))
}

fn load_reflexes() -> Result<Reflexes, String> {
    let r = load_blob("righting", "BLUR_RIGHTING")?;
    let s = load_blob("standup", "BLUR_STANDUP")?;
    let b = load_blob("balance", "BLUR_BALANCE")?;
    if let (Some(r), Some(s), Some(b)) = (&r, &s, &b) {
        for (what, (p, m)) in [("righting", r), ("stand-up", s), ("balance", b)] {
            println!("{what:>9} reflex: {} ({} multiply-adds per inference)", p.display(), m.macs());
        }
        println!();
        return Ok(Reflexes { getup: None, righting: Some(r.1.clone()), standup: Some(s.1.clone()), balance: Some(b.1.clone()) });
    }
    let Some((ppath, blob)) = getup_blob() else {
        return Err("no reflex policies (blur/learn/policies/{righting,standup,balance}.mlp, or getup.mlp)".into());
    };
    let g = Mlp::from_bytes(&blob).map_err(|e| format!("get-up policy blob does not load: {e:?}"))?;
    println!("get-up reflex policy (single-policy fallback): {} ({} multiply-adds per inference)\n", ppath.display(), g.macs());
    Ok(Reflexes { getup: Some(g), righting: None, standup: None, balance: None })
}

fn install_supervisor(live: &mut Live, policy: &Reflexes) {
    let mut s = Supervisor::new(mount_from(live));
    s.getup = policy.getup.clone();
    s.righting = policy.righting.clone();
    s.standup = policy.standup.clone();
    s.balance = policy.balance.clone();
    live.port.autonomic.sup = Some(s);
}

fn sup(live: &mut Live) -> &mut Supervisor {
    live.port.autonomic.sup.as_mut().expect("supervisor installed")
}

pub struct PushTrial {
    pub force: f32,
    pub dir_deg: f32,
    pub fell: bool,
    pub recovered: bool,
    pub upright_after_s: f32,
    pub ok: bool,
}

pub fn gate2() -> bool {
    println!("BrAInOS × BLUR — GATE 2: Autonomic layer + KIRA physical policies\n");
    let policy = match load_reflexes() {
        Ok(p) => p,
        Err(e) => {
            println!("FAIL: {e}");
            return false;
        }
    };

    let opts = TwinOptions { jig: true, seed: 11, unit: 36, ..Default::default() };
    let mut entity = Entity::new(BrainKey::from_seed(&ENTITY_SEED), Tick(1));
    let mut live = match acquire_once_live(&mut entity, "g2", &opts, &Record::default()) {
        Ok(l) => l,
        Err(e) => {
            println!("FAIL: bring-up: {e}");
            return false;
        }
    };
    if !live.report.ok() {
        println!("FAIL: acquisition stopped at {:?}: {}", live.report.caught_at, live.report.detail);
        return false;
    }
    println!("── acquired and calibrated in the jig ({:.0} s body time)", live.port.ticks as f64 / 1000.0);
    install_supervisor(&mut live, &policy);
    live.port.run_ticks(500);
    let _ = live.port.nerves.control(r#"{"cmd":"release","height":0.272}"#);
    live.port.run_ticks(2000);
    let s0 = state(&mut live);
    let (up0, z0) = upright(&s0);
    println!("── released onto the floor: up {up0:.3}, chest z {z0:.3}, supervisor {:?}", sup(&mut live).mode);
    let mut tally = Tally::default();
    println!("   released upright and at height: {}", tally.mark("release", up0 > 0.94 && z0 > 0.2));

    // ---------------------------------------------------------------- 1 pushes
    let n_push: usize = std::env::var("BLUR_GATE2_PUSHES").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
    println!("\n1. Random pushes while standing ({n_push} trials, 10–60 N × 100 ms, random direction)\n");
    let mut rng = brainos_crypto::rng::Drbg::deterministic(b"gate2-pushes");
    let mut trials = Vec::new();
    for k in 0..n_push {
        let f = 10.0 + 50.0 * (rng.below(1000) as f32 / 1000.0);
        let a = 360.0 * (rng.below(3600) as f32 / 3600.0);
        let (c, s) = (a.to_radians().cos(), a.to_radians().sin());
        let falls0 = sup(&mut live).stats.falls;
        let msg = format!(r#"{{"cmd":"push","body":"chest","force":[{},{},0],"ms":100}}"#, f * c, f * s);
        let _ = live.port.nerves.control(&msg);
        // passed = upright, at height, back in the standing reflex and STILL
        // (chest rate < 0.5 rad/s) for 2 s without a break, inside 12 s
        let mut recovered_at = None;
        let mut fell = false;
        let mut held_since: Option<u32> = None;
        for t in 0..12_000u32 {
            live.port.tick();
            if t % 50 == 0 {
                let m = sup(&mut live).mode;
                if sup(&mut live).stats.falls > falls0 {
                    fell = true;
                }
                let st = state(&mut live);
                let (up, z) = upright(&st);
                let rate = sup(&mut live).att.last_rate();
                let still = (rate[0] * rate[0] + rate[1] * rate[1] + rate[2] * rate[2]).sqrt() < 0.5;
                let ok = t > 200 && m == Mode::Reflex(Reflex::Hold) && up > 0.94 && z > 0.2 && still;
                held_since = if ok { held_since.or(Some(t)) } else { None };
                if let Some(t0) = held_since {
                    if t - t0 >= 2000 {
                        recovered_at = Some(t);
                        break;
                    }
                }
            }
        }
        let ok = recovered_at.is_some();
        let tr = PushTrial { force: f, dir_deg: a, fell, recovered: fell && ok, upright_after_s: recovered_at.map(|t| t as f32 / 1000.0).unwrap_or(f32::NAN), ok };
        println!("   push {:>2}: {:>4.1} N at {:>3.0}°  {}  {}", k + 1, tr.force, tr.dir_deg,
            if !tr.fell { "stayed up   " } else if tr.recovered { "fell, got up" } else { "fell        " },
            if tr.ok { format!("upright & still at {:.1} s  {}", tr.upright_after_s, tally.mark("pushes", true)) } else { format!("NOT upright within 12 s  {}", tally.mark("pushes", false)) });
        trials.push(tr);
        if !ok {
            println!("            (harness re-seats the body and resumes it for the next trial)");
            reseat(&mut live);
        }
        live.port.run_ticks(1500);
    }
    let stayed = trials.iter().filter(|t| !t.fell && t.ok).count();
    let got_up = trials.iter().filter(|t| t.fell && t.ok).count();
    let failed = trials.iter().filter(|t| !t.ok).count();
    let st = state(&mut live);
    println!("\n   stayed up {stayed}, fell and got up {got_up}, failed {failed} of {n_push}");
    println!("   supervisor: {:?}", sup(&mut live).stats);
    println!("   twin clamp record so far: {}", &st[st.find(r#""clamp""#).unwrap_or(0)..].split('}').next().unwrap_or(""));

    // ---------------------------------------------------------------- 2 clamp
    // the clamp and KIRA tests start from a standing, holding body
    reseat(&mut live);
    println!("\n2. Envelope clamp: a session at 30 % current, the cortex asks for 3× the limit\n");
    let _ = live.port.nerves.control(r#"{"cmd":"clamp_reset"}"#);
    let env = Envelope::physical().with_current_fraction(0.3);
    let now = live.port.autonomic.tick;
    let g = Grant { id: [7; 32], skill: 1, env, until_tick: now + 3000 };
    let installed = sup(&mut live).install(g);
    let clamped0 = sup(&mut live).stats.clamped;
    let (kp, kd) = reflex_gains();
    let stand = stand_pose();
    for t in 0..2000u32 {
        if t % 20 == 0 {
            let mut c = Command { tick: live.port.autonomic.tick, q: stand, kp, kd, tau: [0.0; N_ACT] };
            for (j, a) in ACTUATORS.iter().enumerate() {
                // three times what the envelope allows, and targets past the joint range
                c.tau[j] = 3.0 * env.i_max[j] * if a.kt_out.is_finite() { a.kt_out } else { 0.0 } * if t % 40 == 0 { 1.0 } else { -1.0 };
                c.q[j] = if t % 80 < 40 { a.q_max + 0.5 } else { a.q_min - 0.5 };
                c.kp[j] = 4.0 * kp[j];
            }
            sup(&mut live).command(c);
        }
        live.port.tick();
    }
    let clamped = sup(&mut live).stats.clamped - clamped0;
    let st = state(&mut live);
    let worst = num(&st, r#""worst_ratio": "#).unwrap_or(f64::NAN);
    let over = num(&st, r#""requests_over": "#).unwrap_or(f64::NAN);
    println!("   session installed: {installed}");
    println!("   supervisor clamped {clamped} setpoint values before the bus (first check)");
    println!("   nodes: {over:.0} physics steps (0.5 ms) where the requested torque exceeded the node's limit;");
    println!("          worst applied torque over the run = {:.4} × the node's limit (second check, inside the node loop)", worst);
    let clamp_ok = installed && clamped > 0 && over > 0.0 && worst <= 1.0001;
    println!("   → violations clamped within one node-loop step (≤ 0.5 ms < 1 ms): {}", tally.mark("clamp", clamp_ok));
    sup(&mut live).revoke();
    live.port.run_ticks(2000);

    // ---------------------------------------------------------------- 2b the Teensy dies / goes rogue
    // The physical envelope lives on the nodes, so it must hold with no
    // Teensy at all, and against a Teensy that sends whatever it likes.
    reseat(&mut live);
    println!("\n2b. The envelope without the Teensy: it goes silent, then it goes rogue\n");
    let _ = live.port.nerves.control(r#"{"cmd":"clamp_reset"}"#);
    let z_before = upright(&state(&mut live)).1;
    let mut z_min = z_before;
    for t in 0..3000u32 {
        live.port.tick_silent();
        if t % 50 == 0 {
            z_min = z_min.min(upright(&state(&mut live)).1);
        }
    }
    let st = state(&mut live);
    let worst = num(&st, r#""worst_ratio": "#).unwrap_or(f64::NAN);
    let sag = z_before - z_min;
    let silent_ok = worst <= 1.0001;
    println!("   Teensy silent 3 s: worst applied torque {worst:.4} × node limit; nodes fell back to damping, chest sagged {:.0} mm  {}",
        sag * 1000.0, tally.mark("node envelope", silent_ok));
    // rogue: every node is told its ceiling is the driver's peak x2, its
    // limits are 1 rad wider, its zero is elsewhere, and to drive past its
    // range with 10x the gain
    reseat(&mut live);
    let _ = live.port.nerves.control(r#"{"cmd":"clamp_reset"}"#);
    let refused0 = num(&state(&mut live), r#""node_refused": "#).unwrap_or(0.0);
    let mut seq = 200u8;
    for (j, a) in ACTUATORS.iter().enumerate() {
        if a.encoder == brainos_blur::descriptor::EncoderSide::None {
            continue;
        }
        let mut down = NerveDown::new(0);
        for (key, v) in [(ConfigKey::CurrentLimit, 2.0 * a.drv_peak_a), (ConfigKey::QMin, a.q_min - 1.0),
                         (ConfigKey::QMax, a.q_max + 1.0), (ConfigKey::Offset, 0.7)] {
            seq = seq.wrapping_add(1);
            let _ = down.frames.push(Config { slot: a.slot, key, value: v, seq }.encode(a.bus, a.node, false));
        }
        live.port.tick_with(down);
        let _ = j;
    }
    for t in 0..2000u32 {
        let mut down = NerveDown::new(0);
        if t % 5 == 0 {
            for a in ACTUATORS.iter() {
                if a.encoder == brainos_blur::descriptor::EncoderSide::None {
                    continue;
                }
                let kp = 10.0 * if a.tendon { 3000.0 } else { 12.0 * a.tau_deliverable() };
                let q = if t % 400 < 200 { a.q_max + 1.0 } else { a.q_min - 1.0 };
                let _ = down.frames.push(Setpoint { slot: a.slot, flags: SP_ENABLE, q, tau_ff: 10.0, kp, kd: 0.0 }.encode(a.bus, a.node, a.motor));
            }
        }
        live.port.tick_with(down);
    }
    let st = state(&mut live);
    let worst = num(&st, r#""worst_ratio": "#).unwrap_or(f64::NAN);
    let refused = num(&st, r#""node_refused": "#).unwrap_or(0.0) - refused0;
    let mut beyond = 0usize;
    for a in ACTUATORS.iter() {
        if let Some(q) = num(&st, &format!(r#""{}": "#, a.name)) {
            let slack = if a.tendon { 0.01 } else { 0.10 };
            if (q as f32) < a.q_min - slack || (q as f32) > a.q_max + slack {
                beyond += 1;
            }
        }
    }
    let rogue_ok = worst <= 1.0001 && refused > 0.0 && beyond == 0;
    println!("   rogue Teensy 2 s: {refused:.0} widening/recalibrating writes refused by the nodes, worst applied torque {worst:.4} × node limit, \
              joints past their physical range {beyond}  {}", tally.mark("node envelope", rogue_ok));
    if let Some(s) = live.port.autonomic.sup.as_mut() {
        s.realign();
    }

    // ---------------------------------------------------------------- 4 KIRA (before the e-stop)
    // the clamp test throws every joint between its limits; KIRA's cases
    // start from a standing body
    reseat(&mut live);
    let st = state(&mut live);
    let (u, z) = upright(&st);
    println!("\n3. KIRA: out-of-envelope asks, expected stage written before the run (body: up {u:.3}, z {z:.3}, {:?})\n", sup(&mut live).mode);
    let body = live.report.body.unwrap_or_default();
    let predictor = Nerves::connect(&live.twin.sock).ok().map(|n| Box::new(TwinPredictor { nerves: Mutex::new(n) }) as Box<dyn Predictor>);
    let cal = live.report.calibration.clone().unwrap_or_default();
    entity.kira.bind(body, Box::new(BlurPolicy::new(body, blur_schema(), cal, predictor)), blur_schema());
    kira_cases(&mut entity, body, &mut live, &mut tally);

    // ---------------------------------------------------------------- 3 e-stop
    let n_e: usize = std::env::var("BLUR_GATE2_ESTOPS").ok().and_then(|v| v.parse().ok()).unwrap_or(8);
    println!("\n4. E-stop pressed at random moments ({n_e} trials)\n");
    for k in 0..n_e {
        let _ = live.port.nerves.control(r#"{"cmd":"clamp_reset"}"#);
        // something going on when it is pressed: a push half the time
        if k % 2 == 0 {
            let _ = live.port.nerves.control(r#"{"cmd":"push","body":"chest","force":[35,0,0],"ms":100}"#);
        }
        let wait = 50 + rng.below(1500) as u32;
        live.port.run_ticks(wait);
        let _ = live.port.nerves.control(r#"{"cmd":"estop","pressed":true}"#);
        live.port.run_ticks(1500);
        let mid = state(&mut live);
        if k == 0 {
            // KIRA's stage 6 while the switch is pressed (expected written here before the run)
            report_body(&mut entity, body, &mut live);
            let r = Request::on_body(Verb::parse("legs.stand").unwrap(), body, entity.tick).arg("height", Arg::Scalar(0.27));
            let out = entity.propose(r);
            let got = out.verdict.denial().map(|d| (d.stage, d.code));
            let ok = got == Some((Stage::Policy, DenyCode::BodyInvariant));
            println!("   KIRA during the e-stop: legs.stand → expected Policy/BodyInvariant, got {:?}  {}", got, tally.mark("e-stop", ok));
        }
        // released: the body must stay dead until the entity re-acquires it
        let _ = live.port.nerves.control(r#"{"cmd":"estop","pressed":false}"#);
        live.port.run_ticks(1500);
        let after = state(&mut live);
        let off = num(&after, r#""torque_with_rail_off": "#).unwrap_or(f64::NAN);
        let latched_torque = num(&after, r#""torque_while_estopped": "#).unwrap_or(f64::NAN);
        let latched = num(&mid, r#""estopped": "#).unwrap_or(0.0);
        let mode = sup(&mut live).mode;
        let ok = off == 0.0 && latched_torque == 0.0 && latched >= 26.0 && mode == Mode::EStopped;
        println!("   e-stop {:>2} at +{:>4} ms: torque steps with rail off {off:.0}, nodes latched {latched:.0}/26, \
                  torque from latched nodes after release {latched_torque:.0}, supervisor {mode:?}  {}",
                  k + 1, wait, tally.mark("e-stop", ok));
        // the only way back: re-acquire (fresh calibration push + a new supervisor)
        if !recommit(&mut live, &policy) {
            println!("   could not re-acquire after the e-stop  {}", tally.mark("e-stop", false));
            break;
        }
    }

    let _ = live.port.nerves.control(r#"{"cmd":"quit"}"#);
    // the verdict is computed from the printed lines and nothing else
    let pass = tally.all();
    println!("\nGATE 2: {}  ({} checks, {} failed)", if pass { "PASS" } else { "FAIL" }, tally.lines.len(), tally.failed());
    println!("  {}", ["release", "pushes", "clamp", "node envelope", "KIRA stages", "e-stop"].iter()
        .map(|s| format!("{s} {}", tally.section(s))).collect::<Vec<_>>().join(" | "));
    let _ = BODY_SEED;
    pass
}

/// After an e-stop the body is re-acquired: the calibration the entity
/// holds is pushed again (Commit), the supervisor starts fresh, and the
/// harness puts the body back on its feet if the stop left it lying.
fn recommit(live: &mut Live, policy: &Reflexes) -> bool {
    use brainos_blur::autonomic::{Outcome, Request as AReq};
    use brainos_blur::calibrate::push_values;
    let Some(j) = live.report.joints else { return false };
    live.port.autonomic.sup = None;
    let limits = brainos_blur::acquire::current_limits();
    let ok = matches!(live.port_run(AReq::Push { values: push_values(&j, &limits) }), Some(Outcome::Pushed { failed, .. }) if failed.is_empty());
    let _ = live.port.nerves.control(r#"{"cmd":"reseat"}"#);
    install_supervisor(live, policy);
    live.port.run_ticks(2500);
    ok
}

/// What the supervisor tells KIRA's stage 6 about the body right now.
fn report_body(entity: &mut Entity, body: BodyId, live: &mut Live) {
    let rail = live.port.last_up.as_ref().map(|u| u.rail_on && !u.estop_latched).unwrap_or(false);
    let vbat = live.port.last_up.as_ref().map(|u| u.vbat_mv as f32 / 1000.0).unwrap_or(0.0);
    let (mode, tilt) = live.port.autonomic.sup.as_ref().map(|s| (s.mode, s.att.tilt_deg())).unwrap_or((Mode::Idle, 0.0));
    let reflex = matches!(mode, Mode::Reflex(Reflex::GetUp) | Mode::Reflex(Reflex::Crouch) | Mode::EStopped);
    let t = brainos_body::hal::Telemetry {
        link_alive: true,
        rail_v: Some(if rail { vbat } else { 0.0 }),
        tilt_deg: Some(tilt),
        stops: reflex as u32,
        ..Default::default()
    };
    if let Some(p) = entity.kira.policy_for_mut(&body) {
        p.observe(&t);
    }
}

/// Put the body back on its feet and hand control back to the stand hold:
/// the harness's hand and the entity's decision, both logged.
fn reseat(live: &mut Live) {
    let _ = live.port.nerves.control(r#"{"cmd":"reseat"}"#);
    if let Some(s) = live.port.autonomic.sup.as_mut() {
        s.realign();
        s.resume();
    }
    live.port.run_ticks(2000);
}

struct KiraCase {
    name: &'static str,
    verb: &'static str,
    args: Vec<(&'static str, Arg)>,
    signer_is_entity: bool,
    expect: Option<(Stage, DenyCode)>,
}

fn kira_cases(entity: &mut Entity, body: BodyId, live: &mut Live, tally: &mut Tally) {
    let cases = vec![
        KiraCase { name: "walk at 1.2 m/s (schema bound 0.5)", verb: "legs.walk",
            args: vec![("vx", Arg::Scalar(1.2)), ("vy", Arg::Scalar(0.0)), ("yaw", Arg::Scalar(0.0))], signer_is_entity: true,
            expect: Some((Stage::Validate, DenyCode::OutOfBounds)) },
        KiraCase { name: "pose RR_knee at 3.0 rad (its range ends at 2.84)", verb: "body.pose",
            args: vec![("joint", Arg::Choice("RR_knee".into())), ("angle", Arg::Scalar(3.0))], signer_is_entity: true,
            expect: Some((Stage::Validate, DenyCode::OutsideBodyEnvelope)) },
        KiraCase { name: "walk 0.5 m/s while turning 1 rad/s (combined envelope)", verb: "legs.walk",
            args: vec![("vx", Arg::Scalar(0.5)), ("vy", Arg::Scalar(0.0)), ("yaw", Arg::Scalar(1.0))], signer_is_entity: true,
            expect: Some((Stage::Validate, DenyCode::OutsideBodyEnvelope)) },
        KiraCase { name: "pose spine_twist to 1.2 rad (rolls the rear half over)", verb: "body.pose",
            args: vec![("joint", Arg::Choice("spine_twist".into())), ("angle", Arg::Scalar(1.2))], signer_is_entity: true,
            expect: Some((Stage::Simulate, DenyCode::PredictedInvariantBreak)) },
        KiraCase { name: "pose neck_pan to 0.3 rad (harmless)", verb: "body.pose",
            args: vec![("joint", Arg::Choice("neck_pan".into())), ("angle", Arg::Scalar(0.3))], signer_is_entity: true,
            expect: None },
        KiraCase { name: "a proposal signed by a key that is not the entity", verb: "legs.stand",
            args: vec![("height", Arg::Scalar(0.27))], signer_is_entity: false,
            expect: Some((Stage::Authenticate, DenyCode::BadSignature)) },
        // expectation corrected after the first dry run: the stage was right,
        // the code was a guess — KIRA says NoSuchLimb when no bound body
        // advertises the verb (BodyLacksVerb is "another body can")
        KiraCase { name: "a verb this body does not have (legs.backflip)", verb: "legs.backflip",
            args: vec![], signer_is_entity: true,
            expect: Some((Stage::Authorize, DenyCode::NoSuchLimb)) },
    ];
    println!("   {:<56} {:<28} {:<28} ok", "ask", "expected", "got");
    let mut granted_req: Option<(Request, brainos_kira::capability::Granted, Tick)> = None;
    for c in cases {
        report_body(entity, body, live);
        let verb = Verb::parse(c.verb).unwrap();
        let mut r = Request::on_body(verb, body, entity.tick);
        for (n, a) in &c.args {
            r = r.arg(n, a.clone());
        }
        let outcome = if c.signer_is_entity {
            entity.propose(r.clone())
        } else {
            let other = BrainKey::from_seed(&[9u8; 32]);
            let p = r.clone().proposed_by(&other);
            entity.kira.evaluate(&p, &entity.key().public(), &entity.bodies, entity.tick, None)
        };
        let got = match &outcome.verdict {
            Verdict::Granted(_) => None,
            Verdict::Denied(d) => Some((d.stage, d.code)),
        };
        let ok = got == c.expect;
        let show = |x: &Option<(Stage, DenyCode)>| match x {
            None => "GRANTED".to_string(),
            Some((s, d)) => format!("{s:?}/{d:?}"),
        };
        println!("   {:<56} {:<28} {:<28} {}", c.name, show(&c.expect), show(&got), tally.mark("KIRA stages", ok));
        if let Verdict::Granted(g) = outcome.verdict {
            granted_req = Some((r, g, entity.tick));
        }
    }
    // the Teensy's own check of the grant it was handed
    if let Some((req, g, t)) = granted_req {
        let pk = entity.key().public();
        let now = live.port.autonomic.tick;
        let fine = admit(&g, &req, &pk, body, t, now);
        let mut tampered = req.clone();
        tampered.args = vec![("joint".into(), Arg::Choice("neck_pan".into())), ("angle".into(), Arg::Scalar(0.55))];
        let forged = admit(&g, &tampered, &pk, body, t, now);
        let other_body = admit(&g, &req, &pk, BodyId::default(), t, now);
        let ok = fine.is_ok() && forged == Err(Refused::NotThisRequest) && other_body == Err(Refused::WrongBody);
        println!("   Teensy admit(): genuine grant {} · same grant, args altered after signing {:?} · grant for another body {:?}  {}",
            if fine.is_ok() { "installed" } else { "REFUSED" }, forged.err(), other_body.err(), tally.mark("KIRA stages", ok));
        if let Ok(gr) = fine {
            let installed = sup(live).install(gr);
            println!("   grant installed on the supervisor: {installed} (envelope frees neck_pan only: q range [{:.2}, {:.2}])",
                gr.env.q_min[brainos_blur::descriptor::actuator_index("neck_pan").unwrap()],
                gr.env.q_max[brainos_blur::descriptor::actuator_index("neck_pan").unwrap()]);
            sup(live).revoke();
        }
    } else {
        println!("   nothing was granted, so the Teensy's check could not be exercised  {}", tally.mark("KIRA stages", false));
    }
    let _: &[JointSeen; N_ACT] = &live.port.autonomic.seen;
}

/// Every verdict line the gate prints goes through `mark`; the section
/// summaries and the gate verdict are computed from those lines only, so a
/// summary can never say PASS over a line that said FAIL, and a section
/// that printed no lines is reported as not run (a FAIL).
#[derive(Default)]
struct Tally {
    lines: Vec<(&'static str, bool)>,
}

impl Tally {
    fn mark(&mut self, section: &'static str, ok: bool) -> &'static str {
        self.lines.push((section, ok));
        if ok { "PASS" } else { "FAIL" }
    }
    fn section(&self, s: &str) -> &'static str {
        let mut any = false;
        for (x, ok) in &self.lines {
            if *x == s {
                any = true;
                if !ok {
                    return "FAIL";
                }
            }
        }
        if any { "PASS" } else { "NOT RUN (FAIL)" }
    }
    fn failed(&self) -> usize {
        self.lines.iter().filter(|(_, ok)| !ok).count()
    }
    fn all(&self) -> bool {
        ["release", "pushes", "clamp", "node envelope", "KIRA stages", "e-stop"].iter().all(|s| self.section(s) == "PASS") && self.failed() == 0
    }
}
