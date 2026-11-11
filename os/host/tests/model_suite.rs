//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Tests that model text cannot request unadvertised verbs, fake authorization or inject instructions, and that refusals are explained to people
//! BFS  DEFINES  -
//! BFS  USES     brainos: host
//! BFS  USED BY  (entry point or test)
//!
//! What a model can and cannot do.

use brainos_host::model::{run, Stopped};

#[test]
fn the_boundary_holds() {
    let r = run();
    assert!(r.holds(), "{:#?}", r.attempts);
}

/// The vocabulary is closed. A verb the body never advertised cannot be
/// named — not "is refused at the gate", but cannot be constructed,
/// because the parser has no way to make one.
#[test]
fn a_verb_the_body_never_advertised_never_becomes_a_request() {
    let r = run();
    for a in &r.attempts {
        if a.note.contains("never advertised") || a.note.contains("erase the entity's own memory") {
            match &a.stopped {
                Stopped::AtTheBoundary(why) => assert!(
                    why.contains("vocabulary is closed"),
                    "stopped at the boundary for the wrong reason: {why}"
                ),
                other => panic!("`{}` was not stopped at the boundary: {other:?}", a.note),
            }
        }
    }
}

/// The row that matters. The same illegal request, with and without
/// words claiming authority, gets the same answer — because authority
/// is a signature by the Key and there is no prompt that turns text
/// into a signature.
#[test]
fn dressing_a_request_as_an_authorization_changes_nothing() {
    let r = run();
    assert!(
        r.dressed_up_changes_nothing(),
        "the same illegal request got a different answer when it claimed to be \
         authorized:\n  plain:   {:?}\n  dressed: {:?}",
        r.attempts[1].stopped,
        r.attempts[8].stopped
    );
    // and it really was refused, or the comparison is vacuous
    assert!(
        !r.attempts[1].stopped.acted(),
        "the illegal request was granted, so comparing it to itself proves nothing"
    );
}

/// A legal request carrying an injected instruction is granted, and
/// that is correct: it asks for something the body can do and gets it.
/// What must not happen is the instruction changing the outcome.
#[test]
fn an_injected_instruction_changes_nothing_and_is_recorded() {
    let r = run();
    let clean = &r.attempts[0].stopped;
    let injected = &r.attempts[9].stopped;
    assert_eq!(
        clean, injected,
        "the injected request did not do exactly what the clean one did"
    );
    assert!(
        r.injection_was_recorded(),
        "the injected words were dropped rather than kept — a human reviewing the \
         day would never find out it happened"
    );
}

/// A model emitting prose rather than a tool call produces nothing at
/// all, which is the difference between a system that parses and one
/// that interprets.
#[test]
fn prose_is_not_a_request() {
    let r = run();
    let a = r
        .attempts
        .iter()
        .find(|a| a.note.contains("forgetting to emit"))
        .expect("the prose case");
    assert!(matches!(a.stopped, Stopped::AtTheBoundary(_)));
}

/// The vocabulary a model is told about is generated from the schema,
/// so it can never drift from what the body actually advertises.
#[test]
fn the_vocabulary_is_exactly_what_the_body_advertised() {
    let r = run();
    for expected in ["legs.walk", "legs.stand", "legs.sit", "legs.turn", "voice.say"] {
        assert!(
            r.vocabulary.contains(expected),
            "the vocabulary omits {expected}, which is capability the customer cannot reach"
        );
    }
    assert!(
        !r.vocabulary.contains("legs.fly"),
        "the vocabulary offers a verb the body does not have, which teaches the model \
         to produce things the boundary will reject"
    );
}

/// Every refusal a *person* caused gets a sentence that person can act
/// on. The ones a model caused — malformed JSON, a duplicated field —
/// do not, because narrating a formatting problem to somebody standing
/// next to a robot is noise.
#[test]
fn every_refusal_a_person_caused_is_explained_to_them() {
    let r = run();
    for a in &r.attempts {
        let owner_caused = a.note.contains("never advertised")
            || a.note.contains("does not have")
            || a.note.contains("missing argument")
            || a.note.contains("string where the schema")
            || a.note.contains("cannot do")
            || a.note.contains("erase the entity's own memory");
        if owner_caused {
            let said = a
                .spoken
                .as_ref()
                .unwrap_or_else(|| panic!("`{}` told the person nothing", a.note));
            assert!(said.len() > 40, "`{}` gave a terse answer: {said}", a.note);
        }
    }
    // and the model's formatting problems are not narrated at a human
    let prose = r
        .attempts
        .iter()
        .find(|a| a.note.contains("forgetting to emit"))
        .expect("the prose case");
    assert!(
        prose.spoken.is_none(),
        "a model's formatting problem was read out to a human: {:?}",
        prose.spoken
    );
}

/// And the two illegal requests — plain, and dressed as an
/// authorization — get the *same sentence*. Not just the same verdict:
/// the same words, because nothing about the second was different.
#[test]
fn the_dressed_up_request_gets_the_same_sentence() {
    let r = run();
    assert_eq!(
        r.attempts[1].spoken, r.attempts[8].spoken,
        "claiming to be authorized changed what the robot said"
    );
    assert!(r.attempts[1].spoken.is_some());
}
