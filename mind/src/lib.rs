//! brainos-mind — everything about the entity that is not a body.
//!
//! The abstraction table (Architecture §1) splits cleanly in two. This
//! crate is the half that is pure thought: identity (key), authority
//! (kira), memory (state, journal), cognition (experience, instance),
//! own-compute (script), and the typed boundary where model text becomes
//! action proposals (proposal). The UEFI core in ../core is the half that
//! is flesh: framebuffer, keys, UART, firmware.
//!
//! Keeping the mind no_std and firmware-free is what makes Stage 1
//! honest: the regression and adversarial suites in ../mind/tests link
//! THIS rlib — the binary-identical logic the booted entity runs — not a
//! reimplementation of it.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod body;
pub mod experience;
pub mod instance;
pub mod journal;
pub mod key;
pub mod kira;
pub mod proposal;
pub mod script;
pub mod state;
