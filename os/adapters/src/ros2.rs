//! BFS  LEVEL    Section 8 Acquisition - adapters (ROS 2, MHS, the model boundary)  [crate brainos-adapters]
//! BFS  PURPOSE  Turns a ROS 2 node graph (topics, services, optional limits) into a CapabilitySchema; commands with no declared limits stay visible but unmovable
//! BFS  DEFINES  Ros2Error, Topic, Service, NodeGraph, from_node_graph, actionable_fraction_pct
//! BFS  USES     brainos: abi, body, rt
//! BFS  USED BY  adapters/lib, adapters/tests/adapters_suite, host/fullstack, host/nerves/ros2, host/ros2_stack, host/tests/fullstack_suite, host/tests/nerves_ros2, nostd/lib
//!
//! The ROS 2 adapter — Phase 1's main engineering deliverable.
//!
//! A ROS 2 robot exposes a node graph: topics with message types, and
//! services. `from_node_graph` reads that graph as a capability schema,
//! so the entity inhabits the robot without the owner ripping anything
//! out. Nothing about the robot changes; ROS keeps moving messages.
//!
//! **The hard part is limits, and the honest answer is usually "none".**
//! A ROS topic declares a message *type* and nothing about what values
//! are safe. `/cmd_vel` takes a `geometry_msgs/Twist`; the type says
//! nothing about whether 12 m/s will tip the robot over. So:
//!
//! * If the graph carries a limits description — a `JointLimits`-shaped
//!   parameter block, a URDF excerpt, a `ControllerInterface` with
//!   bounds — the derived parameters are bounded and the verb is
//!   actionable.
//! * If it does not, the parameters come back **unbounded**, which means
//!   the entity sees the limb, advertises nothing, and cannot move it.
//!
//! That second case will be the common one on a robot nobody prepared,
//! and it is the right outcome rather than a limitation to work around.
//! Risk T4 says this adapter is brittle across robots; it is brittle in
//! the direction of refusing to move.
//!
//! **Mapping.** Topic name to region class is by convention, and the
//! conventions are ROS's own (`/cmd_vel`, `/joint_states`, `/scan`).
//! Anything unrecognized becomes `Tool` — useful, non-consequential,
//! gated like everything else. Guessing `Legs` from an unknown topic
//! would be guessing that something can walk.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::verb::Verb;
use brainos_body::region::RegionClass;
use brainos_body::schema::{CapabilitySchema, ParamKind, ParamSpec, VerbSpec};
use brainos_rt::envelope::Channel;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ros2Error {
    /// the graph had no topics or services at all
    Empty,
    /// a topic name is not a legal ROS name
    BadTopicName,
    /// a derived verb would not satisfy the verb grammar
    Unnameable,
}

/// One topic in the node graph.
#[derive(Clone, Debug, PartialEq)]
pub struct Topic {
    /// e.g. "/cmd_vel"
    pub name: String,
    /// e.g. "geometry_msgs/msg/Twist"
    pub message_type: String,
    /// true when the robot subscribes (we can publish to it)
    pub subscribed: bool,
    /// true when the robot publishes (we can read it)
    pub published: bool,
    /// bounds the robot declared, if any: (field, min, max)
    pub limits: Vec<(String, f64, f64)>,
}

impl Topic {
    pub fn command(name: &str, message_type: &str) -> Topic {
        Topic {
            name: alloc::string::ToString::to_string(name),
            message_type: alloc::string::ToString::to_string(message_type),
            subscribed: true,
            published: false,
            limits: Vec::new(),
        }
    }

    pub fn sensor(name: &str, message_type: &str) -> Topic {
        Topic {
            name: alloc::string::ToString::to_string(name),
            message_type: alloc::string::ToString::to_string(message_type),
            subscribed: false,
            published: true,
            limits: Vec::new(),
        }
    }

