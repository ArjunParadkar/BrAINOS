//! Hosted brain placement — the datacenter business.
//!
//! §2.3: "Brain placement: BrAInOS instances running off-body, bodies
//! anywhere. This is the datacenter business arriving as a natural
//! consequence of the architecture, not a pivot."
//!
//! What a host actually does: runs an entity's Tier 2 and Tier 3
//! cognition. What a host does **not** hold, and cannot:
//!
//! * the **Key** — it stays in the customer's secure element. The host
//!   never signs as the entity; it proposes, and the proposal goes back
//!   to the entity to be signed and gated.
//! * the **state graph** — memory lives with the entity. A host may be
//!   given a *working set* for a session, and it is discarded.
//! * the **gate** — KIRA runs where the body is. A host's output is a
//!   suggestion.
//!
//! That arrangement is why cutting a host off degrades an entity rather
//! than killing it, and why a host is a commodity: there is nothing
//! sticky about one, which is the honest position for a business whose
//! moat is supposed to be accumulated experience rather than captivity.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::ids::EntityId;
use brainos_abi::text::Text;
use brainos_abi::time::{Duration, Instant, Tick};
use brainos_crypto::key::{Challenge, ChallengePurpose, PublicKey, Signature};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HostError {
    /// the host failed its attestation
    Unattested,
    /// the lease has expired
    Expired,
    /// the host was revoked by quorum
    Revoked,
    /// the host is already serving as many entities as it will
    AtCapacity,
    /// the host's measured latency does not support the tier it claims
    TooSlow,
    /// a host cannot be given tiers that must run on the body
    WrongTier,
}

/// How a host is shared.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tenancy {
    /// one entity per host process
    Sole,
    /// several entities, isolated by the host's own mechanisms. Cheaper,
    /// and the customer should be told which they are getting, because
    /// the isolation is now someone else's claim rather than ours.
    Shared,
}

/// What a host is permitted to run for one entity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub entity: EntityId,
    /// the lowest tier this host may run. Never below Deliberate: fast
    /// reasoning has a 100 ms budget and a datacenter round trip is
    /// 120 ms, so placing it remotely is not a policy choice, it is
    /// arithmetic.
    pub lowest_tier: u8,
    pub leased_until: Tick,
    pub tenancy: Tenancy,
    /// the latency the host promised
    pub promised: Duration,
}

/// A host.
pub struct Host {
    pub key: PublicKey,
    pub label: String,
    pub capacity: usize,
    placements: Vec<Placement>,
    revoked: bool,
    /// measured, not promised
    observed_latency: Duration,
    samples: u64,
    pub attested_at: Option<Instant>,
}

impl Host {
    pub fn new(key: PublicKey, label: &str, capacity: usize) -> Host {
        Host {
            key,
            label: alloc::string::ToString::to_string(label),
            capacity,
            placements: Vec::new(),
            revoked: false,
            observed_latency: Duration::ZERO,
            samples: 0,
            attested_at: None,
        }
    }

    pub fn id(&self) -> EntityId {
        self.key.entity_id()
    }

    pub fn is_revoked(&self) -> bool {
        self.revoked
    }

    /// Revoked by quorum. One operation, and every entity it was serving
    /// carries on degraded rather than stopping.
    pub fn revoke(&mut self) {
        self.revoked = true;
        self.placements.clear();
    }

    /// Prove the host is entitled to run this entity's brain.
    ///
    /// Note the purpose: `BrainPlacement`, not `PeerAttestation`. A host
    /// that answered a peer challenge cannot replay that answer to claim
    /// a placement, which is the whole reason the purposes are separate.
    pub fn attest(
        &mut self,
        c: &Challenge,
        sig: &Signature,
        now: Instant,
    ) -> Result<(), HostError> {
        if c.purpose != ChallengePurpose::BrainPlacement {
            return Err(HostError::Unattested);
        }
        if !c.check(&self.key, sig) {
            return Err(HostError::Unattested);
        }
        self.attested_at = Some(now);
        Ok(())
    }

