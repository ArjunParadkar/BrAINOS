//! BFS  LEVEL    Domain 3 KIRA - the eight-stage gate; the only path to action  [crate brainos-kira]
//! BFS  PURPOSE  The per-stage result (pass, refuse, skipped) and the trace of exactly eight of them, in order, that every gate run leaves behind
//! BFS  DEFINES  StageOutcome, Evidence, Trace
//! BFS  USES     brainos: abi
//! BFS  USED BY  kira/audit, kira/gate, kira/lib
//!
//! The stages, and the evidence they leave.
//!
//! A `Trace` always has exactly eight entries, always in `Stage::ALL`
//! order. That is not a convenience for logging — it is the shape the
//! proof harness checks. A pipeline that sometimes produces six entries
//! is a pipeline where "did stage 5 run?" is answered by counting, and
//! counting is a place for an off-by-one to live.
//!
//! Stages short-circuit. Once one refuses, the rest are marked `Skipped`
//! rather than run. This is deliberate and it matters physically: if
//! authorize refused because the body does not exist, then running
//! simulate would mean asking a body that is not there to forward-model
//! something, and the most likely implementation of that is a panic.
//! The exception is audit, which runs whatever happened.

use alloc::string::String;
use brainos_abi::error::{Denied, Stage};

/// What one stage concluded.
#[derive(Clone, Debug, PartialEq)]
pub enum StageOutcome {
    /// it passed, and here is the one-line reason for the record
    Pass(String),
    /// it refused
    Refuse(Denied),
    /// an earlier stage refused, so this one did not run
    Skipped,
}

impl StageOutcome {
    pub fn pass(note: &str) -> StageOutcome {
        StageOutcome::Pass(alloc::string::ToString::to_string(note))
    }

    pub fn is_pass(&self) -> bool {
        matches!(self, StageOutcome::Pass(_))
    }

    pub fn is_refusal(&self) -> bool {
        matches!(self, StageOutcome::Refuse(_))
    }

    pub fn is_skipped(&self) -> bool {
        matches!(self, StageOutcome::Skipped)
    }

    pub fn denial(&self) -> Option<Denied> {
        match self {
            StageOutcome::Refuse(d) => Some(*d),
            _ => None,
        }
    }

    pub fn note(&self) -> &str {
        match self {
            StageOutcome::Pass(s) => s.as_str(),
            StageOutcome::Refuse(d) => d.detail,
            StageOutcome::Skipped => "not reached",
        }
    }
}

/// What one stage did.
#[derive(Clone, Debug, PartialEq)]
pub struct Evidence {
    pub stage: Stage,
    pub outcome: StageOutcome,
}

impl Evidence {
    pub fn new(stage: Stage, outcome: StageOutcome) -> Evidence {
        Evidence { stage, outcome }
    }
}

/// The complete record of one evaluation. Exactly eight entries.
#[derive(Clone, Debug, PartialEq)]
pub struct Trace {
    entries: [Evidence; 8],
}

impl Trace {
    pub fn new() -> Trace {
        Trace {
            entries: [
                Evidence::new(Stage::Parse, StageOutcome::Skipped),
                Evidence::new(Stage::Authenticate, StageOutcome::Skipped),
                Evidence::new(Stage::Authorize, StageOutcome::Skipped),
                Evidence::new(Stage::Validate, StageOutcome::Skipped),
                Evidence::new(Stage::Simulate, StageOutcome::Skipped),
                Evidence::new(Stage::Policy, StageOutcome::Skipped),
                Evidence::new(Stage::Commit, StageOutcome::Skipped),
                Evidence::new(Stage::Audit, StageOutcome::Skipped),
            ],
        }
    }

    pub fn record(&mut self, stage: Stage, outcome: StageOutcome) {
        self.entries[stage.index()] = Evidence::new(stage, outcome);
    }

    pub fn entries(&self) -> &[Evidence; 8] {
        &self.entries
    }

    pub fn at(&self, stage: Stage) -> &StageOutcome {
        &self.entries[stage.index()].outcome
    }

    /// The stage that refused, if any.
    pub fn refusal(&self) -> Option<(Stage, Denied)> {
        self.entries
            .iter()
            .find_map(|e| e.outcome.denial().map(|d| (e.stage, d)))
    }

    pub fn passed_everything(&self) -> bool {
        self.entries.iter().all(|e| e.outcome.is_pass())
    }

    /// The structural invariants of a trace, checked here so that both
    /// the gate's own tests and the proof harness use one definition.
    ///
    /// 1. Entries are in `Stage::ALL` order.
    /// 2. At most one refusal.
    /// 3. Everything after a refusal is `Skipped`, except audit.
    /// 4. Audit is never `Skipped`.
    /// 5. Nothing passes after a refusal.
    pub fn is_well_formed(&self) -> bool {
        for (i, e) in self.entries.iter().enumerate() {
            if e.stage != Stage::ALL[i] {
                return false;
            }
        }
        let refusals = self
            .entries
            .iter()
            .filter(|e| e.outcome.is_refusal())
            .count();
        if refusals > 1 {
            return false;
        }
        if self.entries[Stage::Audit.index()].outcome.is_skipped() {
            return false;
        }
        if let Some(idx) = self
            .entries
            .iter()
            .position(|e| e.outcome.is_refusal())
        {
            for e in &self.entries[idx + 1..] {
                if e.stage == Stage::Audit {
                    continue;
                }
                if !e.outcome.is_skipped() {
                    return false;
                }
            }
        }
        true
    }
}

impl Default for Trace {
    fn default() -> Trace {
        Trace::new()
    }
}
