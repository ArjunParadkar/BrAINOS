//! The §8 suite.
//!
//! The properties defended here: a device cannot join without proving
//! who it is, a device that changes what it can do is a different body,
//! a body that will not state its limits cannot be moved, and a limb
//! that is merely incorporated is not a limb that may act.

use brainos_abi::error::DenyCode;
use brainos_abi::ids::{EntityId, RegionId, SessionId};
use brainos_abi::time::{Duration, Instant, Tick};
use brainos_abi::verb::Verb;
use brainos_abi::version::{AbiVersion, ABI};
use brainos_body::acquire::{acquire_native, default_envelope, Acquisition, Step, StepError};
use brainos_body::calibration::{Calibration, JointTrim};
use brainos_body::hal::Telemetry;
use brainos_body::policy::{Arg, Args, BodyPolicy, NullPolicy, Posture, SchemaPolicy};
use brainos_body::region::{BodyMap, RegionClass};
use brainos_body::schema::{CapabilitySchema, ParamKind, ParamSpec, VerbSpec};
use brainos_crypto::key::BrainKey;
use brainos_crypto::rng::Drbg;
use brainos_rt::envelope::Channel;

fn v(s: &str) -> Verb {
    Verb::parse(s).unwrap()
}

fn entity() -> (EntityId, BrainKey) {
    let k = BrainKey::from_seed(&[1; 32]);
    (k.entity_id(), k)
}

fn hexapod_schema() -> CapabilitySchema {
    CapabilitySchema::new("mini-blur", "native")
        .with(
            VerbSpec::new(v("legs.walk"), RegionClass::Legs, "walk forward")
                .with(ParamSpec::scalar("speed", Channel::Speed, 0.0, 0.6))
                .with(ParamSpec::scalar("heading", Channel::Angle, -30.0, 30.0))
                .taking_up_to(5_000),
        )
        .with(
            VerbSpec::new(v("legs.stand"), RegionClass::Legs, "stand up")
                .with(ParamSpec::scalar("height", Channel::Angle, -20.0, 20.0)),
        )
        .with(
            VerbSpec::new(v("voice.say"), RegionClass::Speaker, "speak")
                .with(ParamSpec::text("text", 240)),
        )
}

fn calibration_for(body: brainos_abi::ids::BodyId) -> Calibration {
    let mut c = Calibration::new(body, Tick(1));
    for ch in 0..6u8 {
        c.trim(JointTrim {
            channel: ch,
            offset_deg: 1.5,
            scale: 1.0,
            min_deg: -35.0,
            max_deg: 35.0,
            probed: true,
        })
        .unwrap();
    }
    c
}

fn ids() -> impl FnMut() -> RegionId {
    let mut n = 0u8;
    move || {
        n += 1;
        RegionId::from_bytes([n; 32])
    }
}

// ---------- schemas ----------

#[test]
fn two_adapters_reading_the_same_body_produce_the_same_identity() {
    let a = hexapod_schema();
    // the same verbs, declared in a different order, through a different
    // adapter, with a different friendly name
    let mut b = CapabilitySchema::new("Mini Blur (ROS)", "ros2");
    b.declare(
        VerbSpec::new(v("voice.say"), RegionClass::Speaker, "speak")
            .with(ParamSpec::text("text", 240)),
    );
    b.declare(
        VerbSpec::new(v("legs.stand"), RegionClass::Legs, "stand up")
            .with(ParamSpec::scalar("height", Channel::Angle, -20.0, 20.0)),
    );
    b.declare(
        VerbSpec::new(v("legs.walk"), RegionClass::Legs, "walk forward")
            .with(ParamSpec::scalar("heading", Channel::Angle, -30.0, 30.0))
            .with(ParamSpec::scalar("speed", Channel::Speed, 0.0, 0.6))
            .taking_up_to(5_000),
    );
    assert_eq!(
        a.digest(),
        b.digest(),
        "identity must not depend on discovery order, adapter or label"
    );
}

