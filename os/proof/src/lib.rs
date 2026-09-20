//! brainos-proof — Stage 6, honestly labelled.
//!
//! The roadmap says Stage 6 is "formal verification of KIRA's eight
//! stages: safety properties proven, not tested" and dates it 2028–29
//! with a formal-methods lead hired in 2028. This crate is not that. It
//! is the part of that which can exist in 2026, and it is stated
//! precisely so nobody mistakes one for the other:
//!
//! **What is proven today, at compile time.** `Granted` has a private
//! field of a private type. No code outside `brainos-kira` can
//! construct one. This is not checked here because it cannot be
//! checked at runtime — it is a property of what the type system
//! permits to exist, and the evidence for it is that this crate cannot
//! write the counterexample. That is the strongest guarantee in the
//! system and it needs no harness.
//!
//! **What is checked here, exhaustively.** A finite domain — every
//! combination of action kind, body state, argument class, rule set and
//! signature validity — is enumerated completely, every case is run
//! through the real gate, and a set of properties is asserted over every
//! resulting trace. "Exhaustively" is literal: the domain is small
//! enough to enumerate, and the harness reports its size so the claim
//! can be checked. Within that domain the properties hold for *all*
//! inputs, not for the ones a test author thought of.
//!
//! **What is not checked, and will not be until 2028–29.** That the
//! properties hold outside the bounded domain. That a body policy's
//! `simulate` is a sound forward model of a physical system — that is a
//! claim about physics and no amount of enumeration touches it. That
//! the ed25519 implementation is correct. That the compiler is correct.
//!
//! So: this is bounded model checking with a machine-generated report,
//! and calling it a proof would be exactly the kind of thing the "no
//! larp" principle exists to prevent. What it does buy is real — the
//! properties below have found ordering bugs that a hand-written test
//! suite passed.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod domain;
pub mod envelopes;
pub mod hostile;
pub mod harness;
pub mod properties;
pub mod report;

pub use domain::{Case, Domain, Scenario};
pub use harness::{run, run_all, verify};
pub use envelopes::{verify_envelopes, EnvelopeCase, EnvelopeDomain};
pub use hostile::{verify_hostile, HostileReport};
pub use properties::{check_all, Property, PropertyResult, PROPERTIES};
pub use report::{Coverage, Report, Verdict};
