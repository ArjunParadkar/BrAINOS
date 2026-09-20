//! Consolidation — "dreaming", and the engine of the moat.
//!
//! An entity that only accumulates episodes has a longer history, not a
//! better mind. Consolidation is the offline pass that turns experience
//! into competence, and it does four things, in this order:
//!
//! 1. **Crystallize.** Find claims that recur across episodes and write
//!    them as semantic memory, linked back to the episodes that support
//!    them. A pattern seen three times becomes a note; a note reinforced
//!    repeatedly rises in confidence.
//!
//! 2. **Promote.** Find verbs that have been attempted enough times to
//!    have a rate, and promote them into the skill graph — with the
//!    preconditions learned by intersecting the context of the successes.
//!
//! 3. **Chain.** Find sequences of verbs that repeatedly occurred
//!    together and succeeded, and compose them into a single skill. This
//!    is how "walk to the door" stops being three decisions.
//!
//! 4. **Decay and prune.** Everything prunable loses salience; what was
//!    never retrieved and never reinforced falls out. Forgetting is not
//!    a failure mode here, it is the other half of remembering — without
//!    it the graph is a log and retrieval degrades toward noise.
//!
//! Consolidation runs when the entity is idle and has no body in motion,
//! because it rewrites memory and a rewrite racing a gait loop is the
//! kind of thing that is fine in testing and catastrophic once.
//!
//! **What it cannot do.** It finds correlation. An episode set where
//! every success happened to be on carpet produces "carpet" as a
//! precondition, and the entity has no way to know whether the carpet
//! mattered. This is stated in `Precondition::is_binding`'s threshold and
//! it is why the published metric is the success curve rather than a
//! claim about understanding.

use crate::episode::{Episode, Outcome};
use crate::graph::{EdgeKind, NodeKind, StateGraph};
use crate::law::MemoryHandle;
use crate::provenance::{Origin, Provenance};
use crate::skill::{SkillGraph, Step};
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::ids::NodeId;
use brainos_abi::time::Tick;
use brainos_abi::verb::Verb;

/// How many times a pattern must recur before it is worth remembering
/// as knowledge rather than as three separate events.
pub const CRYSTALLIZE_AT: usize = 3;
/// How many attempts before a verb is worth promoting to a skill.
pub const PROMOTE_AT: u32 = 3;
/// How many times a sequence must recur before it is chained.
pub const CHAIN_AT: usize = 3;
/// Longest chain considered. Beyond this the combinatorics stop paying.
pub const MAX_CHAIN: usize = 4;

/// What one consolidation pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConsolidationReport {
    pub episodes_read: usize,
    pub semantic_written: usize,
    pub semantic_reinforced: usize,
    pub skills_learned: usize,
    pub skills_updated: usize,
    pub chains_composed: usize,
    pub forgotten: usize,
    /// competence before and after, so the pass's effect is measurable
    pub competence_before: u8,
    pub competence_after: u8,
}

impl ConsolidationReport {
    pub fn did_anything(&self) -> bool {
        self.semantic_written > 0
            || self.skills_learned > 0
            || self.chains_composed > 0
            || self.forgotten > 0
    }
}

/// Parameters for a pass.
#[derive(Clone, Copy, Debug)]
pub struct Dream {
    /// how much salience everything prunable loses
    pub decay: u8,
    /// nodes below this salience are dropped
    pub forget_below: u8,
    /// only consider episodes from this mark onward; `0` means all
    pub from_mark: usize,
    /// whether to compose chains, which is the most speculative step
    pub chain: bool,
}

impl Default for Dream {
    fn default() -> Dream {
        Dream {
            decay: 6,
            forget_below: 12,
            from_mark: 0,
            chain: true,
        }
    }
}

impl Dream {
    /// A light pass: decay and crystallize, no chaining. What runs
    /// between tasks.
    pub fn light() -> Dream {
        Dream {
            decay: 2,
            forget_below: 8,
            from_mark: 0,
            chain: false,
        }
    }

    /// A deep pass. What runs when the entity has been idle a while.
    pub fn deep() -> Dream {
        Dream::default()
    }
}

/// Run one consolidation pass.
pub fn consolidate(
    h: &MemoryHandle,
    graph: &mut StateGraph,
    skills: &mut SkillGraph,
    dream: Dream,
    now: Tick,
) -> ConsolidationReport {
    let mut report = ConsolidationReport {
        competence_before: skills.competence_pct(),
        ..Default::default()
    };

    // --- read the episodes in scope ---
    let episodes: Vec<(NodeId, Episode)> = graph
        .since(dream.from_mark)
        .iter()
        .filter(|n| n.kind == NodeKind::Episode)
        .filter_map(|n| Episode::parse(&n.content, n.tick()).map(|e| (n.id, e)))
        .collect();
    report.episodes_read = episodes.len();

    crystallize(h, graph, &episodes, now, &mut report);
    promote(h, graph, skills, &episodes, now, &mut report);
    if dream.chain {
        chain(h, graph, skills, &episodes, now, &mut report);
    }
    forget(h, graph, dream, &mut report);

    report.competence_after = skills.competence_pct();
    report
}

