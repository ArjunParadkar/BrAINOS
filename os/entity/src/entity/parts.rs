//! BFS  LEVEL    Stage 4 Entity - one Key, one state graph, every body at once  [crate brainos-entity]
//! BFS  PURPOSE  The entity's side of the part model: acquire a part through section 8, feel its readings, remember what matters, track its health
//! BFS  DEFINES  ComputeOffer, Wiring, PartError, PartRecord, part_identity
//! BFS  USES     this crate: Entity | brainos: abi, body, cognition, crypto, memory, part
//! BFS  USED BY  entity/entity
//!
//! Parts, as the entity holds them.
//!
//! `brainos-part` defines what a part is and what a nerve does. This is
//! where those meet the entity, and it is the only place they can,
//! because three things happen here that nothing below the entity is
//! allowed to do:
//!
//! 1. **Acquire.** `acquire_part` runs the same §8 steps as
//!    `Entity::acquire`, with the part's facets in its identity, and
//!    binds a `PartPolicy` in the gate.
//! 2. **Feel.** `feel` takes what a nerve read and (a) lets every bound
//!    policy hear it, which is what makes the cross-part vitals guard
//!    work, (b) asks the interoceptor whether it is worth remembering,
//!    and writes it to the state graph if so — with the entity's own
//!    `MemoryHandle`, the part as provenance — and (c) withdraws a
//!    Compute part from placement when its vitals say it is unwell.
//! 3. **Notice a changed part.** `still_the_same_part` recomputes the
//!    identity from what the part says *now*; if it differs, the part
//!    is released, and must be acquired again as the new part it is.
//!
//! A child module of `entity` rather than a sibling, because it needs
//! the entity's private key, handle and random generator, and those
//! stay private.

use super::Entity;
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::ids::{BodyId, RegionId, SessionId};
use brainos_abi::time::Instant;
use brainos_abi::verb::{Namespace, Verb};
use brainos_body::acquire::{part_digest, Acquisition, StepError};
use brainos_body::calibration::Calibration;
use brainos_body::region::RegionClass;
use brainos_body::vital::{Felt, Health};
use brainos_cognition::placement::Site;
use brainos_cognition::resource::{Resource, Tier};
use brainos_crypto::key::{body_identity, Challenge, PublicKey, Signature};
use brainos_memory::episode::Episode;
use brainos_memory::graph::NodeKind;
use brainos_memory::index::Index;
use brainos_memory::provenance::Provenance;
use brainos_part::intero::Noted;
use brainos_part::nerve::Sensed;
use brainos_part::policy::{Guard, Meter, PartPolicy};
use brainos_part::schema::{PartSchema, SchemaFault};

/// How a Compute part is offered to cognition as somewhere to think.
#[derive(Clone, Debug)]
pub struct ComputeOffer {
    /// the resource's name in the pool
    pub name: &'static str,
    pub tier: Tier,
    pub site: Site,
    /// which verbs work placed here may propose
    pub scope: Namespace,
    pub about: &'static str,
    /// cost per invocation; `None` takes the tier's default
    pub cost: Option<u32>,
}

/// What whoever wires the machine knows that the part does not.
#[derive(Clone, Debug, Default)]
pub struct Wiring {
    /// what this part's verbs must not do to other parts
    pub guards: Vec<Guard>,
    /// which of its verbs spend something finite
    pub meters: Vec<Meter>,
    /// which of its verbs relieve it when it is critical
    pub relief: Vec<Verb>,
    /// for a Compute part: how it is offered for placement
    pub compute: Option<ComputeOffer>,
}

/// Why a part was not acquired.
#[derive(Clone, Debug, PartialEq)]
pub enum PartError {
    /// what it declared is not internally consistent
    Schema(SchemaFault),
    /// §8 refused it
    Step(StepError),
}

/// One part the entity currently has.
pub struct PartRecord {
    pub id: BodyId,
    pub key: PublicKey,
    pub schema: PartSchema,
    /// the placement resource it backs, if it is a Compute part
    pub resource: Option<&'static str>,
    /// is it in the body map, or only felt? A part with no usable verb
    /// is felt and never commanded.
    pub commandable: bool,
    /// the latest judgement of each vital. One entry per vital, so
    /// bounded by the schema; the current state, not a history.
    standing: Vec<(usize, Health)>,
}

