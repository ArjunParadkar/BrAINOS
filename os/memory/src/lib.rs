//! BFS  LEVEL    Domain 5 State - the state graph; the only memory  [crate brainos-memory]
//! BFS  PURPOSE  Crate root: the entity's memory (episodes, general facts, skills, beliefs), how it is searched, consolidated and saved to storage
//! BFS  DEFINES  belief, consolidate, episode, graph, index, journal, law, provenance, skill
//! BFS  USES     this crate: belief, consolidate, episode, graph, index, journal, law, provenance, +1 more
//! BFS  USED BY  crate bpu, crate cognition, crate entity, crate export, crate host, crate kira, crate motor, crate nostd, +3 more
//!
//! brainos-memory — Domain 5, the state graph.
//!
//! This crate is the answer to "what does it remember?", which is one of
//! the three questions in the empty row of the stack table. It is also
//! where the only durable moat lives: an entity that has been in a body
//! for three years is better than a new one *because of what is in
//! here*, and for no other reason.
//!
//! **The Memory-Integrity Law** (§3.7). Memory lives only in the
//! entity's state graph. Never in a device — a body may be discarded.
//! Never in a model — a model may be swapped. The law is not a
//! convention in this codebase; `law::MemoryHandle` is the only way to
//! mutate a graph, it is not `Clone`, and it carries the entity id it was
//! issued for. A body cannot hold one. A cognitive resource cannot hold
//! one. The consequence the architecture claims — "if BrAIn Corp ceased
//! to exist, every entity would survive with its memory intact" — is a
//! property of this arrangement, not a promise.
//!
//! **Four memory systems**, mirroring the biological split, because the
//! split is load-bearing rather than decorative:
//!
//! * *Episodic* — what happened, in order, with outcomes. Bounded, and
//!   the bound is what makes consolidation necessary.
//! * *Semantic* — what is generally true, with the evidence that made it
//!   true. Compressed from episodes; survives their pruning.
//! * *Procedural* — the skill graph. What worked, under what conditions,
//!   how often. This is the one that compounds.
//! * *Beliefs* — the world model. Held with confidence, revised on
//!   contradiction, and never confused with the other three.
//!
//! **Consolidation is where the compounding happens.** `consolidate`
//! runs offline (the architecture calls it "dreaming"): it finds
//! repetition in episodes and crystallizes it, it finds action sequences
//! that succeeded and promotes them to skills, and it decays what turned
//! out not to matter. An entity that never consolidates has a longer
//! history and not a better one.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod belief;
pub mod consolidate;
pub mod episode;
pub mod graph;
pub mod index;
pub mod journal;
pub mod law;
pub mod provenance;
pub mod skill;

pub use belief::{Belief, Revision};
pub use consolidate::{consolidate, ConsolidationReport, Dream};
pub use episode::{Episode, Outcome};
pub use graph::{Edge, EdgeKind, Node, NodeKind, StateGraph};
pub use index::{Index, Match};
pub use journal::{Journal, Record, Segment, Slot};
pub use law::{MemoryHandle, Violation};
pub use provenance::{Origin, Provenance};
pub use skill::{Precondition, Skill, SkillGraph, SkillId, Step};