/// Step 1: repetition becomes knowledge.
///
/// The pattern key is the verb plus the outcome plus the context. Three
/// episodes that share all three are saying the same thing three times,
/// and the thing they are saying is worth one semantic note.
fn crystallize(
    h: &MemoryHandle,
    graph: &mut StateGraph,
    episodes: &[(NodeId, Episode)],
    now: Tick,
    report: &mut ConsolidationReport,
) {
    let mut groups: Vec<(String, Vec<NodeId>)> = Vec::new();
    for (id, e) in episodes {
        let key = pattern_key(e);
        if key.is_empty() {
            continue;
        }
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, ids)) => ids.push(*id),
            None => groups.push((key, alloc::vec![*id])),
        }
    }

    for (key, ids) in groups {
        if ids.len() < CRYSTALLIZE_AT {
            continue;
        }
        // The note's text must not mention how many times the pattern was
        // seen. It is tempting — "(seen 6x)" reads well — and it is a bug:
        // the text is the identity of the note, so a changing count makes
        // every pass write a *new* note instead of strengthening the old
        // one, and the entity ends up wordier rather than surer. The count
        // lives in confidence and in `Provenance::reinforced`, where it
        // belongs.
        let note = render_note(&key);
        // if the note already exists, this is reinforcement rather than
        // a second copy — an entity that learns the same lesson twice
        // should be more sure, not more verbose
        let existing = graph
            .of_kind(NodeKind::Semantic)
            .find(|n| n.content == note)
            .map(|n| n.id);
        match existing {
            Some(id) => {
                graph.reinforce(h, id);
                let before = graph.get(id).map(|n| n.confidence).unwrap_or(0);
                graph.revise(h, id, before.saturating_add(5).min(90));
                report.semantic_reinforced += 1;
                for ep in &ids {
                    graph.link(h, id, *ep, EdgeKind::DerivedFrom);
                }
            }
            None => {
                let prov = Provenance::from_world(Origin::Consolidation, now);
                // confidence starts at the count, capped: three
                // observations is a hypothesis, thirty is knowledge
                let conf = (40 + ids.len().min(10) as u8 * 5).min(90);
                let id = graph.add(h, NodeKind::Semantic, &note, conf, prov);
                for ep in &ids {
                    graph.link(h, id, *ep, EdgeKind::DerivedFrom);
                }
                report.semantic_written += 1;
            }
        }
    }
}

/// The key that makes two episodes "the same thing happening again".
fn pattern_key(e: &Episode) -> String {
    let Some(v) = &e.verb else {
        return String::new();
    };
    let mut k = String::new();
    k.push_str(v.as_str());
    k.push('|');
    k.push_str(e.outcome.name());
    for c in &e.context {
        k.push('|');
        k.push_str(c);
    }
    k
}

fn render_note(key: &str) -> String {
    let mut parts = key.split('|');
    let verb = parts.next().unwrap_or("something");
    let outcome = parts.next().unwrap_or("happened");
    let ctx: Vec<&str> = parts.collect();

    let mut s = String::new();
    s.push_str(verb);
    s.push(' ');
    s.push_str(match outcome {
        "succeeded" => "works",
        "failed" => "does not work",
        "stopped" => "gets stopped",
        "denied" => "is refused",
        "partial" => "half works",
        _ => "recurs",
    });
    if !ctx.is_empty() {
        s.push_str(" when ");
        for (i, c) in ctx.iter().enumerate() {
            if i > 0 {
                s.push_str(" and ");
            }
            s.push_str(c);
        }
    }
    s
}

/// Step 2: attempts become skills.
fn promote(
    h: &MemoryHandle,
    graph: &mut StateGraph,
    skills: &mut SkillGraph,
    episodes: &[(NodeId, Episode)],
    now: Tick,
    report: &mut ConsolidationReport,
) {
    // group by verb
    let mut verbs: Vec<Verb> = Vec::new();
    for (_, e) in episodes {
        if let Some(v) = &e.verb {
            if e.outcome.counts_toward_skill() && !verbs.contains(v) {
                verbs.push(*v);
            }
        }
    }

    for verb in verbs {
        let attempts: Vec<&(NodeId, Episode)> = episodes
            .iter()
            .filter(|(_, e)| e.verb.as_ref() == Some(&verb) && e.outcome.counts_toward_skill())
            .collect();
        if (attempts.len() as u32) < PROMOTE_AT {
            continue;
        }

        let existed = skills.for_verb(&verb).is_some();
        let prov = Provenance::from_world(Origin::Consolidation, now);
        skills.learn(h, graph, verb, verb.action(), prov);
        if existed {
            report.skills_updated += 1;
        } else {
            report.skills_learned += 1;
        }

        // Fold every attempt in. Consolidation is the *only* place a
        // skill's statistics are updated from history, so re-running it
        // over the same episodes would double-count — which is why the
        // entity advances `from_mark` after a pass.
        for (id, e) in attempts {
            skills.observe(
                h,
                graph,
                &verb,
                e.outcome.is_success(),
                e.outcome == Outcome::Stopped,
                e.body,
                &e.context,
                e.at,
                Some(*id),
            );
        }
    }
}

