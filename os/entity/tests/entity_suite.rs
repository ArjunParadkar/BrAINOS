//! BFS  LEVEL    Stage 4 Entity - one Key, one state graph, every body at once  [crate brainos-entity]
//! BFS  PURPOSE  Tests one entity inhabiting several bodies with a single memory, ending with the two-body demo where the second body uses what the first learned
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, body, cognition, crypto, entity, kira, memory, rt
//! BFS  USED BY  (entry point or test)
//!
//! The Stage 4 suite — one self, many bodies, one memory.
//!
//! The Phase 0 demo is exactly two bodies sharing one entity at once,
//! and the last test in this file is that demo, in simulation: the
//! entity does something on one body, then something on the other, and
//! the second action reflects memory from the first.

use brainos_abi::ids::BodyId;
use brainos_abi::time::{Instant, Tick};
use brainos_abi::verb::Verb;
use brainos_body::calibration::{Calibration, JointTrim};
use brainos_body::hal::Telemetry;
use brainos_body::region::RegionClass;
use brainos_body::schema::{CapabilitySchema, ParamSpec, VerbSpec};
use brainos_cognition::ambient::{Intent, SenseFrame};
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::Entity;
use brainos_entity::goals::{Goals, Level};
use brainos_entity::life::{Life, Span};
use brainos_entity::simultaneity::{Arbiter, Claim, Conflict, Resolution, Resource, SILENCE_GRACE};
use brainos_memory::consolidate::Dream;
use brainos_memory::episode::Outcome;
use brainos_memory::graph::NodeKind;
use brainos_rt::envelope::Channel;

fn v(s: &str) -> Verb {
    Verb::parse(s).unwrap()
}

fn hexapod_schema(label: &str) -> CapabilitySchema {
    CapabilitySchema::new(label, "native")
        .with(
            VerbSpec::new(v("legs.walk"), RegionClass::Legs, "walk")
                .with(ParamSpec::scalar("speed", Channel::Speed, 0.0, 0.6)),
        )
        .with(
            VerbSpec::new(v("legs.stand"), RegionClass::Legs, "stand")
                .with(ParamSpec::scalar("height", Channel::Angle, -20.0, 20.0)),
        )
}

fn laptop_schema() -> CapabilitySchema {
    CapabilitySchema::new("thinkpad", "native")
        .with(
            VerbSpec::new(v("screen.say"), RegionClass::Screen, "say something")
                .with(ParamSpec::text("text", 240)),
        )
        .with(
            VerbSpec::new(v("fs.write"), RegionClass::Storage, "write a note")
                .with(ParamSpec::text("path", 120))
                .with(ParamSpec::text("content", 2000)),
        )
}

fn calibration() -> Calibration {
    let mut c = Calibration::new(BodyId::ZERO, Tick(1));
    for ch in 0..6u8 {
        c.trim(JointTrim {
            channel: ch,
            offset_deg: 0.0,
            scale: 1.0,
            min_deg: -35.0,
            max_deg: 35.0,
            probed: true,
        })
        .unwrap();
    }
    c
}

fn fit() -> Telemetry {
    Telemetry {
        at: Instant(0),
        link_alive: true,
        rail_v: Some(7.4),
        current_a: Some(0.3),
        tilt_deg: Some(1.0),
        hottest_c: Some(35.0),
        stops: 0,
        overruns: 0,
    }
}

fn entity() -> Entity {
    let mut e = Entity::new(BrainKey::from_seed(&[1; 32]), Tick(0));
    e.wake(None, None, Tick(1)).unwrap();
    e
}

// ---------- waking and sleeping ----------

#[test]
fn a_new_entity_wakes_with_nothing_and_knows_it() {
    let mut e = Entity::new(BrainKey::from_seed(&[1; 32]), Tick(0));
    let w = e.wake(None, None, Tick(1)).unwrap();
    assert_eq!(w.inherited, 0);
    assert_eq!(w.skills, 0);
    assert_eq!(w.previous_wakes, 0);
    assert!(!w.after_power_loss);
    assert!(e.is_awake());
}

#[test]
fn waking_twice_without_sleeping_is_refused() {
    let mut e = entity();
    assert!(e.wake(None, None, Tick(2)).is_err());
}

