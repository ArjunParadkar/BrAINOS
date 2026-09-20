//! The skill graph — procedural memory, and the thing that compounds.
//!
//! §2.2 says a two-year-old entity should be measurably better than a
//! new one, and §2.4 says that curve gets published. This module is what
//! the curve measures.
//!
//! A skill is not a script. It is:
//!
//! * a **verb** it achieves,
//! * **preconditions** — the context tags that were present when it
//!   worked, intersected across successes and therefore *learned* rather
//!   than declared,
//! * **effects** — what became true afterwards,
//! * **statistics** — attempts, successes, and where it failed, per body,
//! * optionally **steps**, when the skill is a chain of other skills.
//!
//! The precondition learning is the interesting part and the honest
//! caveat sits with it: intersecting the context of successful attempts
//! finds *correlates*, not causes. A skill that only ever succeeded in
//! the morning will learn "morning" as a precondition. That is why
//! preconditions carry a support count and why `applicable` treats a
//! thinly-supported precondition as advisory rather than binding — and
//! why the honest thing to publish is the success-rate curve, not a
//! claim that the entity understands why.

use crate::graph::{EdgeKind, NodeKind, StateGraph};
use crate::law::MemoryHandle;
use crate::provenance::Provenance;
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::codec::{Decode, Decoder, Encode, Encoder};
use brainos_abi::error::{Error, Result};
use brainos_abi::ids::{BodyId, NodeId};
use brainos_abi::time::Tick;
use brainos_abi::verb::Verb;

pub type SkillId = NodeId;

/// A condition learned from experience.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Precondition {
    pub tag: String,
    /// how many successful attempts had this tag present
    pub support: u16,
    /// how many successful attempts did not
    pub against: u16,
}

impl Precondition {
    /// How strongly the evidence backs this being required, 0–100.
    pub fn strength(&self) -> u8 {
        let total = self.support as u32 + self.against as u32;
        if total == 0 {
            return 0;
        }
        ((self.support as u32 * 100) / total) as u8
    }

    /// Enough evidence to treat as binding rather than advisory. Five
    /// supporting observations and no counterexamples is the threshold —
    /// low enough to be useful early, high enough that one coincidence
    /// does not become a rule.
    pub fn is_binding(&self) -> bool {
        self.support >= 5 && self.strength() >= 90
    }
}

impl Encode for Precondition {
    fn encode(&self, e: &mut Encoder) {
        e.str(&self.tag);
        e.u16(self.support);
        e.u16(self.against);
    }
}

impl Decode for Precondition {
    fn decode(d: &mut Decoder) -> Result<Precondition> {
        Ok(Precondition {
            tag: alloc::string::ToString::to_string(d.str()?),
            support: d.u16()?,
            against: d.u16()?,
        })
    }
}

/// One step of a composite skill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub verb: Verb,
    pub argument: String,
}

impl Encode for Step {
    fn encode(&self, e: &mut Encoder) {
        self.verb.encode(e);
        e.str(&self.argument);
    }
}

impl Decode for Step {
    fn decode(d: &mut Decoder) -> Result<Step> {
        Ok(Step {
            verb: Verb::decode(d)?,
            argument: alloc::string::ToString::to_string(d.str()?),
        })
    }
}

/// Per-body statistics. A skill that works on Mini-Blur and not on a
/// Unitree is not a broken skill — it is a skill with a body-dependent
/// success rate, and an entity that conflates the two will keep trying
/// the wrong thing on the wrong body forever.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BodyStats {
    pub body: BodyId,
    pub attempts: u32,
    pub successes: u32,
    pub stops: u32,
}

impl BodyStats {
    pub fn rate_pct(&self) -> u8 {
        if self.attempts == 0 {
            return 0;
        }
        ((self.successes as u64 * 100) / self.attempts as u64) as u8
    }
}

impl Encode for BodyStats {
    fn encode(&self, e: &mut Encoder) {
        self.body.encode(e);
        e.u32(self.attempts);
        e.u32(self.successes);
        e.u32(self.stops);
    }
}

impl Decode for BodyStats {
    fn decode(d: &mut Decoder) -> Result<BodyStats> {
        Ok(BodyStats {
            body: BodyId::decode(d)?,
            attempts: d.u32()?,
            successes: d.u32()?,
            stops: d.u32()?,
        })
    }
}

