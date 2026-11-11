//! BFS  LEVEL    Domain 5 State - the state graph; the only memory  [crate brainos-memory]
//! BFS  PURPOSE  The record of one attempt or observation with how it ended (succeeded, failed, denied, stopped, ...); the raw material skills are learned from
//! BFS  DEFINES  Outcome, Episode
//! BFS  USES     this crate: graph, law, provenance | brainos: abi
//! BFS  USED BY  entity/entity, entity/tests/entity_suite, export/tests/export_suite, host/bench, host/compound, host/day, host/degrade, host/demo, +9 more
//!
//! Episodic memory — what happened.
//!
//! An episode is not a log line. A log line is text with a timestamp; an
//! episode has an *outcome*, and the outcome is what makes consolidation
//! possible. "Tried the tripod gait on carpet at 0.4 m/s" is a log entry.
//! "Tried the tripod gait on carpet at 0.4 m/s — the body stopped on
//! tip-over at 2.1 s" is an episode, and thirty of those are a skill.
//!
//! Episodes are the only memory kind the entity writes at speed, and
//! they are bounded. That is deliberate: a bound forces consolidation,
//! and consolidation is the mechanism by which a two-year-old entity is
//! better than a new one rather than merely longer-winded.

use crate::graph::{EdgeKind, NodeKind, StateGraph};
use crate::law::MemoryHandle;
use crate::provenance::Provenance;
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::codec::{Decode, Decoder, Encode, Encoder};
use brainos_abi::error::{Error, Result};
use brainos_abi::ids::{BodyId, NodeId};
use brainos_abi::time::{Duration, Tick};
use brainos_abi::verb::Verb;

/// How an attempt ended. Closed list, because consolidation branches on
/// it and a free-form outcome would have to be parsed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
#[repr(u8)]
pub enum Outcome {
    /// it did what was intended
    Succeeded = 0,
    /// it completed but not as intended
    Partial = 1,
    /// it was attempted and did not work
    Failed = 2,
    /// KIRA refused it before anything happened
    Denied = 3,
    /// a reflex stopped the body part-way
    Stopped = 4,
    /// nothing was attempted; this episode is an observation
    Observed = 5,
}

impl Outcome {
    pub const ALL: [Outcome; 6] = [
        Outcome::Succeeded,
        Outcome::Partial,
        Outcome::Failed,
        Outcome::Denied,
        Outcome::Stopped,
        Outcome::Observed,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Outcome::Succeeded => "succeeded",
            Outcome::Partial => "partial",
            Outcome::Failed => "failed",
            Outcome::Denied => "denied",
            Outcome::Stopped => "stopped",
            Outcome::Observed => "observed",
        }
    }

    /// Does this outcome count toward a skill's success rate? A denial is
    /// not a failure of the skill — the skill was never run — and
    /// counting it as one would teach the entity that permitted actions
    /// are unreliable.
    pub const fn counts_toward_skill(self) -> bool {
        matches!(
            self,
            Outcome::Succeeded | Outcome::Partial | Outcome::Failed | Outcome::Stopped
        )
    }

    pub const fn is_success(self) -> bool {
        matches!(self, Outcome::Succeeded)
    }

    /// Weight for reinforcement, in percent. A partial success teaches
    /// something; a stop teaches a great deal, because it names a
    /// condition the entity did not predict.
    pub const fn weight(self) -> u8 {
        match self {
            Outcome::Succeeded => 100,
            Outcome::Partial => 60,
            Outcome::Failed => 80,
            Outcome::Stopped => 100,
            Outcome::Denied => 40,
            Outcome::Observed => 30,
        }
    }
}

/// One thing that happened.
#[derive(Clone, Debug, PartialEq)]
pub struct Episode {
    /// what was attempted, if anything
    pub verb: Option<Verb>,
    /// free text: the situation, the arguments, what was observed
    pub what: String,
    pub outcome: Outcome,
    /// which body it happened on
    pub body: Option<BodyId>,
    /// how long it took
    pub took: Duration,
    /// the situation before the attempt — the raw material of a
    /// precondition. Kept as short tags rather than prose so that
    /// consolidation can intersect them across episodes.
    pub context: Vec<String>,
    pub at: Tick,
}

impl Episode {
    pub fn observed(what: &str, at: Tick) -> Episode {
        Episode {
            verb: None,
            what: alloc::string::ToString::to_string(what),
            outcome: Outcome::Observed,
            body: None,
            took: Duration::ZERO,
            context: Vec::new(),
            at,
        }
    }