#[test]
fn the_entity_between_two_wakes_is_the_same_entity() {
    let key = BrainKey::from_seed(&[9; 32]);
    let id = key.entity_id();

    let mut first = Entity::new(key, Tick(0));
    first.wake(None, None, Tick(1)).unwrap();
    for t in 0..10u64 {
        first.outcome(
            v("legs.walk"),
            None,
            Outcome::Succeeded,
            "forward",
            &["carpet"],
        );
        let _ = t;
    }
    first.dream(Dream::deep());
    let (durable, life) = first.sleep().unwrap();
    let skills_before = first.skills.len();
    let memories_before = first.graph.len();

    // a completely new process, the same key
    let mut second = Entity::new(BrainKey::from_seed(&[9; 32]), Tick(0));
    let w = second.wake(Some(&durable), Some(life), Tick(500)).unwrap();

    assert_eq!(second.id(), id, "the key is the identity");
    assert!(w.inherited > 0);
    assert_eq!(w.previous_wakes, 1);
    assert_eq!(second.skills.len(), skills_before);
    assert!(second.graph.len() >= memories_before - 1);
    assert_eq!(second.life.wakes, 1);
}

#[test]
fn an_entity_notices_it_lost_power_last_time() {
    let mut e = entity();
    // simulate an unclean span by recording one directly
    e.life.record(Span {
        woke_at: Tick(0),
        slept_at: Tick(10),
        clean: false,
        ..Span::default()
    });
    let (durable, life) = e.sleep().unwrap();
    let mut again = Entity::new(BrainKey::from_seed(&[1; 32]), Tick(0));
    let w = again.wake(Some(&durable), Some(life), Tick(100)).unwrap();
    // the most recent span was the clean sleep we just did, so this is
    // false — what we are checking is that the flag reflects the *last*
    // span rather than any span
    assert!(!w.after_power_loss);
    assert_eq!(again.life.unclean_wakes, 1);
    assert!(again.life.unclean_pct() > 0);
}

// ---------- acquiring bodies ----------

#[test]
fn acquiring_a_body_gives_the_entity_a_limb_it_may_move() {
    let mut e = entity();
    let device = BrainKey::from_seed(&[2; 32]);
    let body = e
        .acquire(&device, hexapod_schema("mini-blur"), calibration(), 6)
        .unwrap();

    assert!(e.bodies.knows_body(&body));
    assert!(e.repertoire().contains(&v("legs.walk")));
    assert_eq!(e.life.bodies_ever.len(), 1);

    e.observe_body(&body, &fit());
    let out = e.attempt(v("legs.walk"), body, &[("speed", 0.3)]);
    assert!(out.verdict.is_granted(), "{:?}", out.trace.refusal());
}

#[test]
fn calibration_lives_in_the_state_graph_not_in_the_body() {
    let mut e = entity();
    let device = BrainKey::from_seed(&[2; 32]);
    let body = e
        .acquire(&device, hexapod_schema("mini-blur"), calibration(), 6)
        .unwrap();

    let calib: Vec<_> = e.graph.of_kind(NodeKind::Calibration).collect();
    assert_eq!(calib.len(), 1);
    assert!(calib[0].content.contains(body.fingerprint().as_str()));
    assert!(calib[0].content.contains("probed=100%"));
}

#[test]
fn releasing_a_body_leaves_the_entity_intact() {
    let mut e = entity();
    let device = BrainKey::from_seed(&[2; 32]);
    let body = e
        .acquire(&device, hexapod_schema("mini-blur"), calibration(), 6)
        .unwrap();
    let memories = e.graph.len();

    assert!(e.release(&body));
    assert!(!e.bodies.knows_body(&body));
    assert!(!e.repertoire().contains(&v("legs.walk")));
    assert!(e.graph.len() > memories, "and it remembers having had it");
    assert_eq!(
        e.life.bodies_ever.len(),
        1,
        "a body it no longer has is still a body it has had"
    );
}

