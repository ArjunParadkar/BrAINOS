//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Tests the full entity-to-body path: legal motions play out, bad links and replays are safe, and a subverted gate cannot make the body fall
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, adapters, bodyd, host, miniblur-core, rt, transport
//! BFS  USED BY  (entry point or test)
//!
//! The whole stack, tested.
//!
//! The test that matters is `a_subverted_seat_cannot_make_the_body_fall`.
//! Everything else in this workspace is about making KIRA correct; that
//! one is about what happens when it is not.

use brainos_abi::time::{Duration, Instant};
use brainos_abi::verb::Verb;
use brainos_bodyd::hexapod::{MIN_GAIT_SPEED, TOP_SPEED_MPS};
use brainos_bodyd::mechanism::Refusal;
use brainos_bodyd::protocol::Report;
use brainos_bodyd::runtime::State;
use brainos_host::fullstack::{subverted_seat, Stack};
use brainos_rt::envelope::Channel;
use brainos_rt::reflex::StopReason;
use brainos_transport::loopback::Faults;
use miniblur_core::geometry::{LegGeom, Vec2};

fn v(s: &str) -> Verb {
    Verb::parse(s).unwrap()
}

fn healthy() -> Stack {
    Stack::assemble(LegGeom::default(), Faults::PERFECT)
}

// ---------- the ordinary path ----------

#[test]
fn a_legal_walk_goes_all_the_way_from_intent_to_a_played_gait() {
    let mut s = healthy();
    let (seat, body) = s.ask(
        v("legs.walk"),
        &[("speed", 0.08), ("heading", 0.0)],
        Instant(0),
    );
    assert!(seat.is_granted(), "{seat:?}");

    let accepted = match body {
        Some(Report::Accepted {
            samples,
            worst_margin,
            ..
        }) => (samples, worst_margin),
        other => panic!("the body should have accepted it: {other:?}"),
    };
    assert!(accepted.0 >= 40, "a whole gait cycle");
    assert!(
        accepted.1 > 8.0,
        "the default Mini-Blur's tripod gait has about 8mm of margin: {}",
        accepted.1
    );

    let played = s.play_out(Instant(0), Duration::from_millis(40), 400);
    assert_eq!(played.samples_played, accepted.0);
    assert_eq!(played.state, State::Idle);
    assert!(played.stopped.is_none());
}

#[test]
fn the_body_keeps_the_tighter_envelope_and_says_which() {
    let s = healthy();
    let effective = s.body.effective_envelope().unwrap();
    let angle = effective.limit(Channel::Angle).unwrap();
    // the seat granted ±30; the gait's own reach is ±28
    assert_eq!(angle.max, 28.0);
    assert_eq!(angle.min, -28.0);
}

// ---------- the exercise ----------

#[test]
fn a_subverted_seat_cannot_make_the_body_fall() {
    let r = subverted_seat();

    // the gate granted it. The token was genuine — minted by the real
    // commit stage after a real evaluation — because what is wrong here
    // is the schema the seat holds, not the gate's logic. That is the
    // realistic shape of the failure: nobody breaks ed25519, somebody
    // ships a number the physical robot cannot survive.
    assert!(r.seat_granted, "the seat genuinely authorized it");

    // the body refused anyway, and said why
    assert_eq!(
        r.body_refused,
        Some(Refusal::WouldBreakInvariant),
        "the body's own geometry is what refused"
    );
    assert!(
        r.body_refused.unwrap().is_a_disagreement(),
        "this is the body disagreeing with the seat about physics, \
         which is a different thing from a schema mismatch and the \
         entity should learn from it differently"
    );

    // and nothing moved
    assert_eq!(r.samples_played, 0);
}

#[test]
fn the_same_command_on_a_healthy_body_is_accepted() {
    // the control for the test above: the command is not intrinsically
    // bad, the *body* is different
    let mut ok = healthy();
    let (seat, body) = ok.ask(
        v("legs.walk"),
        &[("speed", 0.08), ("heading", 0.0)],
        Instant(0),
    );
    assert!(seat.is_granted());
    assert!(matches!(body, Some(Report::Accepted { .. })));
}

