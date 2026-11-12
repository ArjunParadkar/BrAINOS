//! BFS  LEVEL    Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)  [crate brainos-motor]
//! BFS  PURPOSE  Crate root for motor memory: lists the modules and states the rules they keep (KIRA below everything, no memory in a body, no_std)
//! BFS  DEFINES  condition, control, mandate, mjcf, model, policy, primitive, recipe, reflex, run
//! BFS  USES     this crate: mandate, mjcf, model, run
//! BFS  USED BY  crate bodyd, crate host, crate nostd
//!
//! # brainos-motor: motor memory
//!
//! How a body moves well: what it is (`model`, `mjcf`), what it has been
//! allowed to do for the next little while (`mandate`), and the pieces of
//! movement it knows (`primitive`).
//!
//! Three rules hold in every module here.
//!
//! * **KIRA is below all of it.** Nothing in this crate moves a joint
//!   without a `RunMandate`, and a `RunMandate` is made only from a
//!   `brainos_part::Mandate`, which is made only from a `Granted`.
//! * **A body holds no memory.** No type here owns a `StateGraph` or a
//!   `MemoryHandle`. A primitive is *stored* in the graph by the entity;
//!   the body is handed a copy for the length of one run.
//! * **`no_std`, no unsafe, no new dependencies.**

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod condition;
pub mod control;
pub mod mandate;
pub mod mjcf;
pub mod model;
pub mod policy;
pub mod primitive;
pub mod recipe;
pub mod reflex;
pub mod run;

pub use mjcf::{digest, emit, Drive, Emit};
pub use model::{Base, BodyModel, Inertial, JointKind, JointSpec, Link, ModelError, Num, Shape, Site};
pub use mandate::{run_verb, Bounds, Ceiling, MandateRefusal, RunMandate, Workspace};
pub use run::{Breach, Ending, Felt, Kinematics, Proposal, Runner, Summary};