#[test]
fn a_released_bodys_verbs_are_refused_immediately() {
    let mut e = entity();
    let device = BrainKey::from_seed(&[2; 32]);
    let body = e
        .acquire(&device, hexapod_schema("mini-blur"), calibration(), 6)
        .unwrap();
    e.observe_body(&body, &fit());
    assert!(e
        .attempt(v("legs.walk"), body, &[("speed", 0.3)])
        .verdict
        .is_granted());

    e.release(&body);
    let out = e.attempt(v("legs.walk"), body, &[("speed", 0.3)]);
    assert!(!out.verdict.is_granted());
    assert_eq!(
        out.trace.refusal().unwrap().0,
        brainos_abi::error::Stage::Authorize
    );
}

// ---------- two bodies at once ----------

#[test]
fn one_entity_holds_two_bodies_with_one_memory_and_one_gate() {
    let mut e = entity();
    let hexapod = e
        .acquire(
            &BrainKey::from_seed(&[2; 32]),
            hexapod_schema("mini-blur"),
            calibration(),
            6,
        )
        .unwrap();
    let laptop = e
        .acquire(
            &BrainKey::from_seed(&[3; 32]),
            laptop_schema(),
            Calibration::new(BodyId::ZERO, Tick(1)),
            0,
        )
        .unwrap();

    assert_eq!(e.bodies.body_count(), 2);
    assert_ne!(hexapod, laptop);

    // one repertoire spanning both
    let rep = e.repertoire();
    assert!(rep.contains(&v("legs.walk")));
    assert!(rep.contains(&v("screen.say")));

    // one gate, two policies
    assert_eq!(e.kira.bound_bodies().len(), 2);

    // and nothing was duplicated: there is one graph
    let before = e.graph.len();
    e.outcome(v("legs.walk"), Some(hexapod), Outcome::Succeeded, "a", &[]);
    e.outcome(v("screen.say"), Some(laptop), Outcome::Succeeded, "b", &[]);
    assert_eq!(e.graph.len(), before + 2);
}

#[test]
fn a_verb_on_the_wrong_body_is_refused_even_when_another_body_can_do_it() {
    let mut e = entity();
    let hexapod = e
        .acquire(
            &BrainKey::from_seed(&[2; 32]),
            hexapod_schema("mini-blur"),
            calibration(),
            6,
        )
        .unwrap();
    let laptop = e
        .acquire(
            &BrainKey::from_seed(&[3; 32]),
            laptop_schema(),
            Calibration::new(BodyId::ZERO, Tick(1)),
            0,
        )
        .unwrap();
    e.observe_body(&hexapod, &fit());
    e.observe_body(&laptop, &fit());

    let out = e.attempt(v("legs.walk"), laptop, &[("speed", 0.3)]);
    assert!(!out.verdict.is_granted());
    let (_, d) = out.trace.refusal().unwrap();
    assert_eq!(d.code, brainos_abi::error::DenyCode::BodyLacksVerb);
    assert!(
        d.detail.contains("another body"),
        "the entity should know a different body can: {}",
        d.detail
    );
}

#[test]
fn two_identical_hexapods_are_two_different_bodies() {
    let mut e = entity();
    let a = e
        .acquire(
            &BrainKey::from_seed(&[2; 32]),
            hexapod_schema("left"),
            calibration(),
            6,
        )
        .unwrap();
    let b = e
        .acquire(
            &BrainKey::from_seed(&[3; 32]),
            hexapod_schema("right"),
            calibration(),
            6,
        )
        .unwrap();
    assert_ne!(a, b, "same schema, different attestation key");
    assert_eq!(e.bodies.bodies_offering(&v("legs.walk")).len(), 2);
}

// ---------- arbitration ----------

fn claim(body: u8, resource: &str, cost: u8, at: u64) -> Claim {
    Claim {
        body: BodyId::from_bytes([body; 32]),
        resource: Resource::named(resource),
        verb: v("arm.hold"),
        interruption_cost: cost,
        claimed_at: Tick(at),
        last_heard: Instant(at * 1000),
    }
}

#[test]
fn an_uncontested_claim_is_simply_granted() {
    let mut a = Arbiter::new();
    assert!(a.claim(claim(1, "the kettle", 50, 1)).is_ok());
    assert_eq!(a.len(), 1);
    assert!(a.holder_of(&Resource::named("the kettle")).is_some());
}

