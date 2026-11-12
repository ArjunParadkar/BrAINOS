//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Tests the ROS 2 graph reader's parsing, its network switch, and that a live graph with no limits is refused by section 8
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, adapters, body, crypto, entity, host
//! BFS  USED BY  (entry point or test)
//!
//! The ROS 2 live-graph reader: parsing, and the switches around it.
//! No ROS 2 system is contacted here except in the last test, which
//! runs only where `ros2` is installed and `BRAINOS_NET=on`.

use brainos_adapters::ros2::{actionable_fraction_pct, from_node_graph};
use brainos_host::nerves::ros2::{
    graph_from, live_graph, parse_limits, parse_topic_info, parse_typed_list, Ros2Unavailable,
};

const TOPICS: &str = "/cmd_vel [geometry_msgs/msg/Twist]\n/odom [nav_msgs/msg/Odometry]\n\
/parameter_events [rcl_interfaces/msg/ParameterEvent]\n/rosout [rcl_interfaces/msg/Log]\n";
const SERVICES: &str = "/clear [std_srvs/srv/Empty]\n\
/turtlesim/get_parameters [rcl_interfaces/srv/GetParameters]\n";

fn info(t: &str) -> Option<(u32, u32)> {
    match t {
        "/cmd_vel" => Some((0, 1)),
        "/odom" => Some((1, 0)),
        _ => None,
    }
}

#[test]
fn the_tools_output_is_parsed_into_names_and_types() {
    let t = parse_typed_list(TOPICS);
    assert_eq!(t.len(), 4);
    assert_eq!(t[0], ("/cmd_vel".to_string(), "geometry_msgs/msg/Twist".to_string()));
    assert!(parse_typed_list("garbage\n[nope]\n").is_empty());
    assert_eq!(
        parse_topic_info("Type: x\nPublisher count: 2\nSubscription count: 1\n"),
        Some((2, 1))
    );
    assert_eq!(parse_topic_info("Type: x\n"), None);
}

#[test]
fn a_subscribed_topic_is_a_command_and_ros_plumbing_is_left_out() {
    let g = graph_from("turtle", &parse_typed_list(TOPICS), &parse_typed_list(SERVICES), info, &[]);
    let names: Vec<&str> = g.topics.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["/cmd_vel", "/odom"]);
    assert!(g.topics[0].subscribed && !g.topics[0].published);
    assert!(g.topics[1].published);
    assert_eq!(g.services.len(), 1, "parameter services describe the node, not the robot");
}

#[test]
fn a_live_graph_with_no_declared_limits_has_nothing_actionable() {
    let g = graph_from("turtle", &parse_typed_list(TOPICS), &[], info, &[]);
    let schema = from_node_graph(&g).expect("a usable graph");
    assert_eq!(actionable_fraction_pct(&schema), 0);

    let limits = parse_limits("/cmd_vel linear_x -0.4 1.2\n/cmd_vel angular_z -0.8 0.8\n").unwrap();
    let g = graph_from("turtle", &parse_typed_list(TOPICS), &[], info, &limits);
    let schema = from_node_graph(&g).unwrap();
    assert!(actionable_fraction_pct(&schema) > 0);
}

#[test]
fn a_typo_in_a_limits_file_is_an_error_not_a_missing_limit() {
    assert!(parse_limits("# comment\n\n/cmd_vel linear_x -0.4 1.2  # ok\n").is_ok());
    for bad in [
        "/cmd_vel linear_x -0.4",
        "/cmd_vel linear_x fast 1.2",
        "/cmd_vel linear_x 2.0 1.0",
        "/cmd_vel linear_x 0 inf",
    ] {
        assert!(parse_limits(bad).is_err(), "accepted `{bad}`");
    }
}

#[test]
fn the_reader_refuses_to_touch_the_network_unless_it_is_switched_on() {
    if std::env::var("BRAINOS_NET").map(|v| v == "on").unwrap_or(false) {
        println!("SKIPPED (BRAINOS_NET=on in this environment)");
        return;
    }
    assert_eq!(live_graph("x", &[]).unwrap_err(), Ros2Unavailable::NetworkOff);
}

