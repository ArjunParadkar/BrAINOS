//! The body-side suite.
//!
//! One property matters more than the rest and most of this file is
//! about it: **the body checks the same envelope the seat checked, and
//! refuses anyway.** A compromised seat, a corrupted token, or a bug in
//! the gate must not be able to push the body past what its own reflex
//! layer allows.

use brainos_abi::time::{Duration, Instant};
use brainos_abi::verb::Verb;
use brainos_bodyd::hexapod::{
    Hexapod, HexapodConfig, MARGIN_FLOOR_MM, MAX_MOTION, MIN_GAIT_SPEED, TOP_SPEED_MPS,
};
use brainos_bodyd::mechanism::{Mechanism, Refusal, Sample};
use brainos_bodyd::protocol::{Command, Report, Wire};
use brainos_bodyd::runtime::{BodyRuntime, Measured, State};
use brainos_rt::envelope::{Channel, Envelope, Limit};
use brainos_rt::reflex::StopReason;
use miniblur_core::geometry::{LegGeom, Vec2};
use miniblur_core::servo::ServoTable;

fn v(s: &str) -> Verb {
    Verb::parse(s).unwrap()
}

fn calibrated_hexapod() -> Hexapod {
    let mut h = Hexapod::new(HexapodConfig::default());
    h.calibrate(ServoTable::bom_10_servo());
    h
}

/// An envelope wide enough not to be the thing refusing.
fn generous_envelope() -> Envelope {
    Envelope::new(1, Duration::from_millis(20), Duration::from_millis(200))
        .with(Channel::Angle, -90.0, 90.0)
        .with(Channel::Current, 0.0, 10.0)
        .with(Channel::Tilt, -45.0, 45.0)
        .with(Channel::Voltage, 6.0, 8.4)
}

fn measured() -> Measured {
    Measured {
        rail_v: Some(7.4),
        current_a: Some(0.4),
        tilt_deg: Some(1.0),
        hottest_c: Some(38.0),
    }
}

fn walk(speed: f32) -> Command {
    Command::new(v("legs.walk"))
        .arg("speed", speed)
        .arg("heading", 0.0)
}

// ---------- the mechanism, on its own ----------

#[test]
fn an_uncalibrated_hexapod_refuses_to_walk() {
    let h = Hexapod::new(HexapodConfig::default());
    assert!(!h.is_calibrated());
    assert_eq!(
        h.plan(&v("legs.walk"), &[("speed", 0.08), ("heading", 0.0)]),
        Err(Refusal::Uncalibrated),
        "its numbers would be guesses"
    );
}

#[test]
fn a_calibrated_hexapod_plans_a_real_gait() {
    let h = calibrated_hexapod();
    let m = h
        .plan(&v("legs.walk"), &[("speed", 0.08), ("heading", 0.0)])
        .expect("a walk it can do");
    assert_eq!(m.verb, v("legs.walk"));
    assert!(m.len() >= 40, "a whole gait cycle of samples");
    assert!(m.worst_margin > MARGIN_FLOOR_MM);
    assert!(m.duration > Duration::ZERO);
    assert!(m.summary.contains("worst margin"));
    // every sample carries the margin the body predicted for it
    assert!(m.samples.iter().all(|s| s.margin.is_finite()));
}

#[test]
fn the_body_forgets_its_calibration_at_power_off() {
    let mut h = calibrated_hexapod();
    assert!(h.is_calibrated());
    h.forget_calibration();
    assert!(!h.is_calibrated());
    // the Memory-Integrity Law: a body that remembered its own
    // calibration would be a body whose memory the entity does not
    // control
    assert_eq!(
        h.plan(&v("legs.walk"), &[("speed", 0.05), ("heading", 0.0)]),
        Err(Refusal::Uncalibrated)
    );
}

#[test]
fn a_speed_the_body_cannot_do_is_refused_by_the_body() {
    let h = calibrated_hexapod();
    assert_eq!(
        h.plan(
            &v("legs.walk"),
            &[("speed", TOP_SPEED_MPS * 10.0), ("heading", 0.0)]
        ),
        Err(Refusal::OutsideEnvelope)
    );
}

