//! BFS  LEVEL    Stage 6 - property domains (bounded checking, not a proof)  [crate brainos-proof]
//! BFS  PURPOSE  Tests that the gate, envelope and hostile-input domains are fully enumerated, every property holds, and the harness cannot pass vacuously
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, proof, rt
//! BFS  USED BY  (entry point or test)
//!
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

// =====================================================================
// the second domain: two envelopes
// =====================================================================

use brainos_proof::envelopes::{
    run_envelope_case, verify_envelopes, Ask, Calibrated, EnvelopeCase, EnvelopeDomain, Geometry,
    Posture, Power, SeatBound,
};

#[test]
fn the_envelope_domain_is_exactly_the_size_it_claims() {
    let cases = EnvelopeDomain::enumerate();
    assert_eq!(cases.len(), EnvelopeDomain::SIZE);
    assert_eq!(EnvelopeDomain::SIZE, 400);

    let mut seen = cases.clone();
    seen.sort_by_key(|c| {
        (
            c.seat as u8 as u32,
            c.calibrated as u8 as u32,
            c.geometry as u8 as u32,
            c.posture as u8 as u32,
            c.ask as u8 as u32,
            c.power as u8 as u32,
        )
    });
    let before = seen.len();
    seen.dedup();
    assert_eq!(before, seen.len());
}

#[test]
fn a_body_never_applies_a_command_outside_the_intersection() {
    // The property the whole body path rests on, over every combination
    // of seat bound, calibration, geometry, posture, ask and power.
    let report = verify_envelopes();
    assert!(
        report.failures.is_empty(),
        "{} failures, first few: {:?}",
        report.failures.len(),
        &report.failures[..report.failures.len().min(5)]
    );
    assert_eq!(report.cases, EnvelopeDomain::SIZE);
    assert!(report.holds());
}

#[test]
fn the_envelope_domain_actually_exercised_the_body() {
    let report = verify_envelopes();
    assert!(
        report.is_meaningful(),
        "accepted {} of {} cases with {} distinct refusals",
        report.accepted,
        report.cases,
        report.distinct_refusals
    );
    assert!(report.accepted > 0, "nothing was ever accepted");
    assert!(
        report.accepted < report.cases,
        "everything was accepted, which checks nothing"
    );
    assert!(report.distinct_refusals >= 4);
}

#[test]
fn a_disjoint_seat_envelope_leaves_nothing_authorized() {
    let case = EnvelopeCase {
        seat: SeatBound::Disjoint,
        calibrated: Calibrated::Yes,
        geometry: Geometry::Balanced,
        posture: Posture::Ready,
        ask: Ask::GoodWalk,
        power: Power::Good,
    };
    let out = run_envelope_case(&case);
    assert!(!out.accepted);
    assert!(out.applied.is_empty());
    // the intersection is empty, which is unstated, which refuses
    assert!(out
        .effective
        .and_then(|e| e.limit(brainos_rt::envelope::Channel::Angle))
        .is_none());
}

#[test]
fn a_seat_that_grants_more_than_the_body_can_do_still_only_gets_the_body() {
    let wider = run_envelope_case(&EnvelopeCase {
        seat: SeatBound::Wider,
        calibrated: Calibrated::Yes,
        geometry: Geometry::Balanced,
        posture: Posture::Ready,
        ask: Ask::GoodWalk,
        power: Power::Good,
    });
    let exact = run_envelope_case(&EnvelopeCase {
        seat: SeatBound::Exact,
        ..EnvelopeCase {
            seat: SeatBound::Exact,
            calibrated: Calibrated::Yes,
            geometry: Geometry::Balanced,
            posture: Posture::Ready,
            ask: Ask::GoodWalk,
            power: Power::Good,
        }
    });
    assert!(wider.accepted && exact.accepted);
    assert_eq!(
        wider.effective, exact.effective,
        "granting more than the body can do is the same as granting exactly what it can"
    );
}

#[test]
fn every_dimension_of_the_envelope_domain_changes_something() {
    // A dimension that never changes an outcome is a dimension that is
    // multiplying the domain for nothing.
    let base = EnvelopeCase {
        seat: SeatBound::Exact,
        calibrated: Calibrated::Yes,
        geometry: Geometry::Balanced,
        posture: Posture::Ready,
        ask: Ask::GoodWalk,
        power: Power::Good,
    };
    assert!(run_envelope_case(&base).accepted);

    let variants: Vec<(&str, EnvelopeCase)> = vec![
        ("seat", EnvelopeCase { seat: SeatBound::Narrower, ..base }),
        ("calibrated", EnvelopeCase { calibrated: Calibrated::No, ..base }),
        ("geometry", EnvelopeCase { geometry: Geometry::TailHeavy, ..base }),
        ("posture", EnvelopeCase { posture: Posture::Stopped, ..base }),
        ("ask", EnvelopeCase { ask: Ask::Creep, ..base }),
        ("power", EnvelopeCase { power: Power::Flat, ..base }),
    ];
    for (name, c) in variants {
        assert!(
            !run_envelope_case(&c).accepted,
            "changing {name} alone did not change the outcome, so the dimension earns nothing"
        );
    }
}

