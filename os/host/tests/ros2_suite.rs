//! A robot we did not build, on the middleware everybody actually uses.

use brainos_host::ros2_stack::run;

#[test]
fn the_adapter_is_brittle_in_the_right_direction() {
    let r = run();
    assert!(
        r.is_brittle_in_the_right_direction(),
        "the adapter did not behave as the risk register describes: {r:#?}"
    );
}

/// The adapter reads the same robot the same way both times. The only
/// difference between a robot BrAInOS can use and one it cannot is four
/// numbers from the manual.
#[test]
fn declaring_limits_is_the_only_difference() {
    let r = run();
    assert_eq!(
        r.shipped.verbs, r.declared.verbs,
        "the adapter derived different verbs from the same node graph"
    );
    assert_eq!(r.shipped.sense_only_topics, r.declared.sense_only_topics);
    assert_eq!(r.shipped.actionable_pct, 0);
    assert!(r.declared.actionable_pct > 50);
}

/// A ROS topic declares a message *type* and nothing about what values
/// are safe: `/cmd_vel` takes a Twist, and the type is equally happy
/// with 0.2 m/s and 20 m/s. A robot that publishes no limits is not
/// taken on at all — which is stronger than the risk register's wording
/// and is the behaviour worth having.
#[test]
fn a_robot_that_declares_nothing_is_not_taken_on() {
    let r = run();
    assert!(
        r.shipped.incorporated.is_err(),
        "an entity took on a body it could never command"
    );
    assert!(r.shipped.body.is_none());
}

#[test]
fn a_robot_that_declares_its_limits_works() {
    let r = run();
    assert!(r.declared.incorporated.is_ok());
    assert!(
        r.declared.modest_drive.is_none(),
        "a modest drive command was refused: {:?}",
        r.declared.modest_drive
    );
    assert!(
        r.declared.lift.is_none(),
        "the lift was refused: {:?} — this was the multi-region envelope bug",
        r.declared.lift
    );
}

/// And there is deliberately no default speed. A reasonable default for
/// a speed is how a robot gets driven into a wall at a speed nobody
/// chose.
#[test]
fn there_is_no_default_speed() {
    let r = run();
    let refusal = r
        .declared
        .reckless_drive
        .as_ref()
        .expect("20 m/s was granted");
    assert!(
        refusal.contains("OutOfBounds"),
        "refused for the wrong reason: {refusal}"
    );
}

/// Sensor topics are telemetry, not verbs. There is nothing to
/// authorize about reading an odometer, and a schema that turned every
/// published topic into a capability would make the gate meaningless.
#[test]
fn sensor_topics_do_not_become_verbs() {
    let r = run();
    assert_eq!(r.declared.sense_only_topics, 3);
    assert!(
        !r.declared.verbs.iter().any(|v| v.contains("odom")
            || v.contains("scan")
            || v.contains("battery")),
        "a sensor topic became a verb: {:?}",
        r.declared.verbs
    );
}