#[test]
fn a_body_whose_centre_of_mass_has_moved_refuses_a_gait_it_used_to_do() {
    // the interesting case: the seat's schema has not changed, the
    // command is identical, and the body says no because *its* geometry
    // now says the gait would tip it
    let good = calibrated_hexapod();
    assert!(good
        .plan(&v("legs.walk"), &[("speed", 0.08), ("heading", 0.0)])
        .is_ok());

    let mut geom = LegGeom::default();
    // somebody bolted a battery right at the back
    geom.com = Vec2::new(-70.0, 0.0);
    let mut heavy = Hexapod::new(HexapodConfig {
        geom,
        ..HexapodConfig::default()
    });
    heavy.calibrate(ServoTable::bom_10_servo());
    assert_eq!(
        heavy.plan(&v("legs.walk"), &[("speed", 0.08), ("heading", 0.0)]),
        Err(Refusal::WouldBreakInvariant),
        "the body's own forward model is what refuses"
    );
}

#[test]
fn a_flat_battery_stops_the_body_walking() {
    let mut h = Hexapod::new(HexapodConfig {
        vbat: 6.0,
        ..HexapodConfig::default()
    });
    h.calibrate(ServoTable::bom_10_servo());
    assert_eq!(
        h.plan(&v("legs.walk"), &[("speed", 0.08), ("heading", 0.0)]),
        Err(Refusal::PowerFloor)
    );
}

#[test]
fn a_rail_that_cannot_feed_the_gait_refuses_it() {
    let mut h = Hexapod::new(HexapodConfig {
        power: miniblur_core::power::PowerLimits {
            rail_amps: 0.5, // a UBEC far too small for ten servos
            ..Default::default()
        },
        ..HexapodConfig::default()
    });
    h.calibrate(ServoTable::bom_10_servo());
    assert_eq!(
        h.plan(&v("legs.walk"), &[("speed", 0.08), ("heading", 0.0)]),
        Err(Refusal::PowerFloor),
        "a brown-out mid-write is exactly the failure the journal exists for"
    );
}

#[test]
fn a_motion_whose_forward_model_would_go_stale_is_refused() {
    let h = calibrated_hexapod();
    // four whole gait cycles is long enough that the tip-over margin
    // computed at planning time has had seconds to stop being true
    let long = h.plan(
        &v("legs.walk"),
        &[("speed", 0.05), ("heading", 0.0), ("steps", 4.0)],
    );
    assert_eq!(long, Err(Refusal::TooLong));
    assert!(MAX_MOTION < Duration::from_secs(5));
}

#[test]
fn a_gait_slower_than_the_body_will_walk_is_refused_rather_than_clamped() {
    let h = calibrated_hexapod();
    // a robot creeping is less stable than a robot walking
    let creep = h.plan(
        &v("legs.walk"),
        &[("speed", TOP_SPEED_MPS * MIN_GAIT_SPEED * 0.5), ("heading", 0.0)],
    );
    assert_eq!(
        creep,
        Err(Refusal::OutsideEnvelope),
        "quietly walking faster than asked would be the body lying about what it did"
    );
    // and just above the floor is fine
    assert!(h
        .plan(
            &v("legs.walk"),
            &[("speed", TOP_SPEED_MPS * (MIN_GAIT_SPEED + 0.05)), ("heading", 0.0)]
        )
        .is_ok());
}

#[test]
fn an_unknown_verb_is_refused_as_a_configuration_mismatch_not_a_disagreement() {
    let h = calibrated_hexapod();
    let r = h.plan(&v("arm.grasp"), &[]).unwrap_err();
    assert_eq!(r, Refusal::UnknownVerb);
    assert!(
        !r.is_a_disagreement(),
        "not having a verb is a schema mismatch, not a difference of opinion about physics"
    );
    assert!(Refusal::WouldBreakInvariant.is_a_disagreement());
    assert!(Refusal::OutsideEnvelope.is_a_disagreement());
}

#[test]
fn a_malformed_command_is_refused_before_anything_is_planned() {
    let h = calibrated_hexapod();
    assert_eq!(h.plan(&v("legs.walk"), &[]), Err(Refusal::Malformed));
    assert_eq!(
        h.plan(&v("legs.walk"), &[("speed", f32::NAN), ("heading", 0.0)]),
        Err(Refusal::Malformed)
    );
    assert_eq!(h.plan(&v("legs.turn"), &[("rate", 0.0)]), Err(Refusal::Malformed));
}

