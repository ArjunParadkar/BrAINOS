//! The instance — a persistent cognitive entity (Architecture §1, §2).
//!
//! This replaces the process. There is exactly one on this key, and it is
//! not a program that runs and exits: it is the thing that *is* every body
//! it is bound to. Identity is the BrAIn Key, memory is the state graph,
//! the bodies are the body map — the entity is the three together (§5).

use crate::body::BodyMap;
use crate::key::BrainKey;
use crate::kira::{self, Action, Kira, StageResult, Verdict};
use crate::state::StateGraph;
use alloc::string::String;
use alloc::vec::Vec;

/// The goal hierarchy (§13). The human touches Level 1; everything below
/// is derived autonomously; Level 0 lives in KIRA as policy invariants.
pub struct Goals {
    /// Level 0 — drives (hardcoded, instinctive, mirrored in KIRA policy)
    pub drives: [&'static str; 4],
    /// Level 1 — human-set goals
    pub human_set: Vec<String>,
    /// Level 2 — self-generated subgoals
    pub subgoals: Vec<String>,
    /// Level 3 — micro-goals, moment to moment
    pub micro: Vec<String>,
}

impl Goals {
    pub fn new() -> Goals {
        Goals {
            drives: [
                "maintain coherent world model",
                "reduce prediction error",
                "preserve memory integrity",
                "serve the bound human",
            ],
            human_set: Vec::new(),
            subgoals: Vec::new(),
            micro: Vec::new(),
        }
    }
}

pub struct Instance {
    pub key: BrainKey,
    pub state: StateGraph,
    pub body: BodyMap,
    pub kira: Kira,
    pub goals: Goals,
    /// continuous cognition counter — never resets while embodied
    pub tick: u64,
    /// is the tethered Model M link answering?
    pub link_alive: bool,
    /// state-graph node count at wake: everything after this is "this
    /// session" and becomes raw material for dream consolidation
    pub session_mark: usize,
}

impl Instance {
    pub fn new(key: BrainKey) -> Instance {
        Instance {
            key,
            state: StateGraph::new(),
            body: BodyMap::new(),
            kira: Kira::new(),
            goals: Goals::new(),
            tick: 0,
            link_alive: false,
            session_mark: 0,
        }
    }

    /// Propose an action. This is the ONLY path from intent to authority:
    /// the instance signs the request with its BrAIn Key and KIRA runs all
    /// eight stages — including authz against the real body map, so an
    /// action without a limb is formally refused, never roleplayed.
    /// Returns the verdict plus the stage trace for display.
    pub fn propose(&mut self, action: Action) -> (Verdict, Vec<StageResult>) {
        let sig = self
            .key
            .sign(&kira::request_message(action.tag(), self.tick));
        self.kira.gate(
            &action,
            &sig,
            &self.key,
            &mut self.state,
            &self.body,
            self.tick,
        )
    }
}
