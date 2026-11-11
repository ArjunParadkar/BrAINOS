//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Tests that over a simulated day the entity notices each fault promptly, ignores noise, stays affordable, and copes with a small-memory body
//! BFS  DEFINES  -
//! BFS  USES     brainos: host
//! BFS  USED BY  (entry point or test)
//!
//! Can it be left running?
//!
//! Every other suite asks whether something works. This one asks whether
//! an entity can be left alone with a body for a working day, and it
//! measures the balance that is easy to get wrong in both directions:
//! an entity that surfaces everything is a firehose nobody keeps, and
//! one that surfaces nothing is a machine that quietly fails.
//!
//! The metrics are deliberately not per-occurrence recall. A leg
//! stalling fifty times in a day should be known about — once,
//! promptly, and then counted. Scoring per-occurrence recall would rank
//! the firehose highest, which is the behaviour the damping exists to
//! prevent.

use brainos_host::day::{narrate, run, Happening};

const TICKS: u64 = 20_000;

#[test]
fn the_entity_notices_each_kind_of_fault_on_its_first_occurrence() {
    let d = run(TICKS);
    assert!(
        !d.first_notice.is_empty(),
        "nothing was ever noticed in a whole day"
    );
    for (kind, before) in &d.first_notice {
        assert_eq!(
            *before, 1,
            "{} was not noticed until occurrence {before}",
            kind.name()
        );
    }
    // and both kinds of fault were noticed, not just the loud one
    let kinds: Vec<Happening> = d.first_notice.iter().map(|(k, _)| *k).collect();
    assert!(kinds.contains(&Happening::Stopped));
    assert!(
        kinds.contains(&Happening::LegStall),
        "the quieter fault was never noticed at all"
    );
}

#[test]
fn awareness_is_maintained_across_the_whole_day() {
    let d = run(TICKS);
    assert!(d.windows_with_a_fault >= 5, "too few windows to say anything");
    assert!(
        d.awareness_pct() >= 70,
        "the human heard about a fault in only {}% of the windows one occurred in",
        d.awareness_pct()
    );
}

#[test]
fn it_does_not_become_a_firehose() {
    let d = run(TICKS);
    assert!(
        d.interruptions_per_kilotick <= 40,
        "{} interruptions per thousand ticks",
        d.interruptions_per_kilotick
    );
    assert!(
        d.interrupt_precision_pct() >= 60,
        "only {}% of interruptions deserved one",
        d.interrupt_precision_pct()
    );
}

#[test]
fn pure_sensor_noise_never_interrupts() {
    // The bug a day of operation found: consequence used to be additive,
    // so a three-sigma blip on a tilt sensor — well inside its own noise
    // — interrupted a human, because tilt matters on a body that can
    // fall. Consequence scales evidence now; it does not create it.
    let d = run(TICKS);
    let quiet = d
        .by_kind
        .iter()
        .find(|(k, _, _, _)| *k == Happening::Nothing)
        .expect("quiet ticks are counted");
    assert!(quiet.1 > 15_000, "most of a day should be quiet");
    assert_eq!(
        quiet.3, 0,
        "{} interruptions on ticks where nothing happened",
        quiet.3
    );
}

#[test]
fn a_harmless_recurring_event_is_logged_but_does_not_interrupt() {
    let d = run(TICKS);
    let dips = d
        .by_kind
        .iter()
        .find(|(k, _, _, _)| *k == Happening::RailDip)
        .expect("rail dips happen");
    assert!(dips.1 > 100, "the day should contain plenty of them");
    assert!(dips.2 > 0, "the human should be able to see them happened");
    assert!(
        dips.3 * 20 < dips.1,
        "{} of {} rail dips interrupted, which is a firehose",
        dips.3,
        dips.1
    );
}

#[test]
fn the_entity_is_still_affordable_at_the_end_of_the_day() {
    let d = run(TICKS);
    assert!(
        d.budget_left_pct >= 20,
        "only {}% of the escalation budget left; an entity that spends \
         everything by lunchtime has not been left running, it has been \
         left switched off",
        d.budget_left_pct
    );
    assert_eq!(
        d.budget_refusals, 0,
        "the budget refused {} escalations the entity wanted",
        d.budget_refusals
    );
}

#[test]
fn the_entity_stays_calibrated() {
    let d = run(TICKS);
    assert!(
        d.surprise_rate_pct <= 10,
        "surprise rate {}%; an entity surprised by a tenth of what it sees \
         does not understand where it is",
        d.surprise_rate_pct
    );
}

#[test]
fn consolidation_waits_for_the_body_to_be_put_away() {
    // Not a scheduling accident. A busy workshop never gives two hundred
    // consecutive quiet ticks, and rewriting memory while a gait loop is
    // running is the thing the quiet check exists to prevent. The
    // dreaming happens when the robot is switched off, which is where
    // the architecture's word for it came from.
    let d = run(TICKS);
    assert_eq!(
        d.dreams, 0,
        "consolidation ran during a busy day, which is what the quiet \
         check is supposed to stop"
    );
    assert!(
        d.dreams_when_idle > 0,
        "and it never ran once the body was put away either"
    );
    assert!(
        d.dreams_when_idle < 50,
        "{} passes in a thousand idle ticks; the quiet counter is not \
         resetting after a pass",
        d.dreams_when_idle
    );
}

#[test]
fn a_whole_day_could_be_left_running() {
    let d = run(TICKS);
    assert!(
        d.could_be_left_running(),
        "first notice on occurrence {}, awareness {}%, precision {}%, \
         {} interruptions per kilotick, {}% budget left",
        d.worst_first_notice(),
        d.awareness_pct(),
        d.interrupt_precision_pct(),
        d.interruptions_per_kilotick,
        d.budget_left_pct
    );
}

