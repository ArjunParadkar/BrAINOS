//! The fleet-transfer result, as assertions.
//!
//! These encode what the experiment *found*, including the part that
//! contradicted the prediction made before it ran. If a later change
//! makes blind adoption recover, one of these goes red — which is
//! correct: that would be a different finding and it should be noticed
//! rather than absorbed.

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
        matched_first > alone_first + 4,
        "adoption bought nothing on day one: matched {matched_first}% vs alone {alone_first}%"
    );
    assert!(
        matched_first >= alone_last,
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
        matched + 3 >= oracle,
        "matched {matched}% is well short of the ceiling {oracle}%"
    );
}

/// The finding that matters, and the one the prediction got wrong.
///
/// A skill adopted from a deployment unlike your own does not merely
/// fail to help. It leaves the entity permanently worse than one that
/// started from nothing — over the whole run, not just at the start.
#[test]
fn a_badly_sourced_skill_leaves_an_entity_worse_off_than_no_skill_at_all() {
    let r = run();
    let last = r.slices() - 1;
    let blind = r.mean_judgement(Arm::Blind, last);
    let alone = r.mean_judgement(Arm::Alone, last);
    assert!(
        blind < alone,
        "blind adoption ({blind}%) caught up with learning alone ({alone}%) — \
         this is the pre-registered prediction that the run contradicted, and \
         if it now holds, something changed and it needs looking at"
    );
}

/// And the mechanism behind it, measured separately so the finding is
/// not just a number. The entity is not failing more; it is *refusing*
/// more — declining situations that would have worked, which are
/// exactly the situations that would have taught it otherwise.
#[test]
fn the_cost_is_refusal_and_refusal_is_what_prevents_the_correction() {
    let r = run();
    let last = r.slices() - 1;
    let blind_missed = r.mean_missed(Arm::Blind, last);
    let alone_missed = r.mean_missed(Arm::Alone, last);
    assert!(
        blind_missed > alone_missed,
        "blind {blind_missed}% vs alone {alone_missed}%: the cost was expected to \
         show up as missed opportunity"
    );
    // and it does not shrink over the run
    let first = r.mean_missed(Arm::Blind, 0);
    assert!(
        blind_missed + 5 >= first,
        "the wrong belief decayed on its own ({first}% -> {blind_missed}%), which \
         would contradict the mechanism this result is attributed to"
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