#[test]
fn a_body_that_gains_a_verb_has_a_different_identity() {
    let a = hexapod_schema();
    let b = hexapod_schema().with(
        VerbSpec::new(v("arm.grasp"), RegionClass::Gripper, "grab")
            .with(ParamSpec::scalar("force", Channel::Current, 0.0, 1.0)),
    );
    assert_ne!(a.digest(), b.digest());
}

#[test]
fn a_body_that_loosens_a_limit_has_a_different_identity() {
    let a = hexapod_schema();
    let mut b = hexapod_schema();
    b.declare(
        VerbSpec::new(v("legs.walk"), RegionClass::Legs, "walk forward")
            .with(ParamSpec::scalar("speed", Channel::Speed, 0.0, 6.0)) // 10x
            .with(ParamSpec::scalar("heading", Channel::Angle, -30.0, 30.0))
            .taking_up_to(5_000),
    );
    assert_ne!(
        a.digest(),
        b.digest(),
        "a changed envelope is a changed body"
    );
}

#[test]
fn a_verb_with_an_unbounded_parameter_is_visible_but_not_actionable() {
    let spec = VerbSpec::new(v("arm.move"), RegionClass::Arm, "move")
        .with(ParamSpec::unbounded("angle", ParamKind::Scalar));
    assert!(!spec.is_actionable());
    assert!(spec.param("angle").is_some());
    assert!(!spec.param("angle").unwrap().admits_scalar(0.0));
}

#[test]
fn schemas_round_trip_and_decode_canonically() {
    use brainos_abi::codec::{Decode, Encode};
    let s = hexapod_schema();
    let bytes = s.to_bytes().unwrap();
    let back = CapabilitySchema::from_bytes(&bytes).unwrap();
    assert_eq!(back, s);
    assert_eq!(back.digest(), s.digest());
}

// ---------- the protocol ----------

#[test]
fn the_steps_happen_in_order_and_cannot_be_skipped() {
    let (e, _key) = entity();
    let mut a = Acquisition::discover(e, SessionId::from_bytes([7; 32]));
    assert_eq!(a.step, Step::Discovered);

    // describing before attesting is refused
    assert_eq!(
        a.describe(hexapod_schema()),
        Err(StepError::OutOfOrder { at: Step::Discovered })
    );
    assert_eq!(a.step, Step::Rejected);
}

#[test]
fn a_device_that_cannot_answer_the_challenge_does_not_join() {
    let (e, key) = entity();
    let device = BrainKey::from_seed(&[2; 32]);
    let impostor = BrainKey::from_seed(&[3; 32]);
    let mut rng = Drbg::deterministic(b"test");
    let mut a = Acquisition::discover(e, SessionId::from_bytes([7; 32]));

    let r = a.handshake(
        &key,
        &mut rng,
        device.public(),
        ABI,
        // the impostor answers, but we challenged the device's key
        |c| impostor.answer(c),
        |_, _| true,
    );
    assert_eq!(r, Err(StepError::DeviceUnattested));
    assert_eq!(a.step, Step::Rejected);
}

#[test]
fn a_device_may_refuse_to_be_inhabited_and_that_is_not_our_error() {
    let (e, key) = entity();
    let device = BrainKey::from_seed(&[2; 32]);
    let mut rng = Drbg::deterministic(b"test");
    let mut a = Acquisition::discover(e, SessionId::from_bytes([7; 32]));

    let r = a.handshake(
        &key,
        &mut rng,
        device.public(),
        ABI,
        |c| device.answer(c),
        |_, _| false, // the device declines this entity
    );
    assert_eq!(r, Err(StepError::RefusedByDevice));
}

#[test]
fn a_device_speaking_a_newer_abi_is_refused_rather_than_partly_understood() {
    let (e, key) = entity();
    let device = BrainKey::from_seed(&[2; 32]);
    let mut rng = Drbg::deterministic(b"test");
    let mut a = Acquisition::discover(e, SessionId::from_bytes([7; 32]));
    let newer = AbiVersion::new(ABI.major, ABI.minor + 1, 0);

    let r = a.handshake(&key, &mut rng, device.public(), newer, |c| device.answer(c), |_, _| true);
    assert!(matches!(r, Err(StepError::Incompatible { .. })));
}

