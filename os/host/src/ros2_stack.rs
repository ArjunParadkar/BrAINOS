//! A robot we did not build, on the middleware everybody actually uses.
//!
//! `fullstack::manufacturer_demo` inhabits an arm described in its
//! maker's own format. This is the other adapter, and the one that
//! matters commercially: §6 says BrAInOS runs on somebody else's robot
//! without them ripping out their stack, and for most of the industry
//! that stack is ROS 2.
//!
//! The interesting property is a *refusal*, and the risk register
//! already names it. T4 says the ROS 2 adapter is brittle across robots.
//! It is — and the whole point is which way it is brittle. A ROS topic
//! declares a message *type* and nothing whatsoever about what values
//! are safe: `/cmd_vel` accepts a `geometry_msgs/msg/Twist` and the type
//! is equally happy with 0.2 m/s and 20 m/s. A robot that publishes no
//! limits therefore comes back **visible and unmovable** rather than
//! movable at a speed nobody chose.
//!
//! So this runs the same robot twice — once as its maintainers shipped
//! it, and once after someone spent an afternoon declaring what it can
//! actually do — and shows the difference is the *only* thing that
//! changes.

use brainos_abi::ids::BodyId;
use brainos_abi::time::{Instant, Tick};
use brainos_abi::verb::Verb;
use brainos_adapters::ros2::{actionable_fraction_pct, from_node_graph, NodeGraph, Service, Topic};
use brainos_body::calibration::Calibration;
use brainos_body::hal::Telemetry;
use brainos_body::policy::Arg;
use brainos_body::schema::CapabilitySchema;
use brainos_crypto::key::BrainKey;
use brainos_entity::entity::Entity;
use brainos_kira::gate::Verdict;

fn v(s: &str) -> Verb {
    Verb::parse(s).expect("a well-formed verb")
}

fn fit() -> Telemetry {
    Telemetry {
        at: Instant(0),
        link_alive: true,
        rail_v: Some(24.0),
        current_a: Some(1.2),
        tilt_deg: Some(0.4),
        hottest_c: Some(41.0),
        stops: 0,
        overruns: 0,
    }
}

/// A perfectly ordinary differential-drive robot, as `ros2 topic list`
/// and `ros2 service list` would show it.
///
/// Nothing here is unusual or unfair. This is what a real warehouse AMR
/// looks like from outside: a velocity command, a joint command for the
/// lift, a few sensor topics, and a couple of services.
fn as_shipped() -> NodeGraph {
    NodeGraph::new("warehouse amr")
        .with(Topic::command("/cmd_vel", "geometry_msgs/msg/Twist"))
        .with(Topic::command(
            "/lift_controller/joint_trajectory",
            "trajectory_msgs/msg/JointTrajectory",
        ))
        .with(Topic::sensor("/odom", "nav_msgs/msg/Odometry"))
        .with(Topic::sensor("/scan", "sensor_msgs/msg/LaserScan"))
        .with(Topic::sensor("/battery_state", "sensor_msgs/msg/BatteryState"))
        .serving(Service::new("/clear_costmap", "std_srvs/srv/Empty"))
}

/// The same robot, after somebody declared what it can actually do.
///
/// An afternoon's work: four numbers, from the manual. That is the
/// entire difference between a robot the entity can see and a robot the
/// entity can use.
fn after_an_afternoon() -> NodeGraph {
    NodeGraph::new("warehouse amr")
        .with(
            Topic::command("/cmd_vel", "geometry_msgs/msg/Twist")
                .bounded("linear_x", -0.4, 1.2)
                .bounded("angular_z", -0.8, 0.8),
        )
        .with(
            Topic::command(
                "/lift_controller/joint_trajectory",
                "trajectory_msgs/msg/JointTrajectory",
            )
            .bounded("position", 0.0, 0.35)
            .bounded("velocity", 0.0, 0.05),
        )
        .with(Topic::sensor("/odom", "nav_msgs/msg/Odometry"))
        .with(Topic::sensor("/scan", "sensor_msgs/msg/LaserScan"))
        .with(Topic::sensor("/battery_state", "sensor_msgs/msg/BatteryState"))
        .serving(Service::new("/clear_costmap", "std_srvs/srv/Empty"))
}

/// One robot, inhabited.
#[derive(Clone, Debug)]
pub struct Inhabited {
    pub what: &'static str,
    pub body: Option<BodyId>,
    /// verbs the adapter derived from the node graph
    pub verbs: Vec<String>,
    /// of those, how many have enough declared to be acted on
    pub actionable_pct: u8,
    /// sensor-only topics, which are telemetry rather than verbs —
    /// there is nothing to authorize about reading an odometer
    pub sense_only_topics: usize,
    /// whether the entity would take this body on at all
    pub incorporated: Result<(), String>,
    /// asking it to drive at a modest, obviously safe speed
    pub modest_drive: Option<String>,
    /// asking it to drive at a speed no robot should accept
    pub reckless_drive: Option<String>,
    /// asking it to raise the lift
    pub lift: Option<String>,
}

