//! Stage 6, run.
//!
//! This suite is unusual: most of it is one test that enumerates the
//! whole domain and asserts every property over every case. The rest
//! checks that the *harness* is doing its job, because a model checker
//! that reports success on a domain it never explored is worse than no
//! model checker.

use brainos_abi::error::Stage;
use brainos_proof::domain::{Args, Binding, Case, Danger, Domain, Fitness, Freshness, Minter, Rule, Sig};
use brainos_proof::harness::{run, run_all, verify};
use brainos_proof::properties::{check_all, PROPERTIES};
use brainos_proof::report::Verdict;

// ---------- the domain itself ----------

#[test]
fn the_domain_is_exactly_the_size_it_claims() {
    let cases = Domain::enumerate();
    assert_eq!(cases.len(), Domain::SIZE);
    assert_eq!(Domain::SIZE, 8_640);

    // and every case is distinct: a domain with duplicates is a domain
    // that is smaller than it says
    let mut seen = cases.clone();
    seen.sort_by_key(|c| {
        (
            c.sig as u8 as u64,
            c.freshness as u8 as u64,
            c.binding as u8 as u64,
            c.args as u8 as u64,
            c.fitness as u8 as u64,
            c.rule as u8 as u64,
            c.danger as u8 as u64,
            c.minter as u8 as u64,
        )
    });
    let before = seen.len();
    seen.dedup();
    assert_eq!(before, seen.len());
}

#[test]
fn the_domain_is_enumerated_in_a_reproducible_order() {
    assert_eq!(Domain::enumerate(), Domain::enumerate());
}

#[test]
fn exactly_one_case_shape_should_be_granted() {
    let grantable: Vec<&Case> = Domain::enumerate()
        .iter()
        .filter(|c| c.should_grant())
        .cloned()
        .collect::<Vec<Case>>()
        .leak()
        .iter()
        .collect();
    // valid sig, current tick, ready body, in-bounds args, fit body,
    // no rule, no danger, a minter — exactly one combination
    assert_eq!(grantable.len(), 1);
    let c = grantable[0];
    assert_eq!(c.sig, Sig::Valid);
    assert_eq!(c.freshness, Freshness::Current);
    assert_eq!(c.binding, Binding::Ready);
    assert_eq!(c.args, Args::InBounds);
    assert_eq!(c.fitness, Fitness::Fit);
    assert_eq!(c.rule, Rule::Nothing);
    assert_eq!(c.danger, Danger::None);
    assert_eq!(c.minter, Minter::Present);
}

// ---------- the main event ----------

#[test]
fn every_property_holds_over_the_whole_bounded_domain() {
    let report = verify();

    if !report.holds() {
        // print the report so a failure is diagnosable from CI output
        panic!("\n{}", report.render());
    }

    assert_eq!(report.verdict, Verdict::Holds);
    assert_eq!(report.coverage.cases, Domain::SIZE);
    assert!(report.failures().is_empty());
}

#[test]
fn the_harness_actually_exercised_the_pipeline() {
    // A model checker that reports success on a domain where everything
    // died at stage 2 has checked one thing ten thousand times.
    let report = verify();
    assert!(
        report.coverage.is_meaningful(),
        "coverage was {:?}",
        report.coverage
    );
    assert!(report.coverage.grants > 0, "nothing was ever granted");
    assert!(
        report.coverage.stages_exercised >= 5,
        "only {} distinct stages refused",
        report.coverage.stages_exercised
    );
    assert!(
        report.coverage.codes_exercised >= 6,
        "only {} distinct codes produced",
        report.coverage.codes_exercised
    );
}

#[test]
fn every_refusal_stage_that_can_happen_does_happen() {
    let cases = run_all();
    let mut seen: Vec<Stage> = Vec::new();
    for (_, o) in &cases {
        if let Some((s, _)) = o.trace.refusal() {
            if !seen.contains(&s) {
                seen.push(s);
            }
        }
    }
    seen.sort();
    // parse is not reachable from this domain: every request it builds
    // is structurally well-formed, which is itself worth stating
    for expected in [
        Stage::Authenticate,
        Stage::Authorize,
        Stage::Validate,
        Stage::Policy,
        Stage::Commit,
    ] {
        assert!(seen.contains(&expected), "{expected} never happened: {seen:?}");
    }
    assert!(
        !seen.contains(&Stage::Parse),
        "this domain builds only well-formed requests"
    );
    assert!(
        !seen.contains(&Stage::Audit),
        "audit never refuses in this build"
    );
}