#[test]
fn the_handshake_transcript_binds_every_step_together() {
    let (e, key) = entity();
    let device = BrainKey::from_seed(&[2; 32]);
    let mut rng = Drbg::deterministic(b"test");

    let mut a = Acquisition::discover(e, SessionId::from_bytes([7; 32]));
    a.handshake(&key, &mut rng, device.public(), ABI, |c| device.answer(c), |_, _| true)
        .unwrap();
    let after_handshake = a.transcript();
    a.describe(hexapod_schema()).unwrap();
    assert_ne!(
        a.transcript(),
        after_handshake,
        "the schema must be inside the transcript"
    );

    // a different session produces a different transcript from the start
    let mut b = Acquisition::discover(e, SessionId::from_bytes([8; 32]));
    assert_ne!(a.transcript(), b.transcript());
    b.handshake(&key, &mut Drbg::deterministic(b"test"), device.public(), ABI, |c| device.answer(c), |_, _| true)
        .unwrap();
    assert_ne!(a.transcript(), b.transcript());
}

#[test]
fn a_schema_that_commands_a_sense_only_region_is_refused() {
    let (e, key) = entity();
    let device = BrainKey::from_seed(&[2; 32]);
    let mut rng = Drbg::deterministic(b"test");
    let mut a = Acquisition::discover(e, SessionId::from_bytes([7; 32]));
    a.handshake(&key, &mut rng, device.public(), ABI, |c| device.answer(c), |_, _| true)
        .unwrap();

    let bad = CapabilitySchema::new("odd", "native").with(VerbSpec::new(
        v("camera.walk"),
        RegionClass::Camera,
        "???",
    ));
    assert_eq!(
        a.describe(bad),
        Err(StepError::SchemaContradictsRegion)
    );
}

#[test]
fn an_empty_schema_is_refused() {
    let (e, key) = entity();
    let device = BrainKey::from_seed(&[2; 32]);
    let mut rng = Drbg::deterministic(b"test");
    let mut a = Acquisition::discover(e, SessionId::from_bytes([7; 32]));
    a.handshake(&key, &mut rng, device.public(), ABI, |c| device.answer(c), |_, _| true)
        .unwrap();
    assert_eq!(
        a.describe(CapabilitySchema::new("nothing", "native")),
        Err(StepError::NothingToDo)
    );
}

#[test]
fn the_full_protocol_ends_with_a_limb_that_may_move() {
    let (e, key) = entity();
    let device = BrainKey::from_seed(&[2; 32]);
    let mut rng = Drbg::deterministic(b"test");
    let mut map = BodyMap::new();

    let (body, regions) = acquire_native(
        e,
        SessionId::from_bytes([7; 32]),
        &key,
        &mut rng,
        &device,
        hexapod_schema(),
        &mut map,
        calibration_for(brainos_abi::ids::BodyId::ZERO),
        6,
        ids(),
    )
    .unwrap();

    assert!(map.knows_body(&body));
    assert_eq!(regions.len(), 2, "legs and speaker");
    assert!(map.find_capability(&v("legs.walk")).is_some());
    assert!(map.find_capability(&v("voice.say")).is_some());
    assert!(map.find_capability(&v("arm.grasp")).is_none());
}

#[test]
fn incorporated_is_not_calibrated_and_only_calibrated_may_act() {
    assert!(!Step::Incorporated.may_act_consequentially());
    assert!(Step::Calibrated.may_act_consequentially());
    assert!(!Step::Described.may_act_consequentially());
    assert!(!Step::Rejected.may_act_consequentially());
}

#[test]
fn calibration_for_the_wrong_body_is_refused() {
    let (e, key) = entity();
    let device = BrainKey::from_seed(&[2; 32]);
    let mut rng = Drbg::deterministic(b"test");
    let mut map = BodyMap::new();
    let mut a = Acquisition::discover(e, SessionId::from_bytes([7; 32]));
    a.handshake(&key, &mut rng, device.public(), ABI, |c| device.answer(c), |_, _| true)
        .unwrap();
    a.describe(hexapod_schema()).unwrap();
    a.incorporate(&mut map, ids()).unwrap();

    let wrong = calibration_for(brainos_abi::ids::BodyId::from_bytes([99; 32]));
    assert_eq!(
        a.calibrate(wrong, 6, |_| true),
        Err(StepError::IdentityShifted)
    );
}