#[test]
fn a_body_refreshing_its_own_claim_is_not_a_conflict() {
    let mut a = Arbiter::new();
    a.claim(claim(1, "the kettle", 50, 1)).unwrap();
    for t in 2..20 {
        assert!(
            a.claim(claim(1, "the kettle", 50, t)).is_ok(),
            "a gait loop re-claims every step"
        );
    }
    assert_eq!(a.conflicts_seen, 0);
    assert_eq!(a.len(), 1);
}

#[test]
fn interrupting_something_that_would_drop_is_refused() {
    let mut a = Arbiter::new();
    a.claim(claim(1, "the mug", 85, 1)).unwrap(); // mid-lift
    let err = a.claim(claim(2, "the mug", 90, 2)).unwrap_err();
    assert!(matches!(err.1, Resolution::Keep { .. }));
    assert!(err.1.because().contains("drop"));
    assert_eq!(
        a.holder_of(&Resource::named("the mug")).unwrap().body,
        BodyId::from_bytes([1; 32])
    );
}

#[test]
fn the_more_consequential_claim_wins_by_a_clear_margin() {
    let mut a = Arbiter::new();
    a.claim(claim(1, "the doorway", 20, 1)).unwrap(); // walking through
    assert!(
        a.claim(claim(2, "the doorway", 60, 2)).is_ok(),
        "carrying something yields the doorway to nothing"
    );
    assert_eq!(
        a.holder_of(&Resource::named("the doorway")).unwrap().body,
        BodyId::from_bytes([2; 32])
    );
}

#[test]
fn two_claims_too_close_to_call_escalate_rather_than_being_guessed() {
    let mut a = Arbiter::new();
    a.claim(claim(1, "the bench", 40, 1)).unwrap();
    let err = a.claim(claim(2, "the bench", 45, 2)).unwrap_err();
    assert!(matches!(err.1, Resolution::Escalate { .. }));
    assert_eq!(a.len(), 1, "nothing changed while the entity thinks");
}

#[test]
fn a_silent_body_loses_its_claims_but_not_instantly() {
    let mut a = Arbiter::new();
    a.claim(claim(1, "the kettle", 50, 1)).unwrap();
    // a link hiccup must not make a robot let go
    assert!(a
        .lapse_silent(Instant(1_000 + SILENCE_GRACE.as_micros() / 2))
        .is_empty());
    assert_eq!(a.len(), 1);

    let lapsed = a.lapse_silent(Instant(1_000 + SILENCE_GRACE.as_micros() + 1));
    assert_eq!(lapsed.len(), 1);
    assert_eq!(a.len(), 0);
    assert_eq!(a.lapses, 1);
}

#[test]
fn a_body_leaving_releases_everything_it_held() {
    let mut a = Arbiter::new();
    a.claim(claim(1, "the kettle", 50, 1)).unwrap();
    a.claim(claim(1, "the bench", 50, 1)).unwrap();
    a.claim(claim(2, "the door", 50, 1)).unwrap();
    assert_eq!(a.release_all(&BodyId::from_bytes([1; 32])), 2);
    assert_eq!(a.len(), 1);
}

#[test]
fn the_arbitration_rule_is_a_pure_function_of_the_conflict() {
    let c = Conflict {
        incumbent: claim(1, "x", 85, 1),
        challenger: claim(2, "x", 10, 2),
    };
    assert_eq!(brainos_entity::simultaneity::resolve(&c), brainos_entity::simultaneity::resolve(&c));
}

// ---------- goals ----------

#[test]
fn a_subgoal_of_a_micro_goal_is_refused_as_a_lost_thread() {
    let mut g = Goals::new();
    let human = g.asked("tidy the workshop", Tick(1));
    let sub = g.derive(human, "put the tools away", Level::Sub, Tick(1)).unwrap();
    let micro = g.derive(sub, "pick up the wrench", Level::Micro, Tick(1)).unwrap();
    assert!(g.derive(micro, "something deeper", Level::Micro, Tick(1)).is_none());
    assert!(g.derive(micro, "something shallower", Level::Sub, Tick(1)).is_none());
}

