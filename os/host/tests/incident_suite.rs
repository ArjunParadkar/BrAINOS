//! Accountability, asserted.

use brainos_host::incident::reconstruct;

#[test]
fn an_incident_can_be_reconstructed_and_proven() {
    let i = reconstruct();
    assert!(i.is_accountable(), "the reconstruction did not stand up: {i:#?}");
}

/// The bundle verifies for a party holding nothing but the bundle and
/// thirty-two bytes of public key. That is the whole claim; everything
/// else here is about what happens when someone lies with it.
#[test]
fn a_third_party_can_check_it_without_the_log_or_the_entity() {
    let i = reconstruct();
    assert!(
        i.third_party_verdict.is_ok(),
        "a third party could not verify the bundle: {:?}",
        i.third_party_verdict
    );
    assert!(i.in_window > 10, "only {} decisions in the window", i.in_window);
}

/// The finding people do not expect: what exonerates a machine is not a
/// record of it behaving well. It is a record of the gate having
/// refused, at a named stage, the specific thing it is accused of —
/// written down at the time, before anyone was accused of anything.
#[test]
fn the_refusals_are_the_evidence() {
    let i = reconstruct();
    assert!(
        i.window_refused > 0,
        "the window contains no refusals, so there is nothing that exonerates"
    );
    let answer = i.the_answer.as_ref().expect("no answer was reconstructed");
    assert!(answer.contains("refus"), "the answer does not turn on refusals: {answer}");
    // and the shift as a whole is mostly ordinary work, so the refusals
    // are signal rather than the norm
    assert!(
        i.shift_granted > i.shift_refused * 3,
        "the shift was mostly refusals ({} granted, {} refused), which makes the \
         refusals uninformative",
        i.shift_granted,
        i.shift_refused
    );
}

/// Three of the four ways of lying fail outright.
#[test]
fn a_bundle_cannot_be_forged() {
    let i = reconstruct();
    for (n, (what, how)) in i.forgeries.iter().enumerate() {
        if n == 2 {
            continue; // curation: see the next test
        }
        assert!(
            how.starts_with("refused"),
            "\"{what}\" was not refused: {how}"
        );
    }
}

/// The fourth is the honest limit, and it is the one worth stating
/// plainly: a bundle cannot be forged but it *can* be curated, and
/// nothing in the cryptography stops its producer choosing what to
/// include. What the design can do is make the holes visible — the log
/// is append-only and its indices are consecutive.
#[test]
fn a_bundle_can_be_curated_and_the_curation_is_visible() {
    let i = reconstruct();
    assert!(
        i.honest_bundle_is_contiguous,
        "the honest bundle already has holes in it, so a hole proves nothing"
    );
    assert!(
        i.curated_bundle_gaps > 0,
        "dropping every refusal left no visible gap, which means selective omission \
         is undetectable and the claim in the bundle's own text is false"
    );
    let (_, how) = &i.forgeries[2];
    assert!(
        how.contains("visible holes"),
        "the curated bundle's description does not mention the holes: {how}"
    );
}

/// And the bundle says what it is not. An audit log is a record of
/// decisions, not of physics, and a document that did not say so would
/// be read as an account of events.
#[test]
fn the_bundle_states_its_own_limits() {
    let i = reconstruct();
    let t = &i.bundle_text;
    assert!(t.contains("record of decisions"));
    assert!(t.contains("not"), "the bundle does not say what it is not");
    assert!(
        t.contains("floor was wet") || t.contains("physics") || t.contains("record of events"),
        "the bundle does not warn against reading it as a record of events"
    );
}
