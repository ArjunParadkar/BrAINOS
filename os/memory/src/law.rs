//! BFS  LEVEL    Domain 5 State - the state graph; the only memory  [crate brainos-memory]
//! BFS  PURPOSE  MemoryHandle: the one non-copyable token, issued to the entity, required for every memory write, so bodies and models cannot own memory
//! BFS  DEFINES  MemoryHandle, Violation, DurableStore
//! BFS  USES     brainos: abi, crypto
//! BFS  USED BY  entity/entity, export/archive, export/tests/export_suite, host/demo, host/limits, host/worlds_run, memory/belief, memory/consolidate, +7 more
//!
//! The Memory-Integrity Law, as a type.
//!
//! §3.7: *memory lives only in the entity's state graph; never in a
//! device, never in a model.* Most codebases would express that as a
//! rule in a document and a habit in review. Here it is `MemoryHandle`.
//!
//! Every mutating method on `StateGraph` takes `&MemoryHandle`. The
//! handle is created once, inside the entity, from the entity's own id.
//! It is not `Clone`, not `Copy`, not `Default`, and its one
//! constructor takes the entity's `BrainKey` — so only the holder of
//! the Key can have one.
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
use brainos_crypto::key::BrainKey;
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
    /// Issue the handle for an entity — to whoever holds its Key.
    ///
    /// The argument is the `BrainKey` itself, not an entity id. An id is
    /// public: it is the hash of the public key and appears in every
    /// audit record and every export. A handle that could be issued
    /// from an id could be issued by anything that had ever seen the
    /// entity's name — a nerve, a driver, a model wrapper — and the law
    /// would rest on nobody choosing to. (It did, until 2026-11-12:
    /// this took an `EntityId`, and a source-level test was what stood
    /// between a nerve and a handle.)
    ///
    /// A `BrainKey` cannot be had that way. It has no public
    /// constructor from an id, it is not `Clone`, and the seed never
    /// leaves it. So the type system now says what the architecture
    /// says: **memory belongs to whoever is the entity**, and being the
    /// entity means holding the Key.
    ///
    /// It is still `pub`, because the entity crate is a separate
    /// compilation unit. That is no longer a gap: everything else that
    /// can call it is something that already holds the Key, and
    /// something that holds the Key *is* the entity.
    pub fn issue(key: &BrainKey) -> MemoryHandle {
        MemoryHandle {
            entity: key.entity_id(),
        }
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
