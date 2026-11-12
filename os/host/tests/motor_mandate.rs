//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Gate E8: a primitive runs only under a mandate made from a real KIRA grant, every bound of the mandate is enforced on every tick, and the gate is not asked during a run
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, bodyd, host, motor
//! BFS  USED BY  (entry point or test)
//!
//! Gate E8. Every mandate in this file comes from `Entity::propose`
//! granting a real request; there is no other way to get one, which is
//! the first thing being tested (the compile-fail half of that is in
//! `brainos-motor`'s doc tests).

use brainos_abi::time::{Duration, Instant};
use brainos_bodyd::armloop::{DeskArm, Sense};
use brainos_bodyd::deskarm::{Pose, DOF, GRIP, ROLL};
use brainos_host::motor::Lcg;
use brainos_host::motor_seat::Seat;
use brainos_motor::control::Style;
use brainos_motor::mandate::RunMandate;
use brainos_motor::model::JointKind;
use brainos_motor::run::{Breach, Ending, Felt, Kinematics, Proposal, Runner};

const HOME: Pose = [0.0, 90.0, -90.0, -90.0, 0.0, 0.02];

fn args(pairs: &[(&str, f64)]) -> Vec<(String, f64)> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

/// A grasp request with every bound stated; `edit` changes or removes
/// arguments.
fn grasp_args(edit: &[(&str, Option<f64>)]) -> Vec<(String, f64)> {
    let mut a = args(&[
        ("duration_ms", 2000.0),
        ("force_n", 10.0),
        ("rate_dps", 120.0),
        ("x_min", 0.05),
        ("x_max", 0.25),
        ("y_min", -0.10),
        ("y_max", 0.10),
        ("z_min", 0.0),
        ("z_max", 0.30),
        ("fx", 0.18),
        ("fy", 0.0),
        ("fz", 0.04),
        ("fyaw", 90.0),
        ("width_m", 0.03),
        ("pitch_deg", -90.0),
        ("squeeze_n", 6.0),
    ]);
    for (k, v) in edit {
        a.retain(|(n, _)| n != k);
        if let Some(v) = v {
            a.push((k.to_string(), *v));
        }
    }
    a
}

fn mandate(seat: &mut Seat) -> RunMandate {
    seat.grant("arm.grasp", &grasp_args(&[])).expect("a plain grasp is granted")
}

fn caps(arm: &DeskArm, squeeze: f32) -> [f32; DOF] {
    let mut c = [arm.config.torque_nm; DOF];
    c[GRIP] = squeeze;
    c
}

const CALM: Felt = Felt { force_n: Some(0.0), tilt_deg: None };

#[test]
fn the_gate_decides_whether_a_primitive_may_start() {
    let mut seat = Seat::new(Style::FLUID).unwrap();
    let at = Instant(1_000);

    // more force than the arm says it can survive: refused at Validate
    let err = seat.ask("arm.grasp", &grasp_args(&[("force_n", Some(99.0))]), &HOME, at).unwrap_err();
    assert!(err.starts_with("denied"), "{err}");
    assert!(!seat.body.is_running());

    // a verb the arm does not have
    let err = seat.ask("arm.fly", &args(&[("duration_ms", 10.0)]), &HOME, at).unwrap_err();
    assert!(err.starts_with("denied"), "{err}");
    assert!(!seat.body.is_running());

    // a parameter outside what the primitive declared
    let err = seat.ask("arm.grasp", &grasp_args(&[("width_m", Some(0.5))]), &HOME, at).unwrap_err();
    assert!(err.starts_with("denied"), "{err}");

    // and the plain one is granted and runs
    assert_eq!(seat.ask("arm.grasp", &grasp_args(&[]), &HOME, at), Ok(false));
    assert!(seat.body.is_running());
}