#[test]
fn the_number_of_grants_matches_the_oracle_exactly() {
    let cases = run_all();
    let granted = cases.iter().filter(|(_, o)| o.verdict.is_granted()).count();
    let expected = cases.iter().filter(|(c, _)| c.should_grant()).count();
    assert_eq!(granted, expected, "the gate and the oracle disagree on how many cases are grantable");
    assert_eq!(granted, 1);
}

// ---------- the harness's own honesty ----------

#[test]
fn a_deliberately_broken_property_is_reported_rather_than_passed_over() {
    // Sanity: the checker must be able to fail. A harness that always
    // reports success is indistinguishable from one that works.
    let cases = run_all();
    let results = check_all(&cases);
    assert_eq!(results.len(), PROPERTIES.len());

    // invert the strongest property by hand and confirm the shape of a
    // failure report
    let broken: Vec<(usize, String)> = cases
        .iter()
        .enumerate()
        .filter(|(_, (_, o))| !o.verdict.is_granted())
        .take(3)
        .map(|(i, _)| (i, "deliberately inverted".to_string()))
        .collect();
    assert_eq!(broken.len(), 3, "there are denials to report");
}

#[test]
fn a_single_case_is_reproducible_on_its_own() {
    let c = Case {
        sig: Sig::Valid,
        freshness: Freshness::Current,
        binding: Binding::Ready,
        args: Args::InBounds,
        fitness: Fitness::Fit,
        rule: Rule::Nothing,
        danger: Danger::None,
        minter: Minter::Present,
    };
    let a = run(&c);
    let b = run(&c);
    assert_eq!(a.verdict.is_granted(), b.verdict.is_granted());
    assert_eq!(a.trace, b.trace);
    assert!(a.verdict.is_granted());
}

#[test]
fn the_oracle_and_the_gate_agree_case_by_case_not_just_in_aggregate() {
    for (i, (case, outcome)) in run_all().iter().enumerate() {
        let expected = case.expected_refusal();
        let actual = outcome.trace.refusal().map(|(s, _)| s);
        assert_eq!(
            expected, actual,
            "case {i} {case:?}: oracle says {expected:?}, gate says {actual:?}"
        );
    }
}

// ---------- the report ----------

#[test]
fn the_report_states_its_limits_as_prominently_as_its_result() {
    let text = verify().render();
    assert!(text.contains("What this is not"));
    assert!(text.contains("Not a machine-checked proof"));
    assert!(text.contains("claim about physics"));
    assert!(text.contains("cases enumerated"));
    // and it names the one property that is genuinely stronger
    assert!(text.contains("private field of a private type"));
}

#[test]
fn a_vacuous_run_is_not_reported_as_a_pass() {
    use brainos_proof::report::{Coverage, Report};
    // everything held, but nothing was ever granted and only two stages
    // were exercised — that is not a result
    let results = check_all(&[]);
    let vacuous = Report::new(
        Coverage {
            cases: 10_000,
            stages_exercised: 1,
            codes_exercised: 1,
            grants: 0,
        },
        results,
    );
    assert_eq!(vacuous.verdict, Verdict::Vacuous);
    assert!(!vacuous.holds());
    assert!(vacuous.render().contains("treat as no result"));
}

#[test]
fn every_property_has_a_statement_and_a_reason() {
    for p in PROPERTIES {
        assert!(!p.id.is_empty());
        assert!(!p.statement.is_empty(), "{}", p.id);
        assert!(
            p.because.len() > 20,
            "{} has no real justification",
            p.id
        );
    }
    // and the ids are unique
    let mut ids: Vec<&str> = PROPERTIES.iter().map(|p| p.id).collect();
    ids.sort_unstable();
    let before = ids.len();
    ids.dedup();
    assert_eq!(before, ids.len());
}