/// The whole path against a real, running ROS 2 system: read the live
/// graph, hand it to the existing §8 adapter, and put it to the gate.
///
/// Runs only where a `ros2` tool is on the path and `BRAINOS_NET=on`
/// (`os/tools/ros2-live.sh` arranges both with turtlesim in a
/// container). It reads the graph and asks the gate; it publishes
/// nothing into ROS — that half is not built.
#[test]
fn live_a_running_ros2_graph_is_read_and_goes_through_section_8() {
    use brainos_abi::ids::BodyId;
    use brainos_abi::time::Tick;
    use brainos_body::calibration::Calibration;
    use brainos_body::hal::Telemetry;
    use brainos_crypto::key::BrainKey;
    use brainos_entity::entity::Entity;

    let shipped = match live_graph("live ros2 robot", &[]) {
        Ok(g) => g,
        Err(e) => {
            println!("SKIPPED ({e:?}); run os/tools/ros2-live.sh");
            return;
        }
    };
    assert!(!shipped.topics.is_empty());
    let commands: Vec<&str> = shipped
        .topics
        .iter()
        .filter(|t| t.subscribed && !t.published)
        .map(|t| t.name.as_str())
        .collect();
    println!(
        "LIVE ros2: {} topics ({} commands: {commands:?}), {} services",
        shipped.topics.len(),
        commands.len(),
        shipped.services.len()
    );
    assert!(!commands.is_empty(), "a running robot with nothing to command");

    let acquire = |graph: &brainos_adapters::ros2::NodeGraph, seed: u8| {
        let schema = from_node_graph(graph).expect("the live graph is usable");
        let device = BrainKey::from_seed(&[seed; 32]);
        let mut e = Entity::new(BrainKey::from_seed(&[seed.wrapping_add(1); 32]), Tick(0));
        e.wake(None, None, Tick(1)).unwrap();
        let r = e.acquire(&device, schema.clone(), Calibration::new(BodyId::ZERO, Tick(1)), 0);
        (e, schema, r)
    };

    // as it is running: no limits declared anywhere, so §8 will not
    // take it on
    let (_, schema, r) = acquire(&shipped, 0x41);
    assert_eq!(actionable_fraction_pct(&schema), 0);
    assert!(r.is_err(), "a robot that states no limits was incorporated");
    println!("LIVE ros2: as running, section 8 refuses it ({:?})", r.unwrap_err());

    // with limits a human states for the first command topic
    let topic = commands[0];
    let limits = parse_limits(&format!("{topic} linear_x -1.0 2.0\n{topic} angular_z -2.0 2.0\n")).unwrap();
    let declared = live_graph("live ros2 robot", &limits).unwrap();
    let (mut e, schema, r) = acquire(&declared, 0x43);
    let body = r.expect("with declared limits it is incorporated");
    e.observe_body(
        &body,
        &Telemetry { link_alive: true, ..Default::default() },
    );
    let spec = schema.verbs().iter().find(|v| v.is_actionable()).expect("an actionable verb");
    let verb = spec.verb;
    let modest: Vec<(&str, f64)> = spec.params.iter().map(|p| (p.name.as_str(), (p.min + p.max) / 4.0)).collect();
    let reckless: Vec<(&str, f64)> = spec.params.iter().map(|p| (p.name.as_str(), p.max * 10.0)).collect();
    let ok = e.attempt(verb, body, &modest);
    let no = e.attempt(verb, body, &reckless);
    println!(
        "LIVE ros2: with limits, `{}` {modest:?} -> {}; {reckless:?} -> {:?}",
        verb.as_str(),
        if ok.verdict.is_granted() { "granted" } else { "refused" },
        no.verdict.denial().map(|d| (d.stage, d.code))
    );
    assert!(ok.verdict.is_granted(), "{:?}", ok.verdict.denial());
    assert_eq!(no.verdict.denial().map(|d| d.stage), Some(brainos_abi::error::Stage::Validate));
}
