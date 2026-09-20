//! The state graph — what replaces the filesystem.
//!
//! A UNIX file is a byte stream with a name and a modification time. A
//! state node is a typed memory with an origin, a confidence, a tick, and
//! edges to the memories that explain it. The difference is not
//! cosmetic: "why do I believe the left middle servo binds at 30°" is a
//! graph traversal here and an impossibility in a filesystem.
//!
//! **Node identity is stable and never reused.** A node that is pruned
//! leaves a hole; ids keep counting. Reusing an id would mean an edge
//! from an old memory silently pointing at a new one, which is the
//! memory equivalent of a use-after-free and would corrupt the entity's
//! account of its own past.
//!
//! **Nothing in this module can be mutated without a `MemoryHandle`.**
//! The read side is freely available — a cognitive resource may look at
//! memory all it likes. The write side takes `&MemoryHandle` on every
//! method, and a handle exists only inside the entity.

use crate::law::MemoryHandle;
use crate::provenance::{Origin, Provenance};
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::codec::{Decode, Decoder, Encode, Encoder};
use brainos_abi::error::{Error, Result};
use brainos_abi::ids::NodeId;
use brainos_abi::time::Tick;

/// What kind of memory a node holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum NodeKind {
    /// episodic: something that happened, at a time, with an outcome
    Episode = 0,
    /// semantic: something generally true, compressed from episodes
    Semantic = 1,
    /// the world model: something held true with a confidence
    Belief = 2,
    /// procedural: a learned action pathway (the skill graph's nodes)
    Skill = 3,
    /// what a body's calibration turned out to be
    Calibration = 4,
    /// a KIRA decision. Immutable once written; never pruned.
    Audit = 5,
    /// a body this entity is or was bound to
    Body = 6,
    /// a goal at any level of the hierarchy
    Goal = 7,
    /// a person, place or thing the entity has come to know
    Entity_ = 8,
}

