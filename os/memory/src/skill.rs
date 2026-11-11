//! BFS  LEVEL    Domain 5 State - the state graph; the only memory  [crate brainos-memory]
//! BFS  PURPOSE  Learned skills: per verb, the conditions it worked under (learned and re-tested, not declared), its effects and success counts per body
//! BFS  DEFINES  SkillId, EVIDENCE_WINDOW, STALE_AFTER, MAX_PRECONDITIONS, LOCAL_TESTS_ENOUGH, PROBE_EVERY, Decision, Precondition, Step, BodyStats, Skill, SkillGraph
//! BFS  USES     this crate: graph, law, provenance | brainos: abi
//! BFS  USED BY  entity/entity, export/tests/export_suite, host/demo, host/fleet_experiment, host/limits, host/worlds_run, memory/consolidate, memory/lib, +3 more
//!
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

/// How much evidence a precondition remembers.
///
/// Support and against are exponentially-forgetting counters: when their
/// total passes this, both halve. The ratio survives, the magnitude does
/// not, and that is the point — a belief backed by two hundred
/// observations from last year should not need two hundred
/// counterexamples to shift.
pub const EVIDENCE_WINDOW: u16 = 64;

/// How long a precondition stays binding without being tested.
///
/// A precondition is *tested* when the entity attempts without it and
/// finds out what happens. Confirming that carpet is sufficient is not
/// confirming that carpet is necessary, and only varying the thing
/// distinguishes a correlate from a cause — which `Precondition` has
/// always admitted and which an entity with a binding precondition can
/// never do, because it declines exactly the situations that would
/// tell it.
///
/// So beliefs expire unless tested. The cost is one attempt per window
/// that the entity expects to fail; the return is that when the world
/// changes, it finds out.
pub const STALE_AFTER: u64 = 400;

/// The most candidate preconditions one skill tracks.
///
/// Everything else in this system is bounded and this was not: a world
/// with many distinct tags would grow a skill without limit, and now
/// that a skill is serialized into its node, an unbounded skill is an
/// unbounded node. When the cap is reached the weakest *untested*
/// candidate is dropped, never a tested one — an entity should forget a
/// hunch before it forgets something it found out.
pub const MAX_PRECONDITIONS: usize = 32;

/// How many local tests before an adopted claim stops being taken on
/// trust. The same five that a self-learned one needs, because it is the
/// same question: has this entity found out, here?
pub const LOCAL_TESTS_ENOUGH: u16 = 5;

/// How often an entity deliberately violates a borrowed rule it has not
/// checked, so that it can find out whether the rule is true here.
///
/// One opportunity in twelve. The number is a cost: twelve is roughly
/// sixty attempts to resolve a borrowed claim, and each of those
/// attempts may fail. Measured against the alternative — a wrong
/// borrowed rule left in place, which the fleet experiment found costs
/// seven points of judgement permanently — that is cheap. It is not
/// free, and the experiment reports the failures it causes.
pub const PROBE_EVERY: u16 = 12;

/// What an entity decided to do about a skill, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// nothing binding is missing
    Attempt,
    /// something borrowed is missing, and this is the deliberate
    /// violation that finds out whether it is true here. The tag is the
    /// claim being tested, so the entity can say what it is doing.
    Probe(String),
    /// something the entity established for itself is missing
    Decline(String),
}

impl Decision {
    pub fn acts(&self) -> bool {
        matches!(self, Decision::Attempt | Decision::Probe(_))
    }
}

/// A condition learned from experience.
///
/// **What makes something a precondition is that without it the thing
/// fails.** That is a claim about absence, and the only way to learn it
/// is to act without the tag and find out. Intersecting the context of
/// successes — which is what this used to do — finds *correlates*, and
/// a correlate held as a cause has a nasty property: the entity
/// declines exactly the situations that would tell it otherwise, so
/// caution prevents the correction, and the more experience it has the
/// more confident the mistake.
///
/// A two-thousand-episode run with the world's rule changing halfway
/// found the hole exactly: the entity made **zero** attempts in the new
/// regime and lost its competence permanently.
///
/// So strength is measured from *tests* — attempts made without the tag
/// — and not from confirmations. Attempting with the tag a thousand more
/// times says nothing about whether the tag is needed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Precondition {
    pub tag: String,
    /// successes that had this tag present. Reporting only: it says the
    /// tag co-occurs with success, which is not the claim.
    pub support: u16,
    /// successes that did not have it — counterexamples to necessity
    pub against: u16,
    /// attempts made *without* this tag, whatever the outcome. The
    /// denominator of the only claim that matters.
    pub tests: u16,
    /// of those, how many failed
    pub test_failures: u16,
    /// when the entity last attempted without this tag — the last time
    /// it found out rather than assumed
    pub last_tested: Tick,
    /// This came from somebody else and has not been checked here.
    ///
    /// The fleet experiment measured what happens without this
    /// distinction: a skill adopted from a deployment unlike your own
    /// leaves the entity permanently *worse* than one that started from
    /// nothing, because believing a condition is required means
    /// declining exactly the situations that would disprove it. An
    /// adopted claim is therefore held differently from an established
    /// one — acted on, because that is what makes it worth buying, and
    /// deliberately violated now and then, because that is what makes it
    /// correctable.
    pub adopted: bool,
    /// Blocked opportunities since the last deliberate violation.
    /// Only meaningful while `adopted` and unverified.
    pub since_probe: u16,
}