/// Step 3: sequences become one skill.
///
/// Walk the episode list in order; every window of 2..=MAX_CHAIN
/// consecutive successful attempts is a candidate sequence. A candidate
/// seen `CHAIN_AT` times becomes a composed skill whose verb is
/// `chain.<first>_<last>`.
fn chain(
    h: &MemoryHandle,
    graph: &mut StateGraph,
    skills: &mut SkillGraph,
    episodes: &[(NodeId, Episode)],
    now: Tick,
    report: &mut ConsolidationReport,
) {
    let successes: Vec<&Episode> = episodes
        .iter()
        .map(|(_, e)| e)
        .filter(|e| e.verb.is_some() && e.outcome.is_success())
        .collect();

    let mut candidates: Vec<(Vec<Verb>, usize)> = Vec::new();
    for width in 2..=MAX_CHAIN {
        if successes.len() < width {
            break;
        }
        for w in successes.windows(width) {
            let seq: Vec<Verb> = w.iter().filter_map(|e| e.verb).collect();
            if seq.len() != width {
                continue;
            }
            // a sequence of one verb repeated is not a chain, it is a loop
            if seq.iter().all(|v| *v == seq[0]) {
                continue;
            }
            match candidates.iter_mut().find(|(s, _)| *s == seq) {
                Some((_, n)) => *n += 1,
                None => candidates.push((seq, 1)),
            }
        }
    }

    // longest first: a 4-chain subsumes the 2-chains inside it, and
    // composing the long one first means the short ones are already
    // represented
    candidates.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(b.1.cmp(&a.1)));

    for (seq, count) in candidates {
        if count < CHAIN_AT {
            continue;
        }
        let Some(name) = chain_verb(&seq) else {
            continue;
        };
        if skills.for_verb(&name).is_some() {
            continue;
        }
        // every step must itself be a known skill, or the chain is a
        // description of luck rather than a composition of competence
        if !seq.iter().all(|v| skills.for_verb(v).is_some()) {
            continue;
        }
        let steps: Vec<Step> = seq
            .iter()
            .map(|v| Step {
                verb: *v,
                argument: String::new(),
            })
            .collect();
        let prov = Provenance::from_world(Origin::Consolidation, now);
        skills.compose(h, graph, name, name.action(), steps, prov);
        report.chains_composed += 1;
    }
}

/// Build the verb for a composed chain. Bounded by the verb grammar, so
/// a long chain is named by its ends rather than by every step.
fn chain_verb(seq: &[Verb]) -> Option<Verb> {
    let first = seq.first()?;
    let last = seq.last()?;
    let mut s = String::from("chain.");
    for c in first.action().chars().take(9) {
        s.push(c);
    }
    s.push('_');
    for c in last.action().chars().take(9) {
        s.push(c);
    }
    Verb::parse(&s).ok()
}

/// Step 4: decay, then drop what fell through the floor.
///
/// **Semantic memory is exempt.** Episodes are a ring that must be
/// forgotten — that pressure is what makes consolidation necessary in
/// the first place. Semantic notes are the *result* of that forgetting,
/// and decaying them on the same schedule undoes the work: the note
/// fades, the episodes it was protecting lose their protector, and the
/// entity ends a long quiet period knowing less than when it started.
/// Semantic memory leaves only two ways — evicted by `Bounds` when the
/// cap is reached (least salient first), or abandoned when contradicted.
fn forget(
    h: &MemoryHandle,
    graph: &mut StateGraph,
    dream: Dream,
    report: &mut ConsolidationReport,
) {
    graph.decay(h, dream.decay);

    let doomed: Vec<NodeId> = graph
        .nodes()
        .iter()
        .filter(|n| n.kind != NodeKind::Semantic)
        .filter(|n| n.kind.prunable() && n.salience < dream.forget_below)
        // a node that something else was derived from is load-bearing:
        // forgetting it would leave a semantic note whose explanation is
        // a hole, and an explanation with a hole is worse than none
        .filter(|n| {
            graph.backlinks(n.id, EdgeKind::DerivedFrom).is_empty()
                && graph.backlinks(n.id, EdgeKind::Supports).is_empty()
        })
        .map(|n| n.id)
        .collect();

    for id in doomed {
        if graph.forget(h, id) {
            report.forgotten += 1;
        }
    }
}