impl NodeKind {
    pub const ALL: [NodeKind; 9] = [
        NodeKind::Episode,
        NodeKind::Semantic,
        NodeKind::Belief,
        NodeKind::Skill,
        NodeKind::Calibration,
        NodeKind::Audit,
        NodeKind::Body,
        NodeKind::Goal,
        NodeKind::Entity_,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            NodeKind::Episode => "episode",
            NodeKind::Semantic => "semantic",
            NodeKind::Belief => "belief",
            NodeKind::Skill => "skill",
            NodeKind::Calibration => "calibration",
            NodeKind::Audit => "audit",
            NodeKind::Body => "body",
            NodeKind::Goal => "goal",
            NodeKind::Entity_ => "known",
        }
    }

    /// May pruning remove this kind at all? Audit records are the log of
    /// what the gate decided and are never dropped by the entity itself;
    /// calibration is what a body needs to be usable and is dropped only
    /// when the body is released.
    pub const fn prunable(self) -> bool {
        matches!(
            self,
            NodeKind::Episode | NodeKind::Semantic | NodeKind::Belief | NodeKind::Entity_
        )
    }

    /// Does this kind travel with the entity when it moves bodies?
    /// Everything except audit, which belongs to the machine of record.
    pub const fn durable(self) -> bool {
        !matches!(self, NodeKind::Audit)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum EdgeKind {
    /// temporal succession between episodes
    Follows = 0,
    /// this memory is about that thing
    About = 1,
    /// this caused that
    Caused = 2,
    /// this is evidence for that belief
    Supports = 3,
    /// this is evidence against that belief
    Contradicts = 4,
    /// this was compressed from that
    DerivedFrom = 5,
    /// this skill requires that condition
    Requires = 6,
    /// this skill produces that effect
    Produces = 7,
    /// this skill is composed of that one
    Uses = 8,
    /// this happened on that body
    OnBody = 9,
}

impl EdgeKind {
    pub const ALL: [EdgeKind; 10] = [
        EdgeKind::Follows,
        EdgeKind::About,
        EdgeKind::Caused,
        EdgeKind::Supports,
        EdgeKind::Contradicts,
        EdgeKind::DerivedFrom,
        EdgeKind::Requires,
        EdgeKind::Produces,
        EdgeKind::Uses,
        EdgeKind::OnBody,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            EdgeKind::Follows => "follows",
            EdgeKind::About => "about",
            EdgeKind::Caused => "caused",
            EdgeKind::Supports => "supports",
            EdgeKind::Contradicts => "contradicts",
            EdgeKind::DerivedFrom => "derived-from",
            EdgeKind::Requires => "requires",
            EdgeKind::Produces => "produces",
            EdgeKind::Uses => "uses",
            EdgeKind::OnBody => "on-body",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Edge {
    pub to: NodeId,
    pub kind: EdgeKind,
}

impl Encode for Edge {
    fn encode(&self, e: &mut Encoder) {
        self.to.encode(e);
        e.u8(self.kind as u8);
    }
}

impl Decode for Edge {
    fn decode(d: &mut Decoder) -> Result<Edge> {
        let to = NodeId::decode(d)?;
        let k = d.u8()?;
        Ok(Edge {
            to,
            kind: EdgeKind::ALL
                .get(k as usize)
                .copied()
                .ok_or(Error::Malformed("unknown edge kind"))?,
        })
    }
}

/// One memory.
#[derive(Clone, Debug)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub content: String,
    /// 0–100, capped by the provenance's origin
    pub confidence: u8,
    /// how much this memory currently matters. Decays; reinforced by
    /// retrieval and by being useful. Pruning takes the lowest first.
    pub salience: u8,
    pub provenance: Provenance,
    pub edges: Vec<Edge>,
    /// how many times this node has been retrieved. Retrieval is
    /// evidence of usefulness, and usefulness is what should survive.
    pub retrievals: u32,
    /// The structured form of this memory, for the kinds that have one.
    ///
    /// `content` is what the entity can *read back* — it is prose, it is
    /// what lexical retrieval searches, and it is lossy on purpose. That
    /// was fine until it turned out to be the only durable form a skill
    /// had: an entity would learn across sixty attempts that walking
    /// needs carpet, sleep, and wake up knowing its success rate and
    /// nothing else. The accumulated judgement — which is the whole of
    /// §2.4's moat — evaporated at every reboot.
    ///
    /// So a node may carry its exact form alongside its readable one.
    /// Empty for everything that does not need it, which is most
    /// memories.
    pub state: Vec<u8>,
}

impl Node {
    pub fn tick(&self) -> Tick {
        self.provenance.at
    }

    pub fn origin(&self) -> Origin {
        self.provenance.origin
    }

    pub fn links_to(&self, target: NodeId, kind: EdgeKind) -> bool {
        self.edges.iter().any(|e| e.to == target && e.kind == kind)
    }

    pub fn neighbours(&self, kind: EdgeKind) -> impl Iterator<Item = NodeId> + '_ {
        self.edges
            .iter()
            .filter(move |e| e.kind == kind)
            .map(|e| e.to)
    }
}

impl Encode for Node {
    fn encode(&self, e: &mut Encoder) {
        self.id.encode(e);
        e.u8(self.kind as u8);
        e.str(&self.content);
        e.u8(self.confidence);
        e.u8(self.salience);
        self.provenance.encode(e);
        e.seq(self.edges.len());
        for edge in &self.edges {
            edge.encode(e);
        }
        e.u32(self.retrievals);
        e.bytes(&self.state);
    }
}

impl Decode for Node {
    fn decode(d: &mut Decoder) -> Result<Node> {
        let id = NodeId::decode(d)?;
        let k = d.u8()?;
        let kind = NodeKind::ALL
            .get(k as usize)
            .copied()
            .ok_or(Error::Malformed("unknown node kind"))?;
        let content = alloc::string::ToString::to_string(d.str()?);
        let confidence = d.u8()?;
        let salience = d.u8()?;
        let provenance = Provenance::decode(d)?;
        let n = d.seq()?;
        let mut edges = Vec::new();
        for _ in 0..n {
            edges.push(Edge::decode(d)?);
        }
        Ok(Node {
            id,
            kind,
            content,
            confidence,
            salience,
            provenance,
            edges,
            retrievals: d.u32()?,
            state: d.bytes()?.to_vec(),
        })
    }
}

/// Bounds on the graph. A body has finite storage and an entity lives a
/// long time; without a ceiling the state graph is an unbounded write.
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub max_episodes: usize,
    pub max_semantic: usize,
    pub max_beliefs: usize,
    pub max_audit: usize,
    pub max_content: usize,
    /// the cap on a node's structured form. A node whose state would
    /// exceed it keeps no state at all rather than a truncated one: half
    /// a serialization is not a smaller memory, it is a corrupt one.
    pub max_state: usize,
}

