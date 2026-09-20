//! Brain placement — Stage 5.
//!
//! "Cognition runs wherever it should — on-body for reflexes, edge for
//! fast reasoning, datacenter for deliberation — and the entity does not
//! care which."
//!
//! The entity not caring is the engineering claim, and it is testable:
//! `Plan` decides where each tier runs, and the *only* thing that
//! changes when a plan changes is which `Resource` gets picked. No
//! memory moves. No identity moves. The state graph stays with the
//! entity, which is why a remotely-placed brain is not a cloud
//! dependency in the Jibo sense — cut the link and the entity is less
//! capable, not dead.
//!
//! That degradation is the property worth defending, so it is explicit:
//! `Plan::degraded_ceiling` says what the entity can still do when a
//! site is unreachable, and it is never `None`.

use crate::resource::Tier;
use brainos_abi::text::Text;
use brainos_abi::time::Duration;

/// Where compute physically is.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
#[repr(u8)]
pub enum Site {
    /// the microcontroller inside the body
    Body = 0,
    /// compute carried by the body but not in the control loop
    OnBoard = 1,
    /// a node in the same building — a home hub, a cell controller
    Edge = 2,
    /// a datacenter
    Remote = 3,
}

impl Site {
    pub const ALL: [Site; 4] = [Site::Body, Site::OnBoard, Site::Edge, Site::Remote];

    pub const fn name(self) -> &'static str {
        match self {
            Site::Body => "body",
            Site::OnBoard => "onboard",
            Site::Edge => "edge",
            Site::Remote => "remote",
        }
    }

    /// Round-trip latency to reach this site, as a planning figure.
    pub const fn reach(self) -> Duration {
        match self {
            Site::Body => Duration::from_micros(100),
            Site::OnBoard => Duration::from_millis(2),
            Site::Edge => Duration::from_millis(15),
            Site::Remote => Duration::from_millis(120),
        }
    }

    /// Can the body keep working when this site is unreachable?
    /// Everything except the body itself.
    pub const fn is_optional(self) -> bool {
        !matches!(self, Site::Body)
    }
}

/// Where one tier's work happens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Placement {
    pub tier: Tier,
    pub site: Site,
}

impl Placement {
    /// Is this placement physically coherent? A tier's latency budget
    /// has to be bigger than the round trip to the site, or the resource
    /// cannot answer in time no matter how fast it thinks.
    ///
    /// This is the check that makes a placement plan an engineering
    /// document rather than a diagram: putting Tier 1 in a datacenter
    /// fails here, at configuration time, instead of failing as a robot
    /// that reacts to obstacles 120 ms late.
    pub fn is_coherent(&self) -> bool {
        self.site.reach().as_micros() * 2 < self.tier.budget().as_micros()
    }
}

/// The whole placement plan.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Plan {
    placements: [Site; 4],
}

impl Default for Plan {
    fn default() -> Plan {
        Plan::all_on_body()
    }
}

impl Plan {
    /// Everything on the body. What Phase 0 runs, and what every body
    /// falls back to when it is alone.
    pub fn all_on_body() -> Plan {
        Plan {
            placements: [Site::Body, Site::OnBoard, Site::OnBoard, Site::OnBoard],
        }
    }

    /// The Stage 5 shape: reflexes on the body, fast reasoning on board,
    /// deliberation at the edge, novelty remote.
    pub fn distributed() -> Plan {
        Plan {
            placements: [Site::Body, Site::OnBoard, Site::Edge, Site::Remote],
        }
    }

    /// A body with no compute of its own — a cheap limb driven entirely
    /// from a hub. Only the autonomic loop is local, which is the
    /// minimum a body that can fall is allowed to have.
    pub fn thin_body() -> Plan {
        Plan {
            placements: [Site::Body, Site::Edge, Site::Edge, Site::Remote],
        }
    }

    pub fn site_for(&self, tier: Tier) -> Site {
        self.placements[tier as usize]
    }

    pub fn place(&mut self, tier: Tier, site: Site) -> bool {
        let p = Placement { tier, site };
        if !p.is_coherent() {
            return false;
        }
        // Tier 0 is never anywhere but the body. A reflex that runs
        // elsewhere is not a reflex, it is a remote command with a
        // hopeful name, and a body whose balance depends on a network is
        // a body that falls when the network hiccups.
        if tier == Tier::Autonomic && site != Site::Body {
            return false;
        }
        self.placements[tier as usize] = site;
        true
    }

    pub fn is_coherent(&self) -> bool {
        Tier::ALL.iter().all(|t| {
            Placement {
                tier: *t,
                site: self.site_for(*t),
            }
            .is_coherent()
        }) && self.site_for(Tier::Autonomic) == Site::Body
    }

    /// What the entity can still reach when a site becomes unreachable.
    ///
    /// Never `None`: the autonomic tier is always on the body, so an
    /// entity cut off from everything still has reflexes and can still
    /// hold itself up. That is the difference between degraded and dead,
    /// and it is the difference Jibo's owners did not get.
    pub fn degraded_ceiling(&self, lost: Site) -> Tier {
        Tier::ALL
            .iter()
            .copied()
            .filter(|t| self.site_for(*t) != lost)
            .max()
            .unwrap_or(Tier::Autonomic)
    }

    /// The worst case round trip for a tier under this plan.
    pub fn latency_for(&self, tier: Tier) -> Duration {
        self.site_for(tier).reach().saturating_mul(2) + tier.budget()
    }

    pub fn describe(&self) -> Text<128> {
        let mut t = Text::new();
        for tier in Tier::ALL {
            if tier != Tier::Autonomic {
                t.push(' ');
            }
            t.push_str(tier.name());
            t.push('@');
            t.push_str(self.site_for(tier).name());
        }
        t
    }
}