    pub fn attempt(verb: Verb, what: &str, outcome: Outcome, at: Tick) -> Episode {
        Episode {
            verb: Some(verb),
            what: alloc::string::ToString::to_string(what),
            outcome,
            body: None,
            took: Duration::ZERO,
            context: Vec::new(),
            at,
        }
    }

    pub fn on(mut self, body: BodyId) -> Episode {
        self.body = Some(body);
        self
    }

    pub fn taking(mut self, d: Duration) -> Episode {
        self.took = d;
        self
    }

    pub fn in_context(mut self, tags: &[&str]) -> Episode {
        self.context = tags
            .iter()
            .map(|t| alloc::string::ToString::to_string(*t))
            .collect();
        self.context.sort();
        self.context.dedup();
        self
    }

    pub fn with_context(mut self, tag: &str) -> Episode {
        self.context.push(alloc::string::ToString::to_string(tag));
        self.context.sort();
        self.context.dedup();
        self
    }

    /// The line stored in the graph. Format matters: consolidation and
    /// retrieval both read it, so it is stable and parseable, with the
    /// verb first because that is what a skill is keyed on.
    pub fn render(&self) -> String {
        let mut s = String::new();
        if let Some(v) = &self.verb {
            s.push_str(v.as_str());
            s.push_str(" :: ");
        }
        s.push_str(&self.what);
        s.push_str(" :: ");
        s.push_str(self.outcome.name());
        if !self.context.is_empty() {
            s.push_str(" :: ");
            for (i, c) in self.context.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push_str(c);
            }
        }
        s
    }

    /// Parse back what `render` wrote. Round-tripping matters because
    /// consolidation reads episodes out of the graph rather than keeping
    /// a parallel structure that could drift.
    pub fn parse(line: &str, at: Tick) -> Option<Episode> {
        let mut parts = line.split(" :: ");
        let first = parts.next()?;
        let (verb, what) = match Verb::parse(first) {
            Ok(v) => (Some(v), parts.next()?),
            Err(_) => (None, first),
        };
        let outcome_s = parts.next().unwrap_or("observed");
        let outcome = Outcome::ALL
            .iter()
            .copied()
            .find(|o| o.name() == outcome_s)
            .unwrap_or(Outcome::Observed);
        let context: Vec<String> = match parts.next() {
            Some(c) if !c.is_empty() => c
                .split(',')
                .map(|t| alloc::string::ToString::to_string(t.trim()))
                .filter(|t: &String| !t.is_empty())
                .collect(),
            _ => Vec::new(),
        };
        Some(Episode {
            verb,
            what: alloc::string::ToString::to_string(what),
            outcome,
            body: None,
            took: Duration::ZERO,
            context,
            at,
        })
    }

    /// Write into the graph, linked to the previous episode.
    pub fn record(&self, h: &MemoryHandle, graph: &mut StateGraph, prov: Provenance) -> NodeId {
        let previous = graph.last(NodeKind::Episode).map(|n| n.id);
        // an episode is something the entity observed happening, so its
        // confidence is the origin's ceiling; what is uncertain is the
        // *interpretation*, and that lives in beliefs
        let id = graph.add(h, NodeKind::Episode, &self.render(), 100, prov);
        if let Some(p) = previous {
            graph.link(h, id, p, EdgeKind::Follows);
        }
        id
    }
}

impl Encode for Episode {
    fn encode(&self, e: &mut Encoder) {
        e.opt(&self.verb);
        e.str(&self.what);
        e.u8(self.outcome as u8);
        e.opt(&self.body);
        self.took.encode(e);
        e.seq(self.context.len());
        for c in &self.context {
            e.str(c);
        }
        self.at.encode(e);
    }
}

impl Decode for Episode {
    fn decode(d: &mut Decoder) -> Result<Episode> {
        let verb: Option<Verb> = d.opt()?;
        let what = alloc::string::ToString::to_string(d.str()?);
        let o = d.u8()?;
        let outcome = Outcome::ALL
            .get(o as usize)
            .copied()
            .ok_or(Error::Malformed("unknown outcome"))?;
        let body: Option<BodyId> = d.opt()?;
        let took = Duration::decode(d)?;
        let n = d.seq()?;
        let mut context = Vec::new();
        for _ in 0..n {
            context.push(alloc::string::ToString::to_string(d.str()?));
        }
        Ok(Episode {
            verb,
            what,
            outcome,
            body,
            took,
            context,
            at: Tick::decode(d)?,
        })
    }
}