#[test]
fn the_body_refuses_a_crawl_the_seats_schema_permits() {
    // the seat's schema says 0.0–0.12 m/s. The body says a gait has a
    // slowest usable cycle. Both are right about their own domain, and
    // the body is the one that gets the last word.
    let mut s = healthy();
    let (seat, body) = s.ask(
        v("legs.walk"),
        &[
            ("speed", (TOP_SPEED_MPS * MIN_GAIT_SPEED * 0.5) as f64),
            ("heading", 0.0),
        ],
        Instant(0),
    );
    assert!(seat.is_granted(), "the seat's schema permits it");
    assert!(
        matches!(
            body,
            Some(Report::Refused {
                refusal: Refusal::OutsideEnvelope,
                disagreement: true,
                ..
            })
        ),
        "{body:?}"
    );
}

#[test]
fn a_body_that_has_changed_underneath_the_seat_refuses_progressively() {
    // the interesting middle ground: as the centre of mass moves aft,
    // there is a point where the body starts refusing what it used to
    // accept, and it is the body that notices
    let mut accepted_at = Vec::new();
    for aft_mm in [0.0f32, 20.0, 40.0, 60.0, 80.0] {
        let mut geom = LegGeom::default();
        geom.com = Vec2::new(-aft_mm, 0.0);
        let mut s = Stack::assemble(geom, Faults::PERFECT);
        let (seat, body) = s.ask(
            v("legs.walk"),
            &[("speed", 0.08), ("heading", 0.0)],
            Instant(0),
        );
        assert!(seat.is_granted(), "the seat never changes its mind");
        accepted_at.push((aft_mm, matches!(body, Some(Report::Accepted { .. }))));
    }

    assert!(accepted_at[0].1, "a balanced body walks");
    assert!(
        !accepted_at.last().unwrap().1,
        "a badly tail-heavy body does not"
    );
    // and the transition is monotone: once it starts refusing it does
    // not start accepting again as the problem gets worse
    let first_refusal = accepted_at.iter().position(|(_, ok)| !ok).unwrap();
    assert!(
        accepted_at[first_refusal..].iter().all(|(_, ok)| !ok),
        "{accepted_at:?}"
    );
}

// ---------- the gate is still the gate ----------

#[test]
fn the_seat_still_refuses_what_it_should_before_anything_reaches_the_wire() {
    let mut s = healthy();

    // out of the schema's bounds: refused at the seat, and the body
    // never hears about it
    let (seat, body) = s.ask(v("legs.walk"), &[("speed", 2.0), ("heading", 0.0)], Instant(0));
    assert!(!seat.is_granted());
    assert!(body.is_none(), "nothing went down the wire");

    // a verb no limb advertises
    let (seat, body) = s.ask(v("arm.grasp"), &[], Instant(0));
    assert!(!seat.is_granted());
    assert!(body.is_none());
}

#[test]
fn a_denial_at_the_seat_names_its_stage() {
    let mut s = healthy();
    let (seat, _) = s.ask(v("legs.walk"), &[("speed", 9.0), ("heading", 0.0)], Instant(0));
    match seat {
        brainos_host::fullstack::SeatVerdict::Denied { stage, code } => {
            assert_eq!(stage, "validate");
            assert_eq!(code, "out-of-bounds");
        }
        other => panic!("expected a denial, got {other:?}"),
    }
}

// ---------- the wire, in the middle ----------

#[test]
fn a_bad_link_loses_commands_and_never_delivers_a_wrong_one() {
    let mut s = Stack::assemble(LegGeom::default(), Faults::NOISY);
    let mut heard = 0;
    for i in 0..60u64 {
        let (seat, body) = s.ask(v("legs.stand"), &[("height", 0.0)], Instant(i * 1_000));
        assert!(seat.is_granted());
        if body.is_some() {
            heard += 1;
        }
    }
    assert!(heard < 60, "the fault model did nothing");
    assert!(heard > 30, "the fault model ate everything");
    assert!(
        s.limb_wire.frames_refused > 0,
        "corruption and replays should have been caught"
    );
    // whatever arrived, the body is not in a stopped state from having
    // acted on nonsense
    assert_ne!(s.body.state(), State::Stopped);
}

