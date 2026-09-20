//! One entity through the whole product, in one history.
//!
//! Every other suite builds a fresh entity and checks one property.
//! This checks the seams, which is where the bugs that reach customers
//! live: what an outage does to a skill learned before it, whether an
//! audit chain survives an export, whether an entity that has changed
//! bodies twice can still be proven to have refused something on the
//! first one.

use brainos_host::life::live;

#[test]
fn a_whole_life_holds_together() {
    let l = live();
    assert!(
        l.violations.is_empty(),
        "{} invariant(s) broken across the life:\n{}",
        l.violations.len(),
        l.violations
            .iter()
            .map(|v| format!("  - {v}\n"))
            .collect::<String>()
    );
    assert!(l.holds(), "the life did not hold: {} moments", l.moments.len());
}

/// The identity is the product. It survives a reboot, a torn journal
/// write, a destroyed vendor, and a change of body.
#[test]
fn the_identity_never_changes() {
    let l = live();
    let first = &l.moments[0].identity;
    for m in &l.moments {
        assert_eq!(
            &m.identity, first,
            "the identity changed at `{}`",
            m.step
        );
    }
}

/// Age is the x-axis of §2.4's curve and must be monotone across every
/// transition — including the ones where the host passes an arbitrary
/// tick, which it does twice here on purpose.
#[test]
fn the_age_never_goes_backwards() {
    let l = live();
    for pair in l.moments.windows(2) {
        assert!(
            pair[1].age >= pair[0].age,
            "age went backwards between `{}` ({}) and `{}` ({})",
            pair[0].step,
            pair[0].age,
            pair[1].step,
            pair[1].age
        );
    }
}

/// The moat: what the entity worked out by trying survives everything
/// that happens to it afterwards.
#[test]
fn what_it_learned_by_trying_survives_the_whole_life() {
    let l = live();
    let learned = l
        .moments
        .iter()
        .find(|m| m.step == "works")
        .expect("the working step");
    assert!(
        !learned.judgement.is_empty(),
        "the premise: it learned something testable on the first body"
    );
    let last = l.moments.last().expect("a final moment");
    assert_eq!(
        last.judgement, learned.judgement,
        "the entity arrived at the end of its life having forgotten what it worked out"
    );
}

/// A decision recorded on the first body — two owners and three
/// processes ago — still proves against the root it was issued against.
/// That is what makes the audit log evidence rather than a diary, and
/// it is why the log belongs to the machine of record rather than
/// travelling with the entity.
#[test]
fn a_proof_issued_early_still_proves_at_the_end() {
    assert!(
        live().early_proof_still_verifies,
        "a proof taken on the first body no longer verifies"
    );
}

/// And the life ends the way the freedom principle says it should: with
/// a file the customer owns.
#[test]
fn it_ends_with_a_file_the_customer_owns() {
    let l = live();
    assert!(l.final_export.len() > 1_000);
    assert_eq!(&l.final_export[..8], b"BRAINEX1");
}
