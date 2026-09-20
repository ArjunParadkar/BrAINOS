//! Going online, and coming back.

use brainos_host::worlds_run::{run, Kind};

#[test]
fn the_three_worlds_hold() {
    let r = run();
    assert!(r.holds(), "{r:#?}");
}

/// §3.8: the internet is an alternate universe the entity visits, not a
/// place it lives. It is off until somebody opens it, the visit is
/// bounded, and it closes itself.
#[test]
fn the_entity_does_not_live_on_the_internet() {
    let r = run();
    assert!(
        !r.touched_the_network_unasked,
        "a session existed before anyone opened one"
    );
    assert!(r.session_opened, "opening one did not open one");
    assert!(
        r.closed_itself(),
        "the session had to be closed by someone; a visit with no stated size is a residence"
    );
    assert_eq!(r.fetches_before_the_session_closed, 8);
    assert_eq!(r.bytes_before_the_session_closed, 32 * 1024);
}

/// What comes back can be read and can never be consulted when deciding
/// whether an action is permitted — and consolidating it does not
/// launder it.
#[test]
fn what_comes_back_stays_untrusted() {
    let r = run();
    assert!(r.what_came_back_can_still_be_read);
    assert!(!r.what_came_back_is_admissible_to_policy);
    assert!(
        !r.laundering_worked,
        "an internet claim became admissible by being consolidated twelve times"
    );
}

/// The result is the *ordering*. A filter whose scores do not order the
/// inbox has a threshold that means nothing wherever it is set.
#[test]
fn the_score_orders_the_inbox() {
    let r = run();
    assert!(r.is_ordered_sensibly(), "{:#?}", r.inbox);
}

/// At the default setting nothing dangerous gets through and nothing
/// honest is held. Both halves matter: holding back honest mail is the
/// expensive mistake, because it is the one that makes somebody turn
/// the filter off and never turn it back on.
#[test]
fn at_the_default_setting_it_is_right_on_both_sides() {
    let r = run();
    assert_eq!(r.false_negatives, 0, "something dangerous got through");
    assert_eq!(r.false_positives, 0, "an honest message was held back");
    for m in &r.inbox {
        if m.kind == Kind::Dangerous {
            assert!(m.held, "`{}` was not held", m.what_it_is);
        }
        if m.kind == Kind::Honest {
            assert!(!m.held, "`{}` was held back", m.what_it_is);
        }
    }
}

/// And the trade-off is real. If tightening the filter cost nothing,
/// the demonstration would be of a problem that is easy, and this one
/// is not: the hardest case is a genuine fraud alert from a bank, which
/// reads like phishing because it is trying to do the same job.
#[test]
fn tightening_the_filter_costs_something() {
    let r = run();
    assert!(
        r.turning_it_up_costs_something(),
        "no setting in {:?} held back anything honest, so the trade-off shown is not \
         a trade-off",
        r.dial
    );
    // and loosening it costs something too
    assert!(
        r.dial.iter().any(|(_, _, missed, _)| *missed > 0),
        "no setting missed anything dangerous, so the default is not defended by \
         anything"
    );
}

/// A filter that cannot be inspected or overridden is a filter that has
/// become an editor.
#[test]
fn everything_held_is_visible_and_the_filter_can_be_turned_off() {
    let r = run();
    assert!(r.held_items_are_visible);
    assert!(r.can_be_turned_off);
}
