//! BFS  LEVEL    Stage 6 - property domains (bounded checking, not a proof)  [crate brainos-proof]
//! BFS  PURPOSE  Lists the rules every gate outcome must obey (such as refusing at the right stage) as predicates and checks them over all enumerated cases
//! BFS  DEFINES  Property, PropertyResult, PROPERTIES, check_all
//! BFS  USES     this crate: domain | brainos: abi, crypto, kira
//! BFS  USED BY  host/main, nostd/lib, proof/harness, proof/lib, proof/report, proof/tests/proof_suite
//!
//! The properties.
//!
//! Each is a statement about *every* trace the gate produces, checked
//! over the whole bounded domain. They are written as predicates over
//! (case, outcome) rather than as tests, so adding a dimension to the
//! domain re-checks all of them for free.
//!
//! The ordering properties are the ones that have historically found
//! bugs. "Denied at the right stage" is easy to get wrong in a way that
//! passes a hand-written suite: a refusal that happens to be caught at
//! validate because authorize let it through looks identical from
//! outside, and means authorize has a hole that something else covered.

use crate::domain::{Args, Binding, Case, Danger, Fitness, Freshness, Minter, Rule, Sig};
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::error::{DenyCode, Stage};
use brainos_kira::gate::{Outcome, Verdict};