impl PartRecord {
    /// The worst of this part's vitals. A vital that has not reported
    /// yet does not count against it here — this is what cognition is
    /// told, and "unknown" is reported as such by `is_known`.
    pub fn health(&self) -> Health {
        self.standing
            .iter()
            .map(|(_, h)| *h)
            .max()
            .unwrap_or(Health::Nominal)
    }

    /// Has every vital reported at least once?
    pub fn is_known(&self) -> bool {
        self.standing.len() == self.schema.vitals().len()
    }
}

/// The identity a part will have, from its key and what it declares.
/// The same computation §8 does, exposed so a reconnecting part can be
/// compared with the one that was acquired.
pub fn part_identity(key: &PublicKey, schema: &PartSchema) -> BodyId {
    body_identity(key, &part_digest(&schema.lowered().digest(), &schema.facets()))
}

impl Entity {
    /// Acquire a part.
    ///
    /// `answer` is the part proving it holds `key` (a nerve's
    /// `Nerve::answer`). A part with usable verbs goes through all five
    /// §8 steps and gets a `PartPolicy` in the gate. A part with none —
    /// a sensor, or a part that declared no limits — is attested and
    /// felt, and is never in the body map, so no verb can be authorized
    /// through it.
    pub fn acquire_part(
        &mut self,
        key: PublicKey,
        answer: impl FnOnce(&Challenge) -> Signature,
        schema: PartSchema,
        wiring: Wiring,
    ) -> Result<BodyId, PartError> {
        schema.check().map_err(PartError::Schema)?;

        let session = SessionId::from_bytes(self.rng.nonce32());
        let mut acq = Acquisition::discover(self.id(), session);
        acq.handshake(
            &self.key,
            &mut self.rng,
            key,
            brainos_abi::version::ABI,
            answer,
            |_, _| true,
        )
        .map_err(PartError::Step)?;

        let lowered = schema.lowered();
        let id = part_identity(&key, &schema);
        if self.parts.iter().any(|p| p.id == id) {
            return Err(PartError::Step(StepError::AlreadyBound));
        }
        let commandable = !lowered.is_empty();

        if commandable {
            acq.describe_part(lowered.clone(), &schema.facets())
                .map_err(PartError::Step)?;
            let mut pending: Vec<RegionId> = Vec::new();
            for _ in 0..RegionClass::ALL.len() {
                pending.push(self.next_region_id());
            }
            let mut it = pending.into_iter();
            acq.incorporate(&mut self.bodies, move || it.next().unwrap_or(RegionId::ZERO))
                .map_err(PartError::Step)?;
            debug_assert_eq!(acq.body, Some(id));
            // a part has no joints to probe; its "calibration" is the
            // empty one, recorded so §8 reaches its last step honestly
            acq.calibrate(Calibration::new(id, self.tick), 0, |_| true)
                .map_err(PartError::Step)?;

            let mut policy = PartPolicy::new(id, schema.clone());
            for g in wiring.guards {
                policy = policy.guarding(g);
            }
            for m in wiring.meters {
                policy = policy.metering(m);
            }
            for v in wiring.relief {
                policy = policy.relieved_by(v);
            }
            self.kira.bind(id, policy.boxed(), lowered);
        }

        let resource = match (&wiring.compute, schema.substrate) {
            (Some(offer), Some(substrate)) => {
                let mut r = Resource::new(offer.name, offer.tier, offer.site)
                    .on(substrate)
                    .able_to(offer.scope.clone(), offer.about);
                if let Some(c) = offer.cost {
                    r = r.costing(c);
                }
                self.resources.add(r);
                Some(offer.name)
            }
            _ => None,
        };

        let mut join = String::from("part joined: ");
        join.push_str(&schema.describe());
        if !commandable {
            join.push_str(" (felt only)");
        }
        self.graph.add(
            &self.handle,
            NodeKind::Body,
            &join,
            100,
            Provenance::from_body(id, self.tick),
        );
        self.life.saw_body(id);
        self.index = Index::build(&self.graph);

        self.parts.push(PartRecord {
            id,
            key,
            schema,
            resource,
            commandable,
            standing: Vec::new(),
        });
        Ok(id)
    }

    pub fn part(&self, id: &BodyId) -> Option<&PartRecord> {
        self.parts.iter().find(|p| p.id == *id)
    }