    pub fn observe_latency(&mut self, d: Duration) {
        let total = self
            .observed_latency
            .as_micros()
            .saturating_mul(self.samples)
            .saturating_add(d.as_micros());
        self.samples = self.samples.saturating_add(1);
        self.observed_latency = Duration::from_micros(total / self.samples.max(1));
    }

    pub fn observed_latency(&self) -> Duration {
        self.observed_latency
    }

    /// Lease this host for one entity's cognition.
    pub fn place(&mut self, p: Placement, now: Instant) -> Result<(), HostError> {
        if self.revoked {
            return Err(HostError::Revoked);
        }
        if self.attested_at.is_none() {
            return Err(HostError::Unattested);
        }
        let _ = now;
        // Tier 0 and Tier 1 cannot be hosted. This is arithmetic, not
        // policy: a 100 ms budget does not survive a 120 ms round trip.
        if p.lowest_tier < 2 {
            return Err(HostError::WrongTier);
        }
        if self.placements.len() >= self.capacity {
            return Err(HostError::AtCapacity);
        }
        // a host that has been measured slower than it promised does not
        // get new work
        if self.samples > 0 && self.observed_latency > p.promised.saturating_mul(2) {
            return Err(HostError::TooSlow);
        }
        self.placements.retain(|x| x.entity != p.entity);
        self.placements.push(p);
        Ok(())
    }

    pub fn placements(&self) -> &[Placement] {
        &self.placements
    }

    pub fn serves(&self, entity: &EntityId, now: Tick) -> bool {
        !self.revoked
            && self
                .placements
                .iter()
                .any(|p| &p.entity == entity && now.0 <= p.leased_until.0)
    }

    /// Drop expired leases. A lease that has to be renewed is a host
    /// that cannot quietly keep serving an entity whose owner stopped
    /// paying attention.
    pub fn expire(&mut self, now: Tick) -> usize {
        let before = self.placements.len();
        self.placements.retain(|p| now.0 <= p.leased_until.0);
        before - self.placements.len()
    }

    pub fn load_pct(&self) -> u8 {
        if self.capacity == 0 {
            return 100;
        }
        ((self.placements.len() * 100) / self.capacity).min(100) as u8
    }

    pub fn describe(&self) -> Text<200> {
        let mut t = Text::new();
        t.push_str(&self.label);
        t.push_str(" (");
        t.push_str(self.id().fingerprint().as_str());
        t.push_str(") ");
        if self.revoked {
            t.push_str("REVOKED ");
        } else if self.attested_at.is_none() {
            t.push_str("unattested ");
        }
        t.push_u64(self.placements.len() as u64);
        t.push('/');
        t.push_u64(self.capacity as u64);
        t.push_str(" entities, ");
        t.push_u64(self.observed_latency.as_millis());
        t.push_str("ms");
        t
    }
}

/// What a host is *not* allowed to hold, stated as a type so that the
/// claim is checkable rather than aspirational.
///
/// There is deliberately no field here for a key or a state graph. A
/// host implementation that wanted to keep one would have to add it in
/// public, in a struct named this, which is a change somebody has to
/// defend in review.
#[derive(Clone, Debug, Default)]
pub struct HostWorkingSet {
    /// the entity this working set belongs to
    pub entity: Option<EntityId>,
    /// memories handed over for this session only, as text. Discarded
    /// when the session ends; never the authoritative copy.
    pub context: Vec<String>,
    /// how many ticks this working set may survive
    pub ttl_ticks: u64,
    pub created_at: Tick,
}

impl HostWorkingSet {
    pub fn is_expired(&self, now: Tick) -> bool {
        now.since(self.created_at) > self.ttl_ticks
    }

    /// Discard. Called at session end, and on every expiry sweep.
    pub fn discard(&mut self) {
        self.context.clear();
        self.entity = None;
    }
}
