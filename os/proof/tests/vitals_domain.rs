//! BFS  LEVEL    Stage 6 - property domains (bounded checking, not a proof)  [crate brainos-proof]
//! BFS  PURPOSE  Tests that the parts-and-vitals domain holds against the real policy, is not vacuous, and is violated by a policy with no guard
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, proof
//! BFS  USED BY  (entry point or test)
//!
//! The seventh domain: does it hold, does it mean anything, can it fail.

use brainos_abi::error::{DenyCode, Stage};
use brainos_proof::report::Report;
use brainos_proof::vitals::{
    cases, ought, verify_vitals, verify_vitals_without_the_guard, GpuReading, Own, OWN, READINGS,
    VALUES, VERBS,
};

#[test]
fn the_domain_is_the_full_product_of_its_dimensions() {
    assert_eq!(cases().len(), VERBS.len() * VALUES.len() * READINGS.len() * OWN.len() * 2);
    assert_eq!(cases().len(), 504);
}

#[test]
fn the_real_policy_agrees_with_the_oracle_on_every_case() {
    let r = verify_vitals();
    assert!(
        r.holds(),
        "{} disagreements; first: {:?}",
        r.disagreements.len(),
        r.disagreements.first()
    );
    assert_eq!(r.cases, 504);
}

#[test]
fn the_domain_is_not_vacuous() {
    let r = verify_vitals();
    assert!(r.is_meaningful());
    assert_eq!(
        r.permitted + r.refused_at_validate + r.refused_at_simulate + r.refused_at_policy,
        r.cases
    );
    assert!(r.decided_by_the_guard >= 16, "{}", r.decided_by_the_guard);
}

#[test]
fn a_policy_that_ignores_its_neighbour_is_caught() {
    let old = verify_vitals_without_the_guard();
    assert!(!old.holds(), "the domain cannot tell a guarded policy from an unguarded one");
    // every case the guard alone decides must be among the failures,
    // and each of those is the dangerous direction: granted when it
    // ought to have been refused
    let fail_open = old
        .disagreements
        .iter()
        .filter(|d| {
            d.policy_said.is_none()
                && d.oracle_said == Some((Stage::Simulate, DenyCode::PredictedInvariantBreak))
        })
        .count();
    assert_eq!(fail_open, verify_vitals().decided_by_the_guard);
    assert!(fail_open > 0);
}

#[test]
fn every_dimension_changes_some_outcome() {
    // a dimension that never changes an outcome multiplies the domain
    // for nothing
    let all = cases();
    let differs = |f: &dyn Fn(&brainos_proof::vitals::Case, &brainos_proof::vitals::Case) -> bool| {
        all.iter().any(|a| all.iter().any(|b| f(a, b) && ought(a) != ought(b)))
    };
    let same_but = |a: &brainos_proof::vitals::Case, b: &brainos_proof::vitals::Case, skip: u8| {
        (skip == 0 || a.verb == b.verb)
            && (skip == 1 || a.value == b.value)
            && (skip == 2 || a.gpu == b.gpu)
            && (skip == 3 || a.own == b.own)
            && (skip == 4 || a.relief == b.relief)
    };
    assert!(differs(&|a, b| same_but(a, b, 1) && a.value != b.value), "value");
    assert!(differs(&|a, b| same_but(a, b, 2) && a.gpu != b.gpu), "gpu reading");
    assert!(differs(&|a, b| same_but(a, b, 3) && a.own != b.own), "own vital");
    assert!(differs(&|a, b| same_but(a, b, 4) && a.relief != b.relief), "relief");
    // the verb dimension is deliberately outcome-neutral: both verbs
    // must be guarded identically, which is what running both checks
    assert!(!differs(&|a, b| same_but(a, b, 0) && a.verb != b.verb));
}

#[test]
fn the_oracle_itself_says_the_canonical_case_is_a_predicted_break() {
    let c = brainos_proof::vitals::Case {
        verb: 0,
        value: 0,
        gpu: GpuReading::At(99.0),
        own: Own::Nominal,
        relief: false,
    };
    assert_eq!(ought(&c), Some((Stage::Simulate, DenyCode::PredictedInvariantBreak)));
}

#[test]
fn the_full_report_includes_the_seventh_domain_and_still_holds() {
    let r = Report::full();
    assert!(r.holds());
    assert!(r.vitals.as_ref().is_some_and(|v| v.cases == 504));
    assert!(r.render().contains("Seventh domain"));
}
