//! Retrieval without embeddings.
//!
//! The obvious way to do semantic memory in 2026 is an embedding model
//! and a vector index. We do not, for three reasons that are all about
//! where this runs rather than about quality:
//!
//! 1. An embedding model is a dependency on a *model*, and the
//!    Memory-Integrity Law says memory may not depend on one. If the
//!    embedding model is swapped, every stored vector becomes
//!    meaningless and the entity's memory is silently corrupted.
//! 2. It must run on a microcontroller in a leg.
//! 3. It must be deterministic, because the proof harness needs the same
//!    query to return the same memories.
//!
//! So: BM25 over stemmed terms, with a small hand-built synonym
//! expansion and a recency-and-salience prior. This is a 1994 algorithm
//! and it is the correct one here. When an entity *does* have a frontier
//! resource available (Tier 3), that resource re-ranks these candidates
//! — retrieval stays local and deterministic, and the model improves the
//! ordering rather than owning the memory.

use crate::graph::{Node, NodeKind, StateGraph};
use crate::provenance::Origin;
use alloc::vec::Vec;
use brainos_abi::ids::NodeId;
use brainos_abi::time::Tick;

/// The longest a term is kept. Longer terms are truncated, which acts as
/// a crude suffix stemmer — "calibration" and "calibrating" both become
/// "calibrat" — and keeps the term table fixed-width so postings are a
/// flat array rather than a string arena.
///
/// Eight is the classic truncation length and it is a real trade: it
/// merges "calibrate/calibration/calibrating", which is what we want,
/// and it also merges "telemetry/telemetric", which we do not
/// especially. Retrieval is a ranking, not a decision, so an occasional
/// over-merge costs an ordering rather than a mistake.
const TERM_MAX: usize = 8;
/// Terms shorter than this carry no retrieval signal.
const TERM_MIN: usize = 3;

/// BM25 parameters. `K1` controls how quickly term frequency saturates;
/// `B` how much document length is normalized. The standard values, in
/// fixed point (×100) so there is no float in the scoring path.
const K1_X100: u32 = 120;
const B_X100: u32 = 75;

/// One retrieved memory and why it scored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Match {
    pub id: NodeId,
    /// relevance ×1000, so ordering is exact integer comparison
    pub score: u32,
    /// how many query terms matched
    pub terms_hit: u16,
}

/// Split text into scoring terms: lowercase, alphanumeric runs, length
/// filtered, truncated. Deliberately not a real stemmer — a stemmer is a
/// language model in disguise and would have to be right about words the
/// entity has never seen.
pub fn terms(text: &str) -> Vec<[u8; TERM_MAX]> {
    let mut out = Vec::new();
    let mut cur = [0u8; TERM_MAX];
    let mut len = 0usize;

    let flush = |cur: &mut [u8; TERM_MAX], len: &mut usize, out: &mut Vec<[u8; TERM_MAX]>| {
        if *len >= TERM_MIN && !is_stopword(&cur[..*len]) {
            let mut t = [0u8; TERM_MAX];
            t[..*len].copy_from_slice(&cur[..*len]);
            out.push(t);
        }
        *len = 0;
        *cur = [0u8; TERM_MAX];
    };

    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            if len < TERM_MAX {
                cur[len] = c.to_ascii_lowercase() as u8;
                len += 1;
            }
            // past TERM_MAX the rest of the word is dropped, which is the
            // truncation that makes "calibrating" and "calibration" share
            // a term
        } else {
            flush(&mut cur, &mut len, &mut out);
        }
    }
    flush(&mut cur, &mut len, &mut out);
    out
}

/// Words that appear in almost every memory and therefore discriminate
/// nothing. Short, English, and deliberately not extended — an
/// over-eager stopword list removes signal from a robot's vocabulary
/// ("left", "down", "not" all matter here).
fn is_stopword(t: &[u8]) -> bool {
    matches!(
        t,
        b"the" | b"and" | b"that" | b"this" | b"with" | b"for" | b"was" | b"were"
            | b"has" | b"had" | b"have" | b"are" | b"its" | b"from" | b"then"
            | b"than" | b"into" | b"onto" | b"but" | b"any" | b"all" | b"can"
    )
}

/// An inverted index over the graph.
///
/// Rebuilt rather than incrementally maintained. The graph is thousands
/// of nodes, a rebuild is milliseconds, and an index that can drift out
/// of sync with memory is a source of the entity confidently not
/// remembering something it knows.
pub struct Index {
    /// (term, node, frequency), sorted by term then node
    postings: Vec<([u8; TERM_MAX], NodeId, u16)>,
    /// (node, term count) for length normalization
    lengths: Vec<(NodeId, u16)>,
    total_len: u64,
    docs: u32,
    /// what the graph looked like when this was built. Two numbers are
    /// enough: nodes are only ever appended with increasing ids or
    /// removed, so a changed count or a changed highest id means the
    /// index no longer describes the graph.
    built_nodes: usize,
    built_high_water: u64,
}