impl Precondition {
    /// How strongly the evidence backs this being *required*, 0–100:
    /// the fraction of attempts without it that failed.
    ///
    /// Untested is zero, not unknown-but-probably-fine. An entity that
    /// treated an untested correlate as a requirement would be back
    /// where it started.
    pub fn strength(&self) -> u8 {
        if self.tests == 0 {
            return 0;
        }
        ((self.test_failures as u32 * 100) / self.tests as u32) as u8
    }

    /// How often it merely co-occurred with success. Reported so the
    /// difference between correlation and the tested claim is visible.
    pub fn co_occurrence_pct(&self) -> u8 {
        let total = self.support as u32 + self.against as u32;
        if total == 0 {
            return 0;
        }
        ((self.support as u32 * 100) / total) as u8
    }

    /// Enough tests, failing often enough, to treat as required.
    ///
    /// Five tests and eighty percent. The threshold is below a hundred
    /// on purpose: a real body in a real place fails sometimes for
    /// reasons that have nothing to do with the condition, and a rule
    /// that demanded perfection would never bind.
    pub fn is_well_supported(&self) -> bool {
        self.tests >= 5 && self.strength() >= 80
    }

    /// Has it been tested recently enough to still be binding?
    pub fn is_fresh(&self, now: Tick) -> bool {
        now.since(self.last_tested) <= STALE_AFTER
    }

    /// Binding: tested enough, failing often enough, and recently
    /// enough.
    ///
    /// All three matter. Without the tests a correlate becomes a rule;
    /// without the failures the rule is backed by nothing; without
    /// freshness it outlives the world it was learned in and the
    /// entity's own caution stops it finding out.
    pub fn is_binding_at(&self, now: Tick) -> bool {
        if self.tests >= LOCAL_TESTS_ENOUGH {
            // enough local evidence: what this entity found out decides,
            // whoever it originally came from
            return self.is_well_supported() && self.is_fresh(now);
        }
        // borrowed and not yet checked here. Acted on — that is what
        // makes a bought skill worth buying — and probed, which is what
        // keeps it from being permanent.
        self.adopted && self.is_fresh(now)
    }

    /// Borrowed, and this entity has not yet found out for itself.
    pub fn is_on_trust(&self) -> bool {
        self.adopted && self.tests < LOCAL_TESTS_ENOUGH
    }

    /// Kept for callers that only want the evidence question.
    pub fn is_binding(&self) -> bool {
        self.is_well_supported()
    }

    /// Age the evidence, so the answer reflects what has been tested
    /// recently rather than what was ever tested.
    fn forget(&mut self) {
        if self.tests > EVIDENCE_WINDOW {
            self.tests /= 2;
            self.test_failures /= 2;
        }
        if self.support.saturating_add(self.against) > EVIDENCE_WINDOW {
            self.support /= 2;
            self.against /= 2;
        }
    }
}

impl Encode for Precondition {
    fn encode(&self, e: &mut Encoder) {
        e.str(&self.tag);
        e.u16(self.support);
        e.u16(self.against);
        e.u16(self.tests);
        e.u16(self.test_failures);
        self.last_tested.encode(e);
        e.bool(self.adopted);
        e.u16(self.since_probe);
    }
}

