//! Beliefs — the world model, held with confidence and revisable.
//!
//! A belief is distinguished from a semantic memory by being *about the
//! world right now* rather than about what is generally true, and by
//! being the thing that gets contradicted. "The left middle leg binds
//! past 30°" is semantic. "The floor here is carpet" is a belief, and it
//! is wrong the moment the robot is carried into the kitchen.
//!
//! **Revision is evidence-weighted, not last-write-wins.** A belief
//! accumulates supporting and contradicting evidence as graph edges, and
//! its confidence is recomputed from them. This matters because the
//! naive alternative — whatever was said most recently is true — is
//! exactly the vulnerability that lets one sentence from an untrusted
//! world overwrite something the body established over a hundred
//! observations.

use crate::graph::{EdgeKind, Node, NodeKind, StateGraph};
use crate::law::MemoryHandle;
use crate::provenance::{Origin, Provenance};
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::ids::NodeId;
use brainos_abi::time::Tick;

/// A belief, as a view over its node plus its evidence.
#[derive(Clone, Debug)]
pub struct Belief {
    pub id: NodeId,
    pub claim: String,
    pub confidence: u8,
    pub origin: Origin,
    pub supports: usize,
    pub contradicts: usize,
    pub at: Tick,
}

/// What a revision did, so the caller can decide whether it is worth
/// telling anyone. A belief quietly strengthening is not news; a belief
/// flipping is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Revision {
    /// confidence rose
    Strengthened { from: u8, to: u8 },
    /// confidence fell but the belief still stands
    Weakened { from: u8, to: u8 },
    /// the belief fell below the threshold and was abandoned
    Abandoned { was: u8 },
    /// no change
    Unchanged,
}

impl Revision {
    /// Is this worth surfacing to the human? Only a flip.
    pub fn is_salient(&self) -> bool {
        matches!(self, Revision::Abandoned { .. })
    }
}

/// Below this, a belief is no longer held.
pub const ABANDON_BELOW: u8 = 15;

/// Assert a belief. If the same claim is already held, this becomes
/// supporting evidence rather than a second copy — otherwise an entity
/// told the same thing ten times would hold ten beliefs and be unable to
/// notice it had been told ten times.
pub fn assert_belief(
    h: &MemoryHandle,
    graph: &mut StateGraph,
    claim: &str,
    confidence: u8,
    prov: Provenance,
) -> (NodeId, Revision) {
    if let Some(existing) = find(graph, claim) {
        let before = existing.confidence;
        let id = existing.id;
        // supporting evidence is a node of its own, so the belief's
        // history is visible rather than being a number that moved
        let ev = graph.add(
            h,
            NodeKind::Episode,
            claim,
            confidence,
            prov,
        );
        graph.link(h, ev, id, EdgeKind::Supports);
        let after = recompute(graph, id);
        graph.revise(h, id, after);
        graph.reinforce(h, id);
        return (id, classify(before, after));
    }
    let id = graph.add(h, NodeKind::Belief, claim, confidence, prov);
    (id, Revision::Strengthened { from: 0, to: confidence })
}

/// Contradict a belief with evidence.
pub fn contradict(
    h: &MemoryHandle,
    graph: &mut StateGraph,
    belief: NodeId,
    why: &str,
    prov: Provenance,
) -> Revision {
    let Some(b) = graph.get(belief) else {
        return Revision::Unchanged;
    };
    let before = b.confidence;
    let ev = graph.add(h, NodeKind::Episode, why, 100, prov);
    graph.link(h, ev, belief, EdgeKind::Contradicts);
    let after = recompute(graph, belief);
    graph.revise(h, belief, after);
    if after < ABANDON_BELOW {
        graph.forget(h, belief);
        return Revision::Abandoned { was: before };
    }
    classify(before, after)
}

fn classify(before: u8, after: u8) -> Revision {
    if after < ABANDON_BELOW {
        Revision::Abandoned { was: before }
    } else if after > before {
        Revision::Strengthened {
            from: before,
            to: after,
        }
    } else if after < before {
        Revision::Weakened {
            from: before,
            to: after,
        }
    } else {
        Revision::Unchanged
    }
}

/// Recompute confidence from evidence.
///
/// The rule: each piece of supporting evidence moves confidence a
/// fraction of the way toward 100, weighted by *its own* origin ceiling;
/// each contradiction moves it toward 0, unweighted. The asymmetry is
/// deliberate and is the whole safety argument of this module — it is
/// harder to convince the entity of something than to make it doubt
/// something, and evidence from an untrusted world convinces less.
pub fn recompute(graph: &StateGraph, belief: NodeId) -> u8 {
    let Some(b) = graph.get(belief) else { return 0 };
    let mut conf = b.confidence as i32;

    for ev in graph.backlinks(belief, EdgeKind::Supports) {
        let ceiling = ev.origin().confidence_ceiling() as i32;
        // move a sixth of the way toward the evidence's own ceiling, and
        // never past it: ten internet pages cannot make a belief certain
        let target = ceiling.min(100);
        if conf < target {
            conf += (target - conf) / 6;
        }
    }
    for _ in graph.backlinks(belief, EdgeKind::Contradicts) {
        // a third of the way toward zero, per contradiction
        conf -= conf / 3;
    }

    b.provenance.cap(conf.clamp(0, 100) as u8)
}

fn find<'a>(graph: &'a StateGraph, claim: &str) -> Option<&'a Node> {
    graph
        .of_kind(NodeKind::Belief)
        .find(|n| n.content == claim)
}

/// Every belief currently held, strongest first.
pub fn held(graph: &StateGraph) -> Vec<Belief> {
    let mut out: Vec<Belief> = graph
        .of_kind(NodeKind::Belief)
        .map(|n| Belief {
            id: n.id,
            claim: n.content.clone(),
            confidence: n.confidence,
            origin: n.origin(),
            supports: graph.backlinks(n.id, EdgeKind::Supports).len(),
            contradicts: graph.backlinks(n.id, EdgeKind::Contradicts).len(),
            at: n.tick(),
        })
        .collect();
    out.sort_by(|a, b| b.confidence.cmp(&a.confidence).then(a.id.0.cmp(&b.id.0)));
    out
}

/// Beliefs the policy stage may consult — only those from worlds the
/// entity itself vouches for.
pub fn admissible(graph: &StateGraph) -> Vec<Belief> {
    held(graph)
        .into_iter()
        .filter(|b| b.origin.admissible_to_policy())
        .collect()
}