#[test]
fn a_day_is_reproducible() {
    assert_eq!(run(2_000), run(2_000));
}

#[test]
fn a_shorter_day_still_notices_things_promptly() {
    // the result should not depend on running for long enough to get
    // lucky
    let d = run(5_000);
    for (kind, before) in &d.first_notice {
        assert!(
            *before <= 3,
            "{} took {before} occurrences to notice",
            kind.name()
        );
    }
}

#[test]
fn the_narration_reports_the_numbers_that_decide_the_verdict() {
    let d = run(TICKS);
    let lines = narrate(&d);
    let steps: Vec<&str> = lines.iter().map(|(s, _)| *s).collect();
    for expected in [
        "ran",
        "first notice",
        "awareness",
        "precision",
        "thinking",
        "dreaming",
        "calibration",
        "verdict",
    ] {
        assert!(steps.contains(&expected), "{expected} is missing");
    }
    let all: String = lines
        .iter()
        .map(|(_, t)| t.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(all.contains("firehose"));
    assert!(all.contains("put away"));
}

// ---------------------------------------------------------------------
// the same days, on the board Phase 0 actually runs on
// ---------------------------------------------------------------------
//
// §5 says the bound is a property of the body rather than of who the
// entity is. That is easy to write. An entity that notices what matters
// with four thousand episodes of room and misses it with two hundred
// and fifty-six is an entity whose attention was paid for by its
// memory, and Phase 0 runs on the small one.

use brainos_host::day::{run_on, Body};

const LONG: u64 = 200_000;

/// One day does not test the bound at all — the entity ends with 201
/// memories and the embedded cap is 256. Asserted so that nobody
/// shortens the comparison back to a day and reads a vacuous pass as a
/// result.
#[test]
fn a_single_day_does_not_fill_a_small_body() {
    let short = run_on(20_000, Body::Board);
    let wide = run_on(20_000, Body::Workstation);
    assert_eq!(
        short.memories, wide.memories,
        "one day now fills the embedded bound, so the ten-day comparison should be \
         reconsidered — it was chosen because a day does not"
    );
}

#[test]
fn ten_days_do_fill_it() {
    let board = run_on(LONG, Body::Board);
    let workstation = run_on(LONG, Body::Workstation);
    assert!(
        board.memories < workstation.memories,
        "the bound never bit ({} vs {}), so the comparison below says nothing",
        board.memories,
        workstation.memories
    );
    assert!(
        board.memories * 4 < workstation.memories,
        "the board kept {} of {} — not enough of a difference for the comparison \
         to be interesting",
        board.memories,
        workstation.memories
    );
}

/// The result: what the small body loses is recall, not attention.
#[test]
fn a_small_body_loses_recall_and_not_attention() {
    let board = run_on(LONG, Body::Board);
    let workstation = run_on(LONG, Body::Workstation);

    assert_eq!(
        board.worst_first_notice(),
        workstation.worst_first_notice(),
        "the board took longer to notice a fault than the workstation did"
    );
    assert!(
        board.awareness_pct() + 5 >= workstation.awareness_pct(),
        "awareness fell from {}% to {}% on the small body",
        workstation.awareness_pct(),
        board.awareness_pct()
    );
    assert!(
        board.interrupt_precision_pct() + 5 >= workstation.interrupt_precision_pct(),
        "interrupt precision fell from {}% to {}% on the small body",
        workstation.interrupt_precision_pct(),
        board.interrupt_precision_pct()
    );
    assert!(
        board.interruptions_per_kilotick <= workstation.interruptions_per_kilotick + 2,
        "the small body became more interrupting: {} vs {} per 1k ticks",
        board.interruptions_per_kilotick,
        workstation.interruptions_per_kilotick
    );
}

#[test]
fn a_small_body_can_still_be_left_running() {
    assert!(
        run_on(LONG, Body::Board).could_be_left_running(),
        "an entity on the board it ships on could not be left running for ten days"
    );
}

/// What is worth telling a human is worth remembering.
///
/// This was missing and the comparison above is what made it obvious:
/// over ten simulated days the entity surfaced dozens of rail dips and
/// leg stalls to its human and wrote none of them into memory. A robot
/// that stalls a leg fifty times and remembers none of them cannot
/// learn from them and cannot tell anyone next week that this has
/// happened before.
#[test]
fn what_the_entity_surfaces_it_also_remembers() {
    let d = run_on(LONG, Body::Workstation);
    assert!(
        d.memories_about_faults > 50,
        "only {} of {} memories are about something going wrong, although the \
         entity surfaced faults to its human throughout",
        d.memories_about_faults,
        d.memories
    );
}

/// And the claim the body comparison rests on, measured rather than
/// asserted: the small body loses recall *in the order salience
/// decides*. Forced to forget, it drops the routine walks and keeps the
/// things that went wrong.
#[test]
fn a_small_body_forgets_the_routine_and_keeps_the_faults() {
    let board = run_on(LONG, Body::Board);
    let workstation = run_on(LONG, Body::Workstation);

    let board_pct = (board.memories_about_faults * 100) / board.memories.max(1);
    let workstation_pct = (workstation.memories_about_faults * 100) / workstation.memories.max(1);

    assert!(
        board_pct > workstation_pct + 30,
        "the constrained body's memory is {board_pct}% faults against the \
         unconstrained one's {workstation_pct}% — not enough of a difference to say \
         salience decided the order rather than age"
    );
    assert!(
        board.memories_routine < workstation.memories_routine,
        "the small body kept as much routine work as the large one"
    );
}
