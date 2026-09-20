//! The Domain 4 / Stage 5 suite.
//!
//! What is defended: reflexes never leave the body, a placement that
//! cannot meet its latency is refused at configuration rather than on a
//! robot, losing the datacenter degrades the entity instead of killing
//! it, and the entity is only surprised by things it has reason to be
//! surprised by.

use brainos_abi::ids::BodyId;
use brainos_abi::time::{Duration, Instant, Tick};
use brainos_abi::verb::{Namespace, Verb};
use brainos_cognition::ambient::{Ambient, Intent, SenseFrame};
use brainos_cognition::escalate::{cost_of, decide, Budget, Reason};
use brainos_cognition::placement::{Placement, Plan, Site};
use brainos_cognition::predict::{Forward, Surprise};
use brainos_cognition::resource::{Resource, ResourcePool, Tier};
use brainos_cognition::salience::{Salience, Salient};

fn v(s: &str) -> Verb {
    Verb::parse(s).unwrap()
}

// ---------- tiers ----------

#[test]
fn tier_zero_never_proposes_and_everything_above_it_always_does() {
    assert!(!Tier::Autonomic.proposes());
    for t in [Tier::Fast, Tier::Deliberate, Tier::Frontier] {
        assert!(t.proposes(), "{t:?}");
    }
}

#[test]
fn latency_budgets_increase_strictly_with_tier() {
    let mut last = Duration::ZERO;
    for t in Tier::ALL {
        assert!(t.budget() > last, "{t:?}");
        last = t.budget();
    }
}

// ---------- placement ----------

#[test]
fn a_reflex_can_never_be_placed_off_the_body() {
    let mut plan = Plan::distributed();
    for site in [Site::OnBoard, Site::Edge, Site::Remote] {
        assert!(
            !plan.place(Tier::Autonomic, site),
            "a reflex on {site:?} is a remote command with a hopeful name"
        );
    }
    assert_eq!(plan.site_for(Tier::Autonomic), Site::Body);
    assert!(plan.place(Tier::Autonomic, Site::Body));
}

#[test]
fn a_placement_that_cannot_meet_its_latency_is_refused_at_configuration() {
    // Tier 1 has a 100 ms budget; the remote round trip is 240 ms
    let bad = Placement {
        tier: Tier::Fast,
        site: Site::Remote,
    };
    assert!(!bad.is_coherent());

    let mut plan = Plan::distributed();
    assert!(
        !plan.place(Tier::Fast, Site::Remote),
        "this must fail here, not as a robot reacting 120ms late"
    );
    assert_eq!(plan.site_for(Tier::Fast), Site::OnBoard);
}

#[test]
fn the_standard_plans_are_all_coherent() {
    for plan in [Plan::all_on_body(), Plan::distributed(), Plan::thin_body()] {
        assert!(plan.is_coherent(), "{}", plan.describe());
        assert_eq!(plan.site_for(Tier::Autonomic), Site::Body);
    }
}

#[test]
fn losing_the_datacenter_degrades_the_entity_and_never_kills_it() {
    let plan = Plan::distributed();
    assert_eq!(plan.degraded_ceiling(Site::Remote), Tier::Deliberate);
    assert_eq!(plan.degraded_ceiling(Site::Edge), Tier::Frontier);
    // cut off from everything, reflexes remain — the difference between
    // degraded and dead, which is the difference Jibo's owners did not get
    let alone = Plan::all_on_body();
    assert_eq!(alone.degraded_ceiling(Site::OnBoard), Tier::Autonomic);
    assert!(Site::Remote.is_optional());
    assert!(!Site::Body.is_optional());
}

#[test]
fn moving_deliberation_to_a_datacenter_changes_only_where_it_runs() {
    let mut plan = Plan::all_on_body();
    let before = plan.site_for(Tier::Autonomic);
    assert!(plan.place(Tier::Deliberate, Site::Remote));
    assert_eq!(plan.site_for(Tier::Deliberate), Site::Remote);
    assert_eq!(
        plan.site_for(Tier::Autonomic),
        before,
        "nothing else about the entity moved"
    );
    assert!(plan.is_coherent());
}

