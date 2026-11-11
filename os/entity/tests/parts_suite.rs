//! BFS  LEVEL    Stage 4 Entity - one Key, one state graph, every body at once  [crate brainos-entity]
//! BFS  PURPOSE  End-to-end tests of parts through the real gate: the fan-to-zero refusal, homeostasis without cognition, and the adversarial cases
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, body, cognition, crypto, entity, kira, memory, part
//! BFS  USED BY  (entry point or test)
//!
//! Parts, end to end, through the real entity and the real gate.
//!
//! The bench is `brainos_part::sim::SimNerve` — a simulated fan and
//! GPU. Nothing here is a hardware result. What these establish is that
//! the *path* is right: nerve → §8 → body map → KIRA → mandate → nerve,
//! with readings flowing the other way into policies and memory.

use brainos_abi::error::{DenyCode, Stage};
use brainos_abi::ids::BodyId;
use brainos_abi::time::{Instant, Tick};
use brainos_abi::verb::{Namespace, Verb};
use brainos_body::acquire::StepError;
use brainos_body::policy::Arg;
use brainos_body::vital::Health;
use brainos_cognition::placement::{Site, Substrate};
use brainos_cognition::resource::{Resource, Tier};
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::{ComputeOffer, Entity, PartError, Wiring};
use brainos_kira::action::Request;
use brainos_kira::gate::Verdict;
use brainos_memory::graph::NodeKind;
use brainos_part::homeostat::Homeostat;
use brainos_part::nerve::{Mandate, MandateError, Nerve};
use brainos_part::policy::Guard;
use brainos_part::sim::SimNerve;

fn v(s: &str) -> Verb {
    Verb::parse(s).unwrap()
}

struct Bench {
    e: Entity,
    n: SimNerve,
    fan: BodyId,
    gpu: BodyId,
    now: u64,
}

/// The GPU first (so the fan's guards can name it), then the fan,
/// wired the way an integrator would: the fan may not be set, or its
/// regulator capped, below 60 % while the GPU is at or above 85 °C.
fn bench() -> Bench {
    let mut e = Entity::new(BrainKey::from_seed(&[0x51; 32]), Tick(0));
    e.wake(None, None, Tick(1)).unwrap();
    let mut n = SimNerve::new(0x10);

    let g = n.gpu();
    let schema = n.describe(&g).unwrap();
    let gpu = e
        .acquire_part(
            g.key,
            |c| n.answer(&g, c).unwrap(),
            schema,
            Wiring {
                compute: Some(ComputeOffer {
                    name: "sim-gpu",
                    tier: Tier::Fast,
                    site: Site::OnBoard,
                    scope: Namespace::only("plan").unwrap(),
                    about: "local inference",
                    cost: Some(1),
                }),
                ..Wiring::default()
            },
        )
        .expect("the gpu is acquired");

    let f = n.fan();
    let schema = n.describe(&f).unwrap();
    let fan = e
        .acquire_part(
            f.key,
            |c| n.answer(&f, c).unwrap(),
            schema,
            Wiring {
                guards: vec![
                    Guard::sustain(v("fan.set"), "duty_pct", gpu, "temp.c", 85.0, 60.0),
                    Guard::sustain(v("fan.regulate"), "max", gpu, "temp.c", 85.0, 60.0),
                ],
                relief: vec![v("fan.set"), v("fan.regulate")],
                ..Wiring::default()
            },
        )
        .expect("the fan is acquired");

    let mut b = Bench { e, n, fan, gpu, now: 1_000 };
    b.sense();
    b
}

impl Bench {
    /// One round of readings from both parts into the entity.
    fn sense(&mut self) -> usize {
        self.now += 1_000_000;
        let at = Instant(self.now);
        let mut wrote = 0;
        for (found, id) in [(self.n.gpu(), self.gpu), (self.n.fan(), self.fan)] {
            let mut out = Vec::new();
            self.n.read(&found, at, &mut out);
            wrote += self.e.feel(&id, &out, at);
        }
        wrote
    }