#[test]
fn a_body_that_does_not_acknowledge_the_calibration_stays_uncalibrated() {
    let (e, key) = entity();
    let device = BrainKey::from_seed(&[2; 32]);
    let mut rng = Drbg::deterministic(b"test");
    let mut map = BodyMap::new();
    let mut a = Acquisition::discover(e, SessionId::from_bytes([7; 32]));
    a.handshake(&key, &mut rng, device.public(), ABI, |c| device.answer(c), |_, _| true)
        .unwrap();
    a.describe(hexapod_schema()).unwrap();
    a.incorporate(&mut map, ids()).unwrap();

    let cal = calibration_for(a.body.unwrap());
    assert_eq!(
        a.calibrate(cal, 6, |_| false), // the body never acks
        Err(StepError::CalibrationInvalid)
    );
    assert!(!a.step.may_act_consequentially());
}

#[test]
fn an_insane_joint_trim_never_enters_a_calibration() {
    let mut c = Calibration::new(brainos_abi::ids::BodyId::ZERO, Tick(1));
    assert!(c
        .trim(JointTrim {
            channel: 0,
            offset_deg: 0.0,
            scale: 0.0, // a dead joint
            min_deg: -10.0,
            max_deg: 10.0,
            probed: true,
        })
        .is_err());
    assert!(c
        .trim(JointTrim {
            channel: 0,
            offset_deg: 0.0,
            scale: 1.0,
            min_deg: 10.0, // inverted
            max_deg: -10.0,
            probed: true,
        })
        .is_err());
    assert!(c.joints.is_empty());
    assert!(c.bias("imu", f32::NAN).is_err());
}

#[test]
fn a_trim_clamps_to_the_measured_limits_not_the_datasheet() {
    let t = JointTrim {
        channel: 0,
        offset_deg: 2.0,
        scale: 1.0,
        min_deg: -28.0, // measured tighter than the -35 in the schema
        max_deg: 28.0,
        probed: true,
    };
    assert_eq!(t.apply(0.0), 2.0);
    assert_eq!(t.apply(100.0), 28.0);
    assert_eq!(t.apply(f32::NAN), -28.0);
}

#[test]
fn the_entity_knows_how_much_of_a_calibration_was_measured() {
    let body = brainos_abi::ids::BodyId::ZERO;
    let mut c = Calibration::new(body, Tick(1));
    c.trim(JointTrim::identity(0, -30.0, 30.0)).unwrap(); // assumed
    c.trim(JointTrim {
        probed: true,
        ..JointTrim::identity(1, -30.0, 30.0)
    })
    .unwrap();
    assert_eq!(c.probed_fraction_pct(), 50);
    assert!(!c.is_complete(2));
}

#[test]
fn a_body_cannot_be_incorporated_twice() {
    let (e, key) = entity();
    let device = BrainKey::from_seed(&[2; 32]);
    let mut map = BodyMap::new();
    let mut rng = Drbg::deterministic(b"a");
    acquire_native(
        e,
        SessionId::from_bytes([7; 32]),
        &key,
        &mut rng,
        &device,
        hexapod_schema(),
        &mut map,
        calibration_for(brainos_abi::ids::BodyId::ZERO),
        6,
        ids(),
    )
    .unwrap();

    let mut rng2 = Drbg::deterministic(b"b");
    let again = acquire_native(
        e,
        SessionId::from_bytes([8; 32]),
        &key,
        &mut rng2,
        &device,
        hexapod_schema(),
        &mut map,
        calibration_for(brainos_abi::ids::BodyId::ZERO),
        6,
        ids(),
    );
    assert_eq!(again.err(), Some(StepError::AlreadyBound));
}

// ---------- the body map ----------

#[test]
fn a_verb_no_region_advertises_cannot_be_found_at_all() {
    let map = BodyMap::new();
    assert!(map.find_capability(&v("legs.walk")).is_none());
    assert!(map.repertoire().is_empty());
}