impl Default for Bounds {
    fn default() -> Bounds {
        Bounds {
            // a few thousand episodes is days of continuous operation;
            // consolidation is what keeps the useful part of them
            max_episodes: 4_096,
            max_semantic: 2_048,
            max_beliefs: 1_024,
            // the audit log's real home is the Merkle log in the entity;
            // this is the working window kept in the graph
            max_audit: 8_192,
            max_content: 1_024,
            max_state: 4_096,
        }
    }
}

impl Bounds {
    /// Small enough to live on a microcontroller alongside a gait loop.
    pub const EMBEDDED: Bounds = Bounds {
        max_episodes: 256,
        max_semantic: 128,
        max_beliefs: 64,
        max_audit: 256,
        max_content: 256,
        max_state: 1_024,
    };

    pub fn cap_for(&self, kind: NodeKind) -> Option<usize> {
        match kind {
            NodeKind::Episode => Some(self.max_episodes),
            NodeKind::Semantic => Some(self.max_semantic),
            NodeKind::Belief => Some(self.max_beliefs),
            NodeKind::Audit => Some(self.max_audit),
            _ => None,
        }
    }
}

/// The entity's memory.
pub struct StateGraph {
    nodes: Vec<Node>,
    next_id: u64,
    bounds: Bounds,
    /// episodes inherited from a previous embodiment of this key — the
    /// entity knows which of its memories predate this body
    pub inherited: usize,
    /// node count at wake; everything after is this session's material
    pub session_mark: usize,
    /// nodes pruned over this graph's life, for honest telemetry about
    /// what the entity has forgotten
    pub pruned: u64,
}

impl Default for StateGraph {
    fn default() -> StateGraph {
        StateGraph::new()
    }
}

impl StateGraph {
    pub fn new() -> StateGraph {
        StateGraph::with_bounds(Bounds::default())
    }

    pub fn with_bounds(bounds: Bounds) -> StateGraph {
        StateGraph {
            nodes: Vec::new(),
            next_id: 1,
            bounds,
            inherited: 0,
            session_mark: 0,
            pruned: 0,
        }
    }

    pub fn bounds(&self) -> Bounds {
        self.bounds
    }

    // ---- reading: open to everything ----

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Roughly what this graph occupies in RAM, live — not what it
    /// encodes to.
    ///
    /// The difference matters and was being ignored. The encoded form is
    /// what goes in the journal; the live form is what has to fit in an
    /// ESP32-S3's SRAM, and it is larger, because every `String` and
    /// every `Vec` carries a capacity and a header of its own. A board
    /// budget built from the encoded number is a board budget that is
    /// wrong in the optimistic direction.
    ///
    /// Approximate on purpose: it is for deciding whether something
    /// fits, and being right to a few percent answers that as well as
    /// being exact would.
    pub fn approximate_bytes(&self) -> usize {
        let per_node = core::mem::size_of::<Node>();
        self.nodes
            .iter()
            .map(|n| {
                per_node
                    + n.content.capacity()
                    + n.state.capacity()
                    + n.edges.capacity() * core::mem::size_of::<Edge>()
            })
            .sum::<usize>()
            + core::mem::size_of::<StateGraph>()
    }