    /// What a nerve read from one part. Returns how many memories were
    /// written.
    pub fn feel(&mut self, part: &BodyId, sensed: &[Sensed], at: Instant) -> usize {
        let Some(pi) = self.parts.iter().position(|p| p.id == *part) else {
            return 0;
        };
        let bound: Vec<BodyId> = self.kira.bound_bodies().into_iter().map(|(b, _)| b).collect();
        let mut notes: Vec<Noted> = Vec::new();

        for s in sensed {
            let schema = &self.parts[pi].schema;
            let name = s.sense.as_str();
            // a value outside the sense's plausible range is a broken
            // sensor: pass it on marked stale, so nothing trusts it
            let plausible = schema
                .sense_named(name)
                .map(|spec| spec.is_plausible(s.value))
                .unwrap_or(false);
            let mut f = Felt::new(*part, name, s.value, at);
            f.fresh = s.fresh && plausible;

            // (a) every bound policy hears it
            for b in &bound {
                if let Some(p) = self.kira.policy_for_mut(b) {
                    p.feel(&f);
                }
            }

            // (b) is it worth remembering?
            let health = schema.judge(name, s.value).map(|h| if f.fresh { h } else { Health::Failed });
            let unit = schema.sense_named(name).map(|x| x.quantity.unit()).unwrap_or("");
            let label = alloc::string::ToString::to_string(schema.label());
            if let Some(n) = self.intero.hear(&f, health, &label, unit) {
                notes.push(n);
            }

            // the part's standing, one entry per vital
            if let Some(h) = health {
                let vi = schema
                    .vitals()
                    .iter()
                    .position(|v| v.sense.as_str() == name)
                    .unwrap_or(0);
                let standing = &mut self.parts[pi].standing;
                match standing.iter_mut().find(|(i, _)| *i == vi) {
                    Some(slot) => slot.1 = h,
                    None => standing.push((vi, h)),
                }
            }
        }

        // (c) a Compute part that is unwell stops being offered
        if let Some(name) = self.parts[pi].resource {
            let usable = self.parts[pi].health().is_usable();
            if self.resources.set_available(name, usable) {
                let mut text = String::from("placement: ");
                text.push_str(name);
                text.push_str(if usable {
                    " is offered again, its vitals recovered"
                } else {
                    " is withdrawn, its vitals are past a critical limit"
                });
                let node = Episode::observed(&text, self.tick).record(
                    &self.handle,
                    &mut self.graph,
                    Provenance::from_body(*part, self.tick),
                );
                self.graph.set_salience(&self.handle, node, 80);
            }
        }

        self.remember_notes(notes)
    }

    /// Close the interoceptor's windows and remember the summaries.
    pub fn summarise_parts(&mut self, now: Instant) -> usize {
        let labels: Vec<(BodyId, String)> = self
            .parts
            .iter()
            .map(|p| (p.id, alloc::string::ToString::to_string(p.schema.label())))
            .collect();
        let notes = self.intero.summarise(now, |b| {
            labels
                .iter()
                .find(|(id, _)| id == b)
                .map(|(_, l)| l.clone())
                .unwrap_or_else(|| String::from("a part"))
        });
        self.remember_notes(notes)
    }

    fn remember_notes(&mut self, notes: Vec<Noted>) -> usize {
        let n = notes.len();
        for note in notes {
            let node = Episode::observed(&note.text, self.tick).record(
                &self.handle,
                &mut self.graph,
                Provenance::from_body(note.part, self.tick),
            );
            self.graph.set_salience(&self.handle, node, note.salience);
        }
        if n > 0 {
            self.index = Index::build(&self.graph);
        }
        n
    }

    /// Is the part that answers on this key, describing itself this
    /// way *now*, the part that was acquired?
    ///
    /// If not, the old part is released on the spot — its verbs leave
    /// the body map, its policy is unbound, its placement resource is
    /// withdrawn — and `false` comes back. The caller acquires the new
    /// one, which gets a new id.
    pub fn still_the_same_part(&mut self, id: &BodyId, now: &PartSchema) -> bool {
        let Some(rec) = self.part(id) else {
            return false;
        };
        if part_identity(&rec.key, now) == *id {
            return true;
        }
        self.release_part(id);
        false
    }

    /// A part leaves.
    pub fn release_part(&mut self, id: &BodyId) -> bool {
        let Some(pi) = self.parts.iter().position(|p| p.id == *id) else {
            return false;
        };
        let rec = self.parts.remove(pi);
        if let Some(name) = rec.resource {
            self.resources.set_available(name, false);
        }
        self.intero.release(id);
        if rec.commandable {
            self.release(id);
        } else {
            Episode::observed("a part left", self.tick).record(
                &self.handle,
                &mut self.graph,
                Provenance::from_body(*id, self.tick),
            );
        }
        true
    }
}