    pub fn bounded(mut self, field: &str, min: f64, max: f64) -> Topic {
        self.limits
            .push((alloc::string::ToString::to_string(field), min, max));
        self
    }
}

/// One service.
#[derive(Clone, Debug, PartialEq)]
pub struct Service {
    pub name: String,
    pub service_type: String,
}

impl Service {
    pub fn new(name: &str, service_type: &str) -> Service {
        Service {
            name: alloc::string::ToString::to_string(name),
            service_type: alloc::string::ToString::to_string(service_type),
        }
    }
}

/// What `ros2 node info` and friends report, as data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeGraph {
    pub robot_name: String,
    pub topics: Vec<Topic>,
    pub services: Vec<Service>,
}

impl NodeGraph {
    pub fn new(robot_name: &str) -> NodeGraph {
        NodeGraph {
            robot_name: alloc::string::ToString::to_string(robot_name),
            topics: Vec::new(),
            services: Vec::new(),
        }
    }

    pub fn with(mut self, t: Topic) -> NodeGraph {
        self.topics.push(t);
        self
    }

    pub fn serving(mut self, s: Service) -> NodeGraph {
        self.services.push(s);
        self
    }
}

/// Derive a capability schema from a node graph.
pub fn from_node_graph(graph: &NodeGraph) -> Result<CapabilitySchema, Ros2Error> {
    if graph.topics.is_empty() && graph.services.is_empty() {
        return Err(Ros2Error::Empty);
    }

    let mut schema = CapabilitySchema::new(&graph.robot_name, "ros2");

    for t in &graph.topics {
        if !t.name.starts_with('/') || t.name.len() < 2 {
            return Err(Ros2Error::BadTopicName);
        }
        // a topic the robot only publishes is a sense, not a verb. The
        // entity reads it as telemetry; there is nothing to authorize.
        if !t.subscribed {
            continue;
        }
        let class = class_of(&t.name, &t.message_type);
        if class.is_afferent_only() {
            continue;
        }
        let verb = verb_for(&t.name, class).ok_or(Ros2Error::Unnameable)?;

        let mut spec = VerbSpec::new(
            verb,
            class,
            &summary_for(&t.name, &t.message_type),
        );
        for p in params_for(&t.message_type, &t.limits) {
            spec = spec.with(p);
        }
        // a Twist on a drive base is not reversible in the sense that
        // matters: you cannot un-drive into a wall
        if class.is_consequential() {
            spec = spec.irreversible();
        }
        schema.declare(spec);
    }

    for s in &graph.services {
        let class = class_of(&s.name, &s.service_type);
        let Some(verb) = verb_for(&s.name, class) else {
            continue;
        };
        let mut spec = VerbSpec::new(verb, class, &s.service_type);
        // a service call takes a request whose fields we cannot see from
        // the graph alone, so it is declared with one unbounded
        // parameter — visible, and unusable until someone says what it
        // accepts
        spec = spec.with(ParamSpec::unbounded("request", ParamKind::Text));
        schema.declare(spec);
    }

    if schema.is_empty() {
        return Err(Ros2Error::Empty);
    }
    Ok(schema)
}

/// Which region a topic belongs to, by ROS's own naming conventions.
fn class_of(name: &str, message_type: &str) -> RegionClass {
    let n = name.trim_start_matches('/').to_ascii_lowercase();
    let m = message_type.to_ascii_lowercase();

    if n.starts_with("cmd_vel") || n.contains("drive") || n.contains("base_controller") {
        return RegionClass::Drive;
    }
    if n.contains("joint_trajectory") || n.contains("arm") || n.contains("manipulator") {
        return RegionClass::Arm;
    }
    if n.contains("gripper") || n.contains("hand") {
        return RegionClass::Gripper;
    }
    if n.contains("legs") || n.contains("gait") || n.contains("walk") {
        return RegionClass::Legs;
    }
    if n.contains("scan") || m.contains("laserscan") || m.contains("pointcloud") {
        return RegionClass::Proprioception;
    }
    if n.contains("image") || n.contains("camera") {
        return RegionClass::Camera;
    }
    if n.contains("joint_states") || m.contains("jointstate") || m.contains("imu") {
        return RegionClass::Proprioception;
    }
    if n.contains("speaker") || n.contains("tts") || n.contains("audio_out") {
        return RegionClass::Speaker;
    }
    RegionClass::Tool
}