    /// Change what this graph may hold.
    ///
    /// Takes effect on the next write rather than pruning immediately:
    /// shrinking a graph is a decision about which memories to lose, and
    /// the decay-and-forget pass is where that decision is made, with
    /// salience in front of it.
    pub fn set_bounds(&mut self, bounds: Bounds) {
        self.bounds = bounds;
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        // ids are assigned in order and never reused, so a binary search
        // is correct even after pruning leaves holes
        self.nodes
            .binary_search_by_key(&id.0, |n| n.id.0)
            .ok()
            .map(|i| &self.nodes[i])
    }

    pub fn count(&self, kind: NodeKind) -> usize {
        self.nodes.iter().filter(|n| n.kind == kind).count()
    }

    pub fn of_kind(&self, kind: NodeKind) -> impl Iterator<Item = &Node> {
        self.nodes.iter().filter(move |n| n.kind == kind)
    }

    pub fn last(&self, kind: NodeKind) -> Option<&Node> {
        self.nodes.iter().rev().find(|n| n.kind == kind)
    }

    /// Everything added since `mark` — raw material for consolidation.
    pub fn since(&self, mark: usize) -> &[Node] {
        let m = mark.min(self.nodes.len());
        &self.nodes[m..]
    }

    /// Follow edges of one kind out of a node.
    pub fn follow(&self, from: NodeId, kind: EdgeKind) -> Vec<&Node> {
        match self.get(from) {
            None => Vec::new(),
            Some(n) => n
                .neighbours(kind)
                .filter_map(|id| self.get(id))
                .collect(),
        }
    }

    /// Everything pointing *at* a node with an edge of one kind. Edges
    /// are stored on the source, so this is a scan — acceptable because
    /// it is used in explanation ("what supports this belief") rather
    /// than in any hot path.
    pub fn backlinks(&self, to: NodeId, kind: EdgeKind) -> Vec<&Node> {
        self.nodes
            .iter()
            .filter(|n| n.links_to(to, kind))
            .collect()
    }