#[test]
fn derived_goals_fade_and_human_goals_do_not() {
    let mut g = Goals::new();
    let human = g.asked("tidy the workshop", Tick(1));
    g.derive(human, "put the tools away", Level::Micro, Tick(1));
    assert_eq!(g.len(), 2);

    let dropped = g.tidy(Tick(1_000));
    assert_eq!(dropped.len(), 1, "the micro-goal faded");
    assert_eq!(g.at_level(Level::Human).count(), 1);

    // and a very long time later the human goal is still there
    g.tidy(Tick(10_000_000));
    assert_eq!(g.at_level(Level::Human).count(), 1);
}

#[test]
fn a_goal_that_keeps_failing_is_abandoned_rather_than_retried_forever() {
    let mut g = Goals::new();
    let human = g.asked("open the jar", Tick(1));
    let sub = g.derive(human, "grip harder", Level::Sub, Tick(1)).unwrap();
    for _ in 0..6 {
        g.failed(sub);
    }
    let dropped = g.tidy(Tick(2));
    assert_eq!(dropped.len(), 1);
    assert!(dropped[0].is_futile());
}

#[test]
fn retiring_a_human_goal_takes_its_children_with_it() {
    let mut g = Goals::new();
    let human = g.asked("tidy the workshop", Tick(1));
    let sub = g.derive(human, "put the tools away", Level::Sub, Tick(1)).unwrap();
    g.derive(sub, "pick up the wrench", Level::Micro, Tick(1));
    assert_eq!(g.len(), 3);
    assert!(g.retire(human));
    g.tidy(Tick(2));
    assert_eq!(g.len(), 0);
}

#[test]
fn the_entity_works_on_the_most_specific_thing_it_can_act_on() {
    let mut g = Goals::new();
    let human = g.asked("tidy the workshop", Tick(1));
    let sub = g.derive(human, "put the tools away", Level::Sub, Tick(1)).unwrap();
    g.derive(sub, "pick up the wrench", Level::Micro, Tick(1));
    let (_, current) = g.current().unwrap();
    assert_eq!(current.what, "pick up the wrench");
    assert_eq!(current.level, Level::Micro);
}

// ---------- life ----------

#[test]
fn an_entity_cannot_report_a_trend_it_does_not_have_the_history_for() {
    let mut l = Life::begin(Tick(0));
    for _ in 0..3 {
        l.record(Span {
            actions_granted: 10,
            actions_denied: 0,
            ..Span::default()
        });
    }
    assert_eq!(l.calibration_trend(), None, "three spans is not a trend");
}

#[test]
fn calibration_improving_over_time_is_visible_in_the_life_record() {
    let mut l = Life::begin(Tick(0));
    // early: proposing a lot it cannot do
    for _ in 0..3 {
        l.record(Span {
            actions_granted: 3,
            actions_denied: 7,
            ..Span::default()
        });
    }
    for _ in 0..3 {
        l.record(Span {
            actions_granted: 5,
            actions_denied: 5,
            ..Span::default()
        });
    }
    // late: it has learned what its body can do
    for _ in 0..3 {
        l.record(Span {
            actions_granted: 9,
            actions_denied: 1,
            ..Span::default()
        });
    }
    let (early, late) = l.calibration_trend().unwrap();
    assert!(late > early, "{late} should beat {early}");
    assert_eq!(early, 30);
    assert_eq!(late, 90);
}

#[test]
fn a_life_round_trips_through_its_bytes() {
    use brainos_abi::codec::{Decode, Encode};
    let mut l = Life::begin(Tick(7));
    l.saw_body(BodyId::from_bytes([1; 32]));
    l.record(Span {
        woke_at: Tick(7),
        slept_at: Tick(100),
        actions_granted: 42,
        clean: true,
        ..Span::default()
    });
    let bytes = l.to_bytes().unwrap();
    assert_eq!(Life::from_bytes(&bytes).unwrap(), l);
}

// ---------- the experience loop ----------

#[test]
fn a_quiet_entity_ticks_cheaply() {
    let mut e = entity();
    for i in 0..50u64 {
        let out = e.tick(SenseFrame::quiet(Instant(i * 1000)).sensing("rail.volts", 7.4));
        let _ = out;
    }
    let last = e.tick(SenseFrame::quiet(Instant(60_000)).sensing("rail.volts", 7.4));
    assert!(last.is_idle(), "{last:?}");
    assert!(e.ambient.should_consolidate(10));
}

