//! The fleet-transfer result, as assertions.
//!
//! The experiment ran twice under two adoption rules, and both runs are
//! part of the record.
//!
//! Under the first — copy the seller's evidence wholesale — a skill
//! adopted from a deployment unlike your own left the entity
//! *permanently* worse than one that started from nothing: 80% against
//! alone's 87%, still short after twelve hundred episodes, with the cost
//! showing up as refusal rather than failure. An entity that believes a
//! condition is required declines exactly the situations that would
//! disprove it, so its caution prevents the correction.
//!
//! Under the second, what transfers is the *claim* rather than the
//! seller's numbers: the buyer holds it on trust, acts on it — which is
//! what makes a skill worth buying — and deliberately violates it one
//! blocked opportunity in twelve until it has five tests of its own.
//! These tests assert the second, including what it costs.

use brainos_host::fleet_experiment::{fleet, Arm};

fn run() -> brainos_host::fleet_experiment::FleetResult {
    fleet(6, 900, 6)
}

/// The marketplace's value proposition, measured: a skill bought from
/// the same kind of deployment is worth hundreds of episodes of
/// experience, delivered on the first day.
#[test]
fn a_well_sourced_skill_is_worth_a_career_of_experience() {
    let r = run();
    let matched_first = r.mean_judgement(Arm::Matched, 0);
    let alone_first = r.mean_judgement(Arm::Alone, 0);
    let alone_last = r.mean_judgement(Arm::Alone, r.slices() - 1);
    assert!(
        matched_first > alone_first + 2,
        "adoption bought nothing on day one: matched {matched_first}% vs alone {alone_first}%"
    );
    assert!(
        matched_first + 2 >= alone_last,
        "a bought skill ({matched_first}%) did not reach where learning alone ends up ({alone_last}%)"
    );
}

/// And it is genuinely near the ceiling, not merely better than nothing.
#[test]
fn a_well_sourced_skill_is_close_to_the_true_rule() {
    let r = run();
    let last = r.slices() - 1;
    let matched = r.mean_judgement(Arm::Matched, last);
    let oracle = r.mean_judgement(Arm::Oracle, last);
    assert!(
        matched + 4 >= oracle,
        "matched {matched}% is well short of the ceiling {oracle}%"
    );
}

/// A badly-sourced skill does start out costing something. If it did
/// not, there would be nothing here to fix and nothing to measure.
#[test]
fn a_badly_sourced_skill_does_start_out_costing_something() {
    let r = run();
    let blind = r.mean_judgement(Arm::Blind, 0);
    let alone = r.mean_judgement(Arm::Alone, 0);
    assert!(
        blind < alone,
        "blind adoption ({blind}%) was not a handicap even at the start ({alone}%), \
         so this experiment is not measuring what it claims to"
    );
}

/// The finding that made the adoption rule change: under the probe rule
/// the handicap is temporary rather than permanent.
#[test]
fn a_badly_sourced_skill_recovers_instead_of_being_permanent() {
    let r = run();
    let last = r.slices() - 1;
    let blind = r.mean_judgement(Arm::Blind, last);
    let alone = r.mean_judgement(Arm::Alone, last);
    assert!(
        blind + 2 >= alone,
        "blind adoption ended at {blind}% against alone's {alone}%: the probe rule \
         is supposed to make a wrong borrowed claim temporary rather than permanent"
    );
    let converged = r
        .converges_with_alone_at(Arm::Blind)
        .expect("a blind adopter never reached a fresh entity's contemporaneous level");
    assert!(
        converged < r.slices() - 1,
        "convergence happened only in the final slice, which is not convergence"
    );
}

/// The mechanism, measured separately so the result is not just a
/// number. The cost of a wrong borrowed belief is *refusal* — declining
/// situations that would have worked, which are exactly the situations
/// that would have taught otherwise. Under the probe rule that cost
/// shrinks rather than persisting.
#[test]
fn the_cost_is_refusal_and_the_probe_rule_shrinks_it() {
    let r = run();
    let first = r.mean_missed(Arm::Blind, 0);
    let last = r.mean_missed(Arm::Blind, r.slices() - 1);
    assert!(
        first > r.mean_missed(Arm::Alone, 0),
        "blind adoption did not start out refusing more than learning alone"
    );
    assert!(
        last < first,
        "the wrong belief's cost did not shrink ({first}% -> {last}%), which is \
         exactly what the probe rule exists to do"
    );
}

/// The correction has a price, and the well-sourced entity pays it too:
/// it spends real attempts checking claims that happen to be true. The
/// price has to be small, or the rule is not worth having.
#[test]
fn probing_costs_something_and_not_very_much() {
    let r = run();
    let probes = r.mean_probes(Arm::Matched);
    assert!(probes > 0, "nothing was ever probed, so nothing was checked");
    assert!(
        probes < 40,
        "{probes} deliberate violations per entity is not a cheap correction"
    );
    let matched = r.mean_judgement(Arm::Matched, r.slices() - 1);
    let alone = r.mean_judgement(Arm::Alone, r.slices() - 1);
    assert!(
        matched >= alone,
        "after paying for probes, a well-sourced skill ({matched}%) is no longer \
         better than learning alone ({alone}%)"
    );
}

/// The veterans are selling something real. If a seller's evidence were
/// empty the whole experiment would be comparing nothing to nothing.
#[test]
fn every_deployment_produces_a_veteran_with_something_to_sell() {
    let r = run();
    for (d, ps) in &r.veterans {
        assert!(
            !ps.is_empty(),
            "the veteran of the {} learned nothing it could back up",
            d.name()
        );
        assert!(
            ps.iter().any(|(tag, _, _)| tag == d.true_condition()),
            "the veteran of the {} did not find the condition that actually matters ({}): {ps:?}",
            d.name(),
            d.true_condition()
        );
    }
}

/// And a seller sells findings, not hunches. Getting this wrong made
/// the matched and blind arms adopt near-identical lists and come out
/// to the digit — which is what a bug in an experiment looks like when
/// it is lucky enough to be obvious.
#[test]
fn the_matched_and_blind_arms_are_actually_different() {
    let r = run();
    let matched: Vec<u8> = (0..r.slices()).map(|i| r.mean_judgement(Arm::Matched, i)).collect();
    let blind: Vec<u8> = (0..r.slices()).map(|i| r.mean_judgement(Arm::Blind, i)).collect();
    assert_ne!(
        matched, blind,
        "the two adoption arms behaved identically, which means they adopted the \
         same thing and the experiment is measuring nothing"
    );
}
