//! BFS  LEVEL    Section 8 Acquisition - adapters (ROS 2, MHS, the model boundary)  [crate brainos-adapters]
//! BFS  PURPOSE  Crate root for the adapters that turn a body's own description (ROS 2 graph, MHS tool list, our firmware) into a capability schema, with no I/O
//! BFS  DEFINES  boundary, json, mhs, native, ros2
//! BFS  USES     this crate: json, mhs, native, ros2
//! BFS  USED BY  crate host, crate nostd, crate proof
//!
//! brainos-adapters — how a body we did not build becomes a body.
//!
//! §8 has five steps and the adapters are only steps 1–3: discover,
//! handshake, read the capability schema. Steps 4 and 5 — binding a
//! policy and calibrating — are ours regardless of how the body was
//! found. That split is the whole reason this crate is small: a ROS 2
//! robot and an MHS lab instrument differ in how you *read* them and not
//! at all in how you *inhabit* them.
//!
//! **ROS 2.** ROS is middleware, not an operating system, and it has no
//! concept of an entity — it moves messages. What it does have is a node
//! graph: topics with message types, and services. `ros2` turns that
//! graph into a `CapabilitySchema`. The honest part is where the limits
//! come from: ROS topics do not declare envelopes, so unless the robot
//! publishes a `JointLimits`-shaped description, every parameter comes
//! back **unbounded** — which means the entity can see the limb and
//! cannot move it. That is the correct outcome for a body that will not
//! say what it can survive, and it is why T4 in the risk register says
//! the adapter is brittle across robots. It is brittle on purpose.
//!
//! **MHS.** Anthropic's Model Hardware Standard is a driver standard
//! over MCP — peripheral nerves, in the metaphor. It is the easier of
//! the two because the capability schema is already standardized: a
//! device declares its tools with JSON Schema, including bounds. `mhs`
//! reads that directly.
//!
//! One vocabulary rule, enforced by the type system rather than by
//! discipline: MHS says *agent*, we say *entity*. Nothing in this crate
//! exposes the word.
//!
//! **What this crate does not do.** It does not speak DDS and it does
//! not speak MCP over a socket. Those are I/O, they belong in the host
//! layer, and mixing them in here would make the interesting part — the
//! schema derivation — untestable without a robot in the room. These
//! functions take a *description* and return a schema, which is exactly
//! the part that is hard to get right and easy to test.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod boundary;
pub mod json;
pub mod mhs;
pub mod native;
pub mod ros2;

pub use json::{Json, JsonError};
pub use mhs::{from_mhs, MhsError};
pub use native::native_schema;
pub use ros2::{from_node_graph, NodeGraph, Ros2Error, Service, Topic};