/// Turn a topic name into a verb. ROS names are `/snake_case/paths`;
/// the verb grammar is `namespace.action` with `[a-z0-9_]`.
fn verb_for(topic: &str, class: RegionClass) -> Option<Verb> {
    let tail = topic
        .trim_start_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");
    let mut action = String::new();
    for c in tail.chars() {
        if c.is_ascii_alphanumeric() {
            action.push(c.to_ascii_lowercase());
        } else if !action.is_empty() && !action.ends_with('_') {
            action.push('_');
        }
        if action.len() >= 23 {
            break;
        }
    }
    let action = action.trim_end_matches('_');
    if action.is_empty() {
        return None;
    }
    let mut v = String::new();
    v.push_str(class.name());
    v.push('.');
    v.push_str(action);
    Verb::parse(&v).ok()
}

fn summary_for(name: &str, message_type: &str) -> String {
    let mut s = String::new();
    s.push_str("ros2 topic ");
    s.push_str(name);
    s.push_str(" (");
    s.push_str(message_type);
    s.push(')');
    s
}

/// Parameters for a message type.
///
/// The table is the well-known `geometry_msgs` and `trajectory_msgs`
/// shapes. Anything else gets one unbounded text parameter — the entity
/// knows the topic exists and cannot publish to it, which is the right
/// answer for a message shape nobody has taught it.
fn params_for(message_type: &str, limits: &[(String, f64, f64)]) -> Vec<ParamSpec> {
    let m = message_type.to_ascii_lowercase();
    let bound = |field: &str, channel: Channel, fallback: Option<(f64, f64)>| -> ParamSpec {
        match limits.iter().find(|(f, _, _)| f == field) {
            Some((_, lo, hi)) => ParamSpec::scalar(field, channel, *lo, *hi),
            None => match fallback {
                // a fallback is only ever used when the *robot* declared
                // the bound somewhere else in the graph; there is no
                // built-in "reasonable default", because a reasonable
                // default for a speed is how a robot gets driven into a
                // wall at a speed nobody chose
                Some((lo, hi)) => ParamSpec::scalar(field, channel, lo, hi),
                None => ParamSpec::unbounded(field, ParamKind::Scalar),
            },
        }
    };

    if m.contains("twist") {
        return alloc::vec![
            bound("linear_x", Channel::Speed, None),
            bound("angular_z", Channel::Rate, None),
        ];
    }
    if m.contains("jointtrajectory") || m.contains("jointjog") {
        return alloc::vec![
            bound("position", Channel::Angle, None),
            bound("velocity", Channel::Rate, None),
        ];
    }
    if m.contains("float64") || m.contains("float32") {
        return alloc::vec![bound("data", Channel::Angle, None)];
    }
    if m.contains("bool") {
        return alloc::vec![ParamSpec::flag("data")];
    }
    if m.contains("string") {
        return alloc::vec![ParamSpec::text("data", 2_000)];
    }
    alloc::vec![ParamSpec::unbounded("payload", ParamKind::Text)]
}

/// How much of a graph the entity can actually use, as a percentage.
///
/// The number a design partner asks for first, and the one that says
/// whether their robot is ready: "we read 14 topics and can move 2 of
/// them" is a specific, actionable answer.
pub fn actionable_fraction_pct(schema: &CapabilitySchema) -> u8 {
    if schema.is_empty() {
        return 0;
    }
    let actionable = schema.verbs().iter().filter(|v| v.is_actionable()).count();
    ((actionable * 100) / schema.len()) as u8
}
