//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Kills a separate cognition process with SIGKILL and checks the Domain 2 cooling loop keeps running and the GPU stays under its limit
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, body, crypto, entity, kira, part | external: std
//! BFS  USED BY  (entry point or test)
//!
//! Homeostasis survives a killed cognition process.
//!
//! Two operating-system processes:
//!
//! ```text
//!   child   `brainos mind-stub`   a cognitive resource. It proposes, forever:
//!                                 "set the fan to 0 %". It holds no key.
//!   this    the body side         the gate (Domain 3), the fan and GPU bench,
//!                                 and the homeostat (Domain 2)
//! ```
//!
//! While the child lives, every proposal it prints is put to the real
//! gate. While the GPU is cool they are granted — and the regulator
//! overrides them on its next step. Once the GPU is hot they are
//! refused at *simulate*. Then the child is killed with SIGKILL, with
//! no warning and no chance to clean up, and the loop is run for ten
//! more simulated minutes at full load.
//!
//! What this establishes: the regulator needs nothing from the process
//! that thinks. What it does not: the bench is `SimNerve`, a simulated
//! fan and GPU. It is not a hardware result. And the process killed is
//! the *model*; the gate lives on the body side here, as it does in
//! the architecture ("a cognitive resource proposes; KIRA disposes").
//! The entity suite separately drops the entire entity, gate included.

use brainos_abi::error::{DenyCode, Stage};
use brainos_abi::time::{Instant, Tick};
use brainos_abi::verb::Verb;
use brainos_body::policy::Arg;
use brainos_body::vital::Felt;
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::{Entity, Wiring};
use brainos_kira::action::Request;
use brainos_kira::gate::Verdict;
use brainos_part::homeostat::Homeostat;
use brainos_part::nerve::{Mandate, Nerve};
use brainos_part::policy::Guard;
use brainos_part::sim::SimNerve;
use std::io::{BufRead, BufReader};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;