#[test]
fn standing_and_sitting_work_and_turning_uses_the_real_gait() {
    let h = calibrated_hexapod();
    assert!(h.plan(&v("legs.stand"), &[("height", 0.0)]).is_ok());
    assert!(h.plan(&v("legs.sit"), &[("height", -20.0)]).is_ok());
    let turn = h.plan(&v("legs.turn"), &[("rate", 20.0)]).unwrap();
    assert!(turn.len() > 1, "a turn is a gait, not a pose");
    assert!(turn.worst_margin > MARGIN_FLOOR_MM);
}

#[test]
fn a_legged_body_holds_itself_up_when_it_stops() {
    let h = calibrated_hexapod();
    assert!(h.holds_itself_up(), "releasing torque means falling");
    let safe = h.safe();
    assert!(safe.commanded().any(|(ch, _)| ch == Channel::Current));
}

#[test]
fn the_native_envelope_comes_from_the_gait_rather_than_from_a_typed_number() {
    let h = calibrated_hexapod();
    let e = h.native_envelope(7);
    let angle = e.limit(Channel::Angle).expect("the gait bounds the joint");
    let gait = HexapodConfig::default().gait;
    assert_eq!(angle.max, gait.swing_amp_deg.max(gait.lift_deg));
    assert!(e.limit(Channel::Current).is_some());
    assert!(e.limit(Channel::Tilt).is_some());
    // a channel this body has no opinion about stays unstated, which
    // refuses rather than permitting
    assert!(e.limit(Channel::Clearance).is_none());
}

// ---------- the two envelopes ----------

#[test]
fn the_body_keeps_the_tighter_of_the_two_envelopes() {
    let h = calibrated_hexapod();
    let native_angle = h.native_envelope(1).limit(Channel::Angle).unwrap();
    let mut rt = BodyRuntime::new(h, Instant(0));

    // the seat grants something far wider than the body's own numbers
    rt.narrow_to(
        Envelope::new(1, Duration::from_millis(20), Duration::from_millis(200))
            .with(Channel::Angle, -180.0, 180.0)
            .with(Channel::Current, 0.0, 100.0),
    );
    let effective = rt.effective_envelope().unwrap();
    let angle = effective.limit(Channel::Angle).unwrap();
    assert_eq!(
        (angle.min, angle.max),
        (native_angle.min, native_angle.max),
        "a seat that grants more than the body can do gets what the body can do"
    );
}

#[test]
fn a_seat_that_grants_less_than_the_body_could_do_gets_less() {
    let h = calibrated_hexapod();
    let mut rt = BodyRuntime::new(h, Instant(0));
    rt.narrow_to(
        Envelope::new(1, Duration::from_millis(20), Duration::from_millis(200))
            .with(Channel::Angle, -5.0, 5.0)
            .with(Channel::Current, 0.0, 1.0),
    );
    let angle = rt.effective_envelope().unwrap().limit(Channel::Angle).unwrap();
    assert_eq!((angle.min, angle.max), (-5.0, 5.0));
}

#[test]
fn an_empty_intersection_refuses_rather_than_picking_one() {
    let h = calibrated_hexapod();
    let mut rt = BodyRuntime::new(h, Instant(0));
    // the seat insists on angles the body cannot reach at all
    rt.narrow_to(
        Envelope::new(1, Duration::from_millis(20), Duration::from_millis(200))
            .with(Channel::Angle, 120.0, 180.0),
    );
    let effective = rt.effective_envelope().unwrap();
    assert!(
        effective.limit(Channel::Angle).is_none(),
        "when the seat and the body disagree about what is possible, nothing is authorized"
    );
    assert!(effective.check(Channel::Angle, 0.0).is_err());
}

// ---------- the runtime ----------

fn running_body() -> BodyRuntime<Hexapod> {
    let mut rt = BodyRuntime::new(calibrated_hexapod(), Instant(0));
    rt.narrow_to(generous_envelope());
    rt.ready();
    rt
}

#[test]
fn a_body_with_no_calibration_accepts_nothing() {
    let mut rt = BodyRuntime::new(Hexapod::new(HexapodConfig::default()), Instant(0));
    rt.narrow_to(generous_envelope());
    assert_eq!(rt.state(), State::Uncalibrated);
    assert!(matches!(
        rt.accept(&walk(0.08), Instant(0)),
        Report::Refused {
            refusal: Refusal::Uncalibrated,
            ..
        }
    ));
}