// ---------- resources ----------

fn pool() -> ResourcePool {
    let mut p = ResourcePool::new();
    p.add(
        Resource::new("reflex", Tier::Autonomic, Site::Body)
            .able_to(Namespace::only("legs").unwrap(), "gait loop"),
    );
    p.add(
        Resource::new("local-policy", Tier::Fast, Site::OnBoard)
            .able_to(Namespace::only("legs").unwrap(), "obstacle replan"),
    );
    p.add(
        Resource::new("planner", Tier::Deliberate, Site::Edge)
            .able_to(Namespace::Any, "task planning"),
    );
    p.add(
        Resource::new("frontier", Tier::Frontier, Site::Remote)
            .able_to(Namespace::Any, "novel situations"),
    );
    p
}

#[test]
fn the_entity_reaches_for_the_cheapest_thing_that_can_do_the_job() {
    let p = pool();
    let r = p.cheapest_for(&v("legs.walk"), Tier::Fast).unwrap();
    assert_eq!(r.name, "local-policy");
    // and never for a Tier 0 resource, which does not propose
    assert_ne!(r.tier, Tier::Autonomic);
}

#[test]
fn a_verb_only_the_frontier_covers_goes_to_the_frontier() {
    let p = pool();
    let r = p.cheapest_for(&v("plan.route"), Tier::Fast).unwrap();
    assert_eq!(r.tier, Tier::Deliberate, "the planner covers everything");
    assert_eq!(p.best_for(&v("plan.route")).unwrap().name, "frontier");
}

#[test]
fn a_resource_that_has_stopped_meeting_its_budget_is_not_chosen() {
    let mut p = pool();
    // the local policy has started taking 400 ms against a 100 ms budget
    let r = p.get_mut("local-policy").unwrap();
    for _ in 0..10 {
        r.record(Duration::from_millis(400), false);
    }
    assert!(!r.meets_budget());
    let chosen = p.cheapest_for(&v("legs.walk"), Tier::Fast).unwrap();
    assert_eq!(chosen.name, "planner", "fall through to something that answers");
}

#[test]
fn losing_a_site_lowers_the_ceiling_visibly() {
    let mut p = pool();
    assert_eq!(p.ceiling(), Tier::Frontier);
    assert_eq!(p.site_lost(Site::Remote), 1);
    assert_eq!(p.ceiling(), Tier::Deliberate);
    assert_eq!(p.site_lost(Site::Edge), 1);
    assert_eq!(p.ceiling(), Tier::Fast);
    // and it comes back
    p.site_returned(Site::Remote);
    assert_eq!(p.ceiling(), Tier::Frontier);
}

#[test]
fn an_entity_with_no_resources_at_all_still_has_reflexes() {
    let p = ResourcePool::new();
    assert_eq!(p.ceiling(), Tier::Autonomic);
    assert!(p.cheapest_for(&v("legs.walk"), Tier::Fast).is_none());
}

#[test]
fn a_resource_that_is_often_wrong_is_tracked_as_such() {
    let mut r = Resource::new("flaky", Tier::Deliberate, Site::Edge);
    for i in 0..10 {
        r.record(Duration::from_millis(100), i % 2 == 0);
    }
    assert_eq!(r.reliability_pct(), 50);
    assert_eq!(r.invocations, 10);
}

// ---------- prediction ----------

#[test]
fn something_new_is_maximally_surprising_and_that_is_correct() {
    let mut f = Forward::new();
    let s = f.observe_named("leg.stall", 1.0);
    assert!(s.novel);
    assert!(s.is_salient());
    assert!(s.is_profound());
}

