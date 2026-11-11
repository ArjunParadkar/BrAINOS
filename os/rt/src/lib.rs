//! BFS  LEVEL    Domain 2 Autonomic - envelopes, reflexes, scheduler, watchdogs  [crate brainos-rt]
//! BFS  PURPOSE  Crate root: the fast, allocation-free control layer that runs without thinking (limits, reflex loops, scheduler, watchdogs, queues)
//! BFS  DEFINES  budget, envelope, reflex, ring, sched, watchdog
//! BFS  USES     this crate: budget, envelope, reflex, ring, sched, watchdog
//! BFS  USED BY  crate adapters, crate blur, crate body, crate bodyd, crate cognition, crate entity, crate host, crate kira, +3 more
//!
//! brainos-rt — Domain 2, the autonomic layer.
//!
//! This is the part of the entity that does not think. A gait is a timing
//! problem; a current limit is a timing problem; "stop when the link
//! dies" is a timing problem. None of them may wait on cognition, and
//! none of them may wait on an allocator.
//!
//! **The rule that defines this crate.** Tier 0 never proposes into
//! KIRA. That is not an optimization, it is the architecture: a reflex
//! runs inside an envelope that KIRA *already granted*, before the
//! reflex existed. The gate's decision was made once, at incorporation,
//! about a whole region of behaviour; the reflex loop then operates
//! freely inside it at kilohertz. If a reflex ever needs to leave its
//! envelope, it does not ask — it stops, and the stop is what escalates.
//!
//! This inverts the usual arrangement, where a fast path is a
//! "privileged" path that skips checks. Here the fast path is the
//! *most* constrained path. It is fast because its bounds were decided
//! in advance and are checked by comparison, not by policy.
//!
//! Everything here is allocation-free after construction, bounded in
//! time, and total: no function in this crate can panic on a body,
//! because a panic in a servo loop is a fall.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod budget;
pub mod envelope;
pub mod reflex;
pub mod ring;
pub mod sched;
pub mod watchdog;

pub use budget::{Budget, BudgetReport};
pub use envelope::{Envelope, EnvelopeId, Limit, Violation};
pub use reflex::{Reflex, ReflexBank, ReflexOutcome, Stop, StopReason};
pub use ring::{Ring, Spsc};
pub use sched::{Scheduler, Slot, TaskId, TaskSpec, Tick as SchedTick};
pub use watchdog::{Watchdog, WatchdogSet, Wd};
