//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Gate E9, the memory half: ten simulated minutes of the arm's own senses at 1 kHz add a bounded number of memories, and every injected fault is kept
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, bodyd, host, motor
//! BFS  USED BY  (entry point or test)
//!
//! The stream of what a body feels must not flood memory. The entity's
//! interoceptor (built for parts in an earlier gate) is what decides;
//! this checks that it holds for an arm ticking a thousand times a
//! second under a standing grant.

use brainos_abi::time::Instant;
use brainos_bodyd::armloop::Sense;
use brainos_bodyd::deskarm::Pose;
use brainos_host::motor_seat::Seat;
use brainos_motor::control::Style;

const HOME: Pose = [0.0, 90.0, -90.0, -90.0, 0.0, 0.02];

fn standing() -> (Seat, u64) {
    let mut seat = Seat::new(Style::FLUID).unwrap();
    let args: Vec<(String, f64)> = [
        ("duration_ms", 600_000.0),
        ("force_n", 20.0),
        ("rate_dps", 270.0),
        ("x_min", -0.35),
        ("x_max", 0.35),
        ("y_min", -0.35),
        ("y_max", 0.35),
        ("z_min", -0.009),
        ("z_max", 0.46),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), *v))
    .collect();
    seat.ask("arm.hold", &args, &HOME, Instant(1000)).expect("the standing grant");
    (seat, 1)
}

#[test]
fn ten_minutes_at_a_thousand_ticks_a_second_adds_a_bounded_number_of_memories() {
    let (mut seat, mut ms) = standing();
    let s = Sense { q: HOME, ..Sense::default() };
    let before = seat.e.graph.len();
    let gate = seat.gate_calls;
    // 599 s: the standing grant is for 600
    for _ in 0..599_000u64 {
        ms += 1;
        let (d, _) = seat.tick(Instant(ms * 1000), &s);
        assert!(d.standing, "the standing run ended at {ms} ms");
    }
    let added = seat.e.graph.len() - before;
    println!("599,000 ticks, 5,990 feelings passed up: {added} memories added, {} in the graph", seat.e.graph.len());
    assert!(added <= 64, "{added} memories from a steady stream");
    assert_eq!(seat.gate_calls, gate, "holding still asked the gate something");
    assert_eq!(seat.body.ticks_refused, 0);
}

#[test]
fn every_one_of_a_hundred_faults_is_kept() {
    let (mut seat, mut ms) = standing();
    let s = Sense { q: HOME, ..Sense::default() };
    let mut kept = 0;
    for _ in 0..100 {
        // five quiet seconds
        for _ in 0..5000 {
            ms += 1;
            seat.tick(Instant(ms * 1000), &s);
        }
        let before = seat.e.graph.len();
        // the motors are overloaded for 300 ms
        for _ in 0..3 {
            ms += 100;
            seat.feel(Instant(ms * 1000), 1.3, 0.0);
        }
        if seat.e.graph.len() > before {
            kept += 1;
        }
    }
    println!("100 overloads injected, {kept} left a memory; {} memories in all", seat.e.graph.len());
    assert_eq!(kept, 100);
}