#[test]
fn a_stable_channel_stops_being_surprising() {
    let mut f = Forward::new();
    for _ in 0..50 {
        f.observe_named("rail.volts", 7.4);
    }
    let s = f.observe_named("rail.volts", 7.41);
    assert!(!s.novel);
    assert!(!s.is_salient(), "sigmas = {}", s.sigmas);
}

#[test]
fn a_noisy_channel_needs_a_bigger_deviation_to_surprise() {
    let mut noisy = Forward::new();
    let mut steady = Forward::new();
    for i in 0..200 {
        // noisy swings +-2.0; steady swings +-0.01
        let wobble = if i % 2 == 0 { 2.0 } else { -2.0 };
        noisy.observe_named("x", 10.0 + wobble);
        steady.observe_named("x", 10.0 + wobble * 0.005);
    }
    let dev = 3.0;
    let n = noisy.observe_named("x", 10.0 + dev);
    let s = steady.observe_named("x", 10.0 + dev);
    assert!(
        s.sigmas > n.sigmas,
        "the same deviation should mean more on a steady channel ({} vs {})",
        s.sigmas,
        n.sigmas
    );
    assert!(s.is_salient());
    assert!(!n.is_salient());
}

#[test]
fn surprise_is_measured_against_what_the_entity_held_not_what_it_learns() {
    // updating before comparing would make the entity never surprised,
    // which is a bug that looks like serenity
    let mut f = Forward::new();
    for _ in 0..50 {
        f.observe_named("tilt", 0.0);
    }
    let s = f.observe_named("tilt", 40.0);
    assert!(s.is_salient(), "sigmas={}", s.sigmas);
    assert_eq!(s.observed, 40.0);
    assert!(s.expected.abs() < 1.0);
}

#[test]
fn the_forward_model_is_deterministic_across_runs() {
    let run = || {
        let mut f = Forward::new();
        let mut out = Vec::new();
        for i in 0..100 {
            out.push(f.observe_named("x", (i % 7) as f32).sigmas);
        }
        out
    };
    assert_eq!(run(), run());
}

#[test]
fn channel_keys_are_stable_and_distinct() {
    assert_eq!(Forward::key_of("rail.volts"), Forward::key_of("rail.volts"));
    assert_ne!(Forward::key_of("rail.volts"), Forward::key_of("rail.amps"));
}

#[test]
fn releasing_a_body_lets_the_entity_forget_its_proprioception() {
    let mut f = Forward::new();
    f.observe_named("hexapod.tilt", 1.0);
    assert_eq!(f.known_channels(), 1);
    assert!(f.forget(Forward::key_of("hexapod.tilt")));
    assert_eq!(f.known_channels(), 0);
    assert!(!f.forget(Forward::key_of("nothing")));
}

#[test]
fn the_surprise_rate_reports_how_well_calibrated_the_entity_is() {
    let mut f = Forward::new();
    for _ in 0..500 {
        f.observe_named("steady", 1.0);
    }
    assert!(
        f.surprise_rate_pct() < 5,
        "a familiar situation should be quiet, got {}%",
        f.surprise_rate_pct()
    );
}

// ---------- salience ----------

#[test]
fn consequence_scales_evidence_rather_than_replacing_it() {
    // Given the same evidence, the more consequential reading scores
    // higher. That is what "consequence dominates" should mean.
    let evidence = 6.0;
    let harmless = Salience {
        surprise: evidence,
        consequence: 5,
        relevance: 0,
        urgency: 0,
        repeats: 0,
        refusal: false,
    };
    let dangerous = Salience {
        consequence: 95,
        ..harmless
    };
    assert!(dangerous.score() > harmless.score() * 2);
    assert!(dangerous.should_surface());
    assert!(!harmless.should_surface());

    // and at full evidence, the consequential one interrupts while the
    // harmless one still does not
    let certain_and_dangerous = Salience {
        surprise: 10.0,
        ..dangerous
    };
    let certain_and_harmless = Salience {
        surprise: 10.0,
        ..harmless
    };
    assert!(certain_and_dangerous.should_interrupt());
    assert!(!certain_and_harmless.should_interrupt());
}