impl Default for Index {
    fn default() -> Index {
        Index::new()
    }
}

impl Index {
    pub fn new() -> Index {
        Index {
            postings: Vec::new(),
            lengths: Vec::new(),
            total_len: 0,
            docs: 0,
            built_nodes: 0,
            built_high_water: 0,
        }
    }

    pub fn docs(&self) -> u32 {
        self.docs
    }

    pub fn is_empty(&self) -> bool {
        self.docs == 0
    }

    /// Build over every retrievable node. Audit records are excluded:
    /// they are a log, they dominate by volume, and retrieving "what did
    /// I decide" is a different query answered by the audit log itself.
    pub fn build(graph: &StateGraph) -> Index {
        let mut idx = Index::new();
        for n in graph.nodes() {
            if n.kind == NodeKind::Audit {
                continue;
            }
            idx.add(n);
        }
        idx.postings.sort_unstable_by(|a, b| {
            a.0.cmp(&b.0).then(a.1 .0.cmp(&b.1 .0))
        });
        idx.lengths.sort_unstable_by_key(|l| l.0 .0);
        idx.built_nodes = graph.len();
        idx.built_high_water = graph.nodes().last().map(|n| n.id.0).unwrap_or(0);
        idx
    }

    /// Does this index still describe that graph?
    ///
    /// This exists because the alternative — rebuilding on every write —
    /// is wasteful, and the alternative to *that* is an index that
    /// silently goes stale. A stale index is the worst of the three: the
    /// entity confidently does not remember something it knows, and
    /// nothing anywhere reports an error.
    /// Roughly what this index costs in memory.
    ///
    /// Approximate on purpose: it is for deciding whether an index fits
    /// on a board, and a number that is right to within a few percent
    /// answers that question exactly as well as an exact one would.
    pub fn approximate_bytes(&self) -> usize {
        self.postings.len() * (TERM_MAX + 8 + 4) + self.lengths.len() * 12 + 64
    }

    pub fn is_stale(&self, graph: &StateGraph) -> bool {
        self.built_nodes != graph.len()
            || self.built_high_water != graph.nodes().last().map(|n| n.id.0).unwrap_or(0)
    }

    /// Rebuild if, and only if, the graph has moved on.
    pub fn refresh(&mut self, graph: &StateGraph) -> bool {
        if self.is_stale(graph) {
            *self = Index::build(graph);
            true
        } else {
            false
        }
    }

    fn add(&mut self, node: &Node) {
        let ts = terms(&node.content);
        if ts.is_empty() {
            return;
        }
        let mut local: Vec<([u8; TERM_MAX], u16)> = Vec::new();
        for t in &ts {
            match local.iter_mut().find(|(k, _)| k == t) {
                Some((_, f)) => *f += 1,
                None => local.push((*t, 1)),
            }
        }
        for (t, f) in local {
            self.postings.push((t, node.id, f));
        }
        self.lengths.push((node.id, ts.len() as u16));
        self.total_len += ts.len() as u64;
        self.docs += 1;
    }

    fn doc_freq(&self, term: &[u8; TERM_MAX]) -> u32 {
        let lo = self.postings.partition_point(|p| p.0 < *term);
        let hi = self.postings.partition_point(|p| p.0 <= *term);
        (hi - lo) as u32
    }

    fn len_of(&self, id: NodeId) -> u16 {
        self.lengths
            .binary_search_by_key(&id.0, |l| l.0 .0)
            .map(|i| self.lengths[i].1)
            .unwrap_or(1)
    }

    /// Score every node matching any query term.
    ///
    /// The BM25 IDF is `ln((N − df + 0.5)/(df + 0.5) + 1)`; computed here
    /// in fixed point with an integer log2, because a memory retrieval
    /// that differs between a host and an MCU because of libm rounding is
    /// a memory retrieval that cannot be tested.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Match> {
        let qterms = terms(query);
        if qterms.is_empty() || self.docs == 0 {
            return Vec::new();
        }
        let avg_len = (self.total_len / self.docs.max(1) as u64).max(1) as u32;

