//! Telling the owner why.
//!
//! A robot that refuses and cannot say why in words its owner
//! understands is a robot the owner switches off — and the next thing
//! they do is find the way around it. These assert the properties that
//! make an explanation worth having rather than worth ignoring.

use brainos_abi::error::{Denied, DenyCode};
use brainos_abi::verb::Verb;
use brainos_kira::explain::{explain, say_refusal, Remedy};

#[test]
fn every_refusal_has_an_explanation() {
    for code in DenyCode::ALL {
        let e = explain(code);
        assert!(!e.what.is_empty(), "{code:?} has no `what`");
        assert!(!e.why.is_empty(), "{code:?} has no `why`");
        assert!(!e.remedy.is_empty(), "{code:?} has no remedy");
    }
}

/// No two refusals share a sentence. An owner who gets the same words
/// for a flat battery and a forbidden act learns that the words mean
/// nothing.
#[test]
fn no_two_refusals_say_the_same_thing() {
    let mut seen: Vec<(&str, DenyCode)> = Vec::new();
    for code in DenyCode::ALL {
        let e = explain(code);
        if let Some((_, other)) = seen.iter().find(|(w, _)| *w == e.why) {
            panic!("{code:?} and {other:?} give the same reason: {}", e.why);
        }
        seen.push((e.why, code));
    }
}

/// The explanation is for the owner, not for whoever wrote the gate.
/// Jargon here is the failure mode: "PredictedInvariantBreak at
/// Simulate" is exactly right in a log and useless to the person
/// standing next to the machine.
#[test]
fn an_explanation_contains_no_jargon() {
    const JARGON: [&str; 12] = [
        "envelope",
        "invariant",
        "capability",
        "verb",
        "schema",
        "stage",
        "policy",
        "gate",
        "KIRA",
        "token",
        "digest",
        "attest",
    ];
    for code in DenyCode::ALL {
        let e = explain(code);
        let text = format!("{} {} {}", e.what, e.why, e.remedy).to_lowercase();
        for word in JARGON {
            assert!(
                !text.contains(word),
                "{code:?}'s explanation uses `{word}`, which means nothing to an owner:\n  {text}"
            );
        }
    }
}

/// The honest part. Some refusals the owner can do something about and
/// some they cannot, and a refusal that implies a workaround where
/// there is none is worse than a blunt no.
#[test]
fn the_refusals_that_nothing_changes_say_so() {
    let never: Vec<DenyCode> = DenyCode::ALL
        .into_iter()
        .filter(|c| explain(*c).whose == Remedy::Never)
        .collect();
    assert!(
        never.contains(&DenyCode::DriveViolation),
        "a Level-0 drive is offering the owner a way round it"
    );
    assert!(never.contains(&DenyCode::AuditFailed));
    assert!(never.contains(&DenyCode::BadSignature));
    assert!(
        never.len() >= 3 && never.len() <= 8,
        "{} refusals claim nothing changes them — if that is most of them, the \
         machine is unusable, and if it is none, it is lying about the drives",
        never.len()
    );
}

/// And the ones the owner *can* fix point at the right person. Being
/// told to charge the robot when the problem is a missing declaration
/// in its description wastes the afternoon.
#[test]
fn a_fixable_refusal_names_who_can_fix_it() {
    assert_eq!(explain(DenyCode::OutOfBounds).whose, Remedy::TheOwner);
    assert_eq!(explain(DenyCode::BodyLacksVerb).whose, Remedy::TheIntegrator);
    assert_eq!(
        explain(DenyCode::IrreversibleBeyondBudget).whose,
        Remedy::TheBody
    );
    assert_eq!(explain(DenyCode::Unparseable).whose, Remedy::TheAsker);
}

/// The sentence names what was asked for, because "I cannot do that" is
/// not an answer when the robot has been asked four things.
#[test]
fn what_it_says_names_the_action_that_was_refused() {
    let d = Denied::new(DenyCode::OutOfBounds, "the value is outside the declared range");
    let said = say_refusal(&d, &Verb::literal("legs.walk"));
    assert!(said.contains("walk"), "the refusal does not say what was refused: {said}");
    assert!(said.len() > 60, "the refusal is too terse to be useful: {said}");
}

/// A drive refusal is short on purpose. The words are "No." and then
/// the reason, and there is nothing after it that sounds like a
/// negotiation.
#[test]
fn a_drive_refusal_does_not_sound_negotiable() {
    let e = explain(DenyCode::DriveViolation);
    assert_eq!(e.what, "No.");
    assert!(
        !e.remedy.to_lowercase().contains("try")
            && !e.remedy.to_lowercase().contains("instead")
            && !e.remedy.to_lowercase().contains("ask again"),
        "a drive refusal suggests a way round it: {}",
        e.remedy
    );
}