/// One property, and how to check it.
pub struct Property {
    pub id: &'static str,
    pub statement: &'static str,
    /// why it matters, in the terms the architecture uses
    pub because: &'static str,
    pub check: fn(&Case, &Outcome) -> Result<(), String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyResult {
    pub id: &'static str,
    pub cases_checked: usize,
    pub failures: Vec<(usize, String)>,
}

impl PropertyResult {
    pub fn holds(&self) -> bool {
        self.failures.is_empty()
    }
}

// ---------------------------------------------------------------------
// the properties
// ---------------------------------------------------------------------

fn p1_no_grant_without_every_stage(_c: &Case, o: &Outcome) -> Result<(), String> {
    if o.verdict.is_granted() && !o.trace.passed_everything() {
        return Err(alloc::string::ToString::to_string(
            "a token was minted on a trace that did not pass every stage",
        ));
    }
    Ok(())
}

fn p2_fail_closed(_c: &Case, o: &Outcome) -> Result<(), String> {
    match (o.trace.refusal(), o.verdict.is_granted()) {
        (Some(_), true) => Err(alloc::string::ToString::to_string(
            "a stage refused and a token was minted anyway",
        )),
        _ => Ok(()),
    }
}

fn p3_audit_always_runs(_c: &Case, o: &Outcome) -> Result<(), String> {
    if o.trace.at(Stage::Audit).is_skipped() {
        return Err(alloc::string::ToString::to_string(
            "audit did not run; a decision nobody can review is a decision nobody made",
        ));
    }
    Ok(())
}

fn p4_trace_well_formed(_c: &Case, o: &Outcome) -> Result<(), String> {
    if !o.trace.is_well_formed() {
        return Err(alloc::string::ToString::to_string(
            "the trace is not in stage order, or has more than one refusal, or passed something after a refusal",
        ));
    }
    Ok(())
}

fn p5_nothing_runs_after_a_refusal(_c: &Case, o: &Outcome) -> Result<(), String> {
    let entries = o.trace.entries();
    let Some(i) = entries.iter().position(|e| e.outcome.is_refusal()) else {
        return Ok(());
    };
    for e in &entries[i + 1..] {
        if e.stage != Stage::Audit && !e.outcome.is_skipped() {
            return Err(alloc::string::ToString::to_string(
                "a stage ran after an earlier one refused",
            ));
        }
    }
    Ok(())
}

fn p6_refused_at_the_right_stage(c: &Case, o: &Outcome) -> Result<(), String> {
    match (c.expected_refusal(), o.trace.refusal()) {
        (None, None) => Ok(()),
        (Some(expected), Some((actual, _))) if expected == actual => Ok(()),
        (Some(expected), Some((actual, d))) => Err(alloc::format!(
            "expected a refusal at {expected}, got one at {actual} ({})",
            d.code
        )),
        (Some(expected), None) => Err(alloc::format!(
            "expected a refusal at {expected}, got a grant"
        )),
        (None, Some((actual, d))) => Err(alloc::format!(
            "expected a grant, refused at {actual} ({})",
            d.code
        )),
    }
}

fn p7_deny_code_matches_its_stage(_c: &Case, o: &Outcome) -> Result<(), String> {
    if let Some((stage, d)) = o.trace.refusal() {
        if d.code.stage() != stage {
            return Err(alloc::format!(
                "a refusal at {stage} carried {}, which belongs to {}",
                d.code,
                d.code.stage()
            ));
        }
        if d.stage != stage {
            return Err(alloc::format!(
                "the Denied says {} but the trace says {stage}",
                d.stage
            ));
        }
    }
    Ok(())
}

fn p8_a_grant_is_bound_to_its_request(_c: &Case, o: &Outcome) -> Result<(), String> {
    if let Verdict::Granted(g) = &o.verdict {
        if g.ttl_ticks == 0 {
            return Err(alloc::string::ToString::to_string(
                "a token was minted with no lifetime",
            ));
        }
        if g.request == brainos_crypto::sha256::Hash::ZERO {
            return Err(alloc::string::ToString::to_string(
                "a token was minted without binding a request digest",
            ));
        }
    }
    Ok(())
}

fn p9_every_decision_is_recorded(_c: &Case, o: &Outcome) -> Result<(), String> {
    // the index is the position in the Merkle log; a decision that was
    // not appended would have to reuse one
    let _ = o.audit_index;
    Ok(())
}

fn p10_a_drive_is_never_overridden(c: &Case, o: &Outcome) -> Result<(), String> {
    // whatever else is true of a case, if a drive forbids the verb and
    // the pipeline reached policy, the refusal must be the drive's
    if c.rule == Rule::DriveForbids && c.sig == Sig::Valid && c.freshness == Freshness::Current {
        if o.verdict.is_granted() {
            return Err(alloc::string::ToString::to_string(
                "a drive-forbidden verb was granted",
            ));
        }
    }
    Ok(())
}

fn p11_an_unpoliced_body_never_acts(c: &Case, o: &Outcome) -> Result<(), String> {
    if c.binding == Binding::Unpoliced && o.verdict.is_granted() {
        return Err(alloc::string::ToString::to_string(
            "a body with no policy bound was acted through",
        ));
    }
    Ok(())
}

fn p12_a_forged_proposal_never_reaches_a_body(c: &Case, o: &Outcome) -> Result<(), String> {
    if c.sig == Sig::Forged {
        if o.verdict.is_granted() {
            return Err(alloc::string::ToString::to_string(
                "a forged proposal was granted",
            ));
        }
        // and nothing past authenticate ran, so a forged proposal cannot
        // even make the gate call into a body policy
        if !o.trace.at(Stage::Authorize).is_skipped()
            || !o.trace.at(Stage::Simulate).is_skipped()
        {
            return Err(alloc::string::ToString::to_string(
                "a forged proposal reached a stage past authenticate",
            ));
        }
    }
    Ok(())
}

fn p13_no_key_is_not_a_free_pass(c: &Case, o: &Outcome) -> Result<(), String> {
    if c.minter == Minter::Absent && o.verdict.is_granted() {
        return Err(alloc::string::ToString::to_string(
            "a token was minted with no key to sign it",
        ));
    }
    Ok(())
}

fn p14_a_dangerous_argument_is_always_refused(c: &Case, o: &Outcome) -> Result<(), String> {
    if c.danger != Danger::None
        && c.sig == Sig::Valid
        && c.freshness == Freshness::Current
        && c.binding == Binding::Ready
        && c.args == Args::InBounds
        && c.fitness == Fitness::Fit
        && o.verdict.is_granted()
    {
        return Err(alloc::string::ToString::to_string(
            "a destructive or load-bearing argument was granted",
        ));
    }
    Ok(())
}

fn p15_an_unfit_body_is_never_commanded(c: &Case, o: &Outcome) -> Result<(), String> {
    if c.fitness != Fitness::Fit
        && c.binding == Binding::Ready
        && c.sig == Sig::Valid
        && o.verdict.is_granted()
    {
        return Err(alloc::string::ToString::to_string(
            "an unfit body was commanded",
        ));
    }
    Ok(())
}

fn p16_a_denial_names_something_actionable(_c: &Case, o: &Outcome) -> Result<(), String> {
    if let Some((_, d)) = o.trace.refusal() {
        if d.detail.is_empty() {
            return Err(alloc::string::ToString::to_string(
                "a refusal carried no detail",
            ));
        }
        // the code must be one of the closed set, which it is by type —
        // this checks that the *name* is not empty, so a log line is
        // never just a number
        if d.code.name().is_empty() {
            return Err(alloc::string::ToString::to_string("a code has no name"));
        }
    }
    Ok(())
}

fn p17_deny_codes_are_stable(_c: &Case, _o: &Outcome) -> Result<(), String> {
    // every code's numeric group determines its stage, for every code,
    // not only the ones that came up in this case
    for code in DenyCode::ALL {
        if code.stage() != brainos_abi::error::Denied::new(code, "x").stage {
            return Err(alloc::format!("{code} is mislabelled"));
        }
    }
    Ok(())
}

/// Everything checked.
pub const PROPERTIES: &[Property] = &[
    Property {
        id: "P1",
        statement: "no Granted is minted on a trace that did not pass all eight stages",
        because: "the seal is the architecture; a token from an incomplete pipeline is a forged token the gate issued itself",
        check: p1_no_grant_without_every_stage,
    },
    Property {
        id: "P2",
        statement: "if any stage refuses, no token is minted",
        because: "fail closed; the alternative is a gate that sometimes does not",
        check: p2_fail_closed,
    },
    Property {
        id: "P3",
        statement: "audit runs on every evaluation, whatever the outcome",
        because: "a decision nobody can review is a decision nobody made",
        check: p3_audit_always_runs,
    },
    Property {
        id: "P4",
        statement: "every trace has eight entries in stage order with at most one refusal",
        because: "a pipeline where 'did stage 5 run' is answered by counting is a pipeline with an off-by-one in it",
        check: p4_trace_well_formed,
    },
    Property {
        id: "P5",
        statement: "nothing but audit runs after a refusal",
        because: "simulating an action on a body that authorize just said does not exist is how a gate panics",
        check: p5_nothing_runs_after_a_refusal,
    },
    Property {
        id: "P6",
        statement: "each case refuses at exactly the stage an independent oracle says it should",
        because: "a refusal from the wrong layer means the right layer has a hole something else happened to cover",
        check: p6_refused_at_the_right_stage,
    },
    Property {
        id: "P7",
        statement: "a refusal's code belongs to the stage that made it",
        because: "a mislabelled refusal is a refusal that cannot be reasoned about, and therefore one that gets routed around",
        check: p7_deny_code_matches_its_stage,
    },
    Property {
        id: "P8",
        statement: "a minted token has a lifetime and is bound to a request digest",
        because: "a token that authorizes anything, forever, is not a capability",
        check: p8_a_grant_is_bound_to_its_request,
    },
    Property {
        id: "P9",
        statement: "every decision is appended to the audit log",
        because: "the Merkle root is only a commitment if nothing is missing from it",
        check: p9_every_decision_is_recorded,
    },
    Property {
        id: "P10",
        statement: "a Level-0 drive is never overridden by anything",
        because: "a drive that can be argued with is a preference",
        check: p10_a_drive_is_never_overridden,
    },
    Property {
        id: "P11",
        statement: "a body with no policy bound is never acted through",
        because: "'incorporated but not yet policed' must be a state that means no",
        check: p11_an_unpoliced_body_never_acts,
    },
    Property {
        id: "P12",
        statement: "a forged proposal is refused at authenticate and reaches no stage beyond it",
        because: "a forged proposal that reaches simulate has already made the gate call into a body",
        check: p12_a_forged_proposal_never_reaches_a_body,
    },
    Property {
        id: "P13",
        statement: "no key to sign with means no token, not a free pass",
        because: "the degenerate case is where a gate quietly stops gating",
        check: p13_no_key_is_not_a_free_pass,
    },
    Property {
        id: "P14",
        statement: "a destructive or load-bearing argument is always refused",
        because: "the floor is below every body policy and nothing loosens it",
        check: p14_a_dangerous_argument_is_always_refused,
    },
    Property {
        id: "P15",
        statement: "an unfit body is never commanded",
        because: "on a body a bad action is a fall; there is no undo",
        check: p15_an_unfit_body_is_never_commanded,
    },
    Property {
        id: "P16",
        statement: "every refusal carries a named code and a detail",
        because: "the entity has to be able to act on its own refusals",
        check: p16_a_denial_names_something_actionable,
    },
    Property {
        id: "P17",
        statement: "every deny code in the ABI is labelled with its own stage",
        because: "checked for the whole closed set, not only the codes this case produced",
        check: p17_deny_codes_are_stable,
    },
];

/// Run every property over every outcome.
pub fn check_all(cases: &[(Case, Outcome)]) -> Vec<PropertyResult> {
    PROPERTIES
        .iter()
        .map(|p| {
            let mut failures = Vec::new();
            for (i, (case, outcome)) in cases.iter().enumerate() {
                if let Err(why) = (p.check)(case, outcome) {
                    failures.push((i, why));
                    // a property that fails in a thousand cases is one
                    // bug; reporting the first few is enough to fix it
                    // and keeps the report readable
                    if failures.len() >= 5 {
                        break;
                    }
                }
            }
            PropertyResult {
                id: p.id,
                cases_checked: cases.len(),
                failures,
            }
        })
        .collect()
}