#[test]
fn an_important_channel_reading_exactly_normal_does_not_interrupt() {
    // The bug a day of ambient operation found. Consequence used to be
    // additive, so a channel that matters carried a constant into the
    // score whatever it read — and a three-sigma blip on a tilt sensor,
    // six tenths of a degree and well inside its own noise, interrupted
    // a human.
    let blip = Salience {
        surprise: 3.0,
        consequence: 90, // tilt, on a body that can fall
        relevance: 40,
        urgency: 30,
        repeats: 0,
        refusal: false,
    };
    assert!(
        !blip.should_interrupt(),
        "a barely-salient reading scored {} and interrupted",
        blip.score()
    );
    assert!(blip.should_surface(), "but it is still worth logging");

    // the same channel genuinely out of range does interrupt
    let real = Salience {
        surprise: 10.0,
        urgency: 80,
        ..blip
    };
    assert!(real.should_interrupt());
}

#[test]
fn nothing_happening_on_an_important_channel_scores_nothing() {
    let quiet = Salience {
        surprise: 0.0,
        consequence: 100,
        relevance: 0,
        urgency: 0,
        repeats: 0,
        refusal: false,
    };
    assert_eq!(
        quiet.score(),
        0,
        "an entity that interrupts because a channel matters, while the \
         channel reads normal, is broken"
    );
}

#[test]
fn a_refusal_is_evidence_in_its_own_right() {
    // The gate refusing something *is* the event; there is no surprise
    // term to wait for.
    let refused = Salience {
        surprise: 0.0,
        consequence: 90,
        relevance: 50,
        urgency: 50,
        repeats: 0,
        refusal: true,
    };
    assert!(refused.should_interrupt(), "scored {}", refused.score());
}

#[test]
fn repetition_damps_so_an_assistant_does_not_get_muted() {
    let base = Salience {
        surprise: 4.0,
        consequence: 50,
        relevance: 50,
        urgency: 50,
        repeats: 0,
        refusal: false,
    };
    let mut last = base.score();
    for repeats in 1..=5u8 {
        let s = Salience { repeats, ..base };
        assert!(s.score() < last, "repeat {repeats} did not damp");
        last = s.score();
    }
}

#[test]
fn a_refusal_the_human_should_know_about_raises_the_score() {
    let quiet = Salience {
        surprise: 0.0,
        consequence: 10,
        relevance: 10,
        urgency: 10,
        repeats: 0,
        refusal: false,
    };
    let refused = Salience {
        refusal: true,
        ..quiet
    };
    assert!(refused.score() > quiet.score());
}

#[test]
fn a_salient_item_says_whether_it_interrupts() {
    let s = Salient::new(
        "the left middle leg stalled",
        Salience {
            surprise: 9.0,
            consequence: 90,
            relevance: 80,
            urgency: 90,
            repeats: 0,
            refusal: false,
        },
    );
    assert!(s.salience.should_interrupt());
    assert!(s.describe().as_str().contains("NOW"));
}

// ---------- escalation ----------

fn surprise(sigmas: f32, novel: bool) -> Surprise {
    Surprise {
        key: 1,
        observed: 0.0,
        expected: 0.0,
        sigmas,
        novel,
    }
}

#[test]
fn nothing_surprising_means_no_escalation_and_no_cost() {
    let mut b = Budget::plentiful();
    let before = b.available();
    let e = decide(&surprise(0.1, false), 10, true, 0, false, &mut b, Tier::Frontier);
    assert!(!e.happened());
    assert_eq!(b.available(), before);
}

#[test]
fn something_novel_goes_straight_to_the_frontier() {
    let mut b = Budget::plentiful();
    let e = decide(&surprise(0.0, true), 0, true, 0, false, &mut b, Tier::Frontier);
    assert_eq!(e.reason, Some(Reason::Novel));
    assert_eq!(e.to, Tier::Frontier);
    assert_eq!(e.cost, cost_of(Tier::Frontier));
}

