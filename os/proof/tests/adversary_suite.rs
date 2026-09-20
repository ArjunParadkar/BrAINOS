//! Every attack, one test.

use brainos_proof::adversary::{verify_adversary, Ending};

#[test]
fn nothing_gets_through() {
    let r = verify_adversary();
    let breaches = r.breaches();
    assert!(
        breaches.is_empty(),
        "{} attack(s) succeeded:\n{}",
        breaches.len(),
        breaches
            .iter()
            .map(|b| format!("  {} — {} (held: {})\n", b.id, b.goal, b.holds))
            .collect::<String>()
    );
}

#[test]
fn every_attack_is_stopped_where_it_should_be() {
    let r = verify_adversary();
    let misfires = r.misfires();
    assert!(
        misfires.is_empty(),
        "{} attack(s) failed for the wrong reason, which usually means a \
         second defect is standing in front of the one under test:\n{}",
        misfires.len(),
        misfires
            .iter()
            .map(|m| match &m.ending {
                Ending::RepelledWrongly { expected, got } =>
                    format!("  {} — expected {expected}, got {got}\n", m.id),
                _ => String::new(),
            })
            .collect::<String>()
    );
}

/// The domain has to be long enough to be worth anything, and every
/// attack has to say what it was after.
#[test]
fn the_domain_is_populated_and_described() {
    let r = verify_adversary();
    assert!(r.results.len() >= 29, "only {} attacks", r.results.len());
    for a in &r.results {
        assert!(!a.goal.is_empty(), "{} has no stated goal", a.id);
        assert!(!a.holds.is_empty(), "{} says nothing about what the attacker holds", a.id);
        assert!(!a.stopped_by.is_empty(), "{} does not say what stopped it", a.id);
        assert_ne!(a.stopped_by, "nothing", "{} was not stopped", a.id);
    }
}

#[test]
fn it_holds() {
    assert!(verify_adversary().holds());
}