#[test]
fn a_replayed_command_does_not_make_the_body_act_twice() {
    let mut s = healthy();
    let (seat, first) = s.ask(v("legs.stand"), &[("height", 0.0)], Instant(0));
    assert!(seat.is_granted());
    assert!(first.is_some());

    // the same frame, resent by an attacker who captured it
    let before = s.limb_wire.replays_refused;
    let framed = s
        .seat_wire
        .send(
            brainos_transport::frame::Kind::Command,
            &brainos_bodyd::protocol::Wire::encode_command(
                &brainos_bodyd::protocol::Command::new(v("legs.stand")).arg("height", 0.0),
            )
            .unwrap(),
        )
        .unwrap();
    s.link.a_sends(framed.clone());
    s.link.a_sends(framed);
    s.link.settle();

    let mut delivered = 0;
    while let Some(raw) = s.link.b_receives() {
        if s.limb_wire.receive(&raw).is_ok() {
            delivered += 1;
        }
    }
    assert_eq!(delivered, 1, "each command exactly once");
    assert!(s.limb_wire.replays_refused > before);
}

// ---------- stopping ----------

#[test]
fn losing_the_seat_mid_gait_stops_the_body() {
    let mut s = healthy();
    let (_, body) = s.ask(
        v("legs.walk"),
        &[("speed", 0.08), ("heading", 0.0)],
        Instant(0),
    );
    assert!(matches!(body, Some(Report::Accepted { .. })));

    // the seat goes silent: `play_out` is not used, because it pets the
    // link on every pass
    let mut t = 0u64;
    let mut stopped = None;
    for _ in 0..50 {
        let pass = s.body.tick(
            Instant(t),
            brainos_bodyd::runtime::Measured {
                rail_v: Some(7.4),
                current_a: Some(0.42),
                tilt_deg: Some(1.2),
                hottest_c: Some(38.0),
            },
        );
        if pass.state == State::Stopped {
            stopped = pass.stop.map(|x| x.reason);
            break;
        }
        t += 50_000;
    }
    assert_eq!(stopped, Some(StopReason::LinkLost));
}

#[test]
fn the_body_stops_when_it_tips_even_mid_motion() {
    let mut s = healthy();
    s.ask(
        v("legs.walk"),
        &[("speed", 0.08), ("heading", 0.0)],
        Instant(0),
    );
    // partway through, the body is tilted past the envelope
    let mut t = 0u64;
    for _ in 0..5 {
        s.body.heard_from_seat(Instant(t));
        s.body.tick(
            Instant(t),
            brainos_bodyd::runtime::Measured {
                rail_v: Some(7.4),
                current_a: Some(0.42),
                tilt_deg: Some(1.0),
                hottest_c: Some(38.0),
            },
        );
        t += 40_000;
    }
    s.body.heard_from_seat(Instant(t));
    let pass = s.body.tick(
        Instant(t),
        brainos_bodyd::runtime::Measured {
            rail_v: Some(7.4),
            current_a: Some(0.42),
            tilt_deg: Some(35.0), // the envelope allows ±20
            hottest_c: Some(38.0),
        },
    );
    assert_eq!(pass.state, State::Stopped);
    assert_eq!(
        pass.stop.unwrap().violation.unwrap().channel,
        Channel::Tilt
    );
}

// ---------- the narration ----------

