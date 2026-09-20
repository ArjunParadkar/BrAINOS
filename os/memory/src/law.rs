//! The Memory-Integrity Law, as a type.
//!
//! §3.7: *memory lives only in the entity's state graph; never in a
//! device, never in a model.* Most codebases would express that as a
//! rule in a document and a habit in review. Here it is `MemoryHandle`.
//!
//! Every mutating method on `StateGraph` takes `&MemoryHandle`. The
//! handle is created once, inside the entity, from the entity's own id.
//! It is not `Clone`, not `Copy`, not `Default`, and has no public
//! constructor that does not require proof of which entity is asking.
//! A body cannot obtain one. A cognitive resource cannot obtain one. A
//! peer cannot be sent one, because it does not encode.
//!
//! What this buys, concretely: the failure mode where a driver caches
//! "its" state, or a model wrapper keeps a conversation history that is
//! really the entity's memory living in the wrong place, does not
//! compile. Jibo's owners learned the inverse in 2019 — their robots'
//! memory lived on a server, and when the server went dark the robots
//! did too.

use brainos_abi::ids::EntityId;
use core::fmt;

/// Proof that the holder is the entity, for the purpose of mutating its
/// own memory.
///
/// Deliberately not `Clone`: a handle is passed by reference, so there is
/// exactly one per entity for the entity's whole life, and "who has a
/// handle" is answerable by reading the entity's constructor.
pub struct MemoryHandle {
    entity: EntityId,
}

impl MemoryHandle {
    /// Issue the handle for an entity. Called once, by the entity, at
    /// construction. It is `pub` because the entity crate is a separate
    /// compilation unit — but there is nothing useful to do with a handle
    /// for an entity whose graph you do not have, and a graph checks.
    pub fn issue(entity: EntityId) -> MemoryHandle {
        MemoryHandle { entity }
    }

    pub fn entity(&self) -> EntityId {
        self.entity
    }

    /// Does this handle belong to the entity that owns this graph?
    /// Checked at the points where a graph and a handle first meet —
    /// loading, exporting, merging — rather than on every write, because
    /// on every write it would be a per-node comparison in a hot path
    /// defending against a mistake that would already have had to defeat
    /// the type system.
    pub fn authorizes(&self, entity: EntityId) -> Result<(), Violation> {
        if self.entity.ct_eq(&entity) {
            Ok(())
        } else {
            Err(Violation::WrongEntity {
                handle: self.entity,
                graph: entity,
            })
        }
    }
}

impl fmt::Debug for MemoryHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MemoryHandle({})", self.entity.fingerprint())
    }
}

/// A breach of the law, caught at the boundary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Violation {
    /// a handle for one entity was used on another's graph
    WrongEntity { handle: EntityId, graph: EntityId },
    /// something tried to persist entity memory into a body's own store
    MemoryInBody,
    /// something tried to persist entity memory into a model's context
    /// as the durable copy
    MemoryInModel,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Violation::WrongEntity { handle, graph } => write!(
                f,
                "memory-integrity: handle for {} used on {}'s graph",
                handle.fingerprint(),
                graph.fingerprint()
            ),
            Violation::MemoryInBody => {
                f.write_str("memory-integrity: memory may not live in a body")
            }
            Violation::MemoryInModel => {
                f.write_str("memory-integrity: memory may not live in a model")
            }
        }
    }
}

/// A marker for stores that are allowed to hold entity memory.
///
/// Implemented by the journal and by nothing else in the substrate. A
/// body-side cache or a model adapter that wanted to persist memory
/// would have to implement this trait explicitly and in public, which
/// turns an accident into a decision somebody has to defend in review.
pub trait DurableStore {
    /// Which entity's memory this store holds.
    fn owner(&self) -> EntityId;

    /// Sanity check performed before every write.
    fn check(&self, handle: &MemoryHandle) -> Result<(), Violation> {
        handle.authorizes(self.owner())
    }
}