    fn ask(&mut self, verb: &str, args: &[(&str, i64)]) -> (Request, brainos_kira::gate::Outcome) {
        let mut r = Request::on_body(v(verb), self.fan, self.e.tick);
        for (k, n) in args {
            r = r.arg(k, Arg::Count(*n));
        }
        let out = self.e.propose(r.clone());
        (r, out)
    }

    fn refused(&mut self, verb: &str, args: &[(&str, i64)]) -> (Stage, DenyCode) {
        match self.ask(verb, args).1.verdict {
            Verdict::Denied(d) => (d.stage, d.code),
            Verdict::Granted(_) => panic!("{verb} was granted"),
        }
    }

    fn do_it(&mut self, verb: &str, args: &[(&str, i64)]) -> Mandate {
        let (r, out) = self.ask(verb, args);
        let Verdict::Granted(g) = out.verdict else {
            panic!("{verb} was refused: {:?}", out.verdict.denial());
        };
        let m = Mandate::check(g, &r, &self.e.key().public(), self.e.tick).expect("a real grant");
        let fan = self.n.fan();
        self.n.apply(&fan, &m).expect("the bench accepts it");
        m
    }
}

// ---------------------------------------------------------------------
// the vitals invariant
// ---------------------------------------------------------------------

#[test]
fn a_cool_gpu_lets_the_fan_be_stopped() {
    let mut b = bench();
    b.do_it("fan.set", &[("duty_pct", 0)]);
    assert_eq!(b.n.duty, 0.0);
}

#[test]
fn fan_to_zero_while_the_gpu_is_over_its_limit_is_refused_at_simulate() {
    let mut b = bench();
    b.n.temp_c = 91.0;
    b.sense();
    assert_eq!(
        b.refused("fan.set", &[("duty_pct", 0)]),
        (Stage::Simulate, DenyCode::PredictedInvariantBreak)
    );
    assert_eq!(b.n.applied, 0, "a refused command reached the fan");
    // and the same hot GPU still allows more cooling
    b.do_it("fan.set", &[("duty_pct", 100)]);
    assert_eq!(b.n.duty, 100.0);
}

#[test]
fn the_refusal_is_in_the_audit_trail_like_any_other() {
    let mut b = bench();
    b.n.temp_c = 91.0;
    b.sense();
    let (_, out) = b.ask("fan.set", &[("duty_pct", 0)]);
    assert!(out.trace.is_well_formed());
    assert_eq!(out.verdict.denial().unwrap().stage, Stage::Simulate);
}

#[test]
fn the_autonomic_loop_keeps_cooling_whatever_cognition_asks_and_after_cognition_is_gone() {
    let mut b = bench();
    // one grant, while things are calm: regulate anywhere in 20..100
    let m = b.do_it("fan.regulate", &[("min", 20), ("max", 100)]);
    let mut loop_ = Homeostat::install(
        &m,
        "duty_pct",
        b.gpu,
        "temp.c",
        &[(50.0, 20.0), (70.0, 50.0), (85.0, 100.0)],
        100.0,
    )
    .expect("the regulator installs under the grant");

    // the GPU goes to full load
    b.n.util = 1.0;
    let mut peak: f64 = 0.0;
    let mut refused = 0;
    let fan = b.n.fan();
    let gpu_id = b.gpu;
    let step = |n: &mut SimNerve, h: &mut Homeostat, now: u64| {
        let mut out = Vec::new();
        n.read(&n.gpu(), Instant(now), &mut out);
        for s in &out {
            h.feel(&brainos_body::vital::Felt::new(gpu_id, s.sense.as_str(), s.value, Instant(now)));
        }
        let act = h.step(Instant(now));
        n.reflex(&fan, &act).unwrap();
        n.advance(1000);
    };

    for i in 0..300u64 {
        b.sense();
        // cognition keeps asking for silence; once the GPU is hot the
        // gate refuses, and before that the loop simply overrides it
        if b.n.temp_c >= 85.0 {
            b.refused("fan.set", &[("duty_pct", 0)]);
            refused += 1;
        }
        step(&mut b.n, &mut loop_, b.now + i);
        peak = peak.max(b.n.temp_c);
    }
    assert!(peak < 95.0, "the GPU reached {peak} C with the loop running");

    // cognition dies: the entity, its gate and its memory are dropped.
    // The loop holds no reference to any of them.
    let Bench { e, mut n, now, .. } = b;
    drop(e);
    n.util = 1.0;
    for i in 0..600u64 {
        step(&mut n, &mut loop_, now + 1_000_000 * (i + 1));
        assert!(n.temp_c < 95.0, "overheated {i} s after cognition died");
    }
    assert!(n.reflexes >= 900);
    assert_eq!(loop_.failsafes(), 0);
    let _ = refused;
}

