//! BFS  LEVEL    Domain 4 Cognition - tiers, placement, attention, escalation  [crate brainos-cognition]
//! BFS  PURPOSE  Where each tier of thinking runs (Site), what tissue it runs on (Substrate), and the Plan that assigns both
//! BFS  DEFINES  Site, Substrate, Placement, Plan
//! BFS  USES     this crate: resource | brainos: abi
//! BFS  USED BY  cognition/lib, cognition/resource, cognition/tests/cognition_suite, entity/entity, entity/entity/parts, entity/tests/parts_suite, host/degrade, host/life, +8 more
//!
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

/// What kind of tissue compute is made of.
///
/// `Site` says *where* compute is. `Substrate` says *what it is*, and
/// the two are independent: a GPU can be on board or in a datacenter,
/// and a BPU can be either. The distinction matters to placement
/// because the tissues differ in ways latency does not capture — what
/// they can run, what it costs in energy, and whether changing what
/// they hold wears them out.
///
/// A substrate is how a Compute-class *part* (`brainos-part`) shows up
/// in cognition: acquiring a GPU or a BPU through §8 is what makes a
/// `Resource` on that substrate exist, and the part's vitals are what
/// make it unavailable again.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
#[repr(u8)]
pub enum Substrate {
    /// a microcontroller: where reflexes run
    Mcu = 0,
    /// a general-purpose processor
    Cpu = 1,
    /// a graphics processor
    Gpu = 2,
    /// a neural accelerator (fixed-function inference silicon)
    Npu = 3,
    /// the BrAIn Processing Unit: analog compute-in-memory, weights
    /// held as device conductances (`brainos-bpu`)
    Bpu = 4,
}

impl Substrate {
    pub const ALL: [Substrate; 5] = [
        Substrate::Mcu,
        Substrate::Cpu,
        Substrate::Gpu,
        Substrate::Npu,
        Substrate::Bpu,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Substrate::Mcu => "mcu",
            Substrate::Cpu => "cpu",
            Substrate::Gpu => "gpu",
            Substrate::Npu => "npu",
            Substrate::Bpu => "bpu",
        }
    }

    /// Does loading a model onto this tissue consume its lifetime?
    ///
    /// True only of the BPU, whose weights are physical device states
    /// with a finite number of writes. It is why loading a model there
    /// is a consequential, KIRA-gated verb and not a file copy, and why
    /// the lifecycle is train elsewhere, deploy here.
    pub const fn wears_on_write(self) -> bool {
        matches!(self, Substrate::Bpu)
    }

    /// May a tier be asked to run on this tissue at all?
    ///
    /// Tier 0 runs on the body's own microcontroller and nowhere else —
    /// the same rule `Plan::place` enforces for sites. Nothing above
    /// Tier 0 runs on one: an MCU that is busy deliberating is an MCU
    /// that is late for its control loop.
    pub const fn can_host(self, tier: Tier) -> bool {
        match tier {
            Tier::Autonomic => matches!(self, Substrate::Mcu),
            _ => !matches!(self, Substrate::Mcu),
        }
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
    /// which tissue each tier would rather run on, if one is present
    /// and healthy. `None` means no preference: take the cheapest.
    tissue: [Option<Substrate>; 4],
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
            tissue: [None; 4],
        }
    }

    /// The Stage 5 shape: reflexes on the body, fast reasoning on board,
    /// deliberation at the edge, novelty remote.
    pub fn distributed() -> Plan {
        Plan {
            placements: [Site::Body, Site::OnBoard, Site::Edge, Site::Remote],
            tissue: [None; 4],
        }
    }

    /// A body with no compute of its own — a cheap limb driven entirely
    /// from a hub. Only the autonomic loop is local, which is the
    /// minimum a body that can fall is allowed to have.
    pub fn thin_body() -> Plan {
        Plan {
            placements: [Site::Body, Site::Edge, Site::Edge, Site::Remote],
            tissue: [None; 4],
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

    /// The tissue a tier would rather run on.
    pub fn substrate_for(&self, tier: Tier) -> Option<Substrate> {
        self.tissue[tier as usize]
    }

    /// State a preference: this tier should run on this tissue when it
    /// is there and healthy. Refused when the tissue cannot host the
    /// tier at all (`Substrate::can_host`).
    ///
    /// A preference, not a requirement, and that is the whole point:
    /// `ResourcePool::pick` falls back to whatever else can do the work
    /// when the preferred tissue is absent or unwell. An entity that
    /// *required* a particular chip would be an entity that stops
    /// thinking when that chip overheats.
    pub fn prefer(&mut self, tier: Tier, substrate: Substrate) -> bool {
        if !substrate.can_host(tier) {
            return false;
        }
        self.tissue[tier as usize] = Some(substrate);
        true
    }

    pub fn no_preference(&mut self, tier: Tier) {
        self.tissue[tier as usize] = None;
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
