//! The moat claim, tested.
//!
//! §1.5: "the moat is made of accumulated experience, and today we have
//! none." §2.4: the curve gets published. This suite checks that the
//! mechanism which would produce that curve does what it is supposed to,
//! and — more importantly — that the way it is measured cannot be
//! cheated.

use brainos_host::compound::{experiment, run, Policy, Situation, World};

const EPISODES: usize = 2_000;
const WINDOWS: usize = 8;

// ---------- the measurement, before the result ----------

#[test]
fn the_metric_cannot_be_gamed_by_declining_everything() {
    // The obvious way to cheat a success rate: attempt nothing. An
    // entity that declined every situation would have a perfect success
    // rate and be useless, so the metric counts correct declines and not
    // incorrect ones.
    let coward = run(World::Structured, Policy::Never, EPISODES, WINDOWS, b"never");
    let learner = run(
        World::Structured,
        Policy::Learned,
        EPISODES,
        WINDOWS,
        b"s-l",
    );

    assert_eq!(coward.late().attempted, 0);
    assert_eq!(
        coward.late().success_rate_pct(),
        0,
        "it succeeded at nothing, because it attempted nothing"
    );
    // its judgement is exactly the fraction of situations that would
    // have failed anyway — real, and well short of the learner's
    assert!(coward.late().judgement_pct() > 50, "declining is often right");
    assert!(
        learner.late().judgement_pct() > coward.late().judgement_pct(),
        "an entity that acts when it should must beat one that never acts: {} vs {}",
        learner.late().judgement_pct(),
        coward.late().judgement_pct()
    );
}

#[test]
fn caution_has_a_reported_cost() {
    let learner = run(World::Structured, Policy::Learned, EPISODES, WINDOWS, b"s-l");
    // some of what it declined would have worked; that number is
    // reported rather than hidden inside a success rate
    let late = learner.late();
    assert!(late.declined > 0);
    assert!(
        late.missed_pct() < 40,
        "it declined {}% of things that would have worked",
        late.missed_pct()
    );
}

#[test]
fn a_run_is_reproducible() {
    let a = run(World::Structured, Policy::Learned, 500, 4, b"seed");
    let b = run(World::Structured, Policy::Learned, 500, 4, b"seed");
    assert_eq!(a.windows, b.windows);
    assert_eq!(a.learned, b.learned);
}

#[test]
fn a_different_seed_gives_a_different_run_but_the_same_conclusion() {
    let a = run(World::Structured, Policy::Learned, EPISODES, WINDOWS, b"one");
    let b = run(World::Structured, Policy::Learned, EPISODES, WINDOWS, b"two");
    assert_ne!(a.windows, b.windows, "different seeds, different runs");
    // a result that depended on the seed would not be a result
    assert!(a.late().judgement_pct() > 75, "{}", a.late().judgement_pct());
    assert!(b.late().judgement_pct() > 75, "{}", b.late().judgement_pct());
}

// ---------- the null control ----------

#[test]
fn nothing_is_learned_where_there_is_nothing_to_learn() {
    // The control that makes the whole result mean something. In a world
    // where success is a coin flip, the learner must not beat the
    // control — if it does, the metric is measuring something other than
    // learning.
    let control = run(
        World::Structureless,
        Policy::Always,
        EPISODES,
        WINDOWS,
        b"n-c",
    );
    let learner = run(
        World::Structureless,
        Policy::Learned,
        EPISODES,
        WINDOWS,
        b"n-l",
    );
    let diff = learner.late().judgement_pct() as i16 - control.late().judgement_pct() as i16;
    assert!(
        diff.abs() <= 12,
        "the learner 'improved' by {diff} points in a world with no structure, \
         which would mean the metric is measuring something else"
    );
    // and both sit near the coin flip
    assert!((35..=65).contains(&control.late().judgement_pct()));
}

#[test]
fn a_structureless_world_teaches_no_binding_preconditions_worth_acting_on() {
    let learner = run(
        World::Structureless,
        Policy::Learned,
        EPISODES,
        WINDOWS,
        b"n-l",
    );
    // it may hold weak correlates — intersecting the context of
    // successes always produces something — but it must not end up
    // declining most of the time on the strength of them
    assert!(
        learner.late().declined < learner.late().attempted * 3,
        "it declined {} and attempted {} in a world where nothing predicts anything",
        learner.late().declined,
        learner.late().attempted
    );
}

