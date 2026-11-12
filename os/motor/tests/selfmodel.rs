//! BFS  LEVEL    Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)  [crate brainos-motor]
//! BFS  PURPOSE  Tests that a self-model is refused when malformed, is emitted the same way every time, and names every assumed number in its output
//! BFS  DEFINES  -
//! BFS  USES     brainos: motor
//! BFS  USED BY  (entry point or test)
//!

use brainos_motor::mjcf::{digest, emit, Drive, Emit};
use brainos_motor::model::{Base, BodyModel, JointSpec, Link, ModelError, Num};

fn two_link() -> BodyModel {
    let mut m = BodyModel::new("two", Base::Fixed);
    let base = m.push(Link::new("base", None).block([0.05, 0.05, 0.05]).weighing(Num::Stated(1.0)));
    let a = m.push(
        Link::new("a", Some(base))
            .jointed(
                JointSpec::hinge("ja", [0.0, -1.0, 0.0], -90.0, 90.0)
                    .effort(Num::Stated(2.0))
                    .rate(Num::Stated(90.0)),
            )
            .rod([0.2, 0.0, 0.0], 0.01)
            .weighing(Num::Stated(0.3)),
    );
    m.push(
        Link::new("b", Some(a))
            .at([0.2, 0.0, 0.0])
            .jointed(JointSpec::hinge("jb", [0.0, -1.0, 0.0], -120.0, 0.0).effort(Num::Assumed(1.0)))
            .rod([0.15, 0.0, 0.0], 0.01)
            .weighing(Num::Assumed(0.2)),
    );
    m.site("tip", 2, [0.15, 0.0, 0.0]);
    m
}

#[test]
fn a_sound_model_validates_and_reports_its_mass() {
    let m = two_link();
    assert_eq!(m.validate(), Ok(()));
    assert!((m.total_mass() - 1.5).abs() < 1e-6);
    assert_eq!(m.actuated().count(), 2);
}

#[test]
fn malformed_models_are_refused_each_for_its_own_reason() {
    assert_eq!(BodyModel::new("x", Base::Fixed).validate(), Err(ModelError::Empty));

    let mut m = two_link();
    m.links[1].parent = Some(2);
    assert_eq!(m.validate(), Err(ModelError::BadParent));

    let mut m = two_link();
    m.links[2].joint.as_mut().unwrap().min = 10.0;
    assert_eq!(m.validate(), Err(ModelError::BadLimit));

    let mut m = two_link();
    m.links[2].joint.as_mut().unwrap().name = "ja".into();
    assert_eq!(m.validate(), Err(ModelError::DuplicateName));

    let mut m = two_link();
    m.links[2].joint.as_mut().unwrap().follows = Some((0, 1.0));
    assert_eq!(m.validate(), Err(ModelError::BadFollow));

    let mut m = two_link();
    m.links[1].mass = Num::Stated(f32::NAN);
    assert_eq!(m.validate(), Err(ModelError::BadNumber));

    let mut m = two_link();
    m.links[0].joint = Some(JointSpec::hinge("root", [0.0, 0.0, 1.0], -1.0, 1.0));
    assert_eq!(m.validate(), Err(ModelError::BadRoot));
}

#[test]
fn a_malformed_model_is_never_emitted() {
    let mut m = two_link();
    m.links[2].joint.as_mut().unwrap().max = f32::INFINITY;
    assert!(emit(&m, &Emit::default()).is_err());
}

#[test]
fn every_assumed_number_is_named_in_the_output_and_stated_ones_are_not() {
    let m = two_link();
    let text = emit(&m, &Emit::default()).unwrap();
    assert!(text.contains("ASSUMED: mass of b assumed 0.2000 kg"));
    assert!(text.contains("ASSUMED: effort of jb assumed 1.0000"));
    assert!(text.contains("ASSUMED: rate of jb assumed"));
    assert!(!text.contains("mass of a assumed"));
    assert!(!text.contains("effort of ja assumed"));
    assert_eq!(m.assumptions().len(), 3);
}

#[test]
fn emission_is_deterministic_and_the_digest_follows_the_text() {
    let m = two_link();
    let a = emit(&m, &Emit::default()).unwrap();
    let b = emit(&m, &Emit::default()).unwrap();
    assert_eq!(a, b);
    assert!(digest(&a).ct_eq(&digest(&b)));

    let mut heavier = two_link();
    heavier.links[1].mass = Num::Stated(0.31);
    let c = emit(&heavier, &Emit::default()).unwrap();
    assert!(!digest(&a).ct_eq(&digest(&c)), "a different body has a different identity");
}

#[test]
fn a_follower_is_written_as_a_constraint_and_gets_no_actuator() {
    let mut m = two_link();
    let j = m.links[2].joint.take().unwrap();
    m.links[2].joint = Some(j.following(1, -1.0));
    let text = emit(&m, &Emit::default()).unwrap();
    assert!(text.contains("<joint joint1=\"jb\" joint2=\"ja\" polycoef=\"0 -1.000000 0 0 0\"/>"));
    assert!(text.contains("<motor name=\"ja\""));
    assert!(!text.contains("<motor name=\"jb\""));
}

#[test]
fn a_servo_drive_states_its_range_in_radians() {
    let text = emit(&two_link(), &Emit { drive: Drive::Servo { kp: 5.0, kv: 0.1 }, ..Emit::default() }).unwrap();
    // ja is +-90 degrees: +-1.570796 rad
    assert!(text.contains("ctrlrange=\"-1.570796 1.570796\""), "{text}");
}
