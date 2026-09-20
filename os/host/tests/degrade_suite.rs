//! Degrading honestly, asserted.

use brainos_cognition::resource::Tier;
use brainos_host::degrade::degrade;

#[test]
fn the_entity_degrades_honestly() {
    let d = degrade();
    assert!(d.is_honest(), "the entity degraded without saying so: {d:#?}");
}

/// Reach loss degrades, it does not halt. The body kept moving in every
/// phase, including the one where the entity was down to its reflexes.
#[test]
fn losing_the_cloud_degrades_rather_than_halting() {
    let d = degrade();
    assert!(d.kept_acting, "the body stopped when the network did");
    assert!(
        d.reflexes_never_left,
        "the reflex tier went away, which it cannot: it is on the body"
    );
    let ceilings: Vec<Tier> = d.phases.iter().map(|p| p.ceiling).collect();
    assert_eq!(
        ceilings,
        vec![Tier::Frontier, Tier::Deliberate, Tier::Fast, Tier::Frontier],
        "the ceiling did not follow the reach"
    );
}

/// §3.7 puts memory on the entity, so an outage cannot cost memories —
/// and the record of the outage survives the outage, which is the part
/// that makes it findable afterwards.
#[test]
fn memory_keeps_working_while_the_network_does_not() {
    let d = degrade();
    for p in &d.phases {
        assert!(
            p.memories_written > 0,
            "no memories were written during \"{}\" — if nothing is written, this \
             claim is untested rather than true",
            p.what
        );
    }
    assert!(
        d.outages_recorded >= 2,
        "only {} outages made it into memory",
        d.outages_recorded
    );
}

/// The failure this run exists to prevent: thinking less hard than the
/// situation warranted, and not saying so.
#[test]
fn every_degraded_decision_is_marked_as_degraded() {
    let d = degrade();
    let dark: Vec<_> = d
        .phases
        .iter()
        .filter(|p| p.ceiling != Tier::Frontier)
        .collect();
    assert_eq!(dark.len(), 2, "the scenario did not actually go dark");
    for p in dark {
        assert!(
            p.short_on_reach > 0,
            "\"{}\" had a reduced ceiling and yet no decision was marked short on \
             reach — the entity thought less hard and did not say so",
            p.what
        );
        assert_eq!(
            p.served_pct(),
            0,
            "\"{}\" reported serving situations at the tier they warranted while its \
             ceiling was {}",
            p.what,
            p.ceiling.name()
        );
    }
}

/// And marked with the *right* cause. A dropped link reported as a
/// budget problem sends a human to top up an allowance, which changes
/// nothing, while the robot goes on being quietly less capable.
#[test]
fn a_dropped_link_is_not_reported_as_an_affordability_problem() {
    let d = degrade();
    assert!(
        d.would_have_blamed_the_budget,
        "no reach-limited escalation occurred, so this says nothing"
    );
    assert!(
        d.a_degraded_decision.contains("out of reach"),
        "a reach-limited decision does not name reach: {}",
        d.a_degraded_decision
    );
    assert!(
        !d.a_degraded_decision.contains("budget"),
        "a reach-limited decision still mentions the budget: {}",
        d.a_degraded_decision
    );
    // and the budget genuinely was not the constraint
    for p in &d.phases {
        assert_eq!(
            p.short_on_budget, 0,
            "\"{}\" ran out of allowance, which confounds the thing being measured",
            p.what
        );
    }
}

/// When the link comes back, the entity picks up where it was — the
/// degradation is not sticky.
#[test]
fn reach_returning_restores_the_ceiling() {
    let d = degrade();
    let last = d.phases.last().expect("a final phase");
    assert_eq!(last.ceiling, Tier::Frontier);
    assert_eq!(last.short_on_reach, 0);
    assert_eq!(last.served_pct(), 100);
}
