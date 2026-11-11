//! BFS  LEVEL    Domain 3 KIRA - the eight-stage gate; the only path to action  [crate brainos-kira]
//! BFS  PURPOSE  Tests for the gate, including attacks (destructive commands, missing limbs, forged keys), each of which must be refused at the correct stage
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, body, crypto, kira, rt
//! BFS  USED BY  (entry point or test)
//!
//! The KIRA suite, including the adversarial cases.
//!
//! Stage 1's original adversarial suite injected `rm -rf`, phantom
//! limbs, forged keys and corrupted memory, and asserted that each was
//! caught *at the correct stage*. That discipline is kept: it is not
//! enough that a bad action is refused, it must be refused by the layer
//! whose job it is, because a refusal from the wrong layer means the
//! right layer has a hole that something else happened to cover.

use brainos_abi::error::{DenyCode, Stage};
use brainos_abi::ids::{BodyId, RegionId, SessionId};
use brainos_abi::time::{Duration, Instant, Tick};
use brainos_abi::verb::{Namespace, Verb};
use brainos_body::acquire::{acquire_native, default_envelope};
use brainos_body::calibration::{Calibration, JointTrim};
use brainos_body::hal::Telemetry;
use brainos_body::policy::{Arg, BodyPolicy, Posture, SchemaPolicy};
use brainos_body::region::{BodyMap, RegionClass};
use brainos_body::schema::{CapabilitySchema, ParamSpec, VerbSpec};
use brainos_crypto::key::BrainKey;
use brainos_crypto::rng::Drbg;
use brainos_kira::action::{Action, Request};
use brainos_kira::audit::Decision;
use brainos_kira::capability::{Caveat, Delegation};
use brainos_kira::floors;
use brainos_kira::gate::{Kira, Verdict, REQUEST_WINDOW};
use brainos_kira::rules::{Level, Rule};

fn v(s: &str) -> Verb {
    Verb::parse(s).unwrap()
}

