//! Where this breaks — asserted, so that a regression in any of these
//! is a failing test rather than a slow surprise.

use brainos_host::limits::{measure, Kind};

#[test]
fn the_entity_can_still_find_what_it_knows_at_the_bound_it_ships_with() {
    let l = measure();
    assert!(
        l.retrieval_holds_at_default_bounds(),
        "retrieval fails at the default bound: {:?}",
        l.retrieval
    );
}

/// The planted memory comes back first, not merely somewhere, and it
/// keeps doing so well past the bound. Distractors are deliberately
/// similar — a haystack of unrelated text would flatter BM25.
#[test]
fn retrieval_degrades_gracefully_rather_than_falling_off_a_cliff() {
    let l = measure();
    for p in &l.retrieval {
        assert_eq!(
            p.rank_of_the_needle, 1,
            "at {} memories the answer came back at rank {} — 0 means it was not in \
             the top ten at all",
            p.memories, p.rank_of_the_needle
        );
    }
    // and a query stays fast enough to be in an interactive loop
    let worst = l.retrieval.iter().map(|p| p.query_us).max().unwrap_or(0);
    assert!(worst < 1_000, "a query took {worst} µs");
}

/// The index is the embedded constraint, not the memories. This is
/// asserted rather than remarked on, because the earlier footprint
/// number counted the memories and not the index, and that
/// understatement is exactly the kind that survives into a datasheet.
#[test]
fn the_index_costs_more_than_the_memories_it_indexes() {
    let l = measure();
    let at_default = l
        .retrieval
        .iter()
        .find(|p| p.memories > 4_000)
        .expect("a point at the default bound");
    assert!(
        at_default.index_bytes > 500_000,
        "the index at the default bound is {} bytes, which is smaller than measured \
         before — if this has genuinely improved, the limits text needs updating too",
        at_default.index_bytes
    );
}

/// The root is what every checkpoint needs, and checkpoints are what
/// make the audit log evidence. It must not grow with the history.
#[test]
fn computing_the_root_does_not_grow_with_the_history() {
    let l = measure();
    let small = l.audit.first().expect("a small log");
    let large = l.audit.last().expect("a large log");
    assert!(
        large.decisions >= small.decisions * 100,
        "the audit points do not span enough scale to say anything"
    );
    assert!(
        large.root_us <= small.root_us.max(50) * 4,
        "a root over {} decisions took {} µs against {} µs over {} — that is growth, \
         and a checkpoint that gets more expensive as an entity ages means taking \
         fewer of them",
        large.decisions,
        large.root_us,
        small.root_us,
        small.decisions
    );
}

/// A proof path is logarithmic even though producing it is not.
#[test]
fn a_proof_stays_short_however_long_the_log_is() {
    let l = measure();
    for p in &l.audit {
        assert!(
            p.proof_path <= 24,
            "a proof over {} decisions is {} hashes long",
            p.decisions,
            p.proof_path
        );
    }
}

/// Every limit says what kind it is and where it is set, because "hard",
/// "bound" and "soft" call for completely different reactions and a
/// limit without that label is not actionable.
#[test]
fn every_limit_is_labelled_and_located() {
    let l = measure();
    assert!(l.limits.len() >= 8, "only {} limits recorded", l.limits.len());
    for lim in &l.limits {
        assert!(!lim.value.is_empty(), "{} has no value", lim.what);
        assert!(!lim.consequence.is_empty(), "{} has no consequence", lim.what);
        assert!(!lim.set_in.is_empty(), "{} does not say where it is set", lim.what);
    }
    // and the soft ones exist, because those are the point of the exercise
    assert!(
        l.limits.iter().any(|x| x.kind == Kind::Soft),
        "no soft limits recorded, which would mean nothing degrades silently — \
         unlikely, and worth disbelieving"
    );
}