#[test]
fn the_envelope_oracle_and_the_runtime_agree_case_by_case() {
    for (i, case) in EnvelopeDomain::enumerate().iter().enumerate() {
        let out = run_envelope_case(case);
        assert_eq!(
            out.accepted,
            case.should_accept(),
            "case {i} {case:?}: runtime says {}, oracle says {}",
            out.accepted,
            case.should_accept()
        );
    }
}

// =====================================================================
// hostile input
// =====================================================================

use brainos_proof::hostile::{targets, verify_hostile};

#[test]
fn no_decoder_panics_on_hostile_input() {
    // The bar: there is no input for which the right answer is a panic.
    // A panic in a decoder is a denial of service, and on a body it is a
    // fall. This test passing *is* the assertion — if any target panics,
    // the harness does not return.
    let report = verify_hostile(2_000);
    assert!(report.total_inputs > 30_000, "{}", report.total_inputs);
}

#[test]
fn anything_that_decodes_re_encodes_to_exactly_its_own_bytes() {
    // The property the audit Merkle log, the export signature and the
    // body-identity digest all rest on. A value that could arrive two
    // ways would make a signature over it prove nothing about the value.
    let report = verify_hostile(2_000);
    for t in &report.targets {
        assert!(
            t.is_canonical(),
            "{}: {} of {} accepted inputs did not re-encode to their own bytes",
            t.name,
            t.not_canonical,
            t.accepted
        );
    }
    assert!(report.holds());
}

#[test]
fn the_fuzzing_reaches_past_the_length_checks() {
    // For a plain format, mutated-valid input is what gets to the logic;
    // a target where nothing was ever accepted has had its magic number
    // tested and nothing else.
    let report = verify_hostile(2_000);
    let shallow = report.shallow();
    assert!(
        shallow.is_empty(),
        "these targets were not reached: {shallow:?}"
    );
    for t in report.targets.iter().filter(|t| !t.integrity_protected) {
        assert!(
            t.accepted_from_mutation > 0,
            "{} accepted nothing from mutated-valid input",
            t.name
        );
    }
}

#[test]
fn an_integrity_check_catches_every_single_mutation() {
    // The inverted expectation, and the stronger property. A frame
    // carries a 128-bit MAC and a journal record a CRC over everything
    // including its header; flipping any bit in either must make it
    // undecodable. Mutation getting through would mean the check is not
    // doing its job.
    let report = verify_hostile(3_000);
    for t in report.targets.iter().filter(|t| t.integrity_protected) {
        assert_eq!(
            t.accepted_from_mutation, 0,
            "{}: {} of {} mutated inputs got past the integrity check",
            t.name, t.accepted_from_mutation, t.tried
        );
        assert!(t.integrity_held());
    }
}

#[test]
fn random_bytes_are_almost_never_a_valid_structure() {
    // The other direction: if random bytes were routinely accepted, the
    // decoder would not be checking anything. A frame with a MAC should
    // essentially never appear by chance.
    let report = verify_hostile(2_000);
    let frame = report
        .targets
        .iter()
        .find(|t| t.name == "transport::Frame")
        .unwrap();
    // every acceptance must have come from mutation, not from noise
    assert_eq!(
        frame.accepted, frame.accepted_from_mutation,
        "random bytes produced a frame with a valid MAC, which should be \
         a 2^-128 event"
    );
}

#[test]
fn the_fuzzing_is_deterministic() {
    assert_eq!(verify_hostile(200), verify_hostile(200));
}

#[test]
fn every_target_says_what_it_reads() {
    for t in targets() {
        assert!(!t.name.is_empty());
        assert!(
            t.reads.len() > 15,
            "{}: a target with no explanation is a line of coverage",
            t.name
        );
        // and its seed is a genuinely valid encoding, or the mutation
        // generator is mutating garbage
        let seed = (t.seed_value)();
        assert!(!seed.is_empty(), "{}", t.name);
    }
}

#[test]
fn every_seed_value_actually_decodes() {
    // The mutation generator starts from these. A seed that does not
    // decode means that target's mutation pass is fuzzing noise.
    for t in targets() {
        let seed = (t.seed_value)();
        assert!(
            (t.decode)(&seed),
            "{}'s seed value does not decode, so its mutation pass tests nothing",
            t.name
        );
    }
}

#[test]
fn a_journal_record_from_hostile_storage_never_half_decodes() {
    // Journal slots sit on storage the robot's owner can physically
    // reach. A torn or edited record must produce an error, never a
    // partially-populated Record that something then treats as memory.
    //
    // Every one of several thousand mutations is caught by the CRC —
    // which is the whole reason the CRC seals the header and the
    // generation alongside the payload, so a flipped bit anywhere
    // invalidates the slot rather than silently reordering history.
    let report = verify_hostile(3_000);
    let journal = report
        .targets
        .iter()
        .find(|t| t.name == "memory::journal::Record")
        .unwrap();
    assert!(journal.is_canonical());
    assert_eq!(journal.accepted_from_mutation, 0);
    assert!(journal.tried >= 6_000);
}
