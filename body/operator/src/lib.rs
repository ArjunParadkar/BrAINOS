//! BFS  LEVEL    Operator (Stage 3-4 milestones) - the seat daemon on the Linux body  [crate brainos-operator]
//! BFS  PURPOSE  Library view of the operator crate: exposes its modules so integration tests and the brainos-export binary can use them
//! BFS  DEFINES  body_registry, config, deliberate, fast, export, hands, hexapod, history, ipc, journal_io, key_reader, kira_gate, +8 more
//! BFS  USES     nothing outside core/alloc
//! BFS  USED BY  -
//!
// Re-export modules for integration tests.
pub mod body_registry;
pub mod config;
pub mod deliberate;
pub mod fast;
pub mod export;
pub mod hands;
pub mod hexapod;
pub mod history;
pub mod ipc;
pub mod journal_io;
pub mod key_reader;
pub mod kira_gate;
pub mod persona;
pub mod pool;
pub mod reach;
pub mod skills;
pub mod tasks;
pub mod reflex;
pub mod sse;
pub mod voice;