#[test]
fn a_body_with_no_granted_envelope_accepts_nothing() {
    let mut rt = BodyRuntime::new(calibrated_hexapod(), Instant(0));
    rt.ready();
    assert!(matches!(
        rt.accept(&walk(0.08), Instant(0)),
        Report::Refused {
            refusal: Refusal::OutsideEnvelope,
            ..
        }
    ));
}

#[test]
fn an_accepted_motion_plays_sample_by_sample() {
    let mut rt = running_body();
    let accepted = rt.accept(&walk(0.08), Instant(0));
    let samples = match accepted {
        Report::Accepted { samples, .. } => samples,
        other => panic!("expected acceptance, got {other:?}"),
    };
    assert!(samples > 0);
    assert_eq!(rt.state(), State::Playing);

    // step by roughly one sample's hold, with plenty of slack
    let mut played = 0;
    let mut t = 0u64;
    for _ in 0..(samples as u64 * 3 + 20) {
        rt.heard_from_seat(Instant(t));
        let pass = rt.tick(Instant(t), measured());
        if pass.applied.is_some() {
            played += 1;
        }
        if pass.state == State::Idle {
            break;
        }
        t += 50_000;
    }
    assert_eq!(played, samples, "every sample was played");
    assert_eq!(rt.state(), State::Idle);
}

#[test]
fn a_lost_link_stops_the_body_mid_motion() {
    let mut rt = running_body();
    rt.accept(&walk(0.08), Instant(0));
    // the seat goes quiet
    let mut t = 0u64;
    let mut stopped_at = None;
    for _ in 0..100 {
        let pass = rt.tick(Instant(t), measured());
        if pass.state == State::Stopped {
            stopped_at = pass.stop;
            break;
        }
        t += 20_000;
    }
    let stop = stopped_at.expect("the body stops when the seat goes quiet");
    assert_eq!(stop.reason, StopReason::LinkLost);
    assert_eq!(rt.state(), State::Stopped);
}

#[test]
fn a_stopped_body_needs_a_fresh_envelope_rather_than_a_retry() {
    let mut rt = running_body();
    rt.commanded_stop(Instant(0));
    assert_eq!(rt.state(), State::Stopped);
    assert!(matches!(
        rt.accept(&walk(0.08), Instant(0)),
        Report::Refused {
            refusal: Refusal::Stopped,
            ..
        }
    ));

    // only a decision from above clears it
    rt.resume(generous_envelope(), Instant(1_000));
    assert_eq!(rt.state(), State::Idle);
    assert!(matches!(
        rt.accept(&walk(0.08), Instant(1_000)),
        Report::Accepted { .. }
    ));
}

#[test]
fn a_measurement_outside_the_envelope_stops_the_body() {
    let mut rt = running_body();
    rt.accept(&walk(0.08), Instant(0));
    rt.heard_from_seat(Instant(0));

    // the rail sags below what the envelope permits
    let pass = rt.tick(
        Instant(1_000),
        Measured {
            rail_v: Some(5.0),
            ..measured()
        },
    );
    assert_eq!(pass.state, State::Stopped);
    assert_eq!(pass.stop.unwrap().reason, StopReason::EnvelopeViolation);
    assert_eq!(
        pass.stop.unwrap().violation.unwrap().channel,
        Channel::Voltage
    );
}

#[test]
fn an_implausible_sensor_stops_the_body_rather_than_being_acted_on() {
    let mut rt = running_body();
    rt.accept(&walk(0.08), Instant(0));
    rt.heard_from_seat(Instant(0));
    let pass = rt.tick(
        Instant(1_000),
        Measured {
            tilt_deg: Some(f32::NAN),
            ..measured()
        },
    );
    assert_eq!(pass.stop.unwrap().reason, StopReason::SensorImplausible);
}

#[test]
fn the_body_reports_telemetry_every_pass_even_when_nothing_happens() {
    let mut rt = running_body();
    rt.heard_from_seat(Instant(0));
    let pass = rt.tick(Instant(1_000), measured());
    assert!(pass
        .reports
        .iter()
        .any(|r| matches!(r, Report::Telemetry { .. })));
}

#[test]
fn a_stop_is_reported_up_the_wire() {
    let mut rt = running_body();
    rt.accept(&walk(0.08), Instant(0));
    rt.commanded_stop(Instant(100));
    let pass = rt.tick(Instant(200), measured());
    assert!(pass.reports.iter().any(|r| matches!(
        r,
        Report::Stopped { reason, .. } if *reason == StopReason::Commanded as u8
    )));
}

