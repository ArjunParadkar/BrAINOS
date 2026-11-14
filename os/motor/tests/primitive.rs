//! BFS  LEVEL    Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)  [crate brainos-motor]
//! BFS  PURPOSE  Tests the primitive library: each primitive survives being stored and read back, refuses parameters outside what it was built for, and produces strokes that respect its own squeeze
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, motor
//! BFS  USED BY  (entry point or test)
//!

use brainos_abi::codec::{Decode, Decoder, Encode, Encoder};
use brainos_motor::condition::{follows, Fact, Pred, State};
use brainos_motor::primitive::{desk_arm_library, place, program, Frame, Given, Kind, MotorPrimitive, Until};

fn lib(kind: Kind) -> MotorPrimitive {
    desk_arm_library().into_iter().find(|p| p.kind == kind).unwrap()
}

#[test]
fn the_library_has_the_six_primitives_the_brief_names() {
    let l = desk_arm_library();
    assert_eq!(l.len(), 6);
    for k in Kind::ALL {
        let p = l.iter().find(|p| p.kind == k).unwrap_or_else(|| panic!("no {}", k.name()));
        assert_eq!(p.verb.as_str(), format!("arm.{}", k.name()));
        assert_eq!(p.body_class, "desk_arm");
        assert!(!p.tests.is_empty(), "{} has no success test", k.name());
        assert!(p.envelope.duration_ms > 0 && p.envelope.force_n > 0.0 && p.envelope.rate_dps > 0.0);
        assert!(!p.effect.adds.is_empty(), "{} achieves nothing", k.name());
    }
}

#[test]
fn a_primitive_read_back_from_bytes_is_the_one_that_was_written() {
    for p in desk_arm_library() {
        let mut e = Encoder::new();
        p.encode(&mut e);
        let bytes = e.finish().unwrap();
        let mut d = Decoder::new(&bytes);
        let back = MotorPrimitive::decode(&mut d).unwrap();
        d.finish().unwrap();
        assert_eq!(back, p);
        // and a cut-off copy is refused, not half-read
        assert!(MotorPrimitive::decode(&mut Decoder::new(&bytes[..bytes.len() - 3])).is_err());
    }
}

#[test]
fn every_primitive_produces_strokes_from_its_defaults() {
    for p in desk_arm_library() {
        let strokes = program(&Given { primitive: &p, values: &[] }).unwrap();
        assert!(strokes.len() >= 4, "{}: {} strokes", p.kind.name(), strokes.len());
        let squeeze = p.param("squeeze_n").unwrap().default;
        for s in &strokes {
            assert!(s.ms > 0);
            assert!(s.squeeze_n <= squeeze + 1e-6, "{} squeezes harder than asked", p.kind.name());
            assert!(s.stiff > 0.0 && s.stiff <= 1.0);
            assert!(s.to.iter().all(|v| v.is_finite()));
        }
    }
}

#[test]
fn a_value_outside_what_the_primitive_was_built_for_is_refused() {
    let wipe = lib(Kind::Wipe);
    assert!(program(&Given { primitive: &wipe, values: &[("radius_m", 0.02)] }).is_ok());
    assert!(program(&Given { primitive: &wipe, values: &[("radius_m", 0.20)] }).is_err());
    assert!(program(&Given { primitive: &wipe, values: &[("radius_m", f32::NAN)] }).is_err());
    assert!(program(&Given { primitive: &wipe, values: &[("speed", 3.0)] }).is_err(), "no such parameter");
}

#[test]
fn a_grasp_closes_only_after_arriving_and_lifts_only_once_it_holds() {
    let g = lib(Kind::Grasp);
    let s = program(&Given { primitive: &g, values: &[("width_m", 0.03), ("squeeze_n", 6.0)] }).unwrap();
    // open on the way in
    assert!(s[0].open_m > 0.015 && s[1].open_m > 0.015);
    assert_eq!(s[0].squeeze_n, 0.0);
    // the closing stroke does not move, and ends on feeling the object
    assert_eq!(s[2].to, s[1].to);
    assert!(matches!(s[2].until, Until::Gripped(n) if n > 0.0));
    assert!(s[2].rest);
    // then up, still squeezing
    assert!(s[3].to[2] > s[2].to[2]);
    assert_eq!(s[3].squeeze_n, 6.0);
}

#[test]
fn a_wipe_goes_round_at_the_radius_it_was_given() {
    let w = lib(Kind::Wipe);
    let s = program(&Given { primitive: &w, values: &[("radius_m", 0.025), ("press_m", 0.002), ("turns", 2.0)] }).unwrap();
    let far = s.iter().map(|k| (k.to[0] * k.to[0] + k.to[1] * k.to[1]).sqrt()).fold(0.0f32, f32::max);
    // set out so that the chords between points touch 27 mm
    let chord = far * (core::f32::consts::PI / 16.0).cos();
    assert!((chord - 0.027).abs() < 1e-5, "{far} {chord}");
    let one = program(&Given { primitive: &w, values: &[("radius_m", 0.025), ("turns", 1.0)] }).unwrap();
    assert!(s.len() > one.len(), "more turns, more strokes");
}

#[test]
fn placing_a_program_turns_it_with_its_frame() {
    let g = lib(Kind::Push);
    let s = program(&Given { primitive: &g, values: &[("distance_m", 0.08), ("radius_m", 0.02)] }).unwrap();
    let east = place(&s, &Frame { pos: [0.2, 0.0, 0.0], yaw_deg: 0.0 });
    let north = place(&s, &Frame { pos: [0.2, 0.0, 0.0], yaw_deg: 90.0 });
    for (a, b) in east.iter().zip(north.iter()) {
        // what was along x is now along y
        assert!((a.at[0] - 0.2 - (b.at[1])).abs() < 1e-5);
        assert!((a.at[2] - b.at[2]).abs() < 1e-6);
        assert!((b.at[3] - a.at[3] - 90.0).abs() < 1e-4);
    }
}

#[test]
fn roles_are_bound_and_the_steps_of_a_task_chain_or_do_not() {
    let grasp = lib(Kind::Grasp).bound("brush", "");
    let wipe = lib(Kind::Wipe);
    let place_ = lib(Kind::Place).bound("brush", "holder");
    let mut state = State::start();

    // wiping before holding anything: refused, and it says what is missing
    let mut w = wipe.bound("mug", "");
    for f in w.needs.iter_mut() {
        if f.about.as_str() == "tool" {
            *f = Fact::new(f.pred, "brush");
        }
    }
    assert_eq!(follows(&state, &w), Err(Fact::new(Pred::Holding, "brush")));

    grasp.apply(&mut state).unwrap();
    assert!(state.holds(&Fact::new(Pred::Holding, "brush")));
    assert!(!state.holds(&Fact::hand_free()));
    assert!(follows(&state, &w).is_ok());
    w.apply(&mut state).unwrap();
    // a second grasp with the hand full does not follow
    assert_eq!(follows(&state, &lib(Kind::Grasp).bound("mug", "")), Err(Fact::hand_free()));
    place_.apply(&mut state).unwrap();
    assert!(state.holds(&Fact::hand_free()));
    assert!(state.holds(&Fact::new(Pred::Wiped, "mug")));
    assert!(state.holds(&Fact::new(Pred::Rests, "brush")));
}
