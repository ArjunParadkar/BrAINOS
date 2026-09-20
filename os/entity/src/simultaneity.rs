//! Stage 4 — what "many bodies at once" actually costs.
//!
//! The easy part is free: there is one state graph, so nothing is
//! synced. The hard part is that two bodies can want incompatible
//! things at the same instant, and one of them can stop answering in
//! the middle of an action that the other one is depending on.
//!
//! This module is the arbiter. Three rules, in order, and the order is
//! the argument:
//!
//! 1. **Physical exclusivity beats everything.** Two bodies cannot both
//!    be carrying the same object. A claim on a physical resource is
//!    exclusive and the incumbent keeps it — because taking it away
//!    mid-action is how something gets dropped.
//!
//! 2. **Consequence outranks priority.** When two claims conflict and
//!    neither holds the resource, the one whose refusal is *less
//!    dangerous* yields. A body that is mid-stride yields to one that is
//!    mid-lift, not because lifting matters more, but because
//!    interrupting a stride is a stumble and interrupting a lift is a
//!    drop.
//!
//! 3. **A silent body loses its claims, after a grace period.** Not
//!    immediately: a link hiccup should not make a robot let go. But a
//!    body that has stopped answering cannot be relied on to finish, and
//!    a claim held by something that will never release it is a deadlock
//!    with an entity in it.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::ids::BodyId;
use brainos_abi::text::Text;
use brainos_abi::time::{Duration, Instant, Tick};
use brainos_abi::verb::Verb;

/// Something in the world that only one body may be using.
///
/// Identified by a name rather than a type, because the set is not
/// knowable in advance — "the kettle", "the left half of the workbench",
/// "the doorway" are all resources an entity might need to arbitrate
/// over, and enumerating them would mean a new build per kitchen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource(pub String);

impl Resource {
    pub fn named(s: &str) -> Resource {
        Resource(alloc::string::ToString::to_string(s))
    }
}

/// One body wanting one resource.
#[derive(Clone, Debug, PartialEq)]
pub struct Claim {
    pub body: BodyId,
    pub resource: Resource,
    pub verb: Verb,
    /// how bad it is to be interrupted while holding this, 0–100
    pub interruption_cost: u8,
    pub claimed_at: Tick,
    /// when the body last spoke
    pub last_heard: Instant,
}

/// Two claims that cannot both be honoured.
#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    pub incumbent: Claim,
    pub challenger: Claim,
}