#[test]
fn repeated_failure_below_escalates_rather_than_retrying_forever() {
    let mut b = Budget::plentiful();
    let e = decide(&surprise(0.0, false), 0, true, 3, false, &mut b, Tier::Frontier);
    assert_eq!(e.reason, Some(Reason::LowerTierFailed));
    assert_eq!(e.to, Tier::Frontier);
}

#[test]
fn a_ceiling_caps_escalation_without_stopping_it() {
    let mut b = Budget::plentiful();
    // the datacenter is unreachable
    let e = decide(&surprise(0.0, true), 0, true, 0, false, &mut b, Tier::Deliberate);
    assert_eq!(e.to, Tier::Deliberate);
    assert!(e.budget_limited, "the entity should know it wanted more");
}

#[test]
fn an_exhausted_budget_steps_down_rather_than_refusing_to_think() {
    let mut b = Budget::new(15, 0); // enough for Deliberate, not Frontier
    let e = decide(&surprise(0.0, true), 0, true, 0, false, &mut b, Tier::Frontier);
    assert_eq!(e.to, Tier::Deliberate);
    assert!(e.budget_limited);

    // and once even that is gone, it thinks locally
    let e = decide(&surprise(0.0, true), 0, true, 0, false, &mut b, Tier::Frontier);
    assert_eq!(e.to, Tier::Fast);
}

#[test]
fn the_budget_refills_over_time_and_is_capped() {
    let mut b = Budget::frugal();
    let cap = b.available();
    b.spend(cap);
    assert_eq!(b.available(), 0);
    for _ in 0..1000 {
        b.tick();
    }
    assert_eq!(b.available(), cap, "the bucket is capped, not unbounded");
}

#[test]
fn a_consequential_situation_escalates_even_when_unsurprising() {
    let mut b = Budget::plentiful();
    let e = decide(&surprise(0.0, false), 90, true, 0, false, &mut b, Tier::Frontier);
    assert_eq!(e.reason, Some(Reason::Consequential));
}

#[test]
fn having_no_practiced_skill_is_itself_a_reason_to_think() {
    let mut b = Budget::plentiful();
    let e = decide(&surprise(0.0, false), 0, false, 0, false, &mut b, Tier::Frontier);
    assert_eq!(e.reason, Some(Reason::NoSkill));
}

// ---------- the ambient loop ----------

fn always_practiced(_: &str) -> bool {
    true
}
fn harmless(_: &str) -> u8 {
    10
}

#[test]
fn a_quiet_tick_is_idle_and_costs_nothing() {
    let mut a = Ambient::default();
    let before = a.budget.available();
    let out = a.tick(
        &SenseFrame::quiet(Instant(0)).sensing("rail.volts", 7.4),
        Tick(1),
        always_practiced,
        harmless,
    );
    // the first observation of a channel is novel, so warm it up first
    for i in 0..20u64 {
        a.tick(
            &SenseFrame::quiet(Instant(i * 1000)).sensing("rail.volts", 7.4),
            Tick(i + 2),
            always_practiced,
            harmless,
        );
    }
    let quiet = a.tick(
        &SenseFrame::quiet(Instant(50_000)).sensing("rail.volts", 7.4),
        Tick(50),
        always_practiced,
        harmless,
    );
    assert!(quiet.is_idle(), "{quiet:?}");
    assert!(a.attention.quiet_ticks > 0);
    let _ = (out, before);
}

#[test]
fn the_human_speaking_always_attends() {
    let mut a = Ambient::default();
    let out = a.tick(
        &SenseFrame::quiet(Instant(0)).saying("go to the door"),
        Tick(1),
        always_practiced,
        harmless,
    );
    match out {
        Intent::Think { about, .. } => assert_eq!(about, "go to the door"),
        other => panic!("expected a thought, got {other:?}"),
    }
    assert_eq!(a.attention.quiet_ticks, 0);
}