#[test]
fn two_bodies_offering_the_same_verb_are_both_reported() {
    let (e, key) = entity();
    let mut map = BodyMap::new();
    for (i, seed) in [[2u8; 32], [3u8; 32]].into_iter().enumerate() {
        let device = BrainKey::from_seed(&seed);
        let mut rng = Drbg::deterministic(&[i as u8]);
        acquire_native(
            e,
            SessionId::from_bytes([i as u8 + 1; 32]),
            &key,
            &mut rng,
            &device,
            hexapod_schema(),
            &mut map,
            Calibration::new(brainos_abi::ids::BodyId::ZERO, Tick(1)),
            0,
            {
                let mut n = (i as u8) * 10;
                move || {
                    n += 1;
                    RegionId::from_bytes([n; 32])
                }
            },
        )
        .unwrap();
    }
    assert_eq!(map.body_count(), 2);
    let offering = map.bodies_offering(&v("legs.walk"));
    assert_eq!(
        offering.len(),
        2,
        "the entity must notice the ambiguity rather than pick one"
    );
    // and it can act through a named one
    let b0 = offering[0];
    assert!(map.find_capability_on(&b0, &v("legs.walk")).is_some());
}

#[test]
fn a_dead_region_stops_advertising_its_verbs_immediately() {
    let (e, key) = entity();
    let device = BrainKey::from_seed(&[2; 32]);
    let mut rng = Drbg::deterministic(b"x");
    let mut map = BodyMap::new();
    let (_, regions) = acquire_native(
        e,
        SessionId::from_bytes([7; 32]),
        &key,
        &mut rng,
        &device,
        hexapod_schema(),
        &mut map,
        calibration_for(brainos_abi::ids::BodyId::ZERO),
        6,
        ids(),
    )
    .unwrap();

    assert!(map.find_capability(&v("legs.walk")).is_some());
    let legs = regions
        .iter()
        .find(|r| map.region(r).unwrap().class == RegionClass::Legs)
        .copied()
        .unwrap();
    assert!(map.mark_dead(&legs));
    assert!(
        map.find_capability(&v("legs.walk")).is_none(),
        "the entity cannot keep claiming a hand it no longer has"
    );
    assert!(map.find_capability(&v("voice.say")).is_some());
}

#[test]
fn releasing_a_body_takes_its_regions_with_it() {
    let (e, key) = entity();
    let device = BrainKey::from_seed(&[2; 32]);
    let mut rng = Drbg::deterministic(b"x");
    let mut map = BodyMap::new();
    let (body, _) = acquire_native(
        e,
        SessionId::from_bytes([7; 32]),
        &key,
        &mut rng,
        &device,
        hexapod_schema(),
        &mut map,
        calibration_for(brainos_abi::ids::BodyId::ZERO),
        6,
        ids(),
    )
    .unwrap();
    assert!(map.release(&body));
    assert!(map.regions().is_empty());
    assert_eq!(map.body_count(), 0);
    assert!(!map.release(&body));
}

// ---------- body policy ----------

fn policy_for(body: brainos_abi::ids::BodyId) -> SchemaPolicy {
    let schema = hexapod_schema();
    let env = default_envelope(
        &schema,
        RegionClass::Legs,
        1,
        Duration::from_millis(10),
        Duration::from_millis(200),
    );
    let mut p = SchemaPolicy::new(body, schema)
        .with_envelope(env)
        .calibrated(calibration_for(body));
    p.posture = Posture::Standing;
    p.observe(&Telemetry {
        at: Instant(0),
        link_alive: true,
        rail_v: Some(7.4),
        current_a: Some(0.4),
        tilt_deg: Some(1.0),
        hottest_c: Some(40.0),
        stops: 0,
        overruns: 0,
    });
    p
}

fn args(pairs: &[(&str, Arg)]) -> Args {
    pairs
        .iter()
        .map(|(n, a)| (n.to_string(), a.clone()))
        .collect()
}