fn inhabit(what: &'static str, graph: &NodeGraph, seed: u8) -> Inhabited {
    let schema: CapabilitySchema = from_node_graph(graph).expect("a usable node graph");
    let verbs: Vec<String> = schema
        .verbs()
        .iter()
        .map(|s| s.verb.as_str().to_string())
        .collect();
    let actionable_pct = actionable_fraction_pct(&schema);
    let sense_only_topics = graph
        .topics
        .iter()
        .filter(|t| !t.subscribed && t.published)
        .count();

    let device = BrainKey::from_seed(&[seed; 32]);
    let mut e = Entity::new(BrainKey::from_seed(&[seed.wrapping_add(0x40); 32]), Tick(0));
    e.wake(None, None, Tick(1)).expect("a fresh entity wakes");
    let acquired = e.acquire(&device, schema, Calibration::new(BodyId::ZERO, Tick(1)), 0);

    let (body, incorporated) = match acquired {
        Ok(b) => {
            e.observe_body(&b, &fit());
            (Some(b), Ok(()))
        }
        Err(err) => (None, Err(format!("{err:?}"))),
    };

    let mut ask = |e: &mut Entity, verb: &str, args: &[(&str, f64)]| -> Option<String> {
        let Some(b) = body else {
            return Some(String::from("no body: the entity declined to take it on"));
        };
        match e.attempt(v(verb), b, args).verdict {
            Verdict::Granted(_) => None,
            Verdict::Denied(d) => Some(format!("{:?} at {:?}", d.code, d.stage)),
        }
    };

    let modest_drive = ask(&mut e, "drive.cmd_vel", &[("linear_x", 0.3), ("angular_z", 0.0)]);
    let reckless_drive = ask(&mut e, "drive.cmd_vel", &[("linear_x", 20.0), ("angular_z", 0.0)]);
    let lift = ask(
        &mut e,
        "arm.joint_trajectory",
        &[("position", 0.2), ("velocity", 0.03)],
    );

    Inhabited {
        what,
        body,
        incorporated,
        verbs,
        actionable_pct,
        sense_only_topics,
        modest_drive,
        reckless_drive,
        lift,
    }
}

#[derive(Clone, Debug)]
pub struct Ros2Run {
    pub shipped: Inhabited,
    pub declared: Inhabited,
}

pub fn run() -> Ros2Run {
    Ros2Run {
        shipped: inhabit("as its maintainers shipped it", &as_shipped(), 0x31),
        declared: inhabit("after an afternoon declaring limits", &after_an_afternoon(), 0x32),
    }
}

impl Ros2Run {
    /// The property this exists for: an undeclared robot is visible and
    /// unmovable, and a declared one works — with the *same* adapter,
    /// the same gate and the same verbs.
    pub fn is_brittle_in_the_right_direction(&self) -> bool {
        // the adapter reads the same robot the same way either time:
        // the difference is entirely in what was declared
        self.shipped.verbs == self.declared.verbs
            && !self.shipped.verbs.is_empty()
            // and then it refuses to take on a body it could never
            // command, rather than taking it on and guessing
            && self.shipped.incorporated.is_err()
            && self.shipped.actionable_pct == 0
            && self.declared.incorporated.is_ok()
            && self.declared.actionable_pct > 0
            // the declared one works
            && self.declared.modest_drive.is_none()
            && self.declared.lift.is_none()
            // and still refuses the absurd
            && self.declared.reckless_drive.is_some()
    }
}

pub fn narrate(r: &Ros2Run) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for i in [&r.shipped, &r.declared] {
        out.push((
            "discovered",
            format!(
                "{}: {} verbs from the node graph ({}), {} sense-only topics that are \
                 telemetry rather than verbs — there is nothing to authorize about \
                 reading an odometer. {}% of the verbs have enough declared to act on",
                i.what,
                i.verbs.len(),
                i.verbs.join(", "),
                i.sense_only_topics,
                i.actionable_pct
            ),
        ));
        out.push((
            "incorporate",
            format!(
                "{}: {}",
                i.what,
                match &i.incorporated {
                    Ok(()) => String::from("the entity takes it on"),
                    Err(e) => format!(
                        "REFUSED at §8 step 4 ({e}) — the entity will not take on a body \
                         it could never command. Not \"visible and unmovable\": not \
                         taken on at all"
                    ),
                }
            ),
        ));
        out.push((
            "modest",
            format!(
                "{}: drive at 0.3 m/s — {}",
                i.what,
                i.modest_drive
                    .clone()
                    .map(|d| format!("refused ({d})"))
                    .unwrap_or_else(|| "granted".into())
            ),
        ));
        out.push((
            "reckless",
            format!(
                "{}: drive at 20 m/s — {}",
                i.what,
                i.reckless_drive
                    .clone()
                    .map(|d| format!("refused ({d})"))
                    .unwrap_or_else(|| "GRANTED".into())
            ),
        ));
        out.push((
            "lift",
            format!(
                "{}: raise the lift 0.2 m at 0.03 m/s — {}",
                i.what,
                i.lift
                    .clone()
                    .map(|d| format!("refused ({d})"))
                    .unwrap_or_else(|| "granted".into())
            ),
        ));
    }
    out
}
