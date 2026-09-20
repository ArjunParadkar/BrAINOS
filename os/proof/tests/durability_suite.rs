//! Losing power at every possible instant.

use brainos_proof::durability::{verify_bit_rot, verify_durability, Medium};

#[test]
fn there_is_no_instant_at_which_power_loss_corrupts_an_entity() {
    let r = verify_durability(4);
    assert!(
        r.violations.is_empty(),
        "{} of {} power-loss instants left the entity in a state it was never in:\n{}",
        r.violations.len(),
        r.instants,
        r.violations
            .iter()
            .take(8)
            .map(|v| format!(
                "  {} · generation {} · torn at byte {}: {}\n",
                v.medium.name(),
                v.generation,
                v.torn_at,
                v.why
            ))
            .collect::<String>()
    );
}

/// The weaker property that actually matters: an entity may lose the
/// last thing it was doing, and may not come back as an earlier version
/// of itself or as a mixture of two.
#[test]
fn nothing_is_ever_lost_completely() {
    let r = verify_durability(4);
    assert_eq!(
        r.lost_everything, 0,
        "{} instants left the entity with nothing, although a complete earlier \
         record was in the other slot",
        r.lost_everything
    );
}

/// A domain where every tear happened to leave the newest write intact
/// would be checking nothing. Both outcomes have to occur.
#[test]
fn the_domain_exercises_both_outcomes() {
    let r = verify_durability(4);
    assert!(
        r.is_meaningful(),
        "kept the newest write {} times and fell back {} times — the tears are \
         not where the data is",
        r.kept_newest,
        r.fell_back
    );
    assert!(r.instants > 300, "only {} instants enumerated", r.instants);
}

/// Three media, because a torn write does not look the same on every
/// device, and the nastiest is the one where the unwritten tail still
/// holds valid-looking data from the previous record.
#[test]
fn every_medium_model_is_covered() {
    assert_eq!(Medium::ALL.len(), 3);
    let r = verify_durability(4);
    // instants are enumerated per medium, so the count divides
    assert_eq!(r.instants % 3, 0, "the media were not covered evenly");
}

/// And the smaller question: a single flipped bit in a complete record.
#[test]
fn a_flipped_bit_in_a_complete_record_is_caught() {
    let (caught, slipped) = verify_bit_rot(400);
    assert!(caught > 100, "only {caught} flips were checked");
    assert_eq!(
        slipped, 0,
        "{slipped} single-bit flips produced a record that decoded, verified, and \
         re-encoded to the damaged bytes"
    );
}

/// Every depth, so that all of `plan_write`'s branches are exercised:
/// neither slot valid, one valid, both valid with either one newer.
#[test]
fn power_loss_is_survivable_at_every_depth_of_history() {
    for generations in 0..=6 {
        let r = verify_durability(generations);
        assert!(
            r.violations.is_empty(),
            "at {generations} prior generations, {} instants left the entity in a \
             state it was never in: {:?}",
            r.violations.len(),
            r.violations.first()
        );
        if generations > 0 {
            assert_eq!(
                r.lost_everything, 0,
                "at {generations} prior generations, {} instants lost everything",
                r.lost_everything
            );
        }
    }
}