#[test]
fn a_grant_that_leaves_a_bound_unstated_does_not_become_a_mandate() {
    let mut seat = Seat::new(Style::FLUID).unwrap();
    for missing in ["duration_ms", "force_n", "rate_dps"] {
        let got = seat.grant("arm.grasp", &grasp_args(&[(missing, None)]));
        let err = got.expect_err("no mandate without every bound");
        assert!(err.contains("Missing") || err.starts_with("denied"), "{missing}: {err}");
    }
    // five of the six edges of the box
    let err = seat.grant("arm.grasp", &grasp_args(&[("z_max", None)])).unwrap_err();
    assert!(err.contains("PartialBox") || err.starts_with("denied"), "{err}");
}

#[test]
fn a_mandate_with_no_box_cannot_start_a_body_that_has_a_tool() {
    let mut seat = Seat::new(Style::FLUID).unwrap();
    let edit: Vec<(&str, Option<f64>)> =
        ["x_min", "x_max", "y_min", "y_max", "z_min", "z_max"].iter().map(|k| (*k, None)).collect();
    match seat.grant("arm.grasp", &grasp_args(&edit)) {
        // the gate may already refuse it; if it does not, the runner does
        Err(e) => assert!(e.starts_with("denied"), "{e}"),
        Ok(m) => {
            let arm = seat.body.arm;
            let mut r = Runner::idle();
            assert_eq!(r.begin(m, &arm, &HOME, Instant(0)), Err(Breach::NoWorkspace));
            assert!(!r.is_running());
        }
    }
}

#[test]
fn a_mandate_for_this_arm_does_not_start_another() {
    let mut seat = Seat::new(Style::FLUID).unwrap();
    let m = mandate(&mut seat);
    let mut other = seat.body.arm;
    other.id = brainos_abi::ids::BodyId::from_bytes([9; 32]);
    let mut r = Runner::idle();
    assert_eq!(r.begin(m, &other, &HOME, Instant(0)), Err(Breach::WrongBody));
}

#[test]
fn with_no_mandate_every_tick_is_refused() {
    let seat = Seat::new(Style::FLUID).unwrap();
    let arm = seat.body.arm;
    let mut r = Runner::idle();
    let c = caps(&arm, 3.0);
    for k in 0..50u64 {
        let out = r.admit(&arm, Instant(k * 1000), &Proposal { target: &HOME, effort: &c }, &CALM);
        assert_eq!(out.unwrap_err().0, Breach::NoMandate);
    }
    assert_eq!(r.refused, 50);
}

/// Start a run and return what is needed to poke at it.
fn started(seat: &mut Seat) -> (Runner, DeskArm) {
    let m = mandate(seat);
    let arm = seat.body.arm;
    let mut r = Runner::idle();
    r.begin(m, &arm, &HOME, Instant(0)).unwrap();
    (r, arm)
}

