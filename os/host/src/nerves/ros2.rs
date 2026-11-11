//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Reads a running ROS 2 graph with the ros2 command-line tool and turns it into the NodeGraph the existing ROS 2 adapter consumes
//! BFS  DEFINES  Ros2Unavailable, Limit, parse_limits, parse_typed_list, parse_topic_info, graph_from, live_graph
//! BFS  USES     this crate: net_allowed | brainos: adapters | external: std
//! BFS  USED BY  host/tests/nerves_ros2
//!
//! A *running* ROS 2 graph, read live.
//!
//! `brainos-adapters` already turns a described ROS 2 robot (a
//! `NodeGraph`) into a capability schema, and `brainos ros2`-style runs
//! have only ever fed it a graph written by hand. This reads the graph
//! from a real, running ROS 2 system instead, using the tool every ROS
//! installation has:
//!
//! ```text
//!   ros2 topic list -t        "/cmd_vel [geometry_msgs/msg/Twist]"
//!   ros2 topic info <topic>   "Publisher count: 1" / "Subscription count: 2"
//!   ros2 service list -t      "/clear [std_srvs/srv/Empty]"
//! ```
//!
//! A topic something *subscribes* to and nothing publishes is a
//! command; a topic that is published is a sensor.
//!
//! **Status: Built, not Verified.** ROS 2 is not installed on the
//! machine this was written on, so only the parsing is tested (against
//! the tool's documented output format). The runbook that turns this
//! into a live result is in `docs/NERVES.md`.
//!
//! **What it is not.** It is not yet a `Nerve`: it produces the graph
//! and stops. Publishing a granted command back into ROS is not built.
//!
//! **Limits.** ROS declares message types, not safe values, so the
//! graph that comes back has no bounds, and §8 will refuse to
//! incorporate it — the behaviour `host/src/ros2_stack.rs` documents.
//! `Limits` is how a human supplies the four numbers from the manual.
//!
//! **Network.** ROS 2 discovery opens network sockets. `live_graph`
//! refuses unless `BRAINOS_NET=on`.

use super::net_allowed;
use brainos_adapters::ros2::{NodeGraph, Service, Topic};
use std::process::Command;

/// Why the live graph could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ros2Unavailable {
    /// `BRAINOS_NET` is not `on`
    NetworkOff,
    /// the `ros2` tool is not on the path
    NotInstalled,
    /// the tool ran and reported nothing, or failed
    NoGraph(String),
}

/// One declared bound: on this topic, this field, between these.
#[derive(Clone, Debug, PartialEq)]
pub struct Limit {
    pub topic: String,
    pub field: String,
    pub min: f64,
    pub max: f64,
}

/// Parse a limits file: one `topic field min max` per line, `#` for
/// comments. A line that does not parse is an error, not a skipped
/// line — a typo in a safety limit must not silently remove the limit.
pub fn parse_limits(text: &str) -> Result<Vec<Limit>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let l = line.split('#').next().unwrap_or("").trim();
        if l.is_empty() {
            continue;
        }
        let f: Vec<&str> = l.split_whitespace().collect();
        let bad = || format!("line {}: expected `topic field min max`, got `{l}`", i + 1);
        if f.len() != 4 {
            return Err(bad());
        }
        let (min, max) = (f[2].parse::<f64>().map_err(|_| bad())?, f[3].parse::<f64>().map_err(|_| bad())?);
        if !(min.is_finite() && max.is_finite() && min <= max) {
            return Err(bad());
        }
        out.push(Limit { topic: f[0].into(), field: f[1].into(), min, max });
    }
    Ok(out)
}

/// `"/cmd_vel [geometry_msgs/msg/Twist]"` → `("/cmd_vel", "geometry_msgs/msg/Twist")`.
/// A name with several types lists them comma-separated; the first is
/// taken.
pub fn parse_typed_list(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let (name, rest) = l.split_once(" [")?;
            let ty = rest.strip_suffix(']')?.split(',').next()?.trim();
            (name.starts_with('/') && !ty.is_empty()).then(|| (name.to_string(), ty.to_string()))
        })
        .collect()
}

/// `(publishers, subscribers)` from `ros2 topic info`.
pub fn parse_topic_info(text: &str) -> Option<(u32, u32)> {
    let num = |key: &str| {
        text.lines()
            .find_map(|l| l.trim().strip_prefix(key))
            .and_then(|v| v.trim().parse::<u32>().ok())
    };
    Some((num("Publisher count:")?, num("Subscription count:")?))
}

/// Build the graph from what the tool printed. `info` gives the
/// publisher and subscriber counts for a topic.
pub fn graph_from(
    robot: &str,
    topics: &[(String, String)],
    services: &[(String, String)],
    info: impl Fn(&str) -> Option<(u32, u32)>,
    limits: &[Limit],
) -> NodeGraph {
    let mut g = NodeGraph::new(robot);
    for (name, ty) in topics {
        // ROS's own plumbing is not the robot
        if name == "/rosout" || name == "/parameter_events" {
            continue;
        }
        let (pubs, subs) = info(name).unwrap_or((0, 0));
        let mut t = if subs > 0 && pubs == 0 {
            Topic::command(name, ty)
        } else {
            Topic::sensor(name, ty)
        };
        for l in limits.iter().filter(|l| &l.topic == name) {
            t = t.bounded(&l.field, l.min, l.max);
        }
        g = g.with(t);
    }
    for (name, ty) in services {
        // every node serves these; they describe the node, not the robot
        if name.contains("/get_parameters")
            || name.contains("/set_parameters")
            || name.contains("/describe_parameters")
            || name.contains("/list_parameters")
            || name.contains("/get_parameter_types")
            || name.contains("/get_type_description")
        {
            continue;
        }
        g = g.serving(Service::new(name, ty));
    }
    g
}

fn run(args: &[&str]) -> Result<String, Ros2Unavailable> {
    let out = Command::new("ros2")
        .args(args)
        .output()
        .map_err(|_| Ros2Unavailable::NotInstalled)?;
    if !out.status.success() {
        return Err(Ros2Unavailable::NoGraph(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Read the running graph.
pub fn live_graph(robot: &str, limits: &[Limit]) -> Result<NodeGraph, Ros2Unavailable> {
    if !net_allowed() {
        return Err(Ros2Unavailable::NetworkOff);
    }
    let topics = parse_typed_list(&run(&["topic", "list", "-t"])?);
    if topics.is_empty() {
        return Err(Ros2Unavailable::NoGraph("no topics".into()));
    }
    let services = parse_typed_list(&run(&["service", "list", "-t"]).unwrap_or_default());
    Ok(graph_from(
        robot,
        &topics,
        &services,
        |t| run(&["topic", "info", t]).ok().and_then(|s| parse_topic_info(&s)),
        limits,
    ))
}