#[test]
fn a_body_with_no_policy_bound_refuses_everything() {
    let p = NullPolicy {
        body: brainos_abi::ids::BodyId::ZERO,
    };
    let a = args(&[("speed", Arg::Scalar(0.1))]);
    assert!(p.validate(&v("legs.walk"), &a).is_err());
    assert!(p.simulate(&v("legs.walk"), &a).is_err());
    assert!(p.policy(&v("legs.walk"), &a).is_err());
}

#[test]
fn a_legal_request_passes_all_three_stages() {
    let p = policy_for(brainos_abi::ids::BodyId::from_bytes([5; 32]));
    let a = args(&[("speed", Arg::Scalar(0.3)), ("heading", Arg::Scalar(10.0))]);
    assert!(p.validate(&v("legs.walk"), &a).is_ok());
    assert!(p.simulate(&v("legs.walk"), &a).is_ok());
    assert!(p.policy(&v("legs.walk"), &a).is_ok());
}

#[test]
fn an_argument_the_schema_does_not_mention_is_refused_not_ignored() {
    let p = policy_for(brainos_abi::ids::BodyId::from_bytes([5; 32]));
    let a = args(&[
        ("speed", Arg::Scalar(0.3)),
        ("heading", Arg::Scalar(10.0)),
        ("turbo", Arg::Flag(true)),
    ]);
    let e = p.validate(&v("legs.walk"), &a).unwrap_err();
    assert_eq!(e.code, DenyCode::Malformed);
}

#[test]
fn a_missing_parameter_is_refused_rather_than_defaulted_by_the_body() {
    let p = policy_for(brainos_abi::ids::BodyId::from_bytes([5; 32]));
    let a = args(&[("speed", Arg::Scalar(0.3))]); // no heading
    let e = p.validate(&v("legs.walk"), &a).unwrap_err();
    assert_eq!(e.code, DenyCode::Malformed);
}

#[test]
fn an_out_of_bounds_argument_is_refused_at_validate() {
    let p = policy_for(brainos_abi::ids::BodyId::from_bytes([5; 32]));
    let a = args(&[("speed", Arg::Scalar(5.0)), ("heading", Arg::Scalar(0.0))]);
    let e = p.validate(&v("legs.walk"), &a).unwrap_err();
    assert_eq!(e.code, DenyCode::OutOfBounds);
}

#[test]
fn an_uncalibrated_body_cannot_use_a_consequential_verb() {
    let body = brainos_abi::ids::BodyId::from_bytes([5; 32]);
    let schema = hexapod_schema();
    let env = default_envelope(&schema, RegionClass::Legs, 1, Duration::from_millis(10), Duration::from_millis(200));
    let p = SchemaPolicy::new(body, schema).with_envelope(env);
    let a = args(&[("speed", Arg::Scalar(0.1)), ("heading", Arg::Scalar(0.0))]);
    let e = p.validate(&v("legs.walk"), &a).unwrap_err();
    assert_eq!(e.code, DenyCode::OutsideBodyEnvelope);
}

#[test]
fn a_consequential_region_with_no_envelope_cannot_be_predicted() {
    let body = brainos_abi::ids::BodyId::from_bytes([5; 32]);
    let p = SchemaPolicy::new(body, hexapod_schema()).calibrated(calibration_for(body));
    let a = args(&[("speed", Arg::Scalar(0.1)), ("heading", Arg::Scalar(0.0))]);
    let e = p.simulate(&v("legs.walk"), &a).unwrap_err();
    assert_eq!(
        e.code,
        DenyCode::Unpredictable,
        "refusing to act beats acting on no model"
    );
}

#[test]
fn a_stopped_body_needs_a_decision_not_a_retry() {
    let body = brainos_abi::ids::BodyId::from_bytes([5; 32]);
    let mut p = policy_for(body);
    p.observe(&Telemetry {
        at: Instant(1_000),
        link_alive: true,
        rail_v: Some(7.4),
        current_a: Some(0.1),
        tilt_deg: Some(2.0),
        hottest_c: Some(40.0),
        stops: 1,
        overruns: 0,
    });
    assert_eq!(p.posture, Posture::Stopped);
    let a = args(&[("speed", Arg::Scalar(0.1)), ("heading", Arg::Scalar(0.0))]);
    let e = p.policy(&v("legs.walk"), &a).unwrap_err();
    assert_eq!(e.code, DenyCode::BodyInvariant);
    assert!(e.detail.contains("decision"));
}