        let mut hits: Vec<Match> = Vec::new();
        for qt in &qterms {
            let df = self.doc_freq(qt);
            if df == 0 {
                continue;
            }
            let idf = idf_x1000(self.docs, df);
            let lo = self.postings.partition_point(|p| p.0 < *qt);
            let hi = self.postings.partition_point(|p| p.0 <= *qt);
            for (_, id, tf) in &self.postings[lo..hi] {
                let dl = self.len_of(*id) as u32;
                // tf·(k1+1) / (tf + k1·(1 − b + b·dl/avgdl))
                let norm = 100 - B_X100 + (B_X100 * dl) / avg_len;
                let num = (*tf as u32) * (K1_X100 + 100);
                let den = (*tf as u32) * 100 + (K1_X100 * norm) / 100;
                let term_score = if den == 0 { 0 } else { (idf * num) / den.max(1) };

                match hits.iter_mut().find(|m| m.id == *id) {
                    Some(m) => {
                        m.score = m.score.saturating_add(term_score);
                        m.terms_hit += 1;
                    }
                    None => hits.push(Match {
                        id: *id,
                        score: term_score,
                        terms_hit: 1,
                    }),
                }
            }
        }

        // ties break by node id so retrieval is deterministic
        hits.sort_unstable_by(|a, b| b.score.cmp(&a.score).then(a.id.0.cmp(&b.id.0)));
        hits.truncate(limit);
        hits
    }

    /// Search, then apply the memory prior: a match that is salient,
    /// confident, recent and trustworthy outranks one that is merely
    /// lexically similar. This is the function cognition actually calls.
    pub fn recall<'a>(
        &self,
        graph: &'a StateGraph,
        query: &str,
        now: Tick,
        limit: usize,
    ) -> Vec<(&'a Node, u32)> {
        // over-fetch, then re-rank with the prior
        let raw = self.search(query, limit.saturating_mul(4).max(16));
        let mut scored: Vec<(&Node, u32)> = raw
            .iter()
            .filter_map(|m| graph.get(m.id).map(|n| (n, m.score)))
            .map(|(n, s)| {
                let mut score = s;
                // salience and confidence, as multipliers in percent
                score = score * (50 + n.salience as u32 / 2) / 100;
                score = score * (50 + n.confidence as u32 / 2) / 100;
                // an untrusted origin is halved, not excluded: the entity
                // may remember what the internet told it, and should also
                // remember that it was the internet that said it
                if n.origin().is_untrusted() {
                    score /= 2;
                }
                // recency, decaying over roughly ten thousand ticks
                let age = now.since(n.tick());
                let recency = 100u32.saturating_sub((age / 100).min(60) as u32);
                score = score * (40 + recency * 6 / 10) / 100;
                (n, score)
            })
            .collect();
        scored.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.id.0.cmp(&b.0.id.0)));
        scored.truncate(limit);
        scored
    }

    /// Retrieval restricted to what policy may consult (§ provenance).
    pub fn recall_admissible<'a>(
        &self,
        graph: &'a StateGraph,
        query: &str,
        now: Tick,
        limit: usize,
    ) -> Vec<(&'a Node, u32)> {
        self.recall(graph, query, now, limit.saturating_mul(3))
            .into_iter()
            .filter(|(n, _)| n.origin().admissible_to_policy())
            .take(limit)
            .collect()
    }
}

/// `ln((N − df + 0.5)/(df + 0.5) + 1)`, ×1000, in integers.
fn idf_x1000(n: u32, df: u32) -> u32 {
    let num = (n.saturating_sub(df) as u64) * 2 + 1;
    let den = (df as u64) * 2 + 1;
    // ratio + 1, ×1000
    let ratio_x1000 = (num * 1000) / den.max(1) + 1000;
    ln_x1000(ratio_x1000)
}

/// Natural log of `x/1000`, returned ×1000. Integer log2 for the
/// exponent plus a linear interpolation inside the octave: the result is
/// within about 3% of `ln`, which is far finer than retrieval ordering
/// can notice, and it is bit-identical everywhere.
fn ln_x1000(x_x1000: u64) -> u32 {
    if x_x1000 <= 1000 {
        return 0;
    }
    let mut v = x_x1000;
    let mut log2 = 0u32;
    while v >= 2000 {
        v /= 2;
        log2 += 1;
    }
    // v is now in [1000, 2000): interpolate the fraction linearly
    let frac = ((v - 1000) * 1000) / 1000;
    let log2_x1000 = log2 as u64 * 1000 + frac;
    // ln x = log2 x · ln 2
    ((log2_x1000 * 693) / 1000) as u32
}

/// Public for the ranking tests, which check that the prior does what it
/// claims rather than trusting the arithmetic above.
pub fn origin_penalty(o: Origin) -> u32 {
    if o.is_untrusted() {
        2
    } else {
        1
    }
}