#[test]
fn each_bound_ends_the_run_when_one_tick_crosses_it() {
    let mut seat = Seat::new(Style::FLUID).unwrap();
    let ms = |n: u64| Instant(n * 1000);

    // a tick inside everything is applied
    let (mut r, arm) = started(&mut seat);
    let c = caps(&arm, 6.0);
    assert!(r.admit(&arm, ms(1), &Proposal { target: &HOME, effort: &c }, &CALM).is_ok());
    assert_eq!(r.applied(), 1);

    let breach = |r: &mut Runner, arm: &DeskArm, at: Instant, target: &Pose, effort: &[f32; DOF], felt: &Felt| {
        let (b, summary) = r.admit(arm, at, &Proposal { target, effort }, felt).unwrap_err();
        let s = summary.expect("a run that ends reports");
        assert_eq!(s.ending, Ending::Breached(b));
        assert!(!r.is_running(), "the run is over");
        // and nothing after it is applied either
        let again = r.admit(arm, at, &Proposal { target: &HOME, effort }, &CALM).unwrap_err();
        assert_eq!(again.0, Breach::NoMandate);
        b
    };

    // angle: past the elbow's own limit (it cannot go above 0)
    let (mut r, arm) = started(&mut seat);
    // approach in small steps so that only the angle is at issue
    let mut q = HOME;
    let mut at = 0u64;
    let mut hit = None;
    while hit.is_none() {
        at += 1;
        q[2] = (q[2] + 0.1).min(0.05);
        q[1] = 90.0;
        match r.admit(&arm, ms(at), &Proposal { target: &q, effort: &c }, &CALM) {
            Ok(()) => {}
            Err((b, _)) => hit = Some(b),
        }
        assert!(at < 5000);
    }
    // going from -90 to 0 at the elbow sweeps the tool out of the box or
    // into the limit; either is a refusal, and the limit is the last
    // line: if the box did not catch it, the angle must
    assert!(matches!(hit, Some(Breach::Angle { joint: 2 }) | Some(Breach::Workspace)), "{hit:?}");

    // rate: 5 degrees in one millisecond is 5000 deg/s
    let (mut r, arm) = started(&mut seat);
    let mut t = HOME;
    t[ROLL] = 5.0;
    assert_eq!(breach(&mut r, &arm, ms(1), &t, &c, &CALM), Breach::Rate { joint: ROLL as u8 });

    // effort: more torque than the arm has
    let (mut r, arm) = started(&mut seat);
    let mut e = c;
    e[1] = arm.config.torque_nm * 1.01;
    assert_eq!(breach(&mut r, &arm, ms(1), &HOME, &e, &CALM), Breach::Effort { joint: 1 });

    // squeeze: more than the 10 N that was granted, though the arm has 20
    let (mut r, arm) = started(&mut seat);
    let e = caps(&arm, 10.5);
    assert_eq!(breach(&mut r, &arm, ms(1), &HOME, &e, &CALM), Breach::Effort { joint: GRIP as u8 });

    // felt force: the world pushes back harder than was granted
    let (mut r, arm) = started(&mut seat);
    let felt = Felt { force_n: Some(10.5), tilt_deg: None };
    assert_eq!(breach(&mut r, &arm, ms(1), &HOME, &c, &felt), Breach::FeltForce);

    // duration: one tick after the 2000 ms that were granted
    let (mut r, arm) = started(&mut seat);
    assert!(r.admit(&arm, ms(2000), &Proposal { target: &HOME, effort: &c }, &CALM).is_ok());
    assert_eq!(breach(&mut r, &arm, ms(2001), &HOME, &c, &CALM), Breach::Expired);

    // not a number
    let (mut r, arm) = started(&mut seat);
    let mut t = HOME;
    t[0] = f32::NAN;
    assert_eq!(breach(&mut r, &arm, ms(1), &t, &c, &CALM), Breach::Malformed);
}

#[test]
fn the_box_is_checked_with_the_arms_own_kinematics() {
    let mut seat = Seat::new(Style::FLUID).unwrap();
    let (mut r, arm) = started(&mut seat);
    let c = caps(&arm, 6.0);
    // swing the pan slowly: the tool leaves y in [-0.10, 0.10] at some
    // angle, and the runner must refuse the first tick that is outside
    let mut q = HOME;
    let mut at = 0u64;
    loop {
        at += 1;
        q[0] += 0.1; // 100 deg/s, inside the granted 120
        let inside = {
            let p = arm.tool(&q).unwrap();
            p[0] >= 0.05 && p[0] <= 0.25 && p[1] >= -0.10 && p[1] <= 0.10 && p[2] >= 0.0 && p[2] <= 0.30
        };
        let out = r.admit(&arm, Instant(at * 1000), &Proposal { target: &q, effort: &c }, &CALM);
        if inside {
            assert!(out.is_ok(), "a tick inside the box was refused at pan {}", q[0]);
        } else {
            assert_eq!(out.unwrap_err().0, Breach::Workspace, "pan {}", q[0]);
            break;
        }
        assert!(at < 2000, "the tool never left the box");
    }
    assert!(q[0] > 30.0, "the box is about 45 degrees wide at this reach");
}