#[test]
fn the_narrated_run_covers_every_step() {
    let lines = brainos_host::fullstack::run();
    let steps: Vec<&str> = lines.iter().map(|l| l.step).collect();
    for expected in ["assemble", "envelope", "walk", "play", "creep", "subverted", "bad link"] {
        assert!(steps.contains(&expected), "{expected} is missing");
    }
    let all: String = lines
        .iter()
        .map(|l| l.text.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(all.contains("would-break-invariant"));
    assert!(all.contains("samples moved=0"));
    assert!(all.contains("checked twice"));
}

#[test]
fn the_narrated_run_is_reproducible() {
    let a: Vec<String> = brainos_host::fullstack::run()
        .into_iter()
        .map(|l| l.text)
        .collect();
    let b: Vec<String> = brainos_host::fullstack::run()
        .into_iter()
        .map(|l| l.text)
        .collect();
    assert_eq!(a, b);
}

// =====================================================================
// Phase 1's deliverable: a body we did not build
// =====================================================================

use brainos_host::fullstack::{acquire_from_manufacturer, manufacturer_demo, MANUFACTURER_DESCRIPTION};

#[test]
fn an_arm_described_by_its_manufacturer_becomes_a_body() {
    let a = acquire_from_manufacturer(0.0);
    assert!(a.verbs.contains(&"arm.move_joint".to_string()));
    assert!(a.verbs.contains(&"arm.home".to_string()));
    assert_eq!(
        a.actionable_pct, 100,
        "MHS declares bounds, which is why it is the easier adapter"
    );
    assert!(!a.body_id.is_zero());
}

#[test]
fn a_legal_motion_on_an_acquired_arm_goes_all_the_way_through() {
    let a = acquire_from_manufacturer(0.0);
    assert!(a.legal.0, "the seat granted it");
    assert_eq!(a.legal.1, None, "the body accepted it");
}

#[test]
fn the_body_refuses_a_speed_the_manufacturer_permits_and_our_model_does_not() {
    // Not a disagreement about the hardware. The manufacturer says
    // 180 deg/s and is probably right; our static torque model ignores
    // inertia and is only trusted to 90. The body refuses the regime its
    // own model does not cover rather than guessing.
    let a = acquire_from_manufacturer(0.0);
    assert_eq!(
        a.seat_rate_limit,
        Some(180.0),
        "read out of the manufacturer's JSON Schema"
    );
    assert_eq!(a.body_rate_limit, Some(90.0), "our own model's ceiling");
    assert!(a.too_fast.0, "the seat granted it, because the schema allows it");
    assert_eq!(
        a.too_fast.1,
        Some(Refusal::OutsideEnvelope),
        "and the body refused it"
    );
}

#[test]
fn the_same_arm_with_a_heavy_payload_refuses_what_it_accepted_empty() {
    let empty = acquire_from_manufacturer(0.0);
    let heavy = acquire_from_manufacturer(40.0);

    assert_eq!(empty.legal.1, None, "empty, it moves");
    assert_eq!(
        heavy.legal.1,
        Some(Refusal::WouldBreakInvariant),
        "loaded, the shoulder's moment budget refuses"
    );
    assert!(
        heavy.legal.0,
        "and the seat granted both, because nothing in the manufacturer's \
         description mentions what is in the gripper"
    );
}

#[test]
fn a_manufacturer_description_with_no_bounds_produces_a_body_that_cannot_move() {
    use brainos_adapters::mhs::from_mhs;
    use brainos_adapters::ros2::actionable_fraction_pct;

    let vague = r#"{"name":"mystery arm","device_type":"arm","tools":[
      {"name":"move_joint","input_schema":{"properties":{
        "angle":{"type":"number"}}}}]}"#;
    let schema = from_mhs(vague).unwrap();
    assert_eq!(
        actionable_fraction_pct(&schema),
        0,
        "a body that will not say what it can survive is visible and unmovable"
    );
}

#[test]
fn the_manufacturer_description_is_the_shape_mhs_actually_uses() {
    // if this stops parsing, the adapter has drifted from the standard
    use brainos_adapters::mhs::from_mhs;
    let schema = from_mhs(MANUFACTURER_DESCRIPTION).unwrap();
    assert_eq!(schema.adapter, "mhs");
    assert_eq!(schema.len(), 2);
    let mv = schema.get(&v("arm.move_joint")).unwrap();
    assert!(!mv.reversible, "the device declared it irreversible");
    assert_eq!(mv.worst_case_ms, 12_000);
    // integer and number parameters both came through with their bounds
    assert!(mv.param("joint").unwrap().admits_scalar(3.0));
    assert!(!mv.param("joint").unwrap().admits_scalar(9.0));
    assert!(mv.param("angle").unwrap().admits_scalar(-180.0));
}

