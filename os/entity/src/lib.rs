//! BFS  LEVEL    Stage 4 Entity - one Key, one state graph, every body at once  [crate brainos-entity]
//! BFS  PURPOSE  Crate root: lists the modules that make one entity out of a Key, a memory and every body it holds
//! BFS  DEFINES  entity, goals, life, simultaneity
//! BFS  USES     this crate: entity, goals, life, simultaneity
//! BFS  USED BY  crate bpu, crate export, crate host, crate nostd
//!
//! brainos-entity — the thing that is a someone.
//!
//! Everything below this crate is a domain. This is where they become
//! one self.
//!
//! ```text
//!   Entity = BrAIn Key + state graph + every body it is bound to
//! ```
//!
//! That equation is the architecture's, and the code is written so that
//! it is literally true rather than approximately true. There is one
//! `MemoryHandle`, held here. There is one `BodyMap`, holding every
//! body. There is one `Kira`, gating for all of them. A second hexapod
//! joining does not create a second anything — it adds regions to the
//! map and a policy to the gate.
//!
//! **Stage 4 is this and nothing more.** "One entity, many bodies,
//! simultaneously" is not a synchronization feature; it is the absence
//! of one. Nothing is synced because there is only one copy. The work
//! Stage 4 actually requires is *arbitration* — what happens when two
//! bodies want incompatible things, and what happens when one of them
//! stops answering mid-action — and that is what `simultaneity` is.
//!
//! **What an entity is not.** It is not a process and it does not exit.
//! `wake` and `sleep` bracket an embodiment, not a lifetime; the entity
//! between them is the same entity, and `Life` records that continuity
//! as the thing it is: how many times it has woken, in how many bodies,
//! and how much it has learned since.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod entity;
pub mod goals;
pub mod life;
pub mod simultaneity;

pub use entity::{Entity, Wake};
pub use goals::{Goal, GoalId, Goals, Level};
pub use life::{Life, Span};
pub use simultaneity::{Arbiter, Claim, Conflict, Resolution};