// ---------- the result ----------

#[test]
fn an_entity_with_experience_makes_better_decisions_than_one_without() {
    let e = experiment(EPISODES, WINDOWS);
    assert!(
        e.effect_pts() >= 10,
        "the learner beat the control by only {} points",
        e.effect_pts()
    );
    assert!(
        e.survives_the_control(),
        "effect {} pts, null {} pts",
        e.effect_pts(),
        e.null_effect_pts()
    );
}

#[test]
fn judgement_improves_over_a_life_rather_than_arriving_at_once() {
    let learner = run(World::Structured, Policy::Learned, EPISODES, WINDOWS, b"s-l");
    let first = learner.windows.first().unwrap().judgement_pct();
    let last = learner.windows.last().unwrap().judgement_pct();
    assert!(
        last > first,
        "judgement went from {first}% to {last}%; this is the curve §2.4 publishes"
    );
    assert!(learner.improvement_pts() > 0);

    // and the preconditions accumulate rather than appearing at once
    let early_learned = learner.windows.first().unwrap().binding_preconditions;
    let late_learned = learner.windows.last().unwrap().binding_preconditions;
    assert!(
        late_learned > early_learned,
        "it held {early_learned} preconditions early and {late_learned} late"
    );
}

#[test]
fn the_control_does_not_improve_because_it_has_nothing_to_improve_with() {
    let control = run(World::Structured, Policy::Always, EPISODES, WINDOWS, b"s-c");
    let first = control.windows.first().unwrap().judgement_pct() as i16;
    let last = control.windows.last().unwrap().judgement_pct() as i16;
    assert!(
        (last - first).abs() <= 12,
        "the control moved from {first}% to {last}%, which it should not"
    );
}

#[test]
fn the_entity_finds_the_rule_rather_than_a_correlate() {
    // The world's rule uses `carpet` and `battery_ok` and ignores
    // `loaded`. Intersecting the context of successes finds correlates,
    // not causes, which `Precondition`'s own documentation admits — so
    // whether it latched onto the irrelevant condition is worth
    // asserting rather than hoping.
    let e = experiment(EPISODES, WINDOWS);
    let (carpet, battery, loaded) = e.found_the_real_rule();
    assert!(carpet, "it did not learn that carpet matters");
    assert!(battery, "it did not learn that the battery matters");
    assert!(
        !loaded,
        "it came to believe the load matters, which the world ignores — \
         a correlate held as a cause"
    );
}

// ---------- the world itself ----------

#[test]
fn the_structured_world_is_learnable_but_not_perfectly() {
    use brainos_crypto::rng::Drbg;
    let mut rng = Drbg::deterministic(b"world");
    let good = Situation {
        carpet: true,
        battery_ok: true,
        loaded: false,
    };
    let bad = Situation {
        carpet: false,
        battery_ok: true,
        loaded: false,
    };

    let mut good_hits = 0;
    let mut bad_hits = 0;
    for _ in 0..1_000 {
        if World::Structured.outcome(good, &mut rng) {
            good_hits += 1;
        }
        if World::Structured.outcome(bad, &mut rng) {
            bad_hits += 1;
        }
    }
    // ten percent noise: nothing is perfectly learnable, so a run that
    // claimed a hundred percent would be a bug
    assert!(
        (850..=950).contains(&good_hits),
        "the good situation succeeded {good_hits} times in 1000"
    );
    assert!(
        (50..=150).contains(&bad_hits),
        "the bad situation succeeded {bad_hits} times in 1000"
    );
}

#[test]
fn the_structureless_world_really_is_a_coin_flip() {
    use brainos_crypto::rng::Drbg;
    let mut rng = Drbg::deterministic(b"null world");
    let mut hits = 0;
    for n in 0..4_000u64 {
        if World::Structureless.outcome(Situation::from_bits(n % 8), &mut rng) {
            hits += 1;
        }
    }
    assert!(
        (1_800..=2_200).contains(&hits),
        "{hits} of 4000, which is not a coin flip"
    );
}

#[test]
fn every_situation_is_reachable() {
    let mut seen = std::collections::HashSet::new();
    for n in 0..8u64 {
        seen.insert(Situation::from_bits(n));
    }
    assert_eq!(seen.len(), 8, "three binary conditions is eight situations");
}
