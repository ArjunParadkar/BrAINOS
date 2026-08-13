//! The state graph — replacing files and the filesystem (Architecture §12).
//!
//! There is no filesystem. A UNIX file is a dumb byte stream; a state node
//! is a typed, semantically meaningful entry the instance already
//! understands. Three memory systems live here, mirroring biological
//! memory: episodic (ring buffer of recent events), semantic (compressed
//! knowledge), procedural (the skill graph). The world model is beliefs
//! with confidence.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub type NodeId = u32;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// world-model entry: something the instance holds true, with confidence
    Belief,
    /// episodic memory: what happened and when
    Episode,
    /// semantic memory: compressed, queryable knowledge
    Semantic,
    /// procedural memory: a learned action pathway (the skill graph)
    Skill,
    /// KIRA audit record — immutable once written
    Audit,
}

impl NodeKind {
    pub fn tag(self) -> &'static str {
        match self {
            NodeKind::Belief => "belief",
            NodeKind::Episode => "episode",
            NodeKind::Semantic => "semantic",
            NodeKind::Skill => "skill",
            NodeKind::Audit => "audit",
        }
    }
    fn from_tag(t: &str) -> Option<NodeKind> {
        Some(match t {
            "belief" => NodeKind::Belief,
            "episode" => NodeKind::Episode,
            "semantic" => NodeKind::Semantic,
            "skill" => NodeKind::Skill,
            "audit" => NodeKind::Audit,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy)]
pub enum LinkKind {
    Follows,
    About,
    Caused,
}

pub struct StateNode {
    pub id: NodeId,
    pub kind: NodeKind,
    pub content: String,
    /// 0–100; files have none of this — provenance is the point
    pub confidence: u8,
    pub tick: u64,
    pub links: Vec<(NodeId, LinkKind)>,
}

pub struct StateGraph {
    nodes: Vec<StateNode>,
    next_id: NodeId,
    /// ring capacity for episodic memory (§12: recent events, pruned)
    episodic_cap: usize,
    /// episodes carried over from previous embodiments of this key
    pub inherited_episodes: usize,
}

impl StateGraph {
    pub fn new() -> StateGraph {
        StateGraph {
            nodes: Vec::new(),
            next_id: 1,
            episodic_cap: 48,
            inherited_episodes: 0,
        }
    }

    pub fn add(&mut self, kind: NodeKind, content: String, confidence: u8, tick: u64) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.push(StateNode {
            id,
            kind,
            content,
            confidence,
            tick,
            links: Vec::new(),
        });
        if kind == NodeKind::Episode {
            self.prune_episodic();
        }
        id
    }

    pub fn link(&mut self, from: NodeId, to: NodeId, kind: LinkKind) {
        if let Some(n) = self.nodes.iter_mut().find(|n| n.id == from) {
            n.links.push((to, kind));
        }
    }

    /// Ring-buffer semantics for episodic memory: oldest episodes fall away.
    fn prune_episodic(&mut self) {
        let mut episodes = self.nodes.iter().filter(|n| n.kind == NodeKind::Episode).count();
        while episodes > self.episodic_cap {
            if let Some(pos) = self.nodes.iter().position(|n| n.kind == NodeKind::Episode) {
                self.nodes.remove(pos);
                episodes -= 1;
            } else {
                break;
            }
        }
    }

    pub fn count(&self, kind: NodeKind) -> usize {
        self.nodes.iter().filter(|n| n.kind == kind).count()
    }

    pub fn last(&self, kind: NodeKind) -> Option<&StateNode> {
        self.nodes.iter().rev().find(|n| n.kind == kind)
    }

    // ---- the SkillGraph (§12): learned tool-use pathways ----
    // First successful use of a limb is effortful; it lays down a skill
    // node so the next occasion is recognized and reused cheaply.

    /// Has this capability been used successfully before?
    pub fn knows_skill(&self, verb: &str) -> bool {
        self.nodes
            .iter()
            .any(|n| n.kind == NodeKind::Skill && n.content.starts_with(verb) && n.content[verb.len()..].starts_with(" ::"))
    }

    /// Record (or reinforce) a skill: verb :: how it was used.
    pub fn learn_skill(&mut self, verb: &str, note: &str, tick: u64) -> bool {
        let key = format!("{verb} ::");
        if let Some(n) = self
            .nodes
            .iter_mut()
            .find(|n| n.kind == NodeKind::Skill && n.content.starts_with(&key))
        {
            // reinforce: bump confidence toward mastery
            n.confidence = n.confidence.saturating_add(5).min(100);
            n.tick = tick;
            false // already known
        } else {
            self.add(NodeKind::Skill, format!("{key} {note}"), 60, tick);
            true // newly learned
        }
    }

    pub fn skill_count(&self) -> usize {
        self.count(NodeKind::Skill)
    }

    pub fn last_id(&self) -> NodeId {
        self.next_id.saturating_sub(1)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Naive lexical retrieval — phase-0 stand-in for semantic search.
    /// Finds memories sharing meaningful words with the query so they can
    /// ride the tether as context for Model M.
    pub fn relevant(&self, query: &str, n: usize) -> Vec<&StateNode> {
        let q = query.to_ascii_lowercase();
        let words: Vec<&str> = q
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| w.len() >= 4)
            .collect();
        let mut scored: Vec<(usize, &StateNode)> = self
            .nodes
            .iter()
            .filter(|nd| {
                matches!(
                    nd.kind,
                    NodeKind::Episode | NodeKind::Semantic | NodeKind::Belief
                )
            })
            .filter_map(|nd| {
                let c = nd.content.to_ascii_lowercase();
                let score = words.iter().filter(|w| c.contains(**w)).count();
                (score > 0).then_some((score, nd))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().take(n).map(|(_, nd)| nd).collect()
    }

    /// The most durable knowledge, for priming cognition at wake:
    /// semantic notes and beliefs first, then the freshest episodes.
    pub fn durable_digest(&self, n_sem: usize, n_epi: usize) -> Vec<String> {
        let mut out = Vec::new();
        for nd in self
            .nodes
            .iter()
            .rev()
            .filter(|n| matches!(n.kind, NodeKind::Semantic | NodeKind::Belief))
            .take(n_sem)
        {
            out.push(nd.content.clone());
        }
        for nd in self
            .nodes
            .iter()
            .rev()
            .filter(|n| n.kind == NodeKind::Episode)
            .take(n_epi)
        {
            out.push(nd.content.clone());
        }
        out.reverse();
        out
    }

    /// Everything that happened after `mark` — raw material for dreaming.
    pub fn episodes_since(&self, mark: usize) -> String {
        let mut s = String::new();
        for nd in self.nodes.iter().skip(mark) {
            if nd.kind == NodeKind::Episode {
                if !s.is_empty() {
                    s.push_str(" ; ");
                }
                s.push_str(&nd.content);
            }
        }
        s
    }

    // ---- persistence: memory travels with the key ----

    /// Serialize the durable memories (episodes + semantic + beliefs) as
    /// text lines for the key medium. Audit stays on the machine of record.
    /// Semantic memory is capped so the key file stays bounded across many
    /// lives (oldest compressed knowledge falls away first).
    pub fn serialize(&self) -> String {
        let semantic_total = self.count(NodeKind::Semantic);
        let mut semantic_skip = semantic_total.saturating_sub(64);
        let mut out = String::new();
        for n in &self.nodes {
            if n.kind == NodeKind::Semantic && semantic_skip > 0 {
                semantic_skip -= 1;
                continue;
            }
            if matches!(n.kind, NodeKind::Episode | NodeKind::Semantic | NodeKind::Belief) {
                out.push_str(&format!(
                    "{}|{}|{}|{}\n",
                    n.kind.tag(),
                    n.tick,
                    n.confidence,
                    n.content
                ));
            }
        }
        out
    }

    /// Rehydrate memories written by a previous embodiment.
    pub fn load(&mut self, data: &[u8]) {
        let text = core::str::from_utf8(data).unwrap_or("");
        for line in text.lines() {
            let mut parts = line.splitn(4, '|');
            let (Some(tag), Some(tick), Some(conf), Some(content)) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Some(kind) = NodeKind::from_tag(tag) else { continue };
            let tick: u64 = tick.parse().unwrap_or(0);
            let conf: u8 = conf.parse().unwrap_or(50);
            self.add(kind, String::from(content), conf, tick);
            if kind == NodeKind::Episode {
                self.inherited_episodes += 1;
            }
        }
    }

    // ---- the world model's predictive interface (§7.1) ----
    // Boilerplate: predictions come from a fixed rhythm the model "knows".
    // The real system replaces this with a learned forward model; the
    // contract — predict, compare, escalate on surprise — stays identical.

    pub fn predict_sense(&self, tick: u64) -> &'static str {
        SENSE_RHYTHM[(tick % SENSE_RHYTHM.len() as u64) as usize]
    }
}

pub const SENSE_RHYTHM: [&str; 7] = [
    "room is quiet",
    "photons on the panel",
    "clock drift nominal",
    "nothing surprising",
    "the fans breathe",
    "silicon warm and happy",
    "waiting for you",
];