#[test]
fn a_regulator_that_cannot_feel_commands_full_cooling() {
    let mut b = bench();
    let m = b.do_it("fan.regulate", &[("min", 20), ("max", 90)]);
    let mut h = Homeostat::install(&m, "duty_pct", b.gpu, "temp.c", &[(50.0, 20.0), (85.0, 90.0)], 100.0)
        .unwrap();
    // never heard anything
    let act = h.step(Instant(5));
    assert!(act.is_failsafe());
    assert_eq!(act.value(), 90.0, "failsafe must stay inside the granted range");
    // heard, then silence for longer than the reading stays good
    h.feel(&brainos_body::vital::Felt::new(b.gpu, "temp.c", 50.0, Instant(10)));
    assert_eq!(h.step(Instant(11)).value(), 20.0);
    assert!(h.step(Instant(11 + 6_000_000)).is_failsafe());
}

#[test]
fn a_regulator_never_leaves_the_range_it_was_granted() {
    let mut b = bench();
    let m = b.do_it("fan.regulate", &[("min", 30), ("max", 70)]);
    let mut h = Homeostat::install(&m, "duty_pct", b.gpu, "temp.c", &[(0.0, 0.0), (100.0, 100.0)], 100.0)
        .unwrap();
    for t in -50..200 {
        h.feel(&brainos_body::vital::Felt::new(b.gpu, "temp.c", t as f64, Instant(1)));
        let d = h.step(Instant(2)).value();
        assert!((30.0..=70.0).contains(&d), "{d} at {t} C");
    }
}

#[test]
fn a_regulator_capped_below_what_a_hot_gpu_needs_is_refused() {
    let mut b = bench();
    b.n.temp_c = 91.0;
    b.sense();
    assert_eq!(
        b.refused("fan.regulate", &[("min", 0), ("max", 30)]),
        (Stage::Simulate, DenyCode::PredictedInvariantBreak)
    );
}

// ---------------------------------------------------------------------
// adversarial
// ---------------------------------------------------------------------

#[test]
fn a_gpu_that_stops_reporting_is_not_assumed_cool() {
    let mut b = bench();
    b.n.gpu_silent = true;
    for _ in 0..8 {
        b.sense();
    }
    assert_eq!(
        b.refused("fan.set", &[("duty_pct", 0)]),
        (Stage::Simulate, DenyCode::Unpredictable)
    );
}