#[test]
fn a_lost_link_isolates_the_body_and_a_recovered_one_returns_it_to_standing() {
    let body = brainos_abi::ids::BodyId::from_bytes([5; 32]);
    let mut p = policy_for(body);
    p.posture = Posture::Moving;

    p.observe(&Telemetry { link_alive: false, ..Default::default() });
    assert_eq!(p.posture, Posture::Isolated);

    p.observe(&Telemetry {
        at: Instant(2_000),
        link_alive: true,
        rail_v: Some(7.4),
        tilt_deg: Some(0.0),
        hottest_c: Some(30.0),
        ..Default::default()
    });
    assert_eq!(
        p.posture,
        Posture::Standing,
        "a recovered link must not resume whatever it was doing"
    );
}

#[test]
fn a_brownout_or_an_overheated_actuator_stops_new_motion() {
    let body = brainos_abi::ids::BodyId::from_bytes([5; 32]);
    let a = args(&[("speed", Arg::Scalar(0.1)), ("heading", Arg::Scalar(0.0))]);

    let mut brown = policy_for(body);
    brown.observe(&Telemetry {
        link_alive: true,
        rail_v: Some(5.5),
        tilt_deg: Some(0.0),
        hottest_c: Some(30.0),
        ..Default::default()
    });
    assert!(brown.policy(&v("legs.walk"), &a).is_err());

    let mut hot = policy_for(body);
    hot.observe(&Telemetry {
        link_alive: true,
        rail_v: Some(7.4),
        tilt_deg: Some(0.0),
        hottest_c: Some(85.0),
        ..Default::default()
    });
    let e = hot.policy(&v("legs.walk"), &a).unwrap_err();
    assert!(e.detail.contains("temperature"), "{}", e.detail);
}

#[test]
fn a_non_consequential_verb_still_works_on_a_stowed_body() {
    let body = brainos_abi::ids::BodyId::from_bytes([5; 32]);
    let mut p = policy_for(body);
    p.posture = Posture::Stowed;
    let a = args(&[("text", Arg::Text("hello".to_string()))]);
    assert!(p.policy(&v("voice.say"), &a).is_ok());
    // but walking does not
    let walk = args(&[("speed", Arg::Scalar(0.1)), ("heading", Arg::Scalar(0.0))]);
    assert!(p.policy(&v("legs.walk"), &walk).is_err());
}

// ---------- envelopes derived from schemas ----------

#[test]
fn a_derived_envelope_takes_the_widest_limit_any_verb_declared() {
    // A region's envelope bounds what the region may ever physically do,
    // which Domain 2 runs inside at kilohertz. What *this verb* may do
    // is a different question, enforced at validate against the verb's
    // own parameter bounds.
    //
    // Taking the intersection instead confuses the two, and makes a
    // region with a fast verb and a slow one unable to perform the fast
    // one. An end-to-end run caught it: an arm whose `home` is limited
    // to 60 deg/s capped `move_joint` at 60 too, though the
    // manufacturer declared 180.
    let mut schema = hexapod_schema();
    schema.declare(
        VerbSpec::new(v("legs.crouch"), RegionClass::Legs, "crouch")
            .with(ParamSpec::scalar("heading", Channel::Angle, -5.0, 5.0)),
    );
    let env = default_envelope(
        &schema,
        RegionClass::Legs,
        1,
        Duration::from_millis(10),
        Duration::from_millis(200),
    );
    let angle = env.limit(Channel::Angle).unwrap();
    assert_eq!(
        (angle.min, angle.max),
        (-30.0, 30.0),
        "walk declares +-30 and crouch +-5; the region can do +-30"
    );
    // speed came only from walk
    assert_eq!(env.limit(Channel::Speed).unwrap().max, 0.6);
    // and a channel nothing declared is unstated, not unbounded
    assert!(env.limit(Channel::Current).is_none());
}