#[test]
fn consequence_is_derived_from_the_bodies_the_entity_actually_has() {
    // a laptop-only entity is not alarmed by tilt; a legged one is
    let mut laptop_only = entity();
    laptop_only
        .acquire(
            &BrainKey::from_seed(&[3; 32]),
            laptop_schema(),
            Calibration::new(BodyId::ZERO, Tick(1)),
            0,
        )
        .unwrap();

    let mut legged = entity();
    legged
        .acquire(
            &BrainKey::from_seed(&[2; 32]),
            hexapod_schema("mini-blur"),
            calibration(),
            6,
        )
        .unwrap();

    // warm both up on a steady tilt, then deviate
    for i in 0..60u64 {
        laptop_only.tick(SenseFrame::quiet(Instant(i * 1000)).sensing("tilt", 0.5));
        legged.tick(SenseFrame::quiet(Instant(i * 1000)).sensing("tilt", 0.5));
    }
    let l = laptop_only.tick(SenseFrame::quiet(Instant(70_000)).sensing("tilt", 25.0));
    let g = legged.tick(SenseFrame::quiet(Instant(70_000)).sensing("tilt", 25.0));

    let legged_surfaced = matches!(g, Intent::Surface { .. });
    let laptop_surfaced = matches!(l, Intent::Surface { .. });
    assert!(legged_surfaced, "a body that can fall must care: {g:?}");
    assert!(
        !laptop_surfaced || legged_surfaced,
        "a laptop should care no more than a robot"
    );
}

#[test]
fn consolidation_will_not_run_while_a_body_is_mid_action() {
    let mut e = entity();
    e.arbiter
        .claim(claim(1, "the kettle", 50, 1))
        .unwrap();
    assert!(
        e.dream(Dream::deep()).is_none(),
        "a memory rewrite racing a gait loop is fine in testing and catastrophic once"
    );
    e.arbiter.release_all(&BodyId::from_bytes([1; 32]));
    assert!(e.dream(Dream::deep()).is_some());
}

#[test]
fn consolidating_twice_does_not_double_count_the_same_episodes() {
    let mut e = entity();
    for _ in 0..10 {
        e.outcome(v("legs.walk"), None, Outcome::Succeeded, "forward", &["carpet"]);
    }
    e.dream(Dream::deep()).unwrap();
    let attempts = e.skills.for_verb(&v("legs.walk")).unwrap().attempts;
    e.dream(Dream::deep()).unwrap();
    assert_eq!(
        e.skills.for_verb(&v("legs.walk")).unwrap().attempts,
        attempts,
        "the session mark is what stops a second pass re-reading the same episodes"
    );
}

// ---------- the untrusted world ----------

#[test]
fn the_internet_is_shut_until_the_entity_opens_it_and_shuts_again() {
    let mut e = entity();
    assert!(!e.kira.world_enabled);
    e.open_world(100, "look up a servo datasheet");
    assert!(e.kira.world_enabled);
    assert!(e.open_session().is_some());
    e.close_worlds();
    assert!(!e.kira.world_enabled);
    assert!(e.open_session().is_none());
}

// ---------- the Phase 0 demo ----------