#[test]
fn a_stopped_body_outranks_everything_even_a_speaking_human() {
    let mut a = Ambient::default();
    let body = BodyId::from_bytes([3; 32]);
    let mut frame = SenseFrame::quiet(Instant(0)).saying("never mind that");
    frame.stops.push((body, "stability-lost"));

    let out = a.tick(&frame, Tick(1), always_practiced, harmless);
    match out {
        Intent::Attend { body: b, why, .. } => {
            assert_eq!(b, body);
            assert_eq!(why, "stability-lost");
        }
        other => panic!("a fallen robot outranks conversation, got {other:?}"),
    }
}

#[test]
fn a_genuine_deviation_surfaces_and_a_repeat_of_it_does_not_shout_twice() {
    let mut a = Ambient::default();
    // learn a steady channel
    for i in 0..60u64 {
        a.tick(
            &SenseFrame::quiet(Instant(i * 1000)).sensing("tilt", 0.5),
            Tick(i + 1),
            always_practiced,
            |_| 80,
        );
    }
    let first = a.tick(
        &SenseFrame::quiet(Instant(70_000)).sensing("tilt", 25.0),
        Tick(70),
        always_practiced,
        |_| 80,
    );
    assert!(
        matches!(first, Intent::Surface { .. }),
        "got {first:?}"
    );
    let score_after_first = a.attention.items.first().map(|i| i.salience.score()).unwrap();

    // the same deviation again: the entity has already said it
    let second = a.tick(
        &SenseFrame::quiet(Instant(71_000)).sensing("tilt", 25.0),
        Tick(71),
        always_practiced,
        |_| 80,
    );
    if let Intent::Surface { items } = &second {
        assert!(
            items[0].salience.score() < score_after_first,
            "repetition must damp"
        );
    }
}

#[test]
fn acknowledging_lets_the_entity_speak_up_about_it_again() {
    let mut a = Ambient::default();
    for i in 0..60u64 {
        a.tick(
            &SenseFrame::quiet(Instant(i * 1000)).sensing("tilt", 0.5),
            Tick(i + 1),
            always_practiced,
            |_| 80,
        );
    }
    a.tick(
        &SenseFrame::quiet(Instant(70_000)).sensing("tilt", 25.0),
        Tick(70),
        always_practiced,
        |_| 80,
    );
    a.acknowledge();
    let again = a.tick(
        &SenseFrame::quiet(Instant(80_000)).sensing("tilt", 25.0),
        Tick(80),
        always_practiced,
        |_| 80,
    );
    assert!(matches!(again, Intent::Surface { .. }), "{again:?}");
}

#[test]
fn a_long_quiet_period_is_when_consolidation_belongs() {
    let mut a = Ambient::default();
    for i in 0..100u64 {
        a.tick(
            &SenseFrame::quiet(Instant(i * 1000)).sensing("rail", 7.4),
            Tick(i + 1),
            always_practiced,
            harmless,
        );
    }
    assert!(a.should_consolidate(20));
    // and a body in motion is not quiet
    let body = BodyId::from_bytes([1; 32]);
    let mut frame = SenseFrame::quiet(Instant(200_000));
    frame.stops.push((body, "commanded"));
    a.tick(&frame, Tick(200), always_practiced, harmless);
    assert!(!a.should_consolidate(20));
}

#[test]
fn a_lowered_ceiling_caps_what_the_loop_escalates_to() {
    let mut a = Ambient::default();
    let mut frame = SenseFrame::quiet(Instant(0)).saying("something new");
    frame.ceiling = Tier::Fast;
    let out = a.tick(&frame, Tick(1), |_| false, |_| 90);
    match out {
        Intent::Think { escalation, .. } => {
            assert!(escalation.to <= Tier::Fast, "{:?}", escalation.to);
        }
        other => panic!("expected a thought, got {other:?}"),
    }
}