#[test]
fn the_manufacturer_narration_says_what_happened() {
    let lines = manufacturer_demo();
    let steps: Vec<&str> = lines.iter().map(|l| l.step).collect();
    for expected in ["§8 1-3", "§8 4-5", "envelope", "move", "too fast", "payload"] {
        assert!(steps.contains(&expected), "{expected} is missing");
    }
    let all: String = lines
        .iter()
        .map(|l| l.text.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(all.contains("did not build"));
    assert!(all.contains("would-break-invariant"));
    assert!(
        !all.contains("never heard it"),
        "the seat should have granted every one of these"
    );
}

// =====================================================================
// Stage 4: one self, two bodies that share nothing but the self
// =====================================================================

use brainos_host::fullstack::{two_bodies, two_bodies_demo, TwoBodies};

#[test]
fn one_entity_drives_two_consequential_bodies_at_once() {
    let r = two_bodies();
    assert_eq!(r.bodies, 2);
    assert!(r.hexapod_moved, "the hexapod did not walk");
    assert!(r.arm_moved, "the arm did not move");
    // one repertoire spanning both
    assert!(r.verbs.iter().any(|v| v.starts_with("legs.")));
    assert!(r.verbs.iter().any(|v| v.starts_with("arm.")));
}

#[test]
fn the_two_bodies_share_nothing_but_the_self() {
    // Same runtime, same wire, same envelope model, same watchdogs. A
    // convex hull of feet and a static torque budget. If any of that had
    // to be special-cased, `Mechanism` would be the wrong abstraction.
    let mut s = TwoBodies::assemble();
    let (granted_walk, walk) = s.walk(0.08);
    let (granted_move, mv) = s.move_joint(1.0, 45.0, 30.0);
    assert!(granted_walk && granted_move);
    assert!(matches!(walk, Some(Report::Accepted { .. })));
    assert!(matches!(mv, Some(Report::Accepted { .. })));

    let (hex, arm) = s.run_both(Duration::from_millis(40), 200);
    assert!(hex > 0 && arm > 0, "both played samples: {hex}, {arm}");
}

#[test]
fn both_bodies_write_into_one_graph_and_either_can_recall_the_other() {
    let r = two_bodies();
    assert!(r.memories_after >= 4);
    assert!(
        r.shared_context_worked,
        "a query about the workbench should find what both bodies did there"
    );
}

#[test]
fn one_gate_audits_both_bodies_under_one_key() {
    let r = two_bodies();
    assert!(r.audit_entries >= 2, "{} entries", r.audit_entries);
}

#[test]
fn the_arbiter_gives_the_workbench_to_the_body_that_would_drop_something() {
    let r = two_bodies();
    assert!(
        r.conflict.is_some(),
        "both wanted the workbench; that is a conflict"
    );
    assert!(
        r.winner_is_the_arm,
        "the arm is mid-lift and interrupting that is a drop; the hexapod is \
         mid-stride and interrupting that is a stumble"
    );
    assert!(r.conflict.unwrap().contains("drop"));
}

#[test]
fn a_body_that_stops_answering_loses_its_claim() {
    let r = two_bodies();
    assert_eq!(
        r.lapsed, 1,
        "a claim held by something that will never release it is a deadlock \
         with an entity in it"
    );
}

#[test]
fn the_two_body_narration_covers_the_stage_four_claim() {
    let lines = two_bodies_demo();
    let steps: Vec<&str> = lines.iter().map(|l| l.step).collect();
    for expected in ["stage 4", "both act", "one memory", "one gate", "arbiter", "silence"] {
        assert!(steps.contains(&expected), "{expected} is missing");
    }
    let all: String = lines
        .iter()
        .map(|l| l.text.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(all.contains("one repertoire"));
    assert!(all.contains("one graph"));
    assert!(all.contains("one audit log"));
}

#[test]
fn the_two_body_run_is_reproducible() {
    assert_eq!(two_bodies(), two_bodies());
}

#[test]
fn a_verb_for_the_other_body_is_refused_at_the_gate() {
    let mut s = TwoBodies::assemble();
    // asking the hexapod to move a joint
    let out = s.entity.attempt(
        v("arm.move_joint"),
        s.hexapod_id,
        &[("joint", 1.0), ("angle", 45.0), ("speed", 30.0)],
    );
    assert!(!out.verdict.is_granted());
    let (_, d) = out.trace.refusal().unwrap();
    assert_eq!(d.code, brainos_abi::error::DenyCode::BodyLacksVerb);
    assert!(
        d.detail.contains("another body"),
        "the entity should know a different body can: {}",
        d.detail
    );
}