#[test]
fn the_first_stop_reason_is_the_one_kept() {
    let mut rt = running_body();
    rt.accept(&walk(0.08), Instant(0));
    rt.commanded_stop(Instant(100));
    // and now the link also dies
    let pass = rt.tick(Instant(10_000_000), measured());
    assert_eq!(
        pass.stop.unwrap().reason,
        StopReason::Commanded,
        "the first reason is the true one; later stops are consequences"
    );
}

#[test]
fn a_finished_motion_is_reported_and_the_body_returns_to_idle() {
    let mut rt = BodyRuntime::new(calibrated_hexapod(), Instant(0));
    rt.narrow_to(generous_envelope());
    rt.ready();
    rt.accept(&Command::new(v("legs.stand")).arg("height", 0.0), Instant(0));

    let mut t = 0u64;
    let mut finished = false;
    for _ in 0..20 {
        rt.heard_from_seat(Instant(t));
        let pass = rt.tick(Instant(t), measured());
        if pass
            .reports
            .iter()
            .any(|r| matches!(r, Report::Finished { .. }))
        {
            finished = true;
            break;
        }
        t += 100_000;
    }
    assert!(finished);
    assert_eq!(rt.state(), State::Idle);
}

// ---------- the protocol ----------

#[test]
fn a_command_round_trips_through_the_wire() {
    let c = walk(0.08).under([7; 32]);
    let bytes = Wire::encode_command(&c).unwrap();
    assert_eq!(Wire::decode_command(&bytes).unwrap(), c);
}

#[test]
fn a_command_with_a_duplicated_argument_does_not_decode() {
    use brainos_abi::codec::Encoder;
    let mut e = Encoder::new();
    brainos_abi::codec::Encode::encode(&v("legs.walk"), &mut e);
    e.seq(2);
    e.str("speed");
    e.f32(0.05);
    e.str("speed");
    e.f32(0.5);
    e.raw(&[0u8; 32]);
    let bytes = e.finish().unwrap();
    assert!(
        Wire::decode_command(&bytes).is_err(),
        "'which one did the body use' must not be unanswerable"
    );
}

#[test]
fn every_report_round_trips() {
    let reports = [
        Report::Accepted {
            verb: v("legs.walk"),
            samples: 48,
            duration_ms: 1200,
            worst_margin: 9.5,
        },
        Report::Refused {
            verb: v("legs.walk"),
            refusal: Refusal::WouldBreakInvariant,
            disagreement: true,
        },
        Report::Finished {
            verb: v("legs.walk"),
            samples_played: 48,
        },
        Report::Stopped {
            reason: StopReason::LinkLost as u8,
            samples_played: 12,
        },
        Report::Telemetry {
            rail_v: 7.4,
            current_a: 0.42,
            tilt_deg: 1.2,
            hottest_c: 38.0,
            margin_mm: 9.5,
            stops: 0,
            overruns: 0,
        },
    ];
    for r in reports {
        let bytes = Wire::encode_report(&r).unwrap();
        assert_eq!(Wire::decode_report(&bytes).unwrap(), r);
    }
}

#[test]
fn a_command_fits_in_one_transport_frame() {
    // a command that had to be fragmented would be a command that can
    // arrive half-applied
    let c = walk(0.08).under([1; 32]);
    let bytes = Wire::encode_command(&c).unwrap();
    assert!(
        bytes.len() <= brainos_transport::frame::MAX_PAYLOAD,
        "{} bytes",
        bytes.len()
    );
}

#[test]
fn a_report_fits_in_one_transport_frame() {
    let r = Report::Telemetry {
        rail_v: 7.4,
        current_a: 0.42,
        tilt_deg: 1.2,
        hottest_c: 38.0,
        margin_mm: 9.5,
        stops: 0,
        overruns: 0,
    };
    assert!(Wire::encode_report(&r).unwrap().len() <= brainos_transport::frame::MAX_PAYLOAD);
}

// ---------- samples ----------

#[test]
fn a_sample_carries_a_bounded_number_of_commands() {
    let mut s = Sample::hold_for(0, Duration::from_millis(10));
    for _ in 0..20 {
        s = s.with(Channel::Angle, 1.0);
    }
    assert_eq!(s.commanded().count(), 8, "fixed width, no allocation");
}
