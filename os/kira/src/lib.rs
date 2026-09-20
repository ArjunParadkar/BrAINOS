//! brainos-kira — Domain 3, the gate.
//!
//! Cognition proposes; KIRA disposes. Every action the entity takes
//! passes all eight stages in fixed order. There is no admin mode, no
//! escape hatch, no root, and no path by which Domain 4 skips a stage.
//!
//! **The seal.** `Granted` has a private field of a private type. No code
//! outside this crate can write `Granted { .. }`, and the only expression
//! anywhere that constructs one is inside stage 7, after stages 1–6 have
//! returned `Pass`. "A model cannot forge permission to act" is therefore
//! not a runtime check that could have a bug in it — it is what the type
//! system permits to exist. The `brainos-proof` crate mechanically checks
//! the rest: that the stages run in order, that no stage can be skipped,
//! that every refusal is labelled with the stage that made it, and that
//! attenuation never widens.
//!
//! **What this crate does not claim.** The eight stages run for real —
//! authenticate verifies an ed25519 signature, policy consults an actual
//! ruleset, simulate calls the body's forward model. Stage 6 of the
//! roadmap (machine-checked proof of the whole pipeline) is partially
//! delivered here: the sealed-`Granted` property is a compile-time fact
//! today, and `brainos-proof` exhaustively checks the ordering and
//! fail-closed properties over a bounded domain. A full functional
//! verification of every body policy is 2028–29 work and this file does
//! not pretend otherwise.
//!
//! **Why the order is the argument.**
//!
//! ```text
//!   1 PARSE        is this even a well-formed request?
//!   2 AUTHENTICATE did the entity really ask for this?
//!   3 AUTHORIZE    does a limb that can do it actually exist?
//!   4 VALIDATE     are the arguments inside the declared bounds?
//!   5 SIMULATE     what happens if we do it?
//!   6 POLICY       do the drives, the rules, and the body permit it?
//!   7 COMMIT       mint the token
//!   8 AUDIT        record the decision — always, whatever it was
//! ```
//!
//! Authorize sits before validate because checking arguments against a
//! limb that does not exist is checking nothing. Simulate sits before
//! policy because a rule about consequences needs the consequence. Audit
//! sits last and runs unconditionally, because a decision nobody can
//! review is a decision nobody made.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod action;
pub mod audit;
pub mod capability;
pub mod floors;
pub mod evidence;
pub mod explain;
pub mod gate;
pub mod rules;
pub mod stages;

pub use action::{Action, Proposal, Request};
pub use audit::{AuditRecord, AuditTrail, Decision};
pub use capability::{Caveat, Delegation, Granted, Grant};
pub use floors::{destructive_hit, is_load_bearing, normalize};
pub use gate::{Kira, Outcome, Verdict};
pub use rules::{Drive, Level, Rule, RuleSet};
pub use stages::{Evidence, StageOutcome, Trace};