    /// Why does the entity hold this? Walks `Supports`/`DerivedFrom`
    /// backwards to the observations underneath. This is the function
    /// that makes the graph worth having instead of a log.
    pub fn explain(&self, id: NodeId, depth: usize) -> Vec<&Node> {
        let mut out = Vec::new();
        let mut frontier = alloc::vec![id];
        for _ in 0..depth {
            let mut next = Vec::new();
            for f in frontier {
                if let Some(n) = self.get(f) {
                    for e in &n.edges {
                        if matches!(e.kind, EdgeKind::DerivedFrom | EdgeKind::Supports | EdgeKind::Caused) {
                            if let Some(src) = self.get(e.to) {
                                if !out.iter().any(|o: &&Node| o.id == src.id) {
                                    out.push(src);
                                    next.push(src.id);
                                }
                            }
                        }
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        out
    }

    // ---- writing: requires a MemoryHandle ----

    /// Add a node. The confidence is capped by the provenance's origin,
    /// so a caller cannot smuggle a high-confidence belief in from an
    /// untrusted world by simply passing 100.
    pub fn add(
        &mut self,
        _h: &MemoryHandle,
        kind: NodeKind,
        content: &str,
        confidence: u8,
        provenance: Provenance,
    ) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;

        let mut content = alloc::string::ToString::to_string(content);
        if content.len() > self.bounds.max_content {
            let mut cut = self.bounds.max_content;
            while cut > 0 && !content.is_char_boundary(cut) {
                cut -= 1;
            }
            content.truncate(cut);
        }

        self.nodes.push(Node {
            id,
            kind,
            content,
            confidence: provenance.cap(confidence),
            // a new memory starts salient; the decay pass is what sorts
            // the important from the merely recent
            salience: 80,
            provenance,
            edges: Vec::new(),
            retrievals: 0,
            state: Vec::new(),
        });

        self.enforce_bounds(kind);
        id
    }

    pub fn link(&mut self, _h: &MemoryHandle, from: NodeId, to: NodeId, kind: EdgeKind) -> bool {
        // never create a dangling edge: an edge to a pruned node would
        // make `explain` report a hole as an explanation
        if self.get(to).is_none() {
            return false;
        }
        match self.nodes.binary_search_by_key(&from.0, |n| n.id.0) {
            Err(_) => false,
            Ok(i) => {
                if self.nodes[i].links_to(to, kind) {
                    return true; // idempotent
                }
                self.nodes[i].edges.push(Edge { to, kind });
                true
            }
        }
    }

    /// Record that a node was useful. Retrieval is the signal that keeps
    /// a memory alive through decay.
    pub fn touch(&mut self, _h: &MemoryHandle, id: NodeId) {
        if let Ok(i) = self.nodes.binary_search_by_key(&id.0, |n| n.id.0) {
            let n = &mut self.nodes[i];
            n.retrievals = n.retrievals.saturating_add(1);
            n.salience = n.salience.saturating_add(4).min(100);
        }
    }

    /// Adjust a belief's confidence, respecting the origin ceiling.
    /// Set how much a memory matters.
    ///
    /// Salience decides what survives pruning, and until now it was set
    /// once at 80 for everything and then only decayed. That made the
    /// forgetting order essentially "oldest first", which is not what
    /// the architecture says and not what anyone would want: a routine
    /// walk and a leg stall are not equally worth keeping. The ambient
    /// loop already computes how much something matters in order to
    /// decide whether to interrupt a human; this is where that same
    /// judgement reaches memory, so attention and pruning are one
    /// judgement rather than two that can disagree.
    pub fn set_salience(&mut self, _h: &MemoryHandle, id: NodeId, salience: u8) -> bool {
        let Ok(i) = self.nodes.binary_search_by_key(&id.0, |n| n.id.0) else {
            return false;
        };
        self.nodes[i].salience = salience;
        true
    }

    pub fn revise(&mut self, _h: &MemoryHandle, id: NodeId, confidence: u8) -> bool {
        match self.nodes.binary_search_by_key(&id.0, |n| n.id.0) {
            Err(_) => false,
            Ok(i) => {
                let n = &mut self.nodes[i];
                n.confidence = n.provenance.cap(confidence);
                true
            }
        }
    }

    pub fn reinforce(&mut self, _h: &MemoryHandle, id: NodeId) {
        if let Ok(i) = self.nodes.binary_search_by_key(&id.0, |n| n.id.0) {
            self.nodes[i].provenance.reinforce();
            let n = &mut self.nodes[i];
            n.salience = n.salience.saturating_add(8).min(100);
        }
    }

    /// One decay pass. Every prunable node loses salience; nodes with
    /// retrievals or reinforcement lose less. This is what makes pruning
    /// mean "forget what did not matter" rather than "forget what is old".
    pub fn decay(&mut self, _h: &MemoryHandle, amount: u8) {
        for n in self.nodes.iter_mut() {
            if !n.kind.prunable() {
                continue;
            }
            let resistance = (n.retrievals.min(8) as u8) + (n.provenance.reinforced.min(8) as u8);
            n.salience = n.salience.saturating_sub(amount.saturating_sub(resistance));
        }
    }

    /// Drop a node and every edge pointing at it. Returns false for a
    /// node that is not prunable by kind.
    pub fn forget(&mut self, _h: &MemoryHandle, id: NodeId) -> bool {
        let Ok(i) = self.nodes.binary_search_by_key(&id.0, |n| n.id.0) else {
            return false;
        };
        if !self.nodes[i].kind.prunable() {
            return false;
        }
        self.nodes.remove(i);
        for n in self.nodes.iter_mut() {
            n.edges.retain(|e| e.to != id);
        }
        self.pruned = self.pruned.saturating_add(1);
        true
    }

    /// Keep each bounded kind inside its cap by dropping the least
    /// salient, oldest-first among equals.
    fn enforce_bounds(&mut self, kind: NodeKind) {
        let Some(cap) = self.bounds.cap_for(kind) else {
            return;
        };
        loop {
            let count = self.count(kind);
            if count <= cap {
                break;
            }
            // find the weakest candidate of this kind
            let victim = self
                .nodes
                .iter()
                .filter(|n| n.kind == kind && n.kind.prunable())
                .min_by(|a, b| {
                    a.salience
                        .cmp(&b.salience)
                        .then(a.confidence.cmp(&b.confidence))
                        .then(a.id.0.cmp(&b.id.0))
                })
                .map(|n| n.id);
            match victim {
                Some(id) => {
                    let Ok(i) = self.nodes.binary_search_by_key(&id.0, |n| n.id.0) else {
                        break;
                    };
                    self.nodes.remove(i);
                    for n in self.nodes.iter_mut() {
                        n.edges.retain(|e| e.to != id);
                    }
                    self.pruned = self.pruned.saturating_add(1);
                }
                None => break, // nothing prunable — audit, for instance
            }
        }
    }

    /// Replace a skill node's content. The *only* mutation of existing
    /// content anywhere in the graph, and it is restricted to skill nodes
    /// by an explicit check: a memory of what happened is never edited,
    /// because an entity that rewrites its own episodes cannot be
    /// reasoned about. A skill node is different — it is a live summary
    /// of statistics that live in the skill graph, and letting it drift
    /// would mean an export carried stale numbers.
    /// Replace a skill node's structured form.
    ///
    /// Narrow on purpose, like `rewrite_skill_node` next to it: a skill
    /// keeping its own exact state current is the one caller. A state
    /// that would exceed the bound is refused rather than truncated —
    /// half a serialization is a corrupt memory, not a smaller one, and
    /// the caller is told so it can shed something instead.
    pub fn set_skill_state(&mut self, _h: &MemoryHandle, id: NodeId, state: &[u8]) -> bool {
        if state.len() > self.bounds.max_state {
            return false;
        }
        let Ok(i) = self.nodes.binary_search_by_key(&id.0, |n| n.id.0) else {
            return false;
        };
        if self.nodes[i].kind != NodeKind::Skill {
            return false;
        }
        self.nodes[i].state.clear();
        self.nodes[i].state.extend_from_slice(state);
        true
    }

    pub fn rewrite_skill_node(&mut self, _h: &MemoryHandle, id: NodeId, text: &str) -> bool {
        let Ok(i) = self.nodes.binary_search_by_key(&id.0, |n| n.id.0) else {
            return false;
        };
        if self.nodes[i].kind != NodeKind::Skill {
            return false;
        }
        let mut content = alloc::string::ToString::to_string(text);
        if content.len() > self.bounds.max_content {
            let mut cut = self.bounds.max_content;
            while cut > 0 && !content.is_char_boundary(cut) {
                cut -= 1;
            }
            content.truncate(cut);
        }
        self.nodes[i].content = content;
        true
    }

    pub fn mark_session(&mut self) {
        self.session_mark = self.nodes.len();
    }

    // ---- serialization: memory travels with the key ----

    /// Encode the durable memories. Audit is excluded — it belongs to
    /// the machine of record, and an entity carrying its own audit log
    /// between bodies could edit it in transit.
    pub fn encode_durable(&self) -> Result<alloc::vec::Vec<u8>> {
        let durable: Vec<&Node> = self.nodes.iter().filter(|n| n.kind.durable()).collect();
        let mut e = Encoder::new();
        e.u64(self.next_id);
        e.seq(durable.len());
        for n in durable {
            n.encode(&mut e);
        }
        e.finish()
    }

    /// Rehydrate memories written by a previous embodiment. Node ids are
    /// preserved so that edges keep meaning across the move; the counter
    /// resumes above the highest id seen.
    pub fn load_durable(&mut self, _h: &MemoryHandle, data: &[u8]) -> Result<usize> {
        let mut d = Decoder::new(data);
        let next_id = d.u64()?;
        let n = d.seq()?;
        let mut loaded = 0;
        for _ in 0..n {
            let node = Node::decode(&mut d)?;
            if node.kind == NodeKind::Episode {
                self.inherited += 1;
            }
            self.nodes.push(node);
            loaded += 1;
        }
        d.finish()?;
        self.nodes.sort_by_key(|n| n.id.0);
        self.next_id = self.next_id.max(next_id);
        Ok(loaded)
    }
}
