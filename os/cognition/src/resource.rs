//! BFS  LEVEL    Domain 4 Cognition - tiers, placement, attention, escalation  [crate brainos-cognition]
//! BFS  PURPOSE  Tier (how far up the entity reaches), Resource (one thing that can think) and ResourcePool (pick the cheapest one that can do the job)
//! BFS  DEFINES  Tier, Capability, Resource, ResourcePool
//! BFS  USES     this crate: placement | brainos: abi
//! BFS  USED BY  bpu/tests/seam, cognition/ambient, cognition/escalate, cognition/lib, cognition/placement, cognition/tests/cognition_suite, entity/entity, entity/entity/parts, +6 more
//!
//! Cognitive resources and the tiers they sit in.
//!
//! A resource is described by four things: what it is good at, how long
//! it takes, how much it costs, and where it is. Nothing here knows
//! whether a resource is a lookup table, a small on-body network, or a
//! frontier model behind an API — that is the point. π0, GR00T and
//! Gemini Robotics are Tier 1 resources from this crate's perspective,
//! and the entity escalates to them exactly as it escalates to anything
//! else.
//!
//! The relationship to the model layer is worth stating plainly, because
//! it is the company's position: we are upstream of the most expensive
//! thing in the industry, not downstream of it. A better reflex is still
//! a function. Continuity is not in the model; it is in what holds the
//! model.

use crate::placement::{Site, Substrate};
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::text::Text;
use brainos_abi::time::Duration;
use brainos_abi::verb::Namespace;

/// How far up the entity has reached.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
#[repr(u8)]
pub enum Tier {
    /// reflexes. No cognition in the loop; never proposes into KIRA.
    Autonomic = 0,
    /// fast local reasoning: obstacle → replan, parse an intent
    Fast = 1,
    /// deliberation: task planning, skill selection
    Deliberate = 2,
    /// novel situations, consolidation, anything genuinely new
    Frontier = 3,
}

impl Default for Tier {
    /// Autonomic. An entity whose reachable ceiling is unknown is an
    /// entity that has reflexes and nothing else — the conservative
    /// assumption, and the one that is always true.
    fn default() -> Tier {
        Tier::Autonomic
    }
}

impl Tier {
    pub const ALL: [Tier; 4] = [Tier::Autonomic, Tier::Fast, Tier::Deliberate, Tier::Frontier];

    pub const fn name(self) -> &'static str {
        match self {
            Tier::Autonomic => "autonomic",
            Tier::Fast => "fast",
            Tier::Deliberate => "deliberate",
            Tier::Frontier => "frontier",
        }
    }

    /// The latency this tier is expected to answer within.
    pub const fn budget(self) -> Duration {
        match self {
            Tier::Autonomic => Duration::from_micros(10_000),
            Tier::Fast => Duration::from_millis(100),
            Tier::Deliberate => Duration::from_secs(1),
            Tier::Frontier => Duration::from_secs(30),
        }
    }

    /// Does this tier propose into KIRA? Everything except Tier 0.
    pub const fn proposes(self) -> bool {
        !matches!(self, Tier::Autonomic)
    }

    pub fn next(self) -> Option<Tier> {
        match self {
            Tier::Autonomic => Some(Tier::Fast),
            Tier::Fast => Some(Tier::Deliberate),
            Tier::Deliberate => Some(Tier::Frontier),
            Tier::Frontier => None,
        }
    }
}

/// What a resource is good at.
#[derive(Clone, Debug, PartialEq)]
pub struct Capability {
    /// the verbs it can propose
    pub scope: Namespace,
    /// a short description, for the audit record and for the human
    pub about: String,
}

/// One cognitive resource.
pub struct Resource {
    pub name: &'static str,
    pub tier: Tier,
    pub site: Site,
    /// what kind of tissue it runs on. Defaults from the site — a
    /// microcontroller on the body, a processor anywhere else — and is
    /// set explicitly (`on`) when a Compute part says otherwise.
    pub substrate: Substrate,
    pub capabilities: Vec<Capability>,
    /// measured latency, not declared. Updated from use.
    observed: LatencyStats,
    /// cost in arbitrary units per invocation, for the escalation budget.
    /// Frontier resources cost money and on-body ones cost battery; the
    /// unit is deliberately abstract so the policy is about proportion.
    pub cost: u32,
    /// is it answering right now?
    pub available: bool,
    /// how many times it has been wrong in a way the entity noticed
    pub disappointments: u32,
    pub invocations: u64,
}

