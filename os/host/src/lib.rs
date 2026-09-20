//! brainos-host — the only crate that is allowed to have an operating
//! system underneath it.
//!
//! Everything else in this workspace is `no_std` and would compile for a
//! microcontroller or a UEFI binary. This crate is where that discipline
//! ends on purpose: it holds the clock, the filesystem, and the process,
//! and it exists so that those three things do not leak upward.
//!
//! The split is load-bearing rather than tidy. `brainos-entity` does not
//! know what a file is; the journal it produces is bytes and a slot
//! number, and *this* crate writes them. When the same entity runs on
//! the UEFI core or on an ESP32, the code above this line does not
//! change — which is what "the platforms that eventually matter will not
//! have Linux" has to mean in practice if it is to mean anything.

pub mod clock;
pub mod store;
pub mod bench;
pub mod compound;
pub mod day;
pub mod degrade;
pub mod demo;
pub mod fleet_experiment;
pub mod fullstack;
pub mod incident;
pub mod limits;
pub mod migrate;

pub use clock::SystemClock;
pub use store::{FileStore, StoreError};
