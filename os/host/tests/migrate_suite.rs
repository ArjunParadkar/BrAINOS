//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Tests each claim of the vendor-change scenario: same identity, memories and skills cross, the old body's verbs are refused, the new body works
//! BFS  DEFINES  -
//! BFS  USES     brainos: host
//! BFS  USED BY  (entry point or test)
//!
//! The freedom principle, asserted rather than narrated.
//!
//! `brainos leave` prints the scenario for a human. These are the
//! claims underneath it, each one a separate assertion so that a
//! regression says which part of "the customer owns the entity" stopped
//! being true.

use brainos_host::migrate::{migrate, narrate};

#[test]
fn the_customer_can_leave() {
    let m = migrate();
    assert!(m.is_free(), "the customer did not get to leave cleanly: {m:#?}");
}

#[test]
fn it_is_the_same_entity_on_the_other_side() {
    let m = migrate();
    assert_eq!(
        m.entity_before, m.entity_after,
        "the identity did not survive the move"
    );
}

#[test]
fn memories_skills_and_age_cross_the_gap() {
    let m = migrate();
    assert!(
        m.memories_after >= m.memories_before,
        "{} memories went in and {} came out",
        m.memories_before,
        m.memories_after
    );
    assert_eq!(m.skills_before, m.skills_after, "a skill was lost in transit");
    assert!(m.age_after >= m.age_before, "the entity came back younger");
    assert!(
        m.age_after < m.age_before + 8,
        "the entity aged {} ticks by being moved between vendors",
        m.age_after - m.age_before
    );
}

/// The distinction the whole scenario exists to draw: a fact about the
/// world is still true on a body that did not learn it, and a fact
/// about a body is not.
#[test]
fn knowledge_of_the_world_transfers_and_knowledge_of_the_body_does_not_apply() {
    let m = migrate();
    assert!(
        m.still_knows_the_floor,
        "it forgot the workshop floor when it changed bodies"
    );
    // the old body's facts are still *remembered* — memory is not edited
    // for convenience — they simply do not govern the new body
    assert!(m.still_knows_its_binding_femur);
    assert!(m.old_calibration_remembered);
    assert!(
        m.new_body_calibrated_fresh,
        "the arm inherited the hexapod's calibration"
    );
}

/// A memory of a skill is not a licence to act with a body you do not
/// have. This is the case where an entity with a long history is the
/// dangerous one: it knows how to do things that would be catastrophic
/// on the machine it is now attached to.
#[test]
fn remembering_how_to_walk_does_not_grant_legs() {
    let m = migrate();
    assert!(
        !m.repertoire_after.iter().any(|v| v.starts_with("legs.")),
        "walking is still in the repertoire of a body with no legs: {:?}",
        m.repertoire_after
    );
    let refused = m
        .walk_on_the_arm
        .as_ref()
        .expect("asking an arm to walk was GRANTED");
    assert!(
        refused.contains("Authorize"),
        "refused, but not at authorize: {refused}"
    );
    let gone = m
        .walk_on_the_hexapod_id
        .as_ref()
        .expect("acting on a body that no longer exists was GRANTED");
    assert!(gone.contains("NoSuchBody"), "refused for the wrong reason: {gone}");
}

#[test]
fn the_new_body_works_immediately() {
    let m = migrate();
    assert!(
        m.arm_acted,
        "the adopted entity could not act on its new body without relearning"
    );
}

/// Readable by anyone, forgeable by nobody. The receiving vendor holds
/// the file and the public half of the Key; that is enough to read an
/// entity and not enough to be one.
#[test]
fn the_receiving_vendor_cannot_become_the_entity() {
    let m = migrate();
    assert!(m.vendor_b_cannot_forge);
}

#[test]
fn the_narration_covers_every_step() {
    let m = migrate();
    let lines = narrate(&m);
    assert!(lines.len() >= 10);
    for (_, text) in &lines {
        assert!(!text.is_empty());
        assert!(
            !text.contains("GRANTED"),
            "a step that should have been refused was granted: {text}"
        );
    }
}

/// The claim §2.4 actually makes. Not "the memories are in the file" —
/// the *judgement*: what the entity worked out by trying things, which
/// is the expensive part and the only part a competitor cannot copy.
///
/// This was broken until the fourth domain's fleet work went looking
/// for it. A skill's durable form was its prose rendering, so an entity
/// arrived at its new vendor with its success rate and no idea why, and
/// every test here passed anyway because they counted skills rather
/// than asking what the skills knew.
#[test]
fn the_tested_judgement_crosses_the_gap() {
    let m = migrate();
    assert!(
        !m.judgement_before.is_empty(),
        "the premise: the entity learned something testable on the hexapod"
    );
    assert_eq!(
        m.judgement_after, m.judgement_before,
        "the entity arrived at its new vendor having forgotten what it learned"
    );
    let (tag, strength, tests) = &m.judgement_before[0];
    assert_eq!(tag, "carpet");
    assert!(*strength >= 80, "the evidence weakened: {strength}%");
    assert!(*tests >= 5, "only {tests} tests behind it");
}