#[derive(Clone, Copy, Default)]
struct LatencyStats {
    total_micros: u64,
    samples: u64,
    worst: u64,
}

impl Resource {
    pub fn new(name: &'static str, tier: Tier, site: Site) -> Resource {
        Resource {
            name,
            tier,
            site,
            substrate: if matches!(site, Site::Body) {
                Substrate::Mcu
            } else {
                Substrate::Cpu
            },
            capabilities: Vec::new(),
            observed: LatencyStats::default(),
            cost: match tier {
                Tier::Autonomic => 0,
                Tier::Fast => 1,
                Tier::Deliberate => 10,
                Tier::Frontier => 100,
            },
            available: true,
            disappointments: 0,
            invocations: 0,
        }
    }

    pub fn able_to(mut self, scope: Namespace, about: &str) -> Resource {
        self.capabilities.push(Capability {
            scope,
            about: alloc::string::ToString::to_string(about),
        });
        self
    }

    /// Say what tissue this resource runs on.
    pub fn on(mut self, substrate: Substrate) -> Resource {
        self.substrate = substrate;
        self
    }

    /// Set what an invocation costs. Used when the cost comes from
    /// somewhere real — a Compute part's energy projection — instead of
    /// the per-tier default.
    pub fn costing(mut self, cost: u32) -> Resource {
        self.cost = cost;
        self
    }

    pub fn covers(&self, verb: &brainos_abi::verb::Verb) -> bool {
        self.capabilities.iter().any(|c| verb.matches(&c.scope))
    }

    pub fn record(&mut self, took: Duration, disappointed: bool) {
        self.observed.total_micros = self.observed.total_micros.saturating_add(took.as_micros());
        self.observed.samples = self.observed.samples.saturating_add(1);
        if took.as_micros() > self.observed.worst {
            self.observed.worst = took.as_micros();
        }
        self.invocations = self.invocations.saturating_add(1);
        if disappointed {
            self.disappointments = self.disappointments.saturating_add(1);
        }
    }

    pub fn mean_latency(&self) -> Duration {
        if self.observed.samples == 0 {
            self.tier.budget()
        } else {
            Duration::from_micros(self.observed.total_micros / self.observed.samples)
        }
    }

    pub fn worst_latency(&self) -> Duration {
        Duration::from_micros(self.observed.worst)
    }

    /// Does this resource still answer inside its tier's budget?
    ///
    /// A frontier resource that has started taking two minutes is not a
    /// frontier resource any more; it is a resource that should be
    /// treated as unavailable so the entity stops waiting on it. This is
    /// measured rather than assumed, because a remote resource's latency
    /// is a property of the network that day.
    pub fn meets_budget(&self) -> bool {
        self.observed.samples == 0 || self.mean_latency() <= self.tier.budget()
    }

    /// How often it has been useful.
    pub fn reliability_pct(&self) -> u8 {
        if self.invocations == 0 {
            return 100;
        }
        let good = self.invocations.saturating_sub(self.disappointments as u64);
        ((good * 100) / self.invocations) as u8
    }

    pub fn describe(&self) -> Text<160> {
        let mut t = Text::new();
        t.push_str(self.name);
        t.push_str(" [");
        t.push_str(self.tier.name());
        t.push('@');
        t.push_str(self.site.name());
        t.push('/');
        t.push_str(self.substrate.name());
        t.push_str("] ");
        if !self.available {
            t.push_str("UNAVAILABLE ");
        }
        t.push_u64(self.mean_latency().as_millis());
        t.push_str("ms rel=");
        t.push_u64(self.reliability_pct() as u64);
        t.push('%');
        t
    }
}

/// Everything the entity can reach for.
#[derive(Default)]
pub struct ResourcePool {
    resources: Vec<Resource>,
}

impl ResourcePool {
    pub fn new() -> ResourcePool {
        ResourcePool {
            resources: Vec::new(),
        }
    }

    pub fn add(&mut self, r: Resource) {
        self.resources.retain(|x| x.name != r.name);
        self.resources.push(r);
        self.resources.sort_by_key(|r| r.tier);
    }