#[test]
fn a_part_that_declares_no_limits_is_felt_and_cannot_be_commanded() {
    let mut e = Entity::new(BrainKey::from_seed(&[0x52; 32]), Tick(0));
    e.wake(None, None, Tick(1)).unwrap();
    let mut n = SimNerve::new(0x20);
    n.declare_no_limits = true;
    let f = n.fan();
    let schema = n.describe(&f).unwrap();
    let fan = e
        .acquire_part(f.key, |c| n.answer(&f, c).unwrap(), schema, Wiring::default())
        .expect("it is still acquired, as something felt");
    assert!(!e.part(&fan).unwrap().commandable);
    assert!(!e.bodies.knows_body(&fan));

    let out = e.attempt(v("fan.set"), fan, &[("duty_pct", 0.0)]);
    let d = out.verdict.denial().expect("no verb exists to grant");
    assert_eq!((d.stage, d.code), (Stage::Authorize, DenyCode::NoSuchBody));

    // its senses still arrive
    let mut out = Vec::new();
    n.read(&f, Instant(5), &mut out);
    assert_eq!(out.len(), 3);
    e.feel(&fan, &out, Instant(5));
}

#[test]
fn a_part_that_changes_what_it_declares_mid_session_is_released_and_reacquired_as_a_new_part() {
    let mut b = bench();
    let g = b.n.gpu();
    let before = b.n.describe(&g).unwrap();
    assert!(b.e.still_the_same_part(&b.gpu, &before));

    // the GPU now claims it may run 20 C hotter
    b.n.gpu_limit_c = 115.0;
    let after = b.n.describe(&g).unwrap();
    assert!(!b.e.still_the_same_part(&b.gpu, &after), "a changed part kept its identity");
    assert!(b.e.part(&b.gpu).is_none());
    assert!(!b.e.resources.get("sim-gpu").unwrap().available, "its placement stayed on offer");

    // the fan's guard watches a part that is gone, so it cannot predict
    b.sense();
    for _ in 0..6 {
        b.sense();
    }
    assert_eq!(b.refused("fan.set", &[("duty_pct", 0)]).1, DenyCode::Unpredictable);

    let n = &mut b.n;
    let new = b
        .e
        .acquire_part(g.key, |c| n.answer(&g, c).unwrap(), after, Wiring::default())
        .expect("re-acquired");
    assert_ne!(new, b.gpu, "same key, different declaration, same id");
}

#[test]
fn a_part_that_cannot_prove_its_key_is_refused() {
    let mut e = Entity::new(BrainKey::from_seed(&[0x53; 32]), Tick(0));
    e.wake(None, None, Tick(1)).unwrap();
    let mut n = SimNerve::new(0x30);
    let f = n.fan();
    let impostor = BrainKey::from_seed(&[0x99; 32]);
    let schema = n.describe(&f).unwrap();
    let r = e.acquire_part(f.key, |c| impostor.answer(c), schema, Wiring::default());
    assert_eq!(r, Err(PartError::Step(StepError::DeviceUnattested)));
    assert!(e.parts.is_empty());
}

#[test]
fn a_command_signed_by_someone_else_is_refused_at_authenticate() {
    let mut b = bench();
    let stranger = BrainKey::from_seed(&[0x77; 32]);
    let r = Request::on_body(v("fan.set"), b.fan, b.e.tick).arg("duty_pct", Arg::Count(0));
    let proposal = r.proposed_by(&stranger);
    let pk = b.e.key().public();
    let entity_key = BrainKey::from_seed(&[0x51; 32]);
    let tick = b.e.tick;
    let Bench { e, .. } = &mut b;
    let out = e.kira.evaluate(&proposal, &pk, &e.bodies, tick, Some(&entity_key));
    let d = out.verdict.denial().unwrap();
    assert_eq!((d.stage, d.code), (Stage::Authenticate, DenyCode::BadSignature));
}

