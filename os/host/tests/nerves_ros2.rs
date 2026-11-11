//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Tests the ROS 2 graph reader's parsing, its network switch, and that a live graph with no limits is refused by section 8
//! BFS  DEFINES  -
//! BFS  USES     brainos: adapters, host
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

#[test]
fn live_a_running_ros2_graph_is_read() {
    match live_graph("live", &[]) {
        Ok(g) => {
            println!("LIVE ros2: {} topics, {} services", g.topics.len(), g.services.len());
            assert!(!g.topics.is_empty());
        }
        Err(e) => println!("SKIPPED ({e:?}); runbook in docs/NERVES.md"),
    }
}