    pub fn len(&self) -> usize {
        self.resources.len()
    }

    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }

    pub fn all(&self) -> &[Resource] {
        &self.resources
    }

    pub fn get(&self, name: &str) -> Option<&Resource> {
        self.resources.iter().find(|r| r.name == name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Resource> {
        self.resources.iter_mut().find(|r| r.name == name)
    }

    /// The cheapest available resource at or above a tier that covers a
    /// verb and is still meeting its latency budget.
    ///
    /// Cheapest-that-can-do-it, not best. An entity that reaches for the
    /// frontier every time is an entity with a large bill and a slow
    /// reaction, and the whole point of tiering is that most situations
    /// are not novel.
    pub fn cheapest_for(
        &self,
        verb: &brainos_abi::verb::Verb,
        at_least: Tier,
    ) -> Option<&Resource> {
        self.resources
            .iter()
            .filter(|r| r.available && r.tier >= at_least && r.tier.proposes())
            .filter(|r| r.covers(verb))
            .filter(|r| r.meets_budget())
            .min_by_key(|r| (r.cost, r.tier as u8))
    }

    /// Like `cheapest_for`, but honouring a tissue preference.
    ///
    /// If a resource on the preferred substrate is available, covers the
    /// verb and is meeting its budget, it wins — the cheapest such, if
    /// there are several. Otherwise this is exactly `cheapest_for`.
    /// Falling through is the property that matters: a preferred chip
    /// that has gone unavailable costs capability, never the ability to
    /// answer.
    pub fn pick(
        &self,
        verb: &brainos_abi::verb::Verb,
        at_least: Tier,
        prefer: Option<Substrate>,
    ) -> Option<&Resource> {
        if let Some(want) = prefer {
            let preferred = self
                .resources
                .iter()
                .filter(|r| r.available && r.tier >= at_least && r.tier.proposes())
                .filter(|r| r.substrate == want)
                .filter(|r| r.covers(verb) && r.meets_budget())
                .min_by_key(|r| (r.cost, r.tier as u8));
            if preferred.is_some() {
                return preferred;
            }
        }
        self.cheapest_for(verb, at_least)
    }

    /// Mark one named resource unavailable or available again. This is
    /// how a Compute part's vitals reach placement: the part goes
    /// critical, its resource stops being offered.
    pub fn set_available(&mut self, name: &str, available: bool) -> bool {
        match self.get_mut(name) {
            Some(r) => {
                let changed = r.available != available;
                r.available = available;
                changed
            }
            None => false,
        }
    }

    pub fn on_substrate(&self, s: Substrate) -> impl Iterator<Item = &Resource> {
        self.resources.iter().filter(move |r| r.substrate == s)
    }

    /// The best resource for a situation regardless of cost — used when
    /// the entity has decided a situation is genuinely novel.
    pub fn best_for(&self, verb: &brainos_abi::verb::Verb) -> Option<&Resource> {
        self.resources
            .iter()
            .filter(|r| r.available && r.covers(verb) && r.tier.proposes())
            .max_by_key(|r| (r.tier as u8, r.reliability_pct()))
    }

    pub fn at_tier(&self, tier: Tier) -> impl Iterator<Item = &Resource> {
        self.resources.iter().filter(move |r| r.tier == tier)
    }

    /// Mark everything at a site unavailable — the link to the edge node
    /// dropped, the datacenter is unreachable.
    pub fn site_lost(&mut self, site: Site) -> usize {
        let mut n = 0;
        for r in self.resources.iter_mut().filter(|r| r.site == site) {
            if r.available {
                r.available = false;
                n += 1;
            }
        }
        n
    }

    pub fn site_returned(&mut self, site: Site) -> usize {
        let mut n = 0;
        for r in self.resources.iter_mut().filter(|r| r.site == site) {
            if !r.available {
                r.available = true;
                n += 1;
            }
        }
        n
    }

    /// The highest tier currently reachable. When a body loses its link
    /// this drops, and the entity should know that it has become less
    /// capable rather than discovering it mid-task.
    pub fn ceiling(&self) -> Tier {
        self.resources
            .iter()
            .filter(|r| r.available && r.tier.proposes())
            .map(|r| r.tier)
            .max()
            .unwrap_or(Tier::Autonomic)
    }
}