/// Ten times the pre-registered count (10,000), because half the runs
/// are quiet ones that last until a limit is reached: 100,000 random
/// ticks, none applied outside.
#[test]
fn a_hundred_thousand_random_ticks_and_none_is_applied_outside_its_mandate() {
    let mut seat = Seat::new(Style::FLUID).unwrap();
    let mut rng = Lcg(0xE8);
    let (mut applied, mut refused, mut by_kind) = (0u32, 0u32, std::collections::BTreeMap::new());
    let mut runs = 0u32;
    let mut ticks = 0u32;
    while ticks < 100_000 {
        let (mut r, arm) = started(&mut seat);
        runs += 1;
        let b = *r.bounds().unwrap();
        let w = b.workspace.unwrap();
        let mut last = HOME;
        let mut last_at = 0u64;
        let mut now = 0u64;
        // each run leans one joint steadily one way, inside the granted
        // rate, so that a limit or the box is eventually what is reached
        let lean = (rng.unit() * DOF as f32) as usize % DOF;
        let lean_by = if lean == GRIP { 0.00008 } else { 0.1 } * if rng.unit() < 0.5 { 1.0 } else { -1.0 };
        // every other run has no sudden events at all, so its lean gets
        // as far as a joint limit or the edge of the box
        let p_wild = if runs % 2 == 0 { 0.0 } else { 0.02 };
        while ticks < 100_000 {
            ticks += 1;
            // mostly small steps, sometimes anything at all
            let wild = rng.unit() < p_wild;
            now += if wild { (rng.unit() * 3000.0) as u64 * 1000 } else { 1000 };
            let mut t = last;
            for i in 0..DOF {
                let span = if i == GRIP { 0.00002 } else { 0.015 };
                t[i] += rng.between(-span, span) * if wild { 400.0 } else { 1.0 };
            }
            t[lean] += lean_by;
            let squeeze = if rng.unit() < p_wild { rng.between(0.0, 25.0) } else { rng.between(0.0, 9.9) };
            let mut e = caps(&arm, squeeze);
            if rng.unit() < p_wild * 0.5 {
                e[(rng.unit() * 5.0) as usize] = rng.between(0.0, 6.0);
            }
            let felt = Felt {
                force_n: Some(if rng.unit() < p_wild { rng.between(0.0, 30.0) } else { rng.between(0.0, 9.0) }),
                tilt_deg: None,
            };
            let at = Instant(now);
            let out = r.admit(&arm, at, &Proposal { target: &t, effort: &e }, &felt);

            // the oracle: the same bounds, worked out here
            let dt = (now - last_at).min(10_000) as f32 * 1e-6;
            let mut inside = now <= b.duration.as_micros();
            for i in 0..DOF {
                let (lo, hi) = arm.limits(i);
                inside &= t[i] >= lo && t[i] <= hi;
                let rate = match arm.kind(i) {
                    JointKind::Hinge => arm.max_rate(i).min(b.rate_dps),
                    JointKind::Slide => arm.max_rate(i),
                };
                inside &= (t[i] - last[i]).abs() <= rate * dt * 1.001 + 1e-6;
                let cap = match arm.kind(i) {
                    JointKind::Hinge => arm.max_effort(i),
                    JointKind::Slide => arm.max_effort(i).min(b.force_n),
                };
                inside &= e[i] >= 0.0 && e[i] <= cap;
            }
            inside &= felt.force_n.unwrap() <= b.force_n;
            inside &= arm.tool(&t).map(|p| w.contains(&p)).unwrap_or(false);

            match out {
                Ok(()) => {
                    assert!(inside, "tick {ticks} was applied outside its mandate: {t:?} {e:?} {felt:?}");
                    applied += 1;
                    last = t;
                    last_at = now;
                }
                Err((breach, _)) => {
                    assert!(!inside, "tick {ticks} was inside and refused as {breach:?}");
                    refused += 1;
                    *by_kind.entry(breach.name()).or_insert(0u32) += 1;
                    break;
                }
            }
        }
    }
    assert_eq!(applied + refused, 100_000);
    assert!(applied > 30_000 && refused > 100, "applied {applied}, refused {refused}: the test did not exercise both");
    for kind in ["angle", "rate", "effort", "felt-force", "workspace", "expired"] {
        assert!(by_kind.get(kind).copied().unwrap_or(0) > 0, "no {kind} breach was provoked: {by_kind:?}");
    }
    println!("100000 ticks over {runs} granted runs: {applied} applied, {refused} refused {by_kind:?}");
}

