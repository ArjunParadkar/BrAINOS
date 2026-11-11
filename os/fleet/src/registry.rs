//! BFS  LEVEL    Stage 7 - fleet registry and skill marketplace  [crate brainos-fleet]
//! BFS  PURPOSE  A directory of known entities and their self-reported health and progress; it can list who is unwell or quiet but has no way to command anyone
//! BFS  DEFINES  RegistryError, Health, Report, Entry, Registry, Fleet
//! BFS  USES     brainos: abi, crypto
//! BFS  USED BY  fleet/lib, fleet/tests/fleet_suite, nostd/lib
//!
//! The registry — a directory, not a controller.
//!
//! Every method on this type either reads, or records something an
//! entity told it. There is deliberately no `command`, no `push`, no
//! `configure`. If you want a robot to do something you send its entity
//! a proposal, and its own gate decides.
//!
//! What a registry is genuinely for: knowing what exists, noticing what
//! has gone quiet, and answering the two questions a fleet operator
//! actually has — *which of my machines are unwell*, and *is my fleet
//! getting better*. The second is the moat's measurement at fleet
//! scale, and it is the number Phase 2 publishes.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::ids::{BodyId, EntityId};
use brainos_abi::text::Text;
use brainos_abi::time::{Duration, Instant};
use brainos_crypto::merkle::Root;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegistryError {
    AlreadyRegistered,
    NotFound,
    /// the entity's attestation did not verify
    Unattested,
    /// the registry is full
    Full,
}

/// How an entity is doing, from outside.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Health {
    /// reporting, with no stops and a stable audit root
    Well,
    /// reporting, but something stopped recently
    Recovering,
    /// has not reported within its expected interval
    Quiet,
    /// has not reported for a long time
    Lost,
    /// deactivated by quorum
    Retired,
}

impl Health {
    pub const fn name(self) -> &'static str {
        match self {
            Health::Well => "well",
            Health::Recovering => "recovering",
            Health::Quiet => "quiet",
            Health::Lost => "lost",
            Health::Retired => "retired",
        }
    }

    /// Does this need a human to look at it?
    pub const fn needs_attention(self) -> bool {
        matches!(self, Health::Lost | Health::Recovering)
    }
}

/// What an entity tells the registry about itself.
///
/// Note the shape: it is a *report*, pushed by the entity. The registry
/// never polls into an entity, because polling into something implies a
/// channel that can also be used to reach in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Report {
    pub at: Instant,
    /// the entity's audit log root — a 32-byte commitment to everything
    /// it has decided, which the operator can check proofs against
    pub audit_root: Root,
    pub bodies: u16,
    /// how many gated actions since the last report
    pub granted: u32,
    pub denied: u32,
    /// reflex stops since the last report
    pub stops: u32,
    /// the entity's own competence measure, 0-100
    pub competence: u8,
    /// how many practiced skills it has
    pub skills: u16,
    /// its age in ticks
    pub age: u64,
}

/// One entity in the fleet.
#[derive(Clone, Debug)]
pub struct Entry {
    pub entity: EntityId,
    /// a name the *operator* chose. Advisory; nothing is decided by it.
    pub label: String,
    pub registered_at: Instant,
    pub last_report: Option<Report>,
    /// how often this entity is expected to report
    pub interval: Duration,
    pub reports: u64,
    pub retired: bool,
    /// bodies it has told us about
    pub bodies: Vec<BodyId>,
}

impl Entry {
    pub fn health(&self, now: Instant) -> Health {
        if self.retired {
            return Health::Retired;
        }
        let Some(r) = self.last_report else {
            // never reported: quiet until the first interval has passed,
            // then lost. A fleet that marks a just-registered entity as
            // lost is a fleet that cries wolf on every deployment.
            return if now.since(self.registered_at) > self.interval.saturating_mul(3) {
                Health::Lost
            } else {
                Health::Quiet
            };
        };
        let silence = now.since(r.at);
        if silence > self.interval.saturating_mul(3) {
            Health::Lost
        } else if silence > self.interval {
            Health::Quiet
        } else if r.stops > 0 {
            Health::Recovering
        } else {
            Health::Well
        }
    }

    /// Calibration: of what it proposed, how much was permitted.
    pub fn calibration_pct(&self) -> Option<u8> {
        let r = self.last_report?;
        let total = r.granted as u64 + r.denied as u64;
        if total == 0 {
            return None;
        }
        Some(((r.granted as u64 * 100) / total) as u8)
    }
}

/// The directory.
pub struct Registry {
    entries: Vec<Entry>,
    cap: usize,
    pub default_interval: Duration,
}

impl Default for Registry {
    fn default() -> Registry {
        Registry::new(10_000)
    }
}