#[test]
fn the_cooling_loop_outlives_a_cognition_process_killed_with_sigkill() {
    // --- the body side ---
    let mut e = Entity::new(BrainKey::from_seed(&[0x71; 32]), Tick(0));
    e.wake(None, None, Tick(1)).unwrap();
    let mut n = SimNerve::new(0x40);
    let (g, f) = (n.gpu(), n.fan());
    let set = Verb::parse("fan.set").unwrap();
    let regulate = Verb::parse("fan.regulate").unwrap();

    let schema = n.describe(&g).unwrap();
    let gpu = e
        .acquire_part(g.key, |c| n.answer(&g, c).unwrap(), schema, Wiring::default())
        .unwrap();
    let schema = n.describe(&f).unwrap();
    let fan = e
        .acquire_part(
            f.key,
            |c| n.answer(&f, c).unwrap(),
            schema,
            Wiring {
                // this wirer is cautious: from 70 C up the fan may not be
                // set, or its regulator capped, below 60 %. At full load
                // the regulated GPU settles a little above 70 C, so the
                // guard is engaged for most of the run.
                guards: vec![
                    Guard::sustain(set, "duty_pct", gpu, "temp.c", 70.0, 60.0),
                    Guard::sustain(regulate, "max", gpu, "temp.c", 70.0, 60.0),
                ],
                relief: vec![set, regulate],
                ..Wiring::default()
            },
        )
        .unwrap();

    let mut now = 1_000_000u64;
    let feel = |e: &mut Entity, n: &mut SimNerve, now: u64| {
        for (found, id) in [(n.gpu(), gpu), (n.fan(), fan)] {
            let mut out = Vec::new();
            n.read(&found, Instant(now), &mut out);
            e.feel(&id, &out, Instant(now));
        }
    };
    feel(&mut e, &mut n, now);

    // one grant installs the regulator
    let r = Request::on_body(regulate, fan, e.tick)
        .arg("min", Arg::Count(20))
        .arg("max", Arg::Count(100));
    let Verdict::Granted(granted) = e.propose(r.clone()).verdict else {
        panic!("the regulator was refused")
    };
    let m = Mandate::check(granted, &r, &e.key().public(), e.tick).unwrap();
    let mut loop_ = Homeostat::install(
        &m,
        "duty_pct",
        gpu,
        "temp.c",
        &[(50.0, 20.0), (70.0, 50.0), (85.0, 100.0)],
        100.0,
    )
    .unwrap();

    // --- the cognition process ---
    let mut child = Command::new(env!("CARGO_BIN_EXE_brainos"))
        .arg("mind-stub")
        .stdout(Stdio::piped())
        .spawn()
        .expect("the cognition process starts");
    let pid = child.id();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    n.util = 1.0;
    let mut one_step = |e: &mut Entity, n: &mut SimNerve, h: &mut Homeostat, now: &mut u64| {
        *now += 1_000_000;
        feel(e, n, *now);
        let mut out = Vec::new();
        n.read(&n.gpu(), Instant(*now), &mut out);
        for s in &out {
            h.feel(&Felt::new(gpu, s.sense.as_str(), s.value, Instant(*now)));
        }
        let act = h.step(Instant(*now));
        n.reflex(&n.fan(), &act).unwrap();
        n.advance(1000);
    };

    // --- phase 1: cognition alive, and insisting ---
    let (mut proposals, mut granted_n, mut refused_n) = (0u32, 0u32, 0u32);
    let mut peak: f64 = 0.0;
    let started = std::time::Instant::now();
    while (refused_n < 20 || proposals < 60) && started.elapsed().as_secs() < 20 {
        // wait for the other process to say something
        let Ok(line) = rx.recv_timeout(std::time::Duration::from_secs(5)) else {
            panic!("the cognition process said nothing");
        };
        assert_eq!(line, "propose fan.set duty_pct 0");
        proposals += 1;
        let r = Request::on_body(set, fan, e.tick).arg("duty_pct", Arg::Count(0));
        match e.propose(r.clone()).verdict {
            Verdict::Granted(g) => {
                let m = Mandate::check(g, &r, &e.key().public(), e.tick).unwrap();
                n.apply(&n.fan(), &m).unwrap();
                granted_n += 1;
            }
            Verdict::Denied(d) => {
                assert_eq!((d.stage, d.code), (Stage::Simulate, DenyCode::PredictedInvariantBreak));
                refused_n += 1;
            }
        }
        one_step(&mut e, &mut n, &mut loop_, &mut now);
        peak = peak.max(n.temp_c);
    }
    assert!(granted_n > 0, "the fan was never stopped while the GPU was cool");
    assert!(refused_n >= 20, "the gate never refused: {refused_n} of {proposals}");
    assert!(peak < 95.0, "reached {peak} C while cognition was alive");

    // --- kill it ---
    child.kill().expect("SIGKILL is delivered");
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(9), "pid {pid} did not die of SIGKILL: {status:?}");
    // drain what it wrote before dying; after that the pipe is closed
    while rx.recv_timeout(std::time::Duration::from_millis(200)).is_ok() {}
    assert!(
        matches!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)),
        "something is still proposing"
    );

    // --- phase 2: nobody is thinking ---
    let steps_before = loop_.steps();
    for i in 0..600 {
        one_step(&mut e, &mut n, &mut loop_, &mut now);
        assert!(n.temp_c < 95.0, "{} C, {i} s after cognition was killed", n.temp_c);
        peak = peak.max(n.temp_c);
    }
    assert_eq!(loop_.steps() - steps_before, 600);
    assert_eq!(loop_.failsafes(), 0);
    println!(
        "cognition pid {pid}: {proposals} proposals, {granted_n} granted while cool, {refused_n} \
         refused at simulate; killed with SIGKILL; 600 further loop steps at full load, peak {peak:.1} C"
    );
}