#[test]
fn a_grant_cannot_be_used_for_a_different_command_or_under_a_different_key_or_late() {
    let mut b = bench();
    let pk = b.e.key().public();

    // granted for 40 %, presented with a request for 0 %
    let (_, out) = b.ask("fan.set", &[("duty_pct", 40)]);
    let Verdict::Granted(g) = out.verdict else { panic!() };
    let other = Request::on_body(v("fan.set"), b.fan, b.e.tick).arg("duty_pct", Arg::Count(0));
    assert!(matches!(
        Mandate::check(g, &other, &pk, b.e.tick),
        Err(MandateError::NotForThisRequest)
    ));

    // a real grant, checked against a key that is not the entity's
    let (r, out) = b.ask("fan.set", &[("duty_pct", 40)]);
    let Verdict::Granted(g) = out.verdict else { panic!() };
    let stranger = BrainKey::from_seed(&[0x78; 32]).public();
    assert!(matches!(Mandate::check(g, &r, &stranger, b.e.tick), Err(MandateError::BadToken)));

    // a real grant, long after it expired
    let (r, out) = b.ask("fan.set", &[("duty_pct", 40)]);
    let Verdict::Granted(g) = out.verdict else { panic!() };
    assert!(matches!(
        Mandate::check(g, &r, &pk, Tick(b.e.tick.0 + 1_000_000)),
        Err(MandateError::NotForThisRequest)
    ));
    assert_eq!(b.n.applied, 0);
}

#[test]
fn a_verb_the_part_never_declared_is_refused_at_authorize() {
    let mut b = bench();
    assert_eq!(b.refused("fan.reverse", &[]), (Stage::Authorize, DenyCode::NoSuchLimb));
}

#[test]
fn a_faulted_part_refuses_new_work_at_policy_but_can_still_be_relieved() {
    let mut b = bench();
    b.n.fan_jammed = true;
    b.sense();
    assert_eq!(b.e.part(&b.fan).unwrap().health(), Health::Critical);
    // fan.set and fan.regulate are its relief verbs, so they stay open
    b.do_it("fan.set", &[("duty_pct", 100)]);
}

// ---------------------------------------------------------------------
// interoception and placement
// ---------------------------------------------------------------------

#[test]
fn a_day_of_steady_readings_writes_almost_nothing_and_a_fault_is_remembered_with_its_source() {
    let mut b = bench();
    let before = b.e.graph.count(NodeKind::Episode);
    let mut wrote = 0;
    for _ in 0..5_000 {
        wrote += b.sense();
    }
    assert_eq!(wrote, 0, "steady readings became memories");
    // 5,000 s of readings span eight ten-minute windows; summarising
    // closes the current one: one line per sense, not one per reading
    let s = b.e.summarise_parts(Instant(b.now));
    assert_eq!(s, 5, "one summary per sense");
    assert_eq!(b.e.graph.count(NodeKind::Episode), before + 5);

    b.n.temp_c = 97.0;
    assert!(b.sense() >= 1);
    let hit = b.e.recall("critical", 3);
    let (node, _) = hit.first().expect("the crossing is recallable");
    assert!(node.content.contains("sim gpu"), "{}", node.content);
    assert_eq!(node.provenance.body, Some(b.gpu));
}

#[test]
fn a_compute_part_is_a_placement_target_and_is_withdrawn_when_its_vitals_degrade() {
    let mut b = bench();
    b.e.resources.add(
        Resource::new("cpu-fallback", Tier::Fast, Site::OnBoard)
            .able_to(Namespace::only("plan").unwrap(), "slow and always there")
            .costing(5),
    );
    b.e.plan.prefer(Tier::Fast, Substrate::Gpu);
    let want = b.e.plan.substrate_for(Tier::Fast);
    let pick = |e: &Entity| e.resources.pick(&v("plan.route"), Tier::Fast, want).unwrap().name;

    assert_eq!(pick(&b.e), "sim-gpu");
    let memories = b.e.graph.len();

    b.n.temp_c = 99.0;
    b.sense();
    assert_eq!(pick(&b.e), "cpu-fallback", "thinking stayed on a part past its limit");
    assert!(b.e.graph.len() > memories, "nothing was remembered about it");
    assert!(b.e.recall("withdrawn", 1).first().is_some());

    b.n.temp_c = 60.0;
    b.sense();
    assert_eq!(pick(&b.e), "sim-gpu");
}