impl Registry {
    pub fn new(cap: usize) -> Registry {
        Registry {
            entries: Vec::new(),
            cap,
            default_interval: Duration::from_secs(60),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, entity: &EntityId) -> Option<&Entry> {
        self.entries.iter().find(|e| &e.entity == entity)
    }

    /// Register an entity. The entity does this, not the operator: it is
    /// the entity announcing itself, and the registry believing an
    /// attested announcement.
    pub fn register(
        &mut self,
        entity: EntityId,
        label: &str,
        now: Instant,
    ) -> Result<(), RegistryError> {
        if self.entries.len() >= self.cap {
            return Err(RegistryError::Full);
        }
        if self.get(&entity).is_some() {
            return Err(RegistryError::AlreadyRegistered);
        }
        self.entries.push(Entry {
            entity,
            label: alloc::string::ToString::to_string(label),
            registered_at: now,
            last_report: None,
            interval: self.default_interval,
            reports: 0,
            retired: false,
            bodies: Vec::new(),
        });
        Ok(())
    }

    /// Record a report.
    pub fn report(&mut self, entity: &EntityId, r: Report) -> Result<(), RegistryError> {
        let e = self
            .entries
            .iter_mut()
            .find(|e| &e.entity == entity)
            .ok_or(RegistryError::NotFound)?;
        if e.retired {
            return Err(RegistryError::NotFound);
        }
        e.last_report = Some(r);
        e.reports = e.reports.saturating_add(1);
        Ok(())
    }

    pub fn note_body(&mut self, entity: &EntityId, body: BodyId) -> Result<(), RegistryError> {
        let e = self
            .entries
            .iter_mut()
            .find(|e| &e.entity == entity)
            .ok_or(RegistryError::NotFound)?;
        if !e.bodies.contains(&body) {
            e.bodies.push(body);
        }
        Ok(())
    }

    /// Retire an entity. This does *not* deactivate it — the registry
    /// has no such power. It removes it from the directory, and the
    /// entity carries on being itself, wherever it is.
    pub fn retire(&mut self, entity: &EntityId) -> Result<(), RegistryError> {
        let e = self
            .entries
            .iter_mut()
            .find(|e| &e.entity == entity)
            .ok_or(RegistryError::NotFound)?;
        e.retired = true;
        Ok(())
    }

    /// What needs a human.
    pub fn needs_attention(&self, now: Instant) -> Vec<&Entry> {
        self.entries
            .iter()
            .filter(|e| e.health(now).needs_attention())
            .collect()
    }

    pub fn count_by_health(&self, now: Instant, h: Health) -> usize {
        self.entries.iter().filter(|e| e.health(now) == h).count()
    }

    /// Fleet-level numbers.
    pub fn fleet(&self, now: Instant) -> Fleet {
        let live: Vec<&Entry> = self.entries.iter().filter(|e| !e.retired).collect();
        let reporting: Vec<&Entry> = live
            .iter()
            .copied()
            .filter(|e| e.last_report.is_some())
            .collect();

        let mut total_bodies = 0u64;
        let mut competence_sum = 0u64;
        let mut skills = 0u64;
        let mut stops = 0u64;
        let mut oldest = 0u64;
        for e in &reporting {
            let r = e.last_report.expect("filtered");
            total_bodies += r.bodies as u64;
            competence_sum += r.competence as u64;
            skills += r.skills as u64;
            stops += r.stops as u64;
            oldest = oldest.max(r.age);
        }

        Fleet {
            entities: live.len(),
            reporting: reporting.len(),
            bodies: total_bodies,
            mean_competence: if reporting.is_empty() {
                0
            } else {
                (competence_sum / reporting.len() as u64) as u8
            },
            total_skills: skills,
            stops_since_last_report: stops,
            oldest_entity_ticks: oldest,
            unwell: self.needs_attention(now).len(),
        }
    }

    /// The moat's measurement at fleet scale: do older entities have
    /// more practiced skills than younger ones?
    ///
    /// Returns `(young_mean, old_mean)` over the youngest and oldest
    /// thirds, or `None` when there is not enough of a spread in age to
    /// say anything. The honest answer is `None` far more often than a
    /// dashboard would like.
    pub fn age_vs_competence(&self) -> Option<(u8, u8)> {
        let mut with_age: Vec<(u64, u8)> = self
            .entries
            .iter()
            .filter(|e| !e.retired)
            .filter_map(|e| e.last_report.map(|r| (r.age, r.competence)))
            .collect();
        if with_age.len() < 6 {
            return None;
        }
        with_age.sort_by_key(|(age, _)| *age);
        // a fleet where everything is the same age tells you nothing
        // about age
        if with_age.last()?.0 < with_age.first()?.0.saturating_mul(2) {
            return None;
        }
        let third = with_age.len() / 3;
        let young: u64 = with_age[..third].iter().map(|(_, c)| *c as u64).sum();
        let old: u64 = with_age[with_age.len() - third..]
            .iter()
            .map(|(_, c)| *c as u64)
            .sum();
        Some((
            (young / third as u64) as u8,
            (old / third as u64) as u8,
        ))
    }
}

/// The fleet, in numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Fleet {
    pub entities: usize,
    pub reporting: usize,
    pub bodies: u64,
    pub mean_competence: u8,
    pub total_skills: u64,
    pub stops_since_last_report: u64,
    pub oldest_entity_ticks: u64,
    pub unwell: usize,
}

impl Fleet {
    pub fn describe(&self) -> Text<240> {
        let mut t = Text::new();
        t.push_u64(self.entities as u64);
        t.push_str(" entities, ");
        t.push_u64(self.bodies);
        t.push_str(" bodies, mean competence ");
        t.push_u64(self.mean_competence as u64);
        t.push_str("%, ");
        t.push_u64(self.total_skills);
        t.push_str(" practiced skills");
        if self.unwell > 0 {
            t.push_str(" · ");
            t.push_u64(self.unwell as u64);
            t.push_str(" need attention");
        }
        t
    }
}
