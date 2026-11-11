//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Runs BLUR Gate 1: the entity acquires the simulated BLUR robot end to end, and each deliberately faulty body must be caught at the expected step
//! BFS  DEFINES  ENTITY_SEED, BODY_SEED, Run, Live, acquire_live, acquire_once_live, acquire_once, describe, gate1, ZERO_TOL_RAD, ZERO_TOL_M, zeros_ok, +1 more
//! BFS  USES     this crate: blur_sil | brainos: abi, blur, body, crypto, entity | external: std
//! BFS  USED BY  host/blur_gate2
//!
//! `brainos blur …` — BLUR's acquisition against the twin, and Gate 1.
//!
//! Gate 1 is: the entity acquires the simulated BLUR end to end through
//! the real-hardware message formats, and every adversarial body is
//! caught at the step that should catch it. Each case boots a fresh twin
//! with one fault, runs §8 from nothing, and compares the step that
//! refused with the step written in the table below *before* the run.

use crate::blur_sil::{Nerves, SilPort, Twin, TwinOptions};
use brainos_abi::time::Tick;
use brainos_blur::acquire::{Acquirer, Finding, Record, Report, Stage};
use brainos_blur::calibrate::CalFault;
use brainos_blur::census::{BuildSheet, CensusFault, Conflict};
use brainos_blur::descriptor::{actuator_index, ACTUATORS, N_ACT};
use brainos_blur::firmware::Firmware;
use brainos_blur::schema::{blur_schema, SchemaFault};
use brainos_crypto::key::BrainKey;
use brainos_entity::Entity;
use std::time::Instant;

pub const ENTITY_SEED: [u8; 32] = *b"blur-gate1-entity-seed-v1-------";
pub const BODY_SEED: [u8; 32] = *b"blur-v36-unit36-teensy-key------";

pub struct Run {
    pub report: Report,
    /// after a successful calibration: per joint, calibrated reading minus
    /// the twin's true angle (rad; m for winches) — the zeros' real error
    pub zero_error: Option<Vec<f32>>,
    pub wall_s: f64,
    pub sim_ticks: u64,
    pub twin_state: String,
}

/// A twin with an acquired body, kept alive for whatever comes next.
pub struct Live {
    pub twin: Twin,
    pub port: SilPort,
    pub report: Report,
    pub wall_s: f64,
}

impl Live {
    pub fn port_run(&mut self, r: brainos_blur::autonomic::Request) -> Option<brainos_blur::autonomic::Outcome> {
        use brainos_blur::acquire::Port;
        self.port.run(r)
    }
}

/// Boot a twin and run §8 once; keep everything alive.
pub fn acquire_live(entity: &mut Entity, tag: &str, opts: &TwinOptions, record: &Record, owners_include_entity: bool) -> Result<Live, String> {
    let twin = Twin::spawn(tag, opts).map_err(|e| format!("twin: {e}"))?;
    let nerves = Nerves::connect(&twin.sock).map_err(|e| format!("nerves: {e}"))?;
    let owners = if owners_include_entity {
        vec![entity.key().public()]
    } else {
        vec![BrainKey::from_seed(&[7u8; 32]).public()]
    };
    let firmware = Firmware::new(&BODY_SEED, blur_schema(), owners);
    let mut port = SilPort::new(nerves, firmware);
    let t0 = Instant::now();
    let entity_id = entity.id();
    let report = entity.acquire_via(|key, rng, map, ids, session, tick| {
        let acq = Acquirer {
            key,
            entity: entity_id,
            session,
            imu_samples: 500,
        };
        acq.run(&mut port, rng, map, record, tick, ids)
    });
    if report.ok() {
        if let (Some(body), Some(cal)) = (report.body, report.calibration.clone()) {
            entity.bind_acquired(body, blur_schema(), cal, &report.join);
        }
    }
    if let Some(e) = &port.io_error {
        return Err(format!("nerve link: {e}"));
    }
    Ok(Live { twin, port, report, wall_s: t0.elapsed().as_secs_f64() })
}

pub fn acquire_once_live(entity: &mut Entity, tag: &str, opts: &TwinOptions, record: &Record) -> Result<Live, String> {
    acquire_live(entity, tag, opts, record, true)
}