/// A learned way of doing something.
#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    pub id: SkillId,
    pub verb: Verb,
    /// a short human-readable name
    pub name: String,
    pub preconditions: Vec<Precondition>,
    pub effects: Vec<String>,
    pub steps: Vec<Step>,
    pub attempts: u32,
    pub successes: u32,
    pub per_body: Vec<BodyStats>,
    pub first_learned: Tick,
    pub last_used: Tick,
}

impl Skill {
    pub fn success_rate_pct(&self) -> u8 {
        if self.attempts == 0 {
            return 0;
        }
        ((self.successes as u64 * 100) / self.attempts as u64) as u8
    }

    /// Success rate on one specific body.
    pub fn rate_on(&self, body: &BodyId) -> Option<u8> {
        self.per_body
            .iter()
            .find(|s| s.body == *body)
            .map(|s| s.rate_pct())
    }

    /// Is this skill trustworthy enough to reach for without thinking?
    /// Twenty attempts at eighty percent. Below that it is a candidate,
    /// not a skill, and the entity should escalate rather than assume.
    pub fn is_practiced(&self) -> bool {
        self.attempts >= 20 && self.success_rate_pct() >= 80
    }

    /// Does the current context satisfy this skill's learned conditions?
    ///
    /// Returns the *missing* binding preconditions. Empty means go. This
    /// returns what is missing rather than a bool because "I know how to
    /// do this but the floor is wet" is a far more useful thing for
    /// cognition to receive than `false`.
    pub fn missing(&self, context: &[&str]) -> Vec<&Precondition> {
        self.preconditions
            .iter()
            .filter(|p| p.is_binding() && !context.iter().any(|c| *c == p.tag))
            .collect()
    }

    pub fn applicable(&self, context: &[&str]) -> bool {
        self.missing(context).is_empty()
    }

    /// Fold in one attempt.
    pub fn observe(&mut self, success: bool, stopped: bool, body: Option<BodyId>, context: &[String], at: Tick) {
        self.attempts = self.attempts.saturating_add(1);
        if success {
            self.successes = self.successes.saturating_add(1);
        }
        self.last_used = at;

        if let Some(b) = body {
            match self.per_body.iter_mut().find(|s| s.body == b) {
                Some(s) => {
                    s.attempts = s.attempts.saturating_add(1);
                    if success {
                        s.successes = s.successes.saturating_add(1);
                    }
                    if stopped {
                        s.stops = s.stops.saturating_add(1);
                    }
                }
                None => self.per_body.push(BodyStats {
                    body: b,
                    attempts: 1,
                    successes: success as u32,
                    stops: stopped as u32,
                }),
            }
        }

        // preconditions are learned from successes only: what was present
        // when it failed is not evidence about what it needs
        if !success {
            return;
        }
        for p in self.preconditions.iter_mut() {
            if context.iter().any(|c| c == &p.tag) {
                p.support = p.support.saturating_add(1);
            } else {
                p.against = p.against.saturating_add(1);
            }
        }
        for tag in context {
            if !self.preconditions.iter().any(|p| &p.tag == tag) {
                self.preconditions.push(Precondition {
                    tag: tag.clone(),
                    support: 1,
                    // a tag first seen on the tenth success was absent for
                    // the previous nine, and pretending otherwise would
                    // make a late coincidence look like a strong condition
                    against: self.successes.saturating_sub(1).min(u16::MAX as u32) as u16,
                });
            }
        }
        // a condition that is present about as often as not is noise
        self.preconditions
            .retain(|p| p.support >= 2 || p.against < 2);
    }

    pub fn render(&self) -> String {
        let mut s = String::new();
        s.push_str(self.verb.as_str());
        s.push_str(" :: ");
        s.push_str(&self.name);
        s.push_str(" :: ");
        // the numbers go in the node content so that retrieval can find
        // "the thing I am good at" lexically as well as structurally
        push_u32(&mut s, self.successes);
        s.push('/');
        push_u32(&mut s, self.attempts);
        if !self.preconditions.is_empty() {
            s.push_str(" :: needs ");
            let mut first = true;
            for p in self.preconditions.iter().filter(|p| p.is_binding()) {
                if !first {
                    s.push(',');
                }
                s.push_str(&p.tag);
                first = false;
            }
        }
        s
    }
}