/// Run the arm's own loop against a plant that goes exactly where it is
/// told, and count what the gate was asked.
fn play(seat: &mut Seat, mut now_ms: u64, max_ms: u64) -> (u64, bool) {
    let mut s = Sense { q: HOME, ..Sense::default() };
    s.q = *seat.body.last_target();
    let start = now_ms;
    while seat.body.is_running() && now_ms - start < max_ms {
        now_ms += 1;
        let (_, _) = seat.tick(Instant(now_ms * 1000), &s);
        s.q = *seat.body.last_target();
        // the fingers feel something once they have closed to 12 mm
        s.grip_n = if s.q[GRIP] < 0.0125 { 6.0 } else { 0.0 };
    }
    (now_ms, !seat.body.is_running())
}

#[test]
fn the_gate_is_asked_once_per_primitive_and_never_during_it() {
    let mut seat = Seat::new(Style::FLUID).unwrap();
    assert_eq!(seat.ask("arm.grasp", &grasp_args(&[("duration_ms", Some(6000.0)), ("rate_dps", Some(180.0))]), &HOME, Instant(1000)), Ok(false));
    let asked = seat.gate_calls;
    let decided = seat.e.span().actions_granted + seat.e.span().actions_denied;
    let (ended_ms, finished) = play(&mut seat, 1, 7000);
    assert!(finished, "the grasp did not finish");
    assert!(seat.body.ticks > 2000, "a grasp is a few thousand ticks: {}", seat.body.ticks);
    assert_eq!(seat.gate_calls, asked, "the gate was asked during the run");
    assert_eq!(seat.e.span().actions_granted + seat.e.span().actions_denied, decided);
    assert_eq!(seat.body.ticks_refused, 0);
    let _ = ended_ms;
}

#[test]
fn a_run_stops_when_its_time_is_up_even_if_the_primitive_is_not_done() {
    let mut seat = Seat::new(Style::FLUID).unwrap();
    // 400 ms for a grasp that needs about three seconds
    assert_eq!(seat.ask("arm.grasp", &grasp_args(&[("duration_ms", Some(400.0)), ("rate_dps", Some(180.0))]), &HOME, Instant(1000)), Ok(false));
    let (now, stopped) = play(&mut seat, 1, 3000);
    assert!(stopped);
    assert!((400..=403).contains(&(now - 1)), "stopped after {} ms", now - 1);
    assert_eq!(seat.body.ticks_refused, 1, "exactly the one tick past the deadline");
}

#[test]
fn a_mandate_is_spent_by_one_run() {
    let mut seat = Seat::new(Style::FLUID).unwrap();
    let (mut r, arm) = started(&mut seat);
    let summary = r.finish(Instant(5000)).unwrap();
    assert_eq!(summary.ending, Ending::Finished);
    // the runner is idle again and there is no mandate left to reuse:
    // `begin` took it by value. Starting again needs the gate again.
    let c = caps(&arm, 3.0);
    assert_eq!(r.admit(&arm, Instant(6000), &Proposal { target: &HOME, effort: &c }, &CALM).unwrap_err().0, Breach::NoMandate);
    let before = seat.gate_calls;
    let _ = started(&mut seat);
    assert_eq!(seat.gate_calls, before + 1);
    let _ = Duration::from_millis(0);
}
