//! Tests for the measurements.
//!
//! A test that asserts a timing is a test that fails when someone else's
//! build is running. So this file checks two things that are not flaky:
//! that the measurement *methodology* is right, and that the operations
//! meet their budgets with enough orders of magnitude of headroom that
//! no amount of contention could close the gap.
//!
//! The one thing deliberately not asserted is a tight timing. If the
//! gate becomes ten times slower, this suite will not catch it — the
//! `bench` subcommand's output will, read by a person. Encoding a tight
//! bound here would buy a red build on a busy machine and nothing else.

use brainos_abi::time::Duration;
use brainos_host::bench::{fits_on_the_board, footprint, implications, memory_per_episode, run};

#[test]
fn every_measurement_reports_a_sane_distribution() {
    for m in run() {
        assert!(m.min_ns > 0, "{}: a zero minimum means the operation was optimised away", m.what);
        assert!(m.min_ns <= m.median_ns, "{}", m.what);
        assert!(m.median_ns <= m.p99_ns, "{}", m.what);
        assert!(m.p99_ns <= m.max_ns, "{}", m.what);
        assert!(m.batches >= 5, "{}: too few batches to have a p99", m.what);
        assert!(m.per_batch >= 1, "{}", m.what);
        assert!(!m.note.is_empty(), "{}: a measurement with no explanation is a number", m.what);
    }
}

#[test]
fn nothing_misses_its_budget_by_orders_of_magnitude() {
    // A loose check: every operation with a budget must fit it with at
    // least a 10x margin on the uncontended cost. Anything that only
    // just fits is a design problem, not a scheduling accident.
    for m in run() {
        let Some(budget) = m.budget else { continue };
        let budget_ns = budget.as_micros() * 1_000;
        assert!(
            m.min_ns * 10 <= budget_ns,
            "{} takes {} ns against a {} ns budget — less than 10x headroom",
            m.what,
            m.min_ns,
            budget_ns
        );
    }
}

#[test]
fn the_reflex_pass_is_fast_enough_for_a_kilohertz_loop_by_a_wide_margin() {
    // The one timing worth asserting, because it is the claim Domain 2
    // rests on and it has four orders of magnitude of room.
    let m = run()
        .into_iter()
        .find(|m| m.what.contains("one reflex pass"))
        .expect("the reflex pass is measured");
    assert!(
        m.min_ns < 100_000,
        "a reflex pass took {} ns; a 1 kHz loop has 1,000,000",
        m.min_ns
    );
}

#[test]
fn a_contended_measurement_says_so_rather_than_reporting_a_tail_as_a_property() {
    // synthesised rather than measured, so the test does not depend on
    // how busy the machine is
    let clean = brainos_host::bench::Measurement {
        what: "x",
        budget: Some(Duration::from_millis(1)),
        batches: 50,
        per_batch: 10,
        min_ns: 1_000,
        median_ns: 1_100,
        p99_ns: 1_500,
        max_ns: 2_000,
        note: "n",
    };
    assert!(clean.measurement_is_clean());
    assert!((clean.spread() - 1.5).abs() < 0.01);
    assert_eq!(clean.fits(), Some(true));

    let noisy = brainos_host::bench::Measurement {
        p99_ns: 50_000,
        ..clean
    };
    assert!(!noisy.measurement_is_clean());
    assert_eq!(
        noisy.fits(),
        Some(true),
        "fitting is judged on the min, because the question is about the code"
    );
}

#[test]
fn the_budget_check_uses_the_uncontended_cost() {
    let over = brainos_host::bench::Measurement {
        what: "x",
        budget: Some(Duration::from_micros(500)),
        batches: 50,
        per_batch: 10,
        min_ns: 600_000, // 600us against a 500us budget
        median_ns: 600_000,
        p99_ns: 700_000,
        max_ns: 800_000,
        note: "n",
    };
    assert_eq!(over.fits(), Some(false));
    assert_eq!(over.headroom_pct(), Some(120));
}

// ---------- footprint ----------

#[test]
fn the_footprint_is_reported_for_everything_a_servo_loop_carries() {
    let f = footprint();
    assert!(f.len() >= 6);
    for (what, bytes, note) in &f {
        assert!(*bytes > 0, "{what}");
        assert!(!note.is_empty(), "{what}");
    }

    // the two that matter on a microcontroller
    let denied = f.iter().find(|(w, _, _)| *w == "Denied").unwrap().1;
    assert!(
        denied <= 32,
        "a refusal a servo loop carries is {denied} bytes"
    );
    let envelope = f.iter().find(|(w, _, _)| *w == "Envelope").unwrap().1;
    assert!(envelope <= 256, "an envelope is {envelope} bytes");
}

#[test]
fn a_memory_costs_a_predictable_number_of_bytes() {
    let (empty, full, nodes) = memory_per_episode();
    assert!(nodes > 900);
    assert!(full > empty);
    let per = (full - empty) / (nodes - 1);
    assert!(
        (50..=400).contains(&per),
        "a memory encodes to {per} bytes, which is outside anything reasonable"
    );
}

#[test]
fn the_embedded_bounds_are_what_makes_an_entity_fit_on_the_board() {
    // This is the finding the constant exists for: at default bounds a
    // state graph does not fit in an ESP32-S3's SRAM, and at embedded
    // bounds it does. If that ever stops being true, the bounds are
    // wrong rather than the test.
    let fits = fits_on_the_board();
    let default = fits
        .iter()
        .find(|(w, _, _, _)| w.contains("default"))
        .unwrap();
    let embedded = fits
        .iter()
        .find(|(w, _, _, _)| w.contains("embedded"))
        .unwrap();

    assert!(!default.3, "default bounds should not fit on the board");
    assert!(embedded.3, "embedded bounds should fit on the board");
    assert!(
        embedded.1 * 4 < embedded.2,
        "embedded should fit with room to spare, not just barely"
    );
}

// ---------- the derived conclusions ----------

#[test]
fn the_implications_are_derived_from_the_measurements_rather_than_written_down() {
    let measurements = run();
    let lines = implications(&measurements);
    assert!(lines.len() >= 3, "got {} conclusions", lines.len());

    let all = lines.join(" ");
    // the architectural conclusion that matters
    assert!(
        all.contains("pre-authorized envelope"),
        "the gate-cost conclusion is missing"
    );
    assert!(all.contains("ed25519"));
    assert!(all.contains("segmenting") || all.contains("segment"));

    // and each one quotes a number, so a change in the code changes the
    // sentence rather than leaving a stale paragraph
    for line in &lines {
        assert!(
            line.chars().any(|c| c.is_ascii_digit()),
            "a conclusion with no number in it: {line}"
        );
    }
}

#[test]
fn the_implications_survive_an_empty_measurement_set() {
    assert!(implications(&[]).is_empty());
}