impl Decode for Precondition {
    fn decode(d: &mut Decoder) -> Result<Precondition> {
        Ok(Precondition {
            tag: alloc::string::ToString::to_string(d.str()?),
            support: d.u16()?,
            against: d.u16()?,
            tests: d.u16()?,
            test_failures: d.u16()?,
            last_tested: Tick::decode(d)?,
            adopted: d.bool()?,
            since_probe: d.u16()?,
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
    /// Which binding preconditions the context does not satisfy, as of
    /// `now`. A precondition that has gone stale is not binding, so it
    /// does not appear here and the entity attempts — which is how it
    /// finds out whether it is still right.
    pub fn missing_at(&self, context: &[&str], now: Tick) -> Vec<&Precondition> {
        self.preconditions
            .iter()
            .filter(|p| p.is_binding_at(now) && !context.iter().any(|c| *c == p.tag))
            .collect()
    }

    /// The same question ignoring staleness: what does the evidence say?
    pub fn missing(&self, context: &[&str]) -> Vec<&Precondition> {
        self.preconditions
            .iter()
            .filter(|p| p.is_well_supported() && !context.iter().any(|c| *c == p.tag))
            .collect()
    }

    pub fn applicable_at(&self, context: &[&str], now: Tick) -> bool {
        self.missing_at(context, now).is_empty()
    }

    /// Decide, and remember having decided.
    ///
    /// `applicable_at` is the read-only question and stays that way.
    /// This is the one an entity actually acts on, and it mutates
    /// because a decision to hold off on a borrowed rule is itself
    /// something to keep track of: the entity owes that rule a test, and
    /// this is where the debt is counted.
    pub fn decide_at(&mut self, context: &[&str], now: Tick) -> Decision {
        let blocking: Vec<usize> = self
            .preconditions
            .iter()
            .enumerate()
            .filter(|(_, p)| p.is_binding_at(now) && !context.iter().any(|c| *c == p.tag))
            .map(|(i, _)| i)
            .collect();

        if blocking.is_empty() {
            return Decision::Attempt;
        }

        // Probing is only honest when *every* reason to hold off is one
        // the entity took on trust. A locally-established condition is
        // missing means declining is right, and violating it to check a
        // borrowed one next to it would be finding out the wrong thing.
        let all_on_trust = blocking
            .iter()
            .all(|&i| self.preconditions[i].is_on_trust());
        if !all_on_trust {
            let tag = self.preconditions[blocking[0]].tag.clone();
            return Decision::Decline(tag);
        }

        let mut due = None;
        for &i in &blocking {
            let p = &mut self.preconditions[i];
            p.since_probe = p.since_probe.saturating_add(1);
            if due.is_none() && p.since_probe >= PROBE_EVERY {
                due = Some(i);
            }
        }
        match due {
            Some(i) => {
                for &j in &blocking {
                    self.preconditions[j].since_probe = 0;
                }
                Decision::Probe(self.preconditions[i].tag.clone())
            }
            None => Decision::Decline(self.preconditions[blocking[0]].tag.clone()),
        }
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

        // Every attempt without a tag is a *test* of that precondition,
        // whatever the outcome. This is the only moment the entity finds
        // out rather than assumes.
        for p in self.preconditions.iter_mut() {
            if !context.iter().any(|c| c == &p.tag) {
                p.tests = p.tests.saturating_add(1);
                p.last_tested = at;
                if !success {
                    p.test_failures = p.test_failures.saturating_add(1);
                }
                if success {
                    p.against = p.against.saturating_add(1);
                }
            } else if success {
                p.support = p.support.saturating_add(1);
            }
            p.forget();
        }

        // a tag is only worth tracking once it has co-occurred with a
        // success; a tag seen only during failures is not a candidate
        if !success {
            return;
        }
        for tag in context {
            if !self.preconditions.iter().any(|p| &p.tag == tag) {
                self.preconditions.push(Precondition {
                    tag: tag.clone(),
                    support: 1,
                    against: 0,
                    // untested, and therefore not binding: the entity has
                    // to act without it before it may require it
                    tests: 0,
                    test_failures: 0,
                    last_tested: at,
                    adopted: false,
                    since_probe: 0,
                });
            }
        }
        // a tag that has been tested and did not matter stops being a
        // candidate at all
        self.preconditions
            .retain(|p| p.tests < LOCAL_TESTS_ENOUGH || p.strength() >= 20);

        // and the list itself is bounded: drop the weakest untested
        // candidate first, so a hunch goes before a finding
        while self.preconditions.len() > MAX_PRECONDITIONS {
            let victim = self
                .preconditions
                .iter()
                .enumerate()
                .min_by_key(|(_, p)| (p.tests > 0, p.support, p.tests))
                .map(|(i, _)| i);
            match victim {
                Some(i) => {
                    self.preconditions.remove(i);
                }
                None => break,
            }
        }
    }

    /// This skill's exact form, for the graph to hold alongside its
    /// readable one.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut e = Encoder::new();
        self.encode(&mut e);
        e.finish_bounded()
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
            for p in self.preconditions.iter().filter(|p| p.is_well_supported()) {
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

    /// Roughly what the skill graph occupies in RAM, live.
    pub fn approximate_bytes(&self) -> usize {
        self.skills
            .iter()
            .map(|s| {
                core::mem::size_of::<Skill>()
                    + s.name.capacity()
                    + s.preconditions
                        .iter()
                        .map(|p| core::mem::size_of::<Precondition>() + p.tag.capacity())
                        .sum::<usize>()
                    + s.effects.iter().map(|e| e.capacity()).sum::<usize>()
                    + s.steps.capacity() * core::mem::size_of::<Step>()
                    + s.per_body.capacity() * core::mem::size_of::<BodyStats>()
            })
            .sum::<usize>()
            + core::mem::size_of::<SkillGraph>()
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
        let skill = Skill { id, ..skill };
        graph.set_skill_state(h, id, &skill.to_bytes());
        self.skills.push(skill);
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
        let state = skill.to_bytes();
        graph.revise(h, id, 60 + (skill.success_rate_pct() / 3));
        rewrite_node(h, graph, id, &text);
        // the readable form is what retrieval searches; the exact form is
        // what survives a reboot. Both, or the entity wakes up having
        // forgotten how to judge.
        graph.set_skill_state(h, id, &state);
        if let Some(ep) = episode {
            graph.link(h, ep, id, EdgeKind::About);
        }
        Some(id)
    }

    /// Take on what another entity worked out.
    ///
    /// This is the mechanism a skill marketplace needs and the one
    /// §2.4's "the fleet learns together" rests on. What is transferred
    /// is the *evidence* — how many times the seller attempted without
    /// each condition and how often that failed — not a verdict. The
    /// buyer's own attempts accumulate on top of it, so a claim that is
    /// wrong here gets contradicted here, by this entity, in this place.
    ///
    /// Note what that implies and what this crate deliberately does not
    /// decide: an entity that adopts a precondition learned in a
    /// different world starts out confidently wrong, and how long it
    /// takes to stop being wrong is a question for measurement rather
    /// than for a comment. `host::fleet` measures it.
    ///
    /// Only conditions the receiver does not already have evidence about
    /// are taken. Somebody else's numbers do not overwrite your own.
    pub fn adopt(
        &mut self,
        h: &MemoryHandle,
        graph: &mut StateGraph,
        verb: &Verb,
        taught: &[Precondition],
        at: Tick,
    ) -> usize {
        let Some(skill) = self.skills.iter_mut().find(|s| &s.verb == verb) else {
            return 0;
        };
        let mut taken = 0;
        for p in taught {
            if skill.preconditions.iter().any(|q| q.tag == p.tag) {
                continue;
            }
            if skill.preconditions.len() >= MAX_PRECONDITIONS {
                break;
            }
            skill.preconditions.push(Precondition {
                tag: p.tag.clone(),
                // What transfers is the *claim*, not the seller's
                // numbers. The counts here stay local from the start, so
                // "how many times has this entity found out" has one
                // meaning and a seller cannot inflate it. Until the
                // entity has its own five tests, the claim is held on
                // trust — acted on, and probed.
                adopted: true,
                support: 0,
                against: 0,
                tests: 0,
                test_failures: 0,
                since_probe: 0,
                // the clock is the receiver's: a claim adopted today is
                // as fresh as today, and goes stale on this entity's
                // schedule rather than the seller's
                last_tested: at,
            });
            taken += 1;
        }
        let (id, text, state) = (skill.id, skill.render(), skill.to_bytes());
        rewrite_node(h, graph, id, &text);
        graph.set_skill_state(h, id, &state);
        taken
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
        let mut fresh = Vec::new();
        if let Some(s) = self.skills.iter_mut().find(|s| s.id == id) {
            s.steps = steps;
            fresh = s.to_bytes();
        }
        graph.set_skill_state(h, id, &fresh);
        for c in child_ids {
            graph.link(h, id, c, EdgeKind::Uses);
        }
        id
    }

    /// The skills applicable in a context, best first.
    pub fn applicable_at(&self, context: &[&str], now: Tick) -> Vec<&Skill> {
        let mut out: Vec<&Skill> = self
            .skills
            .iter()
            .filter(|s| s.applicable_at(context, now))
            .collect();
        out.sort_by(|a, b| {
            b.success_rate_pct()
                .cmp(&a.success_rate_pct())
                .then(b.attempts.cmp(&a.attempts))
                .then(a.id.0.cmp(&b.id.0))
        });
        out
    }

    /// The same, ignoring staleness.
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
    ///
    /// A skill node carries its exact form as well as its readable one,
    /// and the exact form is what is used when it is there. Parsing the
    /// prose is the fallback, for a graph written before nodes could
    /// carry state — it recovers the verb, the name and the counts, and
    /// loses the preconditions and the per-body record, which is
    /// precisely the loss that made this necessary.
    pub fn rehydrate(&mut self, graph: &StateGraph) {
        self.skills.clear();
        for n in graph.of_kind(NodeKind::Skill) {
            if !n.state.is_empty() {
                if let Ok(s) = Skill::from_bytes(&n.state) {
                    // the node's id is authoritative: it is where this
                    // skill actually lives in this graph
                    self.skills.push(Skill { id: n.id, ..s });
                    continue;
                }
            }
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