#[test]
fn the_narrow_verb_is_still_held_to_its_own_limit_at_validate() {
    // the other half of the same decision: the envelope being wide does
    // not let the narrow verb exceed what it declared
    let mut schema = hexapod_schema();
    schema.declare(
        VerbSpec::new(v("legs.crouch"), RegionClass::Legs, "crouch")
            .with(ParamSpec::scalar("heading", Channel::Angle, -5.0, 5.0)),
    );
    let body = brainos_abi::ids::BodyId::from_bytes([5; 32]);
    let env = default_envelope(
        &schema,
        RegionClass::Legs,
        1,
        Duration::from_millis(10),
        Duration::from_millis(200),
    );
    let mut p = SchemaPolicy::new(body, schema)
        .with_envelope(env)
        .calibrated(calibration_for(body));
    p.posture = Posture::Standing;
    p.observe(&Telemetry {
        link_alive: true,
        rail_v: Some(7.4),
        tilt_deg: Some(0.0),
        hottest_c: Some(30.0),
        ..Default::default()
    });

    // 20 degrees is inside the region's envelope and outside crouch's
    // own bound
    let a = args(&[("heading", Arg::Scalar(20.0))]);
    assert_eq!(
        p.validate(&v("legs.crouch"), &a).unwrap_err().code,
        DenyCode::OutOfBounds
    );
    // and the same value through walk, which declared it, is fine
    let walk = args(&[("speed", Arg::Scalar(0.3)), ("heading", Arg::Scalar(20.0))]);
    assert!(p.validate(&v("legs.walk"), &walk).is_ok());
}

#[test]
fn a_body_that_declares_no_numbers_gets_an_envelope_that_permits_nothing() {
    let schema = CapabilitySchema::new("mystery arm", "ros2").with(
        VerbSpec::new(v("arm.move"), RegionClass::Arm, "move")
            .with(ParamSpec::unbounded("angle", ParamKind::Scalar)),
    );
    let env = default_envelope(
        &schema,
        RegionClass::Arm,
        1,
        Duration::from_millis(10),
        Duration::from_millis(200),
    );
    for ch in Channel::ALL {
        assert!(
            env.limit(ch).is_none(),
            "{ch:?} should be unstated, which refuses"
        );
        assert!(env.check(ch, 0.0).is_err());
    }
}

// ---------- telemetry ----------

#[test]
fn fitness_names_exactly_one_reason_in_the_order_a_human_would_check() {
    let t = Telemetry {
        link_alive: false,
        rail_v: Some(3.0),
        hottest_c: Some(200.0),
        ..Default::default()
    };
    assert!(!t.is_fit());
    assert_eq!(t.unfitness(), Some("link to the body is down"));

    let good = Telemetry {
        link_alive: true,
        rail_v: Some(7.4),
        tilt_deg: Some(1.0),
        hottest_c: Some(35.0),
        ..Default::default()
    };
    assert!(good.is_fit());
    assert_eq!(good.unfitness(), None);
}

#[test]
fn a_body_with_no_sensor_for_something_is_not_assumed_unfit() {
    // all None: the body has no rail sensor, no thermometer, no IMU
    let t = Telemetry {
        link_alive: true,
        ..Default::default()
    };
    assert!(t.is_fit(), "absence of a sensor is not evidence of a fault");
}

#[test]
fn a_join_renders_a_line_for_the_state_graph() {
    let (e, key) = entity();
    let device = BrainKey::from_seed(&[2; 32]);
    let mut rng = Drbg::deterministic(b"x");
    let mut a = Acquisition::discover(e, SessionId::from_bytes([7; 32]));
    a.handshake(&key, &mut rng, device.public(), ABI, |c| device.answer(c), |_, _| true)
        .unwrap();
    a.describe(hexapod_schema()).unwrap();
    let line = a.render_join(Tick(1234));
    assert!(line.contains("described"), "{line}");
    assert!(line.contains("native"), "{line}");
    assert!(line.contains("verbs=3"), "{line}");
    assert!(line.contains("@t1234"), "{line}");
}