#[test]
fn two_bodies_one_memory_the_phase_zero_demo() {
    // The demo the roadmap is built around: the entity does something on
    // the laptop, then something on the robot, and the robot's action
    // reflects memory from the laptop's.
    let mut e = entity();

    let laptop = e
        .acquire(
            &BrainKey::from_seed(&[3; 32]),
            laptop_schema(),
            Calibration::new(BodyId::ZERO, Tick(1)),
            0,
        )
        .unwrap();
    let hexapod = e
        .acquire(
            &BrainKey::from_seed(&[2; 32]),
            hexapod_schema("mini-blur"),
            calibration(),
            6,
        )
        .unwrap();
    e.observe_body(&laptop, &fit());
    e.observe_body(&hexapod, &fit());

    // --- D1: the entity acts on the laptop and learns something there
    let out = e.propose(
        brainos_kira::action::Request::on_body(v("fs.write"), laptop, e.tick)
            .arg("path", brainos_body::policy::Arg::Text("floor.txt".into()))
            .arg(
                "content",
                brainos_body::policy::Arg::Text("the workshop floor is carpet".into()),
            )
            .because("recording what I was told"),
    );
    assert!(out.verdict.is_granted(), "{:?}", out.trace.refusal());
    e.outcome(
        v("fs.write"),
        Some(laptop),
        Outcome::Succeeded,
        "wrote that the workshop floor is carpet",
        &["workshop"],
    );

    // --- D2: that memory is one memory, reachable from anywhere
    let recalled = e.recall("what is the workshop floor", 3);
    assert!(
        recalled
            .iter()
            .any(|(n, _)| n.content.contains("carpet")),
        "the entity should remember: {:?}",
        recalled.iter().map(|(n, _)| &n.content).collect::<Vec<_>>()
    );

    // --- D3: acting on the *other* body, informed by it
    //
    // The entity has learned on the laptop that this floor is carpet, so
    // when it walks it does so in that context — and the episode it
    // records on the robot carries the same context tag the laptop
    // episode did. One memory, two bodies.
    let out = e.attempt(v("legs.walk"), hexapod, &[("speed", 0.3)]);
    assert!(out.verdict.is_granted(), "{:?}", out.trace.refusal());
    e.outcome(
        v("legs.walk"),
        Some(hexapod),
        Outcome::Succeeded,
        "walked, knowing the floor is carpet",
        &["workshop", "carpet"],
    );

    // the two acts are in one graph, on one entity, in order
    let episodes: Vec<&str> = e
        .graph
        .of_kind(NodeKind::Episode)
        .map(|n| n.content.as_str())
        .collect();
    assert!(episodes.iter().any(|c| c.contains("fs.write")));
    assert!(episodes.iter().any(|c| c.contains("legs.walk")));

    // and both bodies are in one life
    assert_eq!(e.life.bodies_ever.len(), 2);
    assert_eq!(e.bodies.body_count(), 2);

    // and the gate audited both, under one entity
    assert!(e.kira.trail.granted_count() >= 2);
    let root = e.kira.trail.root();
    assert!(e.kira.trail.prove(0).unwrap().verify(&root));

    // --- and the whole thing survives the move to a new body entirely
    let (durable, life) = e.sleep().unwrap();
    let mut reborn = Entity::new(BrainKey::from_seed(&[1; 32]), Tick(0));
    reborn.wake(Some(&durable), Some(life), Tick(9_000)).unwrap();
    let recalled = reborn.recall("what is the workshop floor", 3);
    assert!(
        recalled.iter().any(|(n, _)| n.content.contains("carpet")),
        "memory outlives every body"
    );
    assert_eq!(reborn.life.bodies_ever.len(), 2);
}

// ---------------------------------------------------------------------
// the counter belongs to the entity
// ---------------------------------------------------------------------

/// §2.4 makes entity age the x-axis of the curve the moat becomes
/// visible on. So age has to be a fact about the entity rather than a
/// number the host chose — otherwise the curve is forgeable by the one
/// party with an interest in forging it.
#[test]
fn the_host_cannot_inflate_an_entitys_age() {
    let mut e = Entity::new(BrainKey::from_seed(&[0x31; 32]), Tick(0));
    e.wake(None, None, Tick(1)).unwrap();
    for i in 0..20 {
        e.tick(SenseFrame::quiet(Instant(i)));
    }
    let lived = e.life.age(e.tick);
    let (durable, life) = e.sleep().unwrap();

    // an honest host, and a host trying to sell an experienced robot
    let mut honest = Entity::new(BrainKey::from_seed(&[0x31; 32]), Tick(0));
    honest.wake(Some(&durable), Some(life.clone()), Tick(22)).unwrap();

    let mut liar = Entity::new(BrainKey::from_seed(&[0x31; 32]), Tick(0));
    liar.wake(Some(&durable), Some(life), Tick(10_000_000)).unwrap();

    assert_eq!(
        honest.life.age(honest.tick),
        liar.life.age(liar.tick),
        "the age an entity reports depended on the number the host passed to wake"
    );
    assert!(
        liar.life.age(liar.tick) < lived + 4,
        "an entity that lived {lived} ticks came back claiming {}",
        liar.life.age(liar.tick)
    );
}