fn push_u32(s: &mut String, mut v: u32) {
    if v == 0 {
        s.push('0');
        return;
    }
    let mut buf = [0u8; 10];
    let mut n = 0;
    while v > 0 {
        buf[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        s.push(buf[n] as char);
    }
}

impl Encode for Skill {
    fn encode(&self, e: &mut Encoder) {
        self.id.encode(e);
        self.verb.encode(e);
        e.str(&self.name);
        e.seq(self.preconditions.len());
        for p in &self.preconditions {
            p.encode(e);
        }
        e.seq(self.effects.len());
        for f in &self.effects {
            e.str(f);
        }
        e.seq(self.steps.len());
        for s in &self.steps {
            s.encode(e);
        }
        e.u32(self.attempts);
        e.u32(self.successes);
        e.seq(self.per_body.len());
        for b in &self.per_body {
            b.encode(e);
        }
        self.first_learned.encode(e);
        self.last_used.encode(e);
    }
}

impl Decode for Skill {
    fn decode(d: &mut Decoder) -> Result<Skill> {
        let id = NodeId::decode(d)?;
        let verb = Verb::decode(d)?;
        let name = alloc::string::ToString::to_string(d.str()?);
        let n = d.seq()?;
        let mut preconditions = Vec::new();
        for _ in 0..n {
            preconditions.push(Precondition::decode(d)?);
        }
        let n = d.seq()?;
        let mut effects = Vec::new();
        for _ in 0..n {
            effects.push(alloc::string::ToString::to_string(d.str()?));
        }
        let n = d.seq()?;
        let mut steps = Vec::new();
        for _ in 0..n {
            steps.push(Step::decode(d)?);
        }
        let attempts = d.u32()?;
        let successes = d.u32()?;
        let n = d.seq()?;
        let mut per_body = Vec::new();
        for _ in 0..n {
            per_body.push(BodyStats::decode(d)?);
        }
        Ok(Skill {
            id,
            verb,
            name,
            preconditions,
            effects,
            steps,
            attempts,
            successes,
            per_body,
            first_learned: Tick::decode(d)?,
            last_used: Tick::decode(d)?,
        })
    }
}

/// Every skill the entity has, with the graph nodes that back them.
///
/// The skills live here rather than being reconstructed from node text
/// on every query, and the graph holds a node per skill so that episodes
/// can point at the skill they exercised. The two are kept in step by
/// `learn` and `observe` being the only mutators.
#[derive(Default)]
pub struct SkillGraph {
    skills: Vec<Skill>,
}

impl SkillGraph {
    pub fn new() -> SkillGraph {
        SkillGraph { skills: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    pub fn all(&self) -> &[Skill] {
        &self.skills
    }

    pub fn get(&self, id: SkillId) -> Option<&Skill> {
        self.skills.iter().find(|s| s.id == id)
    }

    pub fn for_verb(&self, verb: &Verb) -> Option<&Skill> {
        self.skills.iter().find(|s| &s.verb == verb)
    }

    pub fn for_verb_mut(&mut self, verb: &Verb) -> Option<&mut Skill> {
        self.skills.iter_mut().find(|s| &s.verb == verb)
    }

    /// Learn a skill, or return the existing one for that verb. Writes a
    /// node into the graph so the skill is part of memory rather than a
    /// parallel structure that would not survive an export.
    pub fn learn(
        &mut self,
        h: &MemoryHandle,
        graph: &mut StateGraph,
        verb: Verb,
        name: &str,
        prov: Provenance,
    ) -> SkillId {
        if let Some(existing) = self.for_verb(&verb) {
            return existing.id;
        }
        let skill = Skill {
            id: NodeId::NONE,
            verb,
            name: alloc::string::ToString::to_string(name),
            preconditions: Vec::new(),
            effects: Vec::new(),
            steps: Vec::new(),
            attempts: 0,
            successes: 0,
            per_body: Vec::new(),
            first_learned: prov.at,
            last_used: prov.at,
        };
        let id = graph.add(h, NodeKind::Skill, &skill.render(), 60, prov);
        self.skills.push(Skill { id, ..skill });
        id
    }

    /// Record an attempt and refresh the node text.
    pub fn observe(
        &mut self,
        h: &MemoryHandle,
        graph: &mut StateGraph,
        verb: &Verb,
        success: bool,
        stopped: bool,
        body: Option<BodyId>,
        context: &[String],
        at: Tick,
        episode: Option<NodeId>,
    ) -> Option<SkillId> {
        let skill = self.skills.iter_mut().find(|s| &s.verb == verb)?;
        skill.observe(success, stopped, body, context, at);
        let id = skill.id;
        let text = skill.render();
        // the graph node mirrors the skill, so an export carries both the
        // statistics and the memory they were learned from
        graph.revise(h, id, 60 + (skill.success_rate_pct() / 3));
        rewrite_node(h, graph, id, &text);
        if let Some(ep) = episode {
            graph.link(h, ep, id, EdgeKind::About);
        }
        Some(id)
    }

    /// Compose a skill out of steps that are themselves skills.
    pub fn compose(
        &mut self,
        h: &MemoryHandle,
        graph: &mut StateGraph,
        verb: Verb,
        name: &str,
        steps: Vec<Step>,
        prov: Provenance,
    ) -> SkillId {
        let id = self.learn(h, graph, verb, name, prov);
        let child_ids: Vec<NodeId> = steps
            .iter()
            .filter_map(|s| self.for_verb(&s.verb).map(|c| c.id))
            .collect();
        if let Some(s) = self.skills.iter_mut().find(|s| s.id == id) {
            s.steps = steps;
        }
        for c in child_ids {
            graph.link(h, id, c, EdgeKind::Uses);
        }
        id
    }

    /// The skills applicable in a context, best first.
    pub fn applicable(&self, context: &[&str]) -> Vec<&Skill> {
        let mut out: Vec<&Skill> = self
            .skills
            .iter()
            .filter(|s| s.applicable(context))
            .collect();
        out.sort_by(|a, b| {
            b.success_rate_pct()
                .cmp(&a.success_rate_pct())
                .then(b.attempts.cmp(&a.attempts))
                .then(a.id.0.cmp(&b.id.0))
        });
        out
    }

    /// The moat's measurement: how good is this entity, overall, right
    /// now? Mean success rate weighted by attempts, so a skill practiced
    /// a thousand times counts for more than one tried twice.
    pub fn competence_pct(&self) -> u8 {
        let mut num = 0u64;
        let mut den = 0u64;
        for s in &self.skills {
            num += s.successes as u64 * 100;
            den += s.attempts as u64;
        }
        if den == 0 {
            0
        } else {
            (num / den / 1).min(100) as u8
        }
    }

    /// Skills that are practiced — the ones the entity can be relied on
    /// for. This number, over time, is the published curve.
    pub fn practiced_count(&self) -> usize {
        self.skills.iter().filter(|s| s.is_practiced()).count()
    }

    /// Rebuild from a graph after an import.
    pub fn rehydrate(&mut self, graph: &StateGraph) {
        self.skills.clear();
        for n in graph.of_kind(NodeKind::Skill) {
            let mut parts = n.content.split(" :: ");
            let Some(verb_s) = parts.next() else { continue };
            let Ok(verb) = Verb::parse(verb_s) else {
                continue;
            };
            let name = parts.next().unwrap_or("");
            let (successes, attempts) = match parts.next() {
                Some(stats) => match stats.split_once('/') {
                    Some((s, a)) => (
                        s.parse::<u32>().unwrap_or(0),
                        a.parse::<u32>().unwrap_or(0),
                    ),
                    None => (0, 0),
                },
                None => (0, 0),
            };
            self.skills.push(Skill {
                id: n.id,
                verb,
                name: alloc::string::ToString::to_string(name),
                preconditions: Vec::new(),
                effects: Vec::new(),
                steps: Vec::new(),
                attempts,
                successes,
                per_body: Vec::new(),
                first_learned: n.tick(),
                last_used: n.tick(),
            });
        }
    }
}

/// Replace a node's content. Lives here rather than on `StateGraph`
/// because rewriting a memory is a narrow, suspicious operation and
/// should be reachable from exactly one place: a skill node keeping its
/// own statistics current.
fn rewrite_node(h: &MemoryHandle, graph: &mut StateGraph, id: NodeId, text: &str) {
    graph.rewrite_skill_node(h, id, text);
}

impl Skill {
    /// Decode a skill from its own bytes; used by the export round-trip.
    pub fn from_bytes(b: &[u8]) -> Result<Skill> {
        let mut d = Decoder::new(b);
        let s = Skill::decode(&mut d)?;
        d.finish().map_err(|_| Error::Malformed("trailing bytes"))?;
        Ok(s)
    }
}
