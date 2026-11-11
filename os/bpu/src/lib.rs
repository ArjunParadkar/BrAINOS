//! BFS  LEVEL    Domain 4 substrate - the BPU seam (descriptor, mock, nerve, planner)  [crate brainos-bpu]
//! BFS  PURPOSE  Crate root: lists the BPU seam's modules and states what is real (the contract) and what is a stand-in (the device)
//! BFS  DEFINES  contract, device, mock, nerve, planner, prng, tables_gen
//! BFS  USES     nothing outside core/alloc
//! BFS  USED BY  crate nostd
//!
//! brainos-bpu — the seam for the BrAIn Processing Unit.
//!
//! The BPU is an analog compute-in-memory chip: weights are stored as
//! pairs of device conductances and a layer is computed by driving rows
//! and reading column currents. Today it exists as a simulator
//! (`bpusim`) and an FPGA tile. The silicon is future.
//!
//! This crate is what lets BrAInOS place cognition on one **later with
//! no architectural change**. It is a seam and a faithful mock, and it
//! says which is which:
//!
//! | module | what it is | how real |
//! |---|---|---|
//! | `contract` | the integer formats and operations of a tile | **real**: a port of the contract, bit-exact against the reference's vectors |
//! | `prng` | the contract's read-noise generator | **real**, same |
//! | `tables_gen` | the contract's lookup tables | generated from the reference |
//! | `device` | `BpuDevice`: what a real driver will implement; every write needs a `Mandate` | the seam itself |
//! | `mock` | a device: tiles, a dead map, a write budget | a **stand-in**. Its arithmetic is the contract; its wear, drift and temperature are knobs, not physics |
//! | `nerve` | acquires a BPU as a Compute part | real code, exercised against the mock |
//! | `planner` | which tiles a model needs, and what it would cost | a **planner, not a compiler**; energy figures are projections |
//!
//! What the seam fixes, so that silicon can arrive behind it:
//!
//! * a BPU is a `Compute` part on `Substrate::Bpu`, acquired through §8
//!   like any other part;
//! * loading a model and writing weights are KIRA-gated verbs that
//!   **debit a write budget** — endurance is finite, so a load is not a
//!   file copy;
//! * placement may choose it for an inference tier and falls back to
//!   GPU or CPU when its vitals degrade.
//!
//! What it deliberately does not do: assume device noise helps. The
//! noise dial is exposed because the hardware has one. Nothing here
//! schedules it, tunes it, or treats it as a feature.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod contract;
pub mod device;
pub mod mock;
pub mod nerve;
pub mod planner;
pub mod prng;
#[rustfmt::skip]
pub mod tables_gen;