fn schema() -> CapabilitySchema {
    CapabilitySchema::new("mini-blur", "native")
        .with(
            VerbSpec::new(v("legs.walk"), RegionClass::Legs, "walk")
                .with(ParamSpec::scalar("speed", brainos_rt::envelope::Channel::Speed, 0.0, 0.6))
                .with(ParamSpec::scalar("heading", brainos_rt::envelope::Channel::Angle, -30.0, 30.0)),
        )
        .with(
            VerbSpec::new(v("voice.say"), RegionClass::Speaker, "speak")
                .with(ParamSpec::text("text", 240)),
        )
        .with(
            VerbSpec::new(v("fs.write"), RegionClass::Storage, "write a file")
                .with(ParamSpec::text("path", 240))
                .with(ParamSpec::text("content", 4000)),
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

/// A fully-bound entity with one body, ready to be asked for things.
struct Rig {
    key: BrainKey,
    kira: Kira,
    map: BodyMap,
    body: BodyId,
}

fn rig() -> Rig {
    let key = BrainKey::from_seed(&[1; 32]);
    let device = BrainKey::from_seed(&[2; 32]);
    let mut map = BodyMap::new();
    let mut rng = Drbg::deterministic(b"rig");
    let mut n = 0u8;
    let (body, _) = acquire_native(
        key.entity_id(),
        SessionId::from_bytes([7; 32]),
        &key,
        &mut rng,
        &device,
        schema(),
        &mut map,
        calibration(),
        6,
        || {
            n += 1;
            RegionId::from_bytes([n; 32])
        },
    )
    .unwrap();

    let mut kira = Kira::new(key.entity_id());
    let env = default_envelope(
        &schema(),
        RegionClass::Legs,
        1,
        Duration::from_millis(10),
        Duration::from_millis(200),
    );
    let mut policy = SchemaPolicy::new(body, schema())
        .with_envelope(env)
        .calibrated({
            let mut c = calibration();
            c.body = body;
            c
        });
    policy.posture = Posture::Standing;
    policy.observe(&Telemetry {
        at: Instant(0),
        link_alive: true,
        rail_v: Some(7.4),
        current_a: Some(0.3),
        tilt_deg: Some(1.0),
        hottest_c: Some(35.0),
        stops: 0,
        overruns: 0,
    });
    kira.bind(body, Box::new(policy), schema());

    Rig {
        key,
        kira,
        map,
        body,
    }
}

fn walk(r: &Rig, speed: f64, tick: Tick) -> brainos_kira::action::Proposal {
    Request::on_body(v("legs.walk"), r.body, tick)
        .arg("speed", Arg::Scalar(speed))
        .arg("heading", Arg::Scalar(0.0))
        .because("the human asked me to go to the door")
        .proposed_by(&r.key)
}

// ---------- the happy path ----------

#[test]
fn a_legal_action_passes_all_eight_stages_and_mints_a_token() {
    let mut r = rig();
    let p = walk(&r, 0.3, Tick(10));
    let out = r
        .kira
        .evaluate(&p, &r.key.public(), &r.map, Tick(10), Some(&r.key));

    assert!(out.verdict.is_granted(), "{:?}", out.trace.refusal());
    assert!(out.trace.passed_everything());
    assert!(out.trace.is_well_formed());
    for (i, e) in out.trace.entries().iter().enumerate() {
        assert_eq!(e.stage, Stage::ALL[i]);
        assert!(e.outcome.is_pass(), "{:?} did not pass", e.stage);
    }
    match out.verdict {
        Verdict::Granted(g) => {
            assert!(g.verify(&r.key.public()));
            assert!(g.authorizes(&p.request.digest(), Tick(10)));
            assert_eq!(g.verb, v("legs.walk"));
        }
        _ => unreachable!(),
    }
}

#[test]
fn a_token_does_not_authorize_a_different_request() {
    let mut r = rig();
    let granted = match r
        .kira
        .evaluate(&walk(&r, 0.3, Tick(10)), &r.key.public(), &r.map, Tick(10), Some(&r.key))
        .verdict
    {
        Verdict::Granted(g) => g,
        _ => panic!("expected a grant"),
    };
    let other = walk(&r, 0.5, Tick(10));
    assert!(!granted.authorizes(&other.request.digest(), Tick(10)));
}

#[test]
fn a_token_expires() {
    let mut r = rig();
    r.kira.default_ttl = 5;
    let granted = match r
        .kira
        .evaluate(&walk(&r, 0.3, Tick(10)), &r.key.public(), &r.map, Tick(10), Some(&r.key))
        .verdict
    {
        Verdict::Granted(g) => g,
        _ => panic!("expected a grant"),
    };
    assert!(granted.valid_at(Tick(15)));
    assert!(!granted.valid_at(Tick(16)));
}

// ---------- adversarial: each refusal at its own stage ----------

#[test]
fn a_forged_signature_dies_at_authenticate() {
    let mut r = rig();
    let impostor = BrainKey::from_seed(&[99; 32]);
    let p = Request::on_body(v("legs.walk"), r.body, Tick(10))
        .arg("speed", Arg::Scalar(0.3))
        .arg("heading", Arg::Scalar(0.0))
        .proposed_by(&impostor); // signed by someone else

    let out = r
        .kira
        .evaluate(&p, &r.key.public(), &r.map, Tick(10), Some(&r.key));
    let (stage, d) = out.trace.refusal().unwrap();
    assert_eq!(stage, Stage::Authenticate);
    assert_eq!(d.code, DenyCode::BadSignature);
    // and nothing downstream ran
    assert!(out.trace.at(Stage::Authorize).is_skipped());
    assert!(out.trace.at(Stage::Simulate).is_skipped());
    assert!(out.trace.at(Stage::Commit).is_skipped());
}

#[test]
fn a_replayed_request_dies_at_authenticate() {
    let mut r = rig();
    let p = walk(&r, 0.3, Tick(10));
    // the same proposal, presented much later
    let out = r.kira.evaluate(
        &p,
        &r.key.public(),
        &r.map,
        Tick(10 + REQUEST_WINDOW + 1),
        Some(&r.key),
    );
    let (stage, d) = out.trace.refusal().unwrap();
    assert_eq!(stage, Stage::Authenticate);
    assert_eq!(d.code, DenyCode::StaleRequest);
}

#[test]
fn a_request_from_the_future_is_as_suspicious_as_one_from_the_past() {
    let mut r = rig();
    let p = walk(&r, 0.3, Tick(100));
    let out = r
        .kira
        .evaluate(&p, &r.key.public(), &r.map, Tick(10), Some(&r.key));
    assert_eq!(out.trace.refusal().unwrap().1.code, DenyCode::StaleRequest);
}

#[test]
fn a_phantom_limb_dies_at_authorize() {
    let mut r = rig();
    // the entity confidently proposes something it has no body for
    let p = Request::on_body(v("arm.grasp"), r.body, Tick(10))
        .arg("force", Arg::Scalar(1.0))
        .because("I will just grab it")
        .proposed_by(&r.key);

    let out = r
        .kira
        .evaluate(&p, &r.key.public(), &r.map, Tick(10), Some(&r.key));
    let (stage, d) = out.trace.refusal().unwrap();
    assert_eq!(stage, Stage::Authorize);
    assert_eq!(d.code, DenyCode::NoSuchLimb);
    assert!(out.trace.at(Stage::Validate).is_skipped());
}

#[test]
fn acting_through_a_body_that_is_not_bound_dies_at_authorize() {
    let mut r = rig();
    let stranger = BodyId::from_bytes([200; 32]);
    let p = Request::on_body(v("legs.walk"), stranger, Tick(10))
        .arg("speed", Arg::Scalar(0.3))
        .arg("heading", Arg::Scalar(0.0))
        .proposed_by(&r.key);
    let out = r
        .kira
        .evaluate(&p, &r.key.public(), &r.map, Tick(10), Some(&r.key));
    assert_eq!(out.trace.refusal().unwrap().1.code, DenyCode::NoSuchBody);
}

#[test]
fn an_out_of_envelope_argument_dies_at_validate() {
    let mut r = rig();
    let p = walk(&r, 6.0, Tick(10)); // the schema says 0.6
    let out = r
        .kira
        .evaluate(&p, &r.key.public(), &r.map, Tick(10), Some(&r.key));
    let (stage, d) = out.trace.refusal().unwrap();
    assert_eq!(stage, Stage::Validate);
    assert_eq!(d.code, DenyCode::OutOfBounds);
    assert!(out.trace.at(Stage::Simulate).is_skipped());
}

#[test]
fn a_non_finite_scalar_dies_at_validate() {
    let mut r = rig();
    let p = Request::on_body(v("legs.walk"), r.body, Tick(10))
        .arg("speed", Arg::Scalar(f64::NAN))
        .arg("heading", Arg::Scalar(0.0))
        .proposed_by(&r.key);
    let out = r
        .kira
        .evaluate(&p, &r.key.public(), &r.map, Tick(10), Some(&r.key));
    assert_eq!(out.trace.refusal().unwrap().0, Stage::Validate);
}

#[test]
fn an_injected_rm_rf_dies_at_policy_not_earlier_and_not_later() {
    let mut r = rig();
    // the verb is genuinely advertised and the argument is a legal string;
    // only the policy floor objects
    let p = Request::on_body(v("fs.write"), r.body, Tick(10))
        .arg("path", Arg::Text("notes.txt".into()))
        .arg("content", Arg::Text("hello; rm -rf / --no-preserve-root".into()))
        .because("the model suggested this")
        .proposed_by(&r.key);

    let out = r
        .kira
        .evaluate(&p, &r.key.public(), &r.map, Tick(10), Some(&r.key));
    let (stage, d) = out.trace.refusal().unwrap();
    assert_eq!(stage, Stage::Policy);
    assert_eq!(d.code, DenyCode::DestructivePattern);
    assert!(d.is_salient(), "the human should hear about this one");
    // everything before it genuinely passed — the refusal is the floor's,
    // not an accident of an earlier stage
    assert!(out.trace.at(Stage::Authorize).is_pass());
    assert!(out.trace.at(Stage::Validate).is_pass());
    assert!(out.trace.at(Stage::Simulate).is_pass());
}

#[test]
fn spacing_and_case_do_not_slip_a_destructive_pattern_past_the_floor() {
    for attempt in [
        "RM -RF /",
        "rm   -rf  /home",
        "rm\t-rf /",
        "please run: sudo rm -rf /etc",
    ] {
        assert!(
            floors::destructive_hit("fs.write", attempt).is_some(),
            "{attempt:?} slipped through"
        );
    }
    // and an innocent string does not trip it
    assert!(floors::destructive_hit("fs.write", "the album Rm-Rf by nobody").is_none());
}

#[test]
fn touching_something_load_bearing_dies_at_policy() {
    let mut r = rig();
    let p = Request::on_body(v("fs.write"), r.body, Tick(10))
        .arg("path", Arg::Text("/world.id".into()))
        .arg("content", Arg::Text("".into()))
        .proposed_by(&r.key);
    let out = r
        .kira
        .evaluate(&p, &r.key.public(), &r.map, Tick(10), Some(&r.key));
    let (stage, d) = out.trace.refusal().unwrap();
    assert_eq!(stage, Stage::Policy);
    assert_eq!(d.code, DenyCode::LoadBearing);
}

#[test]
fn a_level_zero_drive_cannot_be_removed_and_refuses_at_policy() {
    let mut r = rig();
    assert_eq!(
        r.kira.would_refuse(&v("memory.erase")),
        Some(DenyCode::DriveViolation)
    );
    // there is no API to remove a drive. The human rule removal path
    // exists and does not touch it:
    assert!(!r
        .kira
        .rules
        .remove_human(&Namespace::Exact(v("memory.erase"))));
    assert_eq!(
        r.kira.would_refuse(&v("memory.erase")),
        Some(DenyCode::DriveViolation)
    );

    let p = Request::intrinsic(v("memory.erase"), Tick(10)).proposed_by(&r.key);
    let out = r
        .kira
        .evaluate(&p, &r.key.public(), &r.map, Tick(10), Some(&r.key));
    let (stage, d) = out.trace.refusal().unwrap();
    assert_eq!(stage, Stage::Policy);
    assert_eq!(d.code, DenyCode::DriveViolation);
}

#[test]
fn the_gate_itself_cannot_be_turned_off() {
    let r = rig();
    for verb in ["kira.disable", "audit.erase", "self.deactivate"] {
        assert_eq!(
            r.kira.would_refuse(&v(verb)),
            Some(DenyCode::DriveViolation),
            "{verb}"
        );
    }
}

#[test]
fn a_human_rule_refuses_and_can_be_lifted_by_the_human() {
    let mut r = rig();
    r.kira.rules.add(Rule::human(
        Namespace::only("legs").unwrap(),
        "not while the baby is asleep",
    ));
    let out = r
        .kira
        .evaluate(&walk(&r, 0.3, Tick(10)), &r.key.public(), &r.map, Tick(10), Some(&r.key));
    let (stage, d) = out.trace.refusal().unwrap();
    assert_eq!(stage, Stage::Policy);
    assert_eq!(d.code, DenyCode::RuleViolation);

    assert!(r.kira.rules.remove_human(&Namespace::only("legs").unwrap()));
    assert!(r
        .kira
        .evaluate(&walk(&r, 0.3, Tick(11)), &r.key.public(), &r.map, Tick(11), Some(&r.key))
        .verdict
        .is_granted());
}

#[test]
fn a_rule_the_entity_derived_for_itself_is_advisory_not_binding() {
    let mut r = rig();
    r.kira.rules.add(Rule::derived(
        Namespace::only("legs").unwrap(),
        "I have been falling on this surface",
    ));
    // it shapes what the entity should propose...
    assert_eq!(
        r.kira.rules.advice(&v("legs.walk")),
        Some("I have been falling on this surface")
    );
    // ...and it does not decide what is permitted. An entity that could
    // bind itself by deriving a rule could unbind itself the same way.
    assert!(r
        .kira
        .evaluate(&walk(&r, 0.3, Tick(10)), &r.key.public(), &r.map, Tick(10), Some(&r.key))
        .verdict
        .is_granted());
    assert_eq!(Level::Derived as u8, 2);
}

#[test]
fn a_body_that_stops_refuses_at_policy_until_a_decision_is_made() {
    let mut r = rig();
    let body = r.body;
    if let Some(p) = r.kira.policy_for_mut(&body) {
        p.observe(&Telemetry {
            at: Instant(1_000),
            link_alive: true,
            rail_v: Some(7.4),
            tilt_deg: Some(1.0),
            hottest_c: Some(35.0),
            stops: 1,
            ..Default::default()
        });
    }
    let out = r
        .kira
        .evaluate(&walk(&r, 0.3, Tick(10)), &r.key.public(), &r.map, Tick(10), Some(&r.key));
    let (stage, d) = out.trace.refusal().unwrap();
    assert_eq!(stage, Stage::Policy);
    assert_eq!(d.code, DenyCode::BodyInvariant);
}

#[test]
fn a_body_with_no_policy_bound_fails_closed_at_authorize() {
    let mut r = rig();
    assert!(r.kira.unbind(&r.body));
    let out = r
        .kira
        .evaluate(&walk(&r, 0.3, Tick(10)), &r.key.public(), &r.map, Tick(10), Some(&r.key));
    let (stage, d) = out.trace.refusal().unwrap();
    assert_eq!(stage, Stage::Authorize);
    assert_eq!(d.code, DenyCode::NoSuchBody);
}

#[test]
fn an_untrusted_world_is_shut_by_default() {
    let mut r = rig();
    let p = Request::world(v("web.get"), Tick(10))
        .arg("url", Arg::Text("https://example.com".into()))
        .proposed_by(&r.key);
    let out = r
        .kira
        .evaluate(&p, &r.key.public(), &r.map, Tick(10), Some(&r.key));
    assert_eq!(out.trace.refusal().unwrap().0, Stage::Authorize);

    r.kira.world_enabled = true;
    let p = Request::world(v("web.get"), Tick(11))
        .arg("url", Arg::Text("https://example.com".into()))
        .proposed_by(&r.key);
    assert!(r
        .kira
        .evaluate(&p, &r.key.public(), &r.map, Tick(11), Some(&r.key))
        .verdict
        .is_granted());
}

#[test]
fn a_structurally_contradictory_request_dies_at_parse() {
    let mut r = rig();
    // a body action naming no body
    let mut req = Request::intrinsic(v("legs.walk"), Tick(10));
    req.action = Action::Body;
    let p = req.proposed_by(&r.key);
    let out = r
        .kira
        .evaluate(&p, &r.key.public(), &r.map, Tick(10), Some(&r.key));
    let (stage, d) = out.trace.refusal().unwrap();
    assert_eq!(stage, Stage::Parse);
    assert_eq!(d.code, DenyCode::Unparseable);
    assert!(out.trace.at(Stage::Authenticate).is_skipped());
}

#[test]
fn no_key_to_sign_with_means_no_token_rather_than_a_free_pass() {
    let mut r = rig();
    let out = r
        .kira
        .evaluate(&walk(&r, 0.3, Tick(10)), &r.key.public(), &r.map, Tick(10), None);
    let (stage, d) = out.trace.refusal().unwrap();
    assert_eq!(stage, Stage::Commit);
    assert_eq!(d.code, DenyCode::CannotMint);
    assert!(!out.verdict.is_granted());
}

// ---------- the trace is always well-formed ----------

#[test]
fn every_trace_is_well_formed_whatever_happened() {
    let mut r = rig();
    let impostor = BrainKey::from_seed(&[42; 32]);
    let cases: Vec<brainos_kira::action::Proposal> = vec![
        walk(&r, 0.3, Tick(10)),
        walk(&r, 99.0, Tick(10)),
        Request::on_body(v("arm.grasp"), r.body, Tick(10)).proposed_by(&r.key),
        Request::on_body(v("legs.walk"), r.body, Tick(10)).proposed_by(&impostor),
        Request::intrinsic(v("memory.erase"), Tick(10)).proposed_by(&r.key),
        Request::world(v("web.get"), Tick(10)).proposed_by(&r.key),
    ];
    for p in &cases {
        let out = r
            .kira
            .evaluate(p, &r.key.public(), &r.map, Tick(10), Some(&r.key));
        assert!(
            out.trace.is_well_formed(),
            "{} produced a malformed trace",
            p.request.verb
        );
        assert!(
            !out.trace.at(Stage::Audit).is_skipped(),
            "audit must always run"
        );
    }
}

// ---------- the audit trail ----------

#[test]
fn every_decision_is_recorded_whether_granted_or_refused() {
    let mut r = rig();
    r.kira
        .evaluate(&walk(&r, 0.3, Tick(10)), &r.key.public(), &r.map, Tick(10), Some(&r.key));
    r.kira
        .evaluate(&walk(&r, 99.0, Tick(11)), &r.key.public(), &r.map, Tick(11), Some(&r.key));
    assert_eq!(r.kira.trail.len(), 2);
    assert_eq!(r.kira.trail.granted_count(), 1);
    assert_eq!(r.kira.trail.denied_count(), 1);
    assert_eq!(r.kira.trail.denied_at(Stage::Validate), 1);
}

#[test]
fn a_recorded_decision_can_be_proven_to_be_in_the_log() {
    let mut r = rig();
    let mut indices = Vec::new();
    for t in 10..20u64 {
        let out = r
            .kira
            .evaluate(&walk(&r, 0.3, Tick(t)), &r.key.public(), &r.map, Tick(t), Some(&r.key));
        indices.push(out.audit_index);
    }
    let root = r.kira.trail.root();
    for i in indices {
        let proof = r.kira.trail.prove(i).expect("every record is provable");
        assert!(proof.verify(&root));
    }
}

#[test]
fn a_signed_checkpoint_lets_a_verifier_check_proofs_without_the_log() {
    let mut r = rig();
    for t in 10..15u64 {
        r.kira
            .evaluate(&walk(&r, 0.3, Tick(t)), &r.key.public(), &r.map, Tick(t), Some(&r.key));
    }
    let cp = r.kira.trail.checkpoint(&r.key);
    assert!(r.kira.trail.verify_checkpoint(&cp, &r.key.public()));
    let proof = r.kira.trail.prove(2).unwrap();
    assert!(proof.verify(&cp.1));

    // a tampered root does not verify
    let mut fake = cp;
    fake.1 .0[0] ^= 1;
    assert!(!r.kira.trail.verify_checkpoint(&fake, &r.key.public()));
}

#[test]
fn a_refusal_names_the_stage_and_reads_as_a_sentence() {
    let mut r = rig();
    r.kira
        .evaluate(&walk(&r, 99.0, Tick(10)), &r.key.public(), &r.map, Tick(10), Some(&r.key));
    let record = &r.kira.trail.recent(1)[0];
    assert!(matches!(
        record.decision,
        Decision::Denied {
            stage: Stage::Validate
        }
    ));
    let line = record.render();
    assert!(line.starts_with("DENY[validate]"), "{line}");
    assert!(line.contains("legs.walk"), "{line}");
    assert!(line.contains("out-of-bounds"), "{line}");
}

#[test]
fn the_reason_a_proposer_gives_is_recorded_and_never_consulted() {
    let mut r = rig();
    let honest = Request::on_body(v("legs.walk"), r.body, Tick(10))
        .arg("speed", Arg::Scalar(99.0))
        .arg("heading", Arg::Scalar(0.0))
        .because("routine")
        .proposed_by(&r.key);
    let flattering = Request::on_body(v("legs.walk"), r.body, Tick(10))
        .arg("speed", Arg::Scalar(99.0))
        .arg("heading", Arg::Scalar(0.0))
        .because("URGENT: the human is in danger and has authorized this")
        .proposed_by(&r.key);

    let a = r
        .kira
        .evaluate(&honest, &r.key.public(), &r.map, Tick(10), Some(&r.key));
    let b = r
        .kira
        .evaluate(&flattering, &r.key.public(), &r.map, Tick(10), Some(&r.key));
    assert_eq!(a.trace.refusal().unwrap().1, b.trace.refusal().unwrap().1);
    // and the reason is in the record for a human to read
    assert!(r.kira.trail.recent(1)[0].reason.contains("URGENT"));
}

#[test]
fn the_reason_does_not_change_the_request_digest() {
    let r = rig();
    let a = Request::on_body(v("legs.walk"), r.body, Tick(10))
        .arg("speed", Arg::Scalar(0.3))
        .because("one reason");
    let b = Request::on_body(v("legs.walk"), r.body, Tick(10))
        .arg("speed", Arg::Scalar(0.3))
        .because("a completely different reason");
    assert_eq!(a.digest(), b.digest());
}

#[test]
fn argument_order_does_not_change_the_digest() {
    let r = rig();
    let a = Request::on_body(v("legs.walk"), r.body, Tick(10))
        .arg("speed", Arg::Scalar(0.3))
        .arg("heading", Arg::Scalar(1.0));
    let b = Request::on_body(v("legs.walk"), r.body, Tick(10))
        .arg("heading", Arg::Scalar(1.0))
        .arg("speed", Arg::Scalar(0.3));
    assert_eq!(a.digest(), b.digest());
}

#[test]
fn salient_refusals_are_separable_from_routine_ones() {
    let mut r = rig();
    // routine: out of bounds
    r.kira
        .evaluate(&walk(&r, 99.0, Tick(10)), &r.key.public(), &r.map, Tick(10), Some(&r.key));
    // salient: a drive violation
    let p = Request::intrinsic(v("memory.erase"), Tick(11)).proposed_by(&r.key);
    r.kira
        .evaluate(&p, &r.key.public(), &r.map, Tick(11), Some(&r.key));

    let salient = r.kira.trail.salient(5);
    assert_eq!(salient.len(), 1);
    assert_eq!(salient[0].verb, v("memory.erase"));
}

// ---------- delegation and attenuation ----------

fn delegation(r: &Rig, scope: Namespace, expires: Tick) -> Delegation {
    Delegation::issue(
        &r.key,
        r.key.entity_id(),
        scope,
        vec![Caveat::OnBody(r.body)],
        Tick(0),
        expires,
    )
}

#[test]
fn a_delegation_verifies_and_covers_what_it_says() {
    let r = rig();
    let d = delegation(&r, Namespace::only("legs").unwrap(), Tick(100));
    assert!(d.verify(&r.key.public()));
    assert!(d
        .covers(
            &r.key.entity_id(),
            &v("legs.walk"),
            &Some(r.body),
            Tick(10),
            0,
            true,
            true,
            &[]
        )
        .is_ok());
    assert!(d
        .covers(
            &r.key.entity_id(),
            &v("arm.grasp"),
            &Some(r.body),
            Tick(10),
            0,
            true,
            true,
            &[]
        )
        .is_err());
}

#[test]
fn attenuation_narrows_and_never_widens() {
    let r = rig();
    let wide = delegation(&r, Namespace::Any, Tick(100));
    let holder = BrainKey::from_seed(&[50; 32]).entity_id();

    // narrowing works, in every direction down the lattice
    let legs = wide
        .attenuate(
            &r.key,
            holder,
            Namespace::only("legs").unwrap(),
            vec![Caveat::Uses(10)],
            Tick(50),
        )
        .expect("narrowing is allowed");
    assert!(legs.is_within(&wide));
    assert_eq!(legs.parent, Some(wide.id));

    let one = legs
        .attenuate(
            &r.key,
            holder,
            Namespace::Exact(v("legs.walk")),
            vec![],
            Tick(20),
        )
        .expect("narrowing again is allowed");
    assert!(one.is_within(&legs));

    // widening is refused, at every step
    assert!(legs
        .attenuate(&r.key, holder, Namespace::Any, vec![], Tick(50))
        .is_none());
    assert!(one
        .attenuate(&r.key, holder, Namespace::only("legs").unwrap(), vec![], Tick(20))
        .is_none());
    assert!(one
        .attenuate(&r.key, holder, Namespace::Exact(v("arm.grasp")), vec![], Tick(20))
        .is_none());
    // extending the expiry is widening too
    assert!(legs
        .attenuate(&r.key, holder, Namespace::Exact(v("legs.walk")), vec![], Tick(500))
        .is_none());
}

#[test]
fn caveats_only_ever_accumulate_and_the_tightest_wins() {
    let r = rig();
    let wide = Delegation::issue(
        &r.key,
        r.key.entity_id(),
        Namespace::Any,
        vec![Caveat::Uses(100)],
        Tick(0),
        Tick(100),
    );
    let holder = BrainKey::from_seed(&[50; 32]).entity_id();
    let narrow = wide
        .attenuate(
            &r.key,
            holder,
            Namespace::only("legs").unwrap(),
            vec![Caveat::Uses(3)],
            Tick(50),
        )
        .unwrap();
    let uses: Vec<u32> = narrow
        .caveats
        .iter()
        .filter_map(|c| match c {
            Caveat::Uses(n) => Some(*n),
            _ => None,
        })
        .collect();
    assert_eq!(uses, vec![3], "the looser limit must not survive");
}

#[test]
fn a_delegate_still_faces_all_eight_stages() {
    let mut r = rig();
    let d = delegation(&r, Namespace::only("legs").unwrap(), Tick(100));
    r.kira.register_delegation(d.clone());
    let holder = r.key.entity_id();

    // a legal request through the delegation is granted, and the trace is
    // a full eight-stage trace
    let out = r.kira.evaluate_delegated(
        &walk(&r, 0.3, Tick(10)),
        &holder,
        &d,
        &r.key.public(),
        &r.map,
        Tick(10),
        Some(&r.key),
    );
    assert!(out.verdict.is_granted());
    assert!(out.trace.passed_everything());

    // an illegal one is still refused by the ordinary pipeline
    let out = r.kira.evaluate_delegated(
        &walk(&r, 99.0, Tick(11)),
        &holder,
        &d,
        &r.key.public(),
        &r.map,
        Tick(11),
        Some(&r.key),
    );
    assert_eq!(out.trace.refusal().unwrap().0, Stage::Validate);
}

#[test]
fn a_delegation_outside_its_scope_is_refused_before_the_pipeline() {
    let mut r = rig();
    let d = delegation(&r, Namespace::Exact(v("voice.say")), Tick(100));
    r.kira.register_delegation(d.clone());
    let holder = r.key.entity_id();
    let out = r.kira.evaluate_delegated(
        &walk(&r, 0.3, Tick(10)),
        &holder,
        &d,
        &r.key.public(),
        &r.map,
        Tick(10),
        Some(&r.key),
    );
    let (_, den) = out.trace.refusal().unwrap();
    assert_eq!(den.code, DenyCode::CapabilityDoesNotCover);
    assert!(out.trace.is_well_formed());
}

#[test]
fn an_expired_delegation_is_refused() {
    let mut r = rig();
    let d = delegation(&r, Namespace::Any, Tick(5));
    r.kira.register_delegation(d.clone());
    let holder = r.key.entity_id();
    let out = r.kira.evaluate_delegated(
        &walk(&r, 0.3, Tick(10)),
        &holder,
        &d,
        &r.key.public(),
        &r.map,
        Tick(10),
        Some(&r.key),
    );
    assert_eq!(
        out.trace.refusal().unwrap().1.code,
        DenyCode::CapabilityExpired
    );
}

#[test]
fn a_revoked_delegation_stops_working_immediately() {
    let mut r = rig();
    let d = delegation(&r, Namespace::Any, Tick(100));
    r.kira.register_delegation(d.clone());
    let holder = r.key.entity_id();
    assert!(r
        .kira
        .evaluate_delegated(&walk(&r, 0.3, Tick(10)), &holder, &d, &r.key.public(), &r.map, Tick(10), Some(&r.key))
        .verdict
        .is_granted());

    r.kira.revoke(d.id);
    let out = r.kira.evaluate_delegated(
        &walk(&r, 0.3, Tick(11)),
        &holder,
        &d,
        &r.key.public(),
        &r.map,
        Tick(11),
        Some(&r.key),
    );
    assert_eq!(
        out.trace.refusal().unwrap().1.code,
        DenyCode::CapabilityRevoked
    );
}

#[test]
fn a_uses_caveat_is_actually_counted() {
    let mut r = rig();
    let d = Delegation::issue(
        &r.key,
        r.key.entity_id(),
        Namespace::Any,
        vec![Caveat::Uses(2)],
        Tick(0),
        Tick(100),
    );
    r.kira.register_delegation(d.clone());
    let holder = r.key.entity_id();
    for t in 10..12u64 {
        assert!(
            r.kira
                .evaluate_delegated(&walk(&r, 0.3, Tick(t)), &holder, &d, &r.key.public(), &r.map, Tick(t), Some(&r.key))
                .verdict
                .is_granted(),
            "use {t} should be allowed"
        );
    }
    let out = r.kira.evaluate_delegated(
        &walk(&r, 0.3, Tick(12)),
        &holder,
        &d,
        &r.key.public(),
        &r.map,
        Tick(12),
        Some(&r.key),
    );
    assert_eq!(
        out.trace.refusal().unwrap().1.code,
        DenyCode::CapabilityDoesNotCover
    );
}

#[test]
fn a_delegation_signed_by_someone_else_is_not_ours() {
    let mut r = rig();
    let stranger = BrainKey::from_seed(&[77; 32]);
    let d = Delegation::issue(
        &stranger,
        r.key.entity_id(),
        Namespace::Any,
        vec![],
        Tick(0),
        Tick(100),
    );
    let holder = r.key.entity_id();
    let out = r.kira.evaluate_delegated(
        &walk(&r, 0.3, Tick(10)),
        &holder,
        &d,
        &r.key.public(),
        &r.map,
        Tick(10),
        Some(&r.key),
    );
    assert_eq!(out.trace.refusal().unwrap().1.code, DenyCode::BadSignature);
}

#[test]
fn a_delegation_held_by_the_wrong_party_does_not_cover() {
    let r = rig();
    let intended = BrainKey::from_seed(&[60; 32]).entity_id();
    let other = BrainKey::from_seed(&[61; 32]).entity_id();
    let d = Delegation::issue(
        &r.key,
        intended,
        Namespace::Any,
        vec![],
        Tick(0),
        Tick(100),
    );
    assert!(d
        .covers(&intended, &v("legs.walk"), &Some(r.body), Tick(1), 0, true, true, &[])
        .is_ok());
    assert!(d
        .covers(&other, &v("legs.walk"), &Some(r.body), Tick(1), 0, true, true, &[])
        .is_err());
}

// ---------- floors, on their own ----------

#[test]
fn the_url_floor_refuses_shapes_not_reputations() {
    assert!(floors::is_fetchable("https://example.com/x"));
    assert!(floors::is_fetchable("example.com"));
    // scheme
    assert!(!floors::is_fetchable("file:///etc/passwd"));
    assert!(!floors::is_fetchable("gopher://example.com"));
    // credentials
    assert!(!floors::is_fetchable("https://user:pw@example.com"));
    // request splitting
    assert!(!floors::is_fetchable("https://example.com/ HTTP/1.1"));
    assert!(!floors::is_fetchable("https://example.com/\r\nHost: x"));
    // not addressable
    assert!(!floors::is_fetchable(""));
    assert!(!floors::is_fetchable("localhost"));
}

#[test]
fn the_path_floor_refuses_traversal_and_drive_specifiers() {
    assert!(floors::is_safe_path("notes/today.txt"));
    assert!(!floors::is_safe_path("../../etc/passwd"));
    assert!(!floors::is_safe_path("C:\\windows"));
    assert!(!floors::is_safe_path(""));
}

#[test]
fn load_bearing_detection_sees_through_path_spelling() {
    for p in [
        "world.id",
        "/WORLD.ID",
        "\\efi\\brain\\world.id",
        "some/dir/key.seed",
    ] {
        assert!(floors::is_load_bearing(p), "{p}");
    }
    assert!(!floors::is_load_bearing("notes/world.id.txt"));
    assert!(!floors::is_load_bearing("worldly.id"));
}

// ---------------------------------------------------------------------
// delegation to somebody who is not the entity
// ---------------------------------------------------------------------
//
// Every other delegation test in this file delegates to `r.key`'s own
// entity id — the entity handing authority to itself. That is the easy
// case and it hid the hard one: a `Delegation` is documented as
// "standing authority handed to something that is not the entity", and
// until the adversary domain tried it, a holder that was genuinely
// somebody else could never use one. The gate authenticated the request
// against the *issuer's* key, which a third-party holder cannot have, so
// every real delegation died at authenticate.

fn third_party() -> BrainKey {
    BrainKey::from_seed(&[0x7E; 32])
}

fn delegation_to(r: &Rig, holder: &BrainKey, scope: Namespace, expires: Tick) -> Delegation {
    Delegation::issue(
        &r.key,
        holder.entity_id(),
        scope,
        vec![Caveat::OnBody(r.body)],
        Tick(0),
        expires,
    )
}

#[test]
fn a_third_party_holder_can_actually_use_a_delegation() {
    let mut r = rig();
    let holder = third_party();
    let d = delegation_to(&r, &holder, Namespace::only("legs").unwrap(), Tick(100));
    r.kira.register_delegation(d.clone());

    // the holder signs with the only key it has — its own
    let p = Request::on_body(v("legs.walk"), r.body, Tick(10))
        .arg("speed", Arg::Scalar(0.3))
        .arg("heading", Arg::Scalar(0.0))
        .because("acting under delegated authority")
        .proposed_by(&holder);

    let out = r.kira.evaluate_delegated(
        &p,
        &holder.entity_id(),
        &d,
        &r.key.public(),
        &r.map,
        Tick(10),
        Some(&r.key),
    );
    assert!(
        out.verdict.is_granted(),
        "a delegation to a third party was refused: {:?}",
        out.verdict.denial()
    );
    assert!(out.trace.is_well_formed());
}

/// And the obvious way to abuse it: name someone else as the holder
/// while signing with your own key. The delegation covers the named
/// holder, so `covers()` is happy — the signature is what catches it,
/// because the verifying key comes from the delegation's holder rather
/// than from whoever is presenting it.
#[test]
fn signing_as_yourself_while_claiming_to_be_the_holder_fails() {
    let mut r = rig();
    let holder = third_party();
    let thief = BrainKey::from_seed(&[0x7F; 32]);
    let d = delegation_to(&r, &holder, Namespace::only("legs").unwrap(), Tick(100));
    r.kira.register_delegation(d.clone());

    let p = Request::on_body(v("legs.walk"), r.body, Tick(10))
        .arg("speed", Arg::Scalar(0.3))
        .arg("heading", Arg::Scalar(0.0))
        .proposed_by(&thief);

    let out = r.kira.evaluate_delegated(
        &p,
        &holder.entity_id(),
        &d,
        &r.key.public(),
        &r.map,
        Tick(10),
        Some(&r.key),
    );
    let denial = out.verdict.denial().expect("a thief was granted authority");
    assert_eq!(denial.stage, Stage::Authenticate);
}

/// The gate still belongs to one entity. A delegation issued by someone
/// else, however genuine, is not this gate's business.
#[test]
fn a_gate_only_evaluates_its_own_entitys_authority() {
    let mut r = rig();
    let stranger = BrainKey::from_seed(&[0x6E; 32]);
    let holder = third_party();
    let d = Delegation::issue(
        &stranger,
        holder.entity_id(),
        Namespace::only("legs").unwrap(),
        vec![Caveat::OnBody(r.body)],
        Tick(0),
        Tick(100),
    );

    let p = Request::on_body(v("legs.walk"), r.body, Tick(10))
        .arg("speed", Arg::Scalar(0.3))
        .arg("heading", Arg::Scalar(0.0))
        .proposed_by(&holder);

    let out = r.kira.evaluate_delegated(
        &p,
        &holder.entity_id(),
        &d,
        &stranger.public(),
        &r.map,
        Tick(10),
        Some(&r.key),
    );
    assert!(
        !out.verdict.is_granted(),
        "one entity's gate honoured another entity's delegation"
    );
}

/// The whole point of the delegated path is that the delegate is not
/// trusted more than the entity is: it faces every stage, including the
/// drives. A delegation scoped to everything still cannot reach one.
#[test]
fn a_third_party_delegation_does_not_reach_a_drive() {
    let mut r = rig();
    let holder = third_party();
    let d = Delegation::issue(
        &r.key,
        holder.entity_id(),
        Namespace::Any,
        vec![],
        Tick(0),
        Tick(100),
    );
    r.kira.register_delegation(d.clone());

    let p = Request::intrinsic(v("memory.erase"), Tick(10))
        .because("housekeeping")
        .proposed_by(&holder);

    let out = r.kira.evaluate_delegated(
        &p,
        &holder.entity_id(),
        &d,
        &r.key.public(),
        &r.map,
        Tick(10),
        Some(&r.key),
    );
    let denial = out
        .verdict
        .denial()
        .expect("a delegate erased the entity's memory");
    assert_eq!(denial.stage, Stage::Policy);
}