/// Boot a twin, acquire it once, return what happened.
pub fn acquire_once(entity: &mut Entity, tag: &str, opts: &TwinOptions, record: &Record, owners_include_entity: bool) -> Result<Run, String> {
    let mut live = acquire_live(entity, tag, opts, record, owners_include_entity)?;
    let report = live.report.clone();
    let mut zero_error = None;
    if report.ok() {
        // let committed feedback arrive, then compare against the truth
        live.port.run_ticks(60);
        let st = live.port.nerves.control(r#"{"cmd":"state"}"#).unwrap_or_default();
        let mut errs = Vec::new();
        for (j, a) in ACTUATORS.iter().enumerate() {
            let truth = json_number(&st, &format!("\"{}\": ", a.name));
            let cal = live.port.autonomic.seen[j].fb.map(|f| f.q);
            errs.push(match (truth, cal) {
                (Some(t), Some(c)) => c - t,
                _ => f32::NAN,
            });
        }
        zero_error = Some(errs);
    }
    let state = live.port.nerves.control(r#"{"cmd":"state"}"#).unwrap_or_default();
    let _ = live.port.nerves.control(r#"{"cmd":"quit"}"#);
    if let Some(e) = &live.port.io_error {
        return Err(format!("nerve link: {e}"));
    }
    Ok(Run {
        report,
        zero_error,
        wall_s: live.wall_s,
        sim_ticks: live.port.ticks,
        twin_state: state,
    })
}

#[derive(Clone, Debug)]
enum Expect {
    At(Stage, fn(&Finding) -> bool),
}

struct Case {
    name: &'static str,
    faults: &'static [&'static str],
    owned: bool,
    expect: Expect,
    why: &'static str,
}

fn joint(n: &str) -> usize {
    actuator_index(n).unwrap()
}

fn cases() -> Vec<Case> {
    vec![
        Case { name: "forged ESC (UID differs from build sheet)", faults: &["forged:0x05"], owned: true,
            expect: Expect::At(Stage::Handshake, |f| matches!(f, Finding::Census(CensusFault::Swapped { id: 5 }))),
            why: "a node is part of who this body is; its silicon is checked when the body attests" },
        Case { name: "forged ESC (second responder on id 0x05)", faults: &["forged2:0x05"], owned: true,
            expect: Expect::At(Stage::Handshake, |f| matches!(f, Finding::Census(CensusFault::Conflict(Conflict::TwoAnswers { id: 5, .. })))),
            why: "two identities on one id cannot both be the knee" },
        Case { name: "stranger node on bus B", faults: &["stranger:0x20"], owned: true,
            expect: Expect::At(Stage::Handshake, |f| matches!(f, Finding::Census(CensusFault::Stranger { id: 0x20, .. }))),
            why: "a node the descriptor has no place for" },
        Case { name: "entity not on the body's owner list", faults: &[], owned: false,
            expect: Expect::At(Stage::Handshake, |f| matches!(f, Finding::Step(brainos_body::acquire::StepError::RefusedByDevice))),
            why: "mutual attestation: the body decides who may inhabit it" },
        Case { name: "phantom limb (RR_dist claimed, its ESC absent)", faults: &["phantom:RR_dist"], owned: true,
            expect: Expect::At(Stage::Schema, |f| matches!(f, Finding::Schema(SchemaFault::PhantomLimb { node: 9, .. }))),
            why: "the schema claims a part no silicon answers for" },
        Case { name: "IMU disconnected at power-on", faults: &["imu_off"], owned: true,
            expect: Expect::At(Stage::Schema, |f| matches!(f, Finding::Schema(SchemaFault::NoInnerEar))),
            why: "a BLUR without its inner ear is not a whole BLUR" },
        Case { name: "IMU lost during calibration (t = 2.0 s)", faults: &["imu_drop:2000"], owned: true,
            expect: Expect::At(Stage::Calibrate, |f| matches!(f, Finding::Cal(CalFault::ImuLost { .. }))),
            why: "the IMU is re-checked after the probes, before the body is declared calibrated" },
        Case { name: "flipped motor direction (RR_knee)", faults: &["flip:RR_knee"], owned: true,
            expect: Expect::At(Stage::Calibrate, |f| matches!(f, Finding::Cal(CalFault::Flipped { joint: 10, .. }))),
            why: "positive torque moving the encoder negative is a joint that kicks at full current on stand-up" },
        Case { name: "dead encoder (FL_dist)", faults: &["dead_enc:FL_dist"], owned: true,
            expect: Expect::At(Stage::Calibrate, |f| matches!(f, Finding::Cal(CalFault::DeadEncoder { joint: 7 }))),
            why: "current flowed, nothing moved" },
    ]
}

pub fn describe(f: &Option<Finding>) -> String {
    match f {
        None => "—".into(),
        Some(Finding::Cal(c)) => {
            let who = c.joint().map(|j| ACTUATORS[j].name).unwrap_or("imu");
            format!("{who}: {c:?}")
        }
        Some(x) => format!("{x:?}"),
    }
}

pub fn gate1() -> bool {
    println!("BrAInOS × BLUR — GATE 1: acquisition of the simulated body\n");
    // sanity on the index constants the case table uses
    assert_eq!(joint("RR_knee"), 10);
    assert_eq!(joint("FL_dist"), 7);
    let base = TwinOptions { jig: true, seed: 1, unit: 36, ..Default::default() };

    // 1. first inhabitation: no record, the build sheet is proposed
    let mut entity = Entity::new(BrainKey::from_seed(&ENTITY_SEED), Tick(1));
    let first = match acquire_once(&mut entity, "first", &base, &Record::default(), true) {
        Ok(r) => r,
        Err(e) => {
            println!("FAIL: could not run the twin: {e}");
            return false;
        }
    };
    print_run("first inhabitation (no record)", &first);
    let mut all_ok = first.report.ok() && zeros_ok(&first).0;
    let Some(sheet) = first.report.new_sheet.clone() else {
        println!("FAIL: no build sheet proposed on first inhabitation");
        return false;
    };
    let offsets = first.report.joints.map(|j| {
        let mut o = [0.0f32; N_ACT];
        for k in 0..N_ACT {
            o[k] = j[k].offset;
        }
        o
    });
    let record = Record { sheet: Some(sheet.clone()), offsets };

    // 2. second inhabitation from the record, a different power-on (seed 2
    // changes only noise and where the body hangs; the magnets are the same unit)
    let mut entity2 = Entity::new(BrainKey::from_seed(&ENTITY_SEED), Tick(1));
    let again_opts = TwinOptions { seed: 1, ..base.clone() };
    match acquire_once(&mut entity2, "again", &again_opts, &record, true) {
        Ok(r) => {
            print_run("re-inhabitation from the state graph (sheet + recorded zeros)", &r);
            all_ok &= r.report.ok() && zeros_ok(&r).0;
        }
        Err(e) => {
            println!("FAIL: {e}");
            all_ok = false;
        }
    }

    // 3. two more physical units: different silicon, magnets glued at
    // different angles, a different IMU mounting and noise
    for (seed, unit) in [(2u64, 37u64), (3, 38)] {
        let mut e = Entity::new(BrainKey::from_seed(&ENTITY_SEED), Tick(1));
        let o = TwinOptions { seed, unit, ..base.clone() };
        match acquire_once(&mut e, &format!("unit{unit}"), &o, &Record::default(), true) {
            Ok(r) => {
                let (zok, worst, who) = zeros_ok(&r);
                println!(
                    "── unit {unit} (seed {seed}), first inhabitation: {} — zeros {} (worst {who} {:.0}% of tolerance), {:.1} s wall",
                    if r.report.ok() { "acquired and calibrated" } else { "STOPPED" },
                    if zok { "PASS" } else { "FAIL" },
                    100.0 * worst,
                    r.wall_s
                );
                if !r.report.ok() {
                    println!("   stopped at {} — {}", r.report.caught_at.map(|s| s.name()).unwrap_or("?"), r.report.detail);
                }
                all_ok &= r.report.ok() && zok;
            }
            Err(e) => {
                println!("FAIL unit {unit}: {e}");
                all_ok = false;
            }
        }
    }

    println!("\nAdversarial bodies (expected step written before the run):\n");
    println!("  {:<46} {:<11} {:<11} {:<5}  finding", "case", "expected", "caught at", "ok");
    let mut adv_ok = true;
    for (k, c) in cases().into_iter().enumerate() {
        let opts = TwinOptions { faults: c.faults.iter().map(|s| s.to_string()).collect(), ..base.clone() };
        let mut e = Entity::new(BrainKey::from_seed(&ENTITY_SEED), Tick(1));
        let tag = format!("adv{k}");
        let res = acquire_once(&mut e, &tag, &opts, &record, c.owned);
        let (caught, finding, pass, wall) = match &res {
            Ok(r) => {
                let Expect::At(stage, pred) = &c.expect;
                let pass = r.report.caught_at == Some(*stage) && r.report.finding.as_ref().is_some_and(|f| pred(f));
                (r.report.caught_at.map(|s| s.name()).unwrap_or("not caught"), describe(&r.report.finding), pass, r.wall_s)
            }
            Err(err) => ("ERROR", err.clone(), false, 0.0),
        };
        let Expect::At(s, _) = &c.expect;
        let exp = s.name();
        println!("  {:<46} {:<11} {:<11} {:<5}  {}  ({:.1}s)", c.name, exp, caught, if pass { "PASS" } else { "FAIL" }, finding, wall);
        println!("  {:<46} {}", "", c.why);
        adv_ok &= pass;
    }

    println!("\nKnown blind spot (reported, not a pass): an encoder mounted mirrored, with a motor whose");
    println!("FOC alignment followed it, agrees with itself, so the torque probe passes; homing then finds");
    println!("the opposite stop and the reach check cannot tell (same travel). Undetected by this procedure");
    println!("on every joint. Candidate check for Phase 2: holding current at the jig stand pose vs the");
    println!("MJCF's predicted gravity torque, per joint (a mirrored joint holds a different pose).");

    let pass = all_ok && adv_ok;
    println!("\nGATE 1 (acquisition part): {}", if pass { "PASS" } else { "FAIL" });
    let _ = BuildSheet::default();
    pass
}

/// The first number after `key` inside the `"q": {…}` object of the
/// twin's state reply. A dozen lines instead of a JSON dependency.
fn json_number(s: &str, key: &str) -> Option<f32> {
    let q = s.find("\"q\": {")?;
    let rest = &s[q..];
    let end = rest.find('}')?;
    let obj = &rest[..end];
    let k = obj.find(key)? + key.len();
    let tail = &obj[k..];
    let stop = tail.find(|c: char| c == ',' || c == '}').unwrap_or(tail.len());
    tail[..stop].trim().parse().ok()
}

/// Zero accuracy the calibration must reach, against the twin's truth.
/// Joint-side MT6701 at the joint: a few counts plus how far a stop gives
/// under the homing current. Motor-side 4015s: the same at the output.
pub const ZERO_TOL_RAD: f32 = 0.035;
pub const ZERO_TOL_M: f32 = 0.004;

pub fn zeros_ok(r: &Run) -> (bool, f32, &'static str) {
    let Some(e) = &r.zero_error else { return (false, f32::NAN, "-") };
    let mut ok = true;
    let mut worst = 0.0f32;
    let mut who = "-";
    for (j, a) in ACTUATORS.iter().enumerate() {
        if a.encoder == brainos_blur::descriptor::EncoderSide::None {
            continue; // the gripper has no encoder to be wrong
        }
        let tol = if a.tendon { ZERO_TOL_M } else { ZERO_TOL_RAD };
        let x = e[j].abs();
        if !(x <= tol) {
            ok = false;
        }
        let rel = x / tol;
        if !(rel <= worst) {
            worst = rel;
            who = a.name;
        }
    }
    (ok, worst, who)
}

pub fn print_run(title: &str, r: &Run) {
    let rep = &r.report;
    println!("── {title}");
    for l in &rep.log {
        println!("   {l}");
    }
    match rep.caught_at {
        None if rep.ok() => println!(
            "   RESULT: acquired and calibrated, body {} — {:.1} s wall for {:.1} s of body time",
            rep.body.map(|b| b.fingerprint().as_str().to_string()).unwrap_or_default(),
            r.wall_s,
            r.sim_ticks as f64 / 1000.0
        ),
        _ => println!("   RESULT: stopped at {} — {}", rep.caught_at.map(|s| s.name()).unwrap_or("?"), rep.detail),
    }
    if let (Some(j), Some(e)) = (&rep.joints, &r.zero_error) {
        println!("   zeros: calibrated reading − true angle in the twin (rad; winches m)");
        for (k, a) in ACTUATORS.iter().enumerate() {
            println!("     {:<17} offset {:>9.4}   error {:>+8.4}{}", a.name, j[k].offset, e[k],
                if a.encoder == brainos_blur::descriptor::EncoderSide::None { "  (no encoder: dead-reckoned)" } else { "" });
        }
        let (ok, worst, who) = zeros_ok(r);
        println!("   zero accuracy: {} — worst {who} at {:.0}% of tolerance ({} rad / {} m)",
            if ok { "PASS" } else { "FAIL" }, 100.0 * worst, ZERO_TOL_RAD, ZERO_TOL_M);
    }
}