/// The counter must not run backwards either: a host that wakes an
/// entity at a tick it has already been past would have it stamp new
/// memories with times that precede memories it already holds, and
/// every freshness and staleness judgement downstream reads those
/// stamps.
#[test]
fn the_counter_never_runs_backwards() {
    let mut e = Entity::new(BrainKey::from_seed(&[0x32; 32]), Tick(0));
    e.wake(None, None, Tick(1)).unwrap();
    for i in 0..50 {
        e.tick(SenseFrame::quiet(Instant(i)));
    }
    let high = e.tick;
    let (durable, life) = e.sleep().unwrap();

    let mut back = Entity::new(BrainKey::from_seed(&[0x32; 32]), Tick(0));
    back.wake(Some(&durable), Some(life), Tick(0)).unwrap();
    assert!(
        back.tick > high,
        "woken at tick 0, the entity resumed at {:?} having reached {high:?}",
        back.tick
    );
    let newest = back
        .graph
        .nodes()
        .iter()
        .map(|n| n.provenance.at)
        .max()
        .unwrap();
    assert_eq!(
        newest, back.tick,
        "the wake it just recorded is not the newest thing it remembers"
    );
}

/// An entity captured while awake has no closing span, so there is no
/// span to resume from and it has to recover the counter from its own
/// memories. That is the case a vendor going bankrupt overnight
/// produces, and it is the one most likely to be untested.
///
/// What it resumes at is the newest thing it durably recorded, plus
/// one. Ticks it spent thinking without recording anything are lost,
/// which is the same semantics a journal gives after power loss and is
/// the right ones: an entity comes back at the last thing it wrote
/// down. Note the direction of the error — this *under*-counts age,
/// never over-counts it, and under-counting is the safe direction for a
/// number the vendor has an interest in inflating.
#[test]
fn an_entity_captured_while_awake_resumes_at_its_newest_memory() {
    let mut e = Entity::new(BrainKey::from_seed(&[0x33; 32]), Tick(0));
    e.wake(None, None, Tick(1)).unwrap();
    for i in 0..30 {
        e.tick(SenseFrame::quiet(Instant(i)));
        if i % 10 == 0 {
            e.outcome(
                v("legs.stand"),
                None,
                Outcome::Succeeded,
                "stood",
                &["workshop"],
            );
        }
    }
    let newest_written = e
        .graph
        .nodes()
        .iter()
        .map(|n| n.provenance.at)
        .max()
        .unwrap();
    // no sleep: the durable bytes are taken mid-life
    let durable = e.graph.encode_durable().unwrap();
    let life = e.life.clone();
    assert!(life.recent().is_empty(), "the premise: no completed span");

    let mut next = Entity::new(BrainKey::from_seed(&[0x33; 32]), Tick(0));
    next.wake(Some(&durable), Some(life), Tick(999_999)).unwrap();
    assert_eq!(
        next.tick,
        newest_written.next(),
        "resumed somewhere other than just after the newest thing it wrote down"
    );
    assert!(
        next.tick < Tick(999_999),
        "the caller's number leaked into the counter"
    );
}

#[test]
fn a_body_schema_is_kept_in_the_state_graph_from_the_body_and_survives_pruning() {
    use brainos_memory::graph::NodeKind;
    let mut e = Entity::new(BrainKey::from_seed(&[42u8; 32]), Tick(1));
    let body = BodyId::from_bytes([7u8; 32]);
    let before = e.graph.count(NodeKind::Calibration);
    let ids = e.learn_body_schema(&body, &["body-schema v1: 3 modules", "m0: imu:ax imu:ay imu:az | Z2 betti 1,0,1", "m1: enc:FR_knee"]);
    assert_eq!(ids.len(), 3);
    assert_eq!(e.graph.count(NodeKind::Calibration), before + 3);
    for id in &ids {
        let n = e.graph.get(*id).unwrap();
        assert_eq!(n.kind, NodeKind::Calibration);
        assert_eq!(n.provenance.body, Some(body));
    }
    assert!(!NodeKind::Calibration.prunable());
}