/// How a conflict was settled, and why.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    /// the incumbent keeps it
    Keep { because: &'static str },
    /// the incumbent yields
    Yield { because: &'static str },
    /// both wait; the entity must think about this one
    Escalate { because: &'static str },
}

impl Resolution {
    pub fn because(&self) -> &'static str {
        match self {
            Resolution::Keep { because }
            | Resolution::Yield { because }
            | Resolution::Escalate { because } => because,
        }
    }
}

/// How long a body may be silent before its claims lapse.
pub const SILENCE_GRACE: Duration = Duration::from_millis(1_500);

/// The arbiter over all bodies of one entity.
#[derive(Default)]
pub struct Arbiter {
    claims: Vec<Claim>,
    pub conflicts_seen: u64,
    pub lapses: u64,
}

impl Arbiter {
    pub fn new() -> Arbiter {
        Arbiter {
            claims: Vec::new(),
            conflicts_seen: 0,
            lapses: 0,
        }
    }

    pub fn claims(&self) -> &[Claim] {
        &self.claims
    }

    pub fn len(&self) -> usize {
        self.claims.len()
    }

    pub fn is_empty(&self) -> bool {
        self.claims.is_empty()
    }

    pub fn holder_of(&self, resource: &Resource) -> Option<&Claim> {
        self.claims.iter().find(|c| &c.resource == resource)
    }

    /// Try to claim. Returns `Ok` if the claim is now held, or the
    /// conflict and its resolution if not.
    pub fn claim(&mut self, claim: Claim) -> Result<(), (Conflict, Resolution)> {
        // a body re-claiming what it already holds is a refresh, not a
        // conflict — this is what a gait loop does every step
        if let Some(existing) = self
            .claims
            .iter_mut()
            .find(|c| c.resource == claim.resource && c.body == claim.body)
        {
            existing.last_heard = claim.last_heard;
            existing.interruption_cost = claim.interruption_cost;
            return Ok(());
        }

        match self.holder_of(&claim.resource).cloned() {
            None => {
                self.claims.push(claim);
                Ok(())
            }
            Some(incumbent) => {
                self.conflicts_seen = self.conflicts_seen.saturating_add(1);
                let conflict = Conflict {
                    incumbent: incumbent.clone(),
                    challenger: claim.clone(),
                };
                let resolution = resolve(&conflict);
                match resolution {
                    Resolution::Yield { .. } => {
                        self.claims
                            .retain(|c| c.resource != claim.resource);
                        self.claims.push(claim);
                        Ok(())
                    }
                    _ => Err((conflict, resolution)),
                }
            }
        }
    }

    /// A body finished with a resource.
    pub fn release(&mut self, body: &BodyId, resource: &Resource) -> bool {
        let before = self.claims.len();
        self.claims
            .retain(|c| !(&c.body == body && &c.resource == resource));
        self.claims.len() != before
    }

    /// A body left entirely.
    pub fn release_all(&mut self, body: &BodyId) -> usize {
        let before = self.claims.len();
        self.claims.retain(|c| &c.body != body);
        before - self.claims.len()
    }

    /// Drop claims held by bodies that have gone silent past the grace
    /// period. Returns them, because a lapsed claim is an event the
    /// entity should know about — something it was relying on stopped.
    pub fn lapse_silent(&mut self, now: Instant) -> Vec<Claim> {
        let mut lapsed = Vec::new();
        let mut kept = Vec::new();
        for c in self.claims.drain(..) {
            if now.since(c.last_heard) > SILENCE_GRACE {
                lapsed.push(c);
            } else {
                kept.push(c);
            }
        }
        self.claims = kept;
        self.lapses = self.lapses.saturating_add(lapsed.len() as u64);
        lapsed
    }

    /// Every body currently doing something.
    pub fn busy_bodies(&self) -> Vec<BodyId> {
        let mut out: Vec<BodyId> = self.claims.iter().map(|c| c.body).collect();
        out.sort();
        out.dedup();
        out
    }

    pub fn describe(&self) -> Text<200> {
        let mut t = Text::new();
        t.push_u64(self.claims.len() as u64);
        t.push_str(" claims across ");
        t.push_u64(self.busy_bodies().len() as u64);
        t.push_str(" bodies");
        if self.lapses > 0 {
            t.push_str(", ");
            t.push_u64(self.lapses);
            t.push_str(" lapsed");
        }
        t
    }
}

/// The arbitration rule.
pub fn resolve(c: &Conflict) -> Resolution {
    // 1. an incumbent that would be badly hurt by interruption keeps it
    if c.incumbent.interruption_cost >= 70 {
        return Resolution::Keep {
            because: "interrupting the incumbent would drop or break something",
        };
    }

    // 2. otherwise the more consequential claim wins, by a clear margin.
    // A margin rather than a comparison: two claims within a few points
    // of each other are not meaningfully different, and swapping on
    // noise is thrashing.
    let inc = c.incumbent.interruption_cost as i16;
    let cha = c.challenger.interruption_cost as i16;
    if cha - inc >= 20 {
        return Resolution::Yield {
            because: "the challenger's interruption would cost more",
        };
    }
    if inc - cha >= 20 {
        return Resolution::Keep {
            because: "the incumbent's interruption would cost more",
        };
    }

    // 3. too close to call. This is the one case the entity has to think
    // about, and saying so is better than picking.
    Resolution::Escalate {
        because: "two bodies want the same thing and neither obviously yields",
    }
}
