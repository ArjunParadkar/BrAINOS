//! BFS  LEVEL    Section 8 Acquisition - a device becomes a region of the body  [crate brainos-body]
//! BFS  PURPOSE  The five-step protocol by which a device becomes a body: discover, handshake, schema, incorporate, calibrate; fixes the BodyId
//! BFS  DEFINES  Step, StepError, Acquisition, part_digest, default_envelope, acquire_native, AcquireResult
//! BFS  USES     this crate: calibration, region, schema | brainos: abi, crypto, rt
//! BFS  USED BY  blur/acquire, blur/tests/blur_suite, body/lib, body/tests/body_suite, entity/entity, host/blur_run, kira/tests/gate_suite
//!
//! The §8 acquisition protocol, as a state machine.
//!
//! Five steps, in order, no skipping. The machine exists rather than a
//! sequence of function calls because the failure modes matter: a device
//! that passes discovery and fails attestation must not reach schema
//! reading, and "incorporated but uncalibrated" must be a state the rest
//! of the system can see and refuse to act through — not a window
//! between two calls where a body is half-joined.
//!
//! ```text
//!   Discovered ──handshake──▶ Attested ──schema──▶ Described
//!        │                        │                    │
//!        └────── any failure ─────┴──────▶ Rejected ◀──┘
//!                                                       │
//!                     Incorporated ◀──incorporate───────┘
//!                          │
//!                          └──calibrate──▶ Calibrated  ← only here may
//!                                                        a consequential
//!                                                        verb be granted
//! ```
//!
//! The asymmetry at the end is the safety property: reaching
//! `Incorporated` gives the entity a limb it can *see*; reaching
//! `Calibrated` gives it a limb it may *move*.

use crate::calibration::Calibration;
use crate::region::{BodyMap, Region, RegionClass};
use crate::schema::CapabilitySchema;
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::error::Result;
use brainos_abi::ids::{BodyId, EntityId, RegionId, SessionId};
use brainos_abi::time::Tick;
use brainos_abi::version::{AbiVersion, ABI};
use brainos_crypto::key::{body_identity, BrainKey, Challenge, ChallengePurpose, PublicKey, Signature};
use brainos_crypto::rng::Drbg;
use brainos_crypto::sha256::{Hash, Sha256};
use brainos_crypto::domain::Tag;

/// Where a device is in the protocol.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// 1 — found, nothing believed about it yet
    Discovered,
    /// 2 — both sides proved who they are
    Attested,
    /// 3 — its capabilities are known and its identity is therefore fixed
    Described,
    /// 4 — a policy is bound; the entity has a limb
    Incorporated,
    /// 5 — this specific body is known; the limb may move
    Calibrated,
    /// failed somewhere; carries where
    Rejected,
}

impl Step {
    pub const fn name(self) -> &'static str {
        match self {
            Step::Discovered => "discovered",
            Step::Attested => "attested",
            Step::Described => "described",
            Step::Incorporated => "incorporated",
            Step::Calibrated => "calibrated",
            Step::Rejected => "rejected",
        }
    }

    /// May a consequential verb be granted through a body at this step?
    pub const fn may_act_consequentially(self) -> bool {
        matches!(self, Step::Calibrated)
    }
}

/// Why acquisition failed. Each is a different thing to tell the human.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StepError {
    /// the step was called out of order
    OutOfOrder { at: Step },
    /// the device speaks an ABI we will not accept
    Incompatible { ours: AbiVersion, theirs: AbiVersion },
    /// the device could not answer our challenge
    DeviceUnattested,
    /// the device rejected *our* attestation — it will not be inhabited
    /// by this entity, which is its right and is not an error in us
    RefusedByDevice,
    /// the schema was empty or declared nothing actionable
    NothingToDo,
    /// the schema declares a commandable verb on a sense-only region
    SchemaContradictsRegion,
    /// this body is already bound
    AlreadyBound,
    /// calibration was incomplete or insane
    CalibrationInvalid,
    /// the device's identity changed mid-protocol
    IdentityShifted,
    /// the parts the device reports (its own census of what it is made
    /// of) are not the parts the entity recorded for this body — a
    /// swapped, duplicated or unknown component. Raised by an adapter
    /// during the handshake, through `reject`.
    PartsUnrecognised,
    /// the schema declares a part the body does not physically have —
    /// a phantom limb — or omits one the body type requires. Raised by an
    /// adapter at the schema step, through `reject`.
    SchemaContradictsBody,
}

/// One device going through the protocol.
pub struct Acquisition {
    pub step: Step,
    pub session: SessionId,
    /// the entity doing the acquiring
    pub entity: EntityId,
    /// what the device says its attestation key is
    pub device_key: Option<PublicKey>,
    pub schema: Option<CapabilitySchema>,
    /// fixed at step 3, once the schema is known
    pub body: Option<BodyId>,
    pub calibration: Option<Calibration>,
    pub failure: Option<StepError>,
    /// the nonce we challenged with, kept so an answer can be checked
    challenge: Option<Challenge>,
    /// the running transcript hash of everything exchanged, so a
    /// man-in-the-middle that reorders or substitutes a step is caught
    /// at incorporation rather than never
    transcript: [u8; 32],
}

impl Acquisition {
    /// Step 1 — discovery. Nothing is believed yet; a device at this
    /// step has told us only that it exists.
    pub fn discover(entity: EntityId, session: SessionId) -> Acquisition {
        let mut h = Sha256::tagged(Tag::Session);
        h.field(entity.as_bytes());
        h.field(session.as_bytes());
        Acquisition {
            step: Step::Discovered,
            session,
            entity,
            device_key: None,
            schema: None,
            body: None,
            calibration: None,
            failure: None,
            challenge: None,
            transcript: *h.finish().as_bytes(),
        }
    }

    fn absorb(&mut self, label: &str, data: &[u8]) {
        let mut h = Sha256::tagged(Tag::Session);
        h.field(&self.transcript);
        h.field(label.as_bytes());
        h.field(data);
        self.transcript = *h.finish().as_bytes();
    }

    pub fn transcript(&self) -> [u8; 32] {
        self.transcript
    }

    /// Reject at the current step for a reason only an adapter can see
    /// (the body's own census, a probe that went the wrong way). The
    /// machine stays the single place a body's standing is recorded:
    /// an adapter that finds a problem says so here rather than keeping
    /// a private verdict next to an `Acquisition` that still looks fine.
    pub fn reject(&mut self, e: StepError) -> StepError {
        let _ = self.fail::<()>(e);
        e
    }

    /// Absorb adapter evidence (a signed census, a probe log digest) into
    /// the transcript, so a later step is bound to what was seen.
    pub fn witness(&mut self, label: &str, data: &[u8]) {
        if self.step != Step::Rejected {
            self.absorb(label, data);
        }
    }

    /// Record a failure and return it. Generic in the success type so
    /// every step can `return self.fail(..)` without a turbofish — the
    /// value is never produced, so `T` is free.
    fn fail<T>(&mut self, e: StepError) -> core::result::Result<T, StepError> {
        self.step = Step::Rejected;
        self.failure = Some(e);
        Err(e)
    }

    /// Step 2 — handshake. Mutual, and in that order: we challenge the
    /// device first, and only then does the device get to challenge us.
    ///
    /// Mutual attestation is not symmetry for its own sake. A body that
    /// cannot check *who is inhabiting it* can be taken over by anything
    /// on the same bus, and "the robot is now being driven by someone
    /// else's entity" is a worse failure than "the entity was fooled
    /// about which robot it has".
    pub fn handshake(
        &mut self,
        key: &BrainKey,
        rng: &mut Drbg,
        device_key: PublicKey,
        device_abi: AbiVersion,
        answer: impl FnOnce(&Challenge) -> Signature,
        device_accepts: impl FnOnce(&PublicKey, &Signature) -> bool,
    ) -> core::result::Result<(), StepError> {
        if self.step != Step::Discovered {
            return self.fail(StepError::OutOfOrder { at: self.step });
        }
        if !ABI.accepts(&device_abi) {
            return self.fail(StepError::Incompatible {
                ours: ABI,
                theirs: device_abi,
            });
        }

        // we challenge them
        let c = Challenge::new(
            rng.nonce32(),
            ChallengePurpose::BodyHandshake,
            self.entity,
        );
        let sig = answer(&c);
        if !c.check(&device_key, &sig) {
            return self.fail(StepError::DeviceUnattested);
        }
        self.absorb("device", &device_key.to_bytes());

        // they challenge us: we sign the transcript so far, which binds
        // our answer to this exchange and to no other
        let ours = key.sign(Tag::Attestation, &self.transcript);
        if !device_accepts(&key.public(), &ours) {
            return self.fail(StepError::RefusedByDevice);
        }
        self.absorb("entity", &key.public().to_bytes());

        self.device_key = Some(device_key);
        self.challenge = Some(c);
        self.step = Step::Attested;
        Ok(())
    }

    /// Step 3 — read the capability schema. This is where the body's
    /// identity is *fixed*: `BodyId` is the hash of the attested key and
    /// the schema digest, so from here on, a device that changes what it
    /// claims to do is a different body.
    pub fn describe(&mut self, schema: CapabilitySchema) -> core::result::Result<(), StepError> {
        self.describe_with(schema, None)
    }

    /// Step 3 for a *part*: the same step, with more in the identity.
    ///
    /// A part declares more than verbs — a class, senses, vitals and
    /// their limits (`brainos-part`). `facets` is the digest of those.
    /// It is mixed into the identity so that a part which comes back
    /// with the same verbs but different declared limits — a fan that
    /// now says its processor may run 20 °C hotter — is a different
    /// part and must be acquired again, for the same reason a changed
    /// verb makes a different body.
    ///
    /// `describe` is this with no facets, and produces exactly the
    /// identity it always did.
    pub fn describe_part(
        &mut self,
        schema: CapabilitySchema,
        facets: &Hash,
    ) -> core::result::Result<(), StepError> {
        self.describe_with(schema, Some(facets))
    }

    fn describe_with(
        &mut self,
        schema: CapabilitySchema,
        facets: Option<&Hash>,
    ) -> core::result::Result<(), StepError> {
        if self.step != Step::Attested {
            return self.fail(StepError::OutOfOrder { at: self.step });
        }
        if schema.is_empty() {
            return self.fail(StepError::NothingToDo);
        }
        // a verb declared on a sense-only region is a schema that does
        // not describe a real device, and acting on it would mean
        // commanding a camera to walk
        if schema
            .verbs()
            .iter()
            .any(|v| v.region.is_afferent_only())
        {
            return self.fail(StepError::SchemaContradictsRegion);
        }

        let Some(dk) = self.device_key else {
            return self.fail(StepError::DeviceUnattested);
        };
        let digest = match facets {
            None => schema.digest(),
            Some(f) => part_digest(&schema.digest(), f),
        };
        self.absorb("schema", digest.as_bytes());
        self.body = Some(body_identity(&dk, &digest));
        self.schema = Some(schema);
        self.step = Step::Described;
        Ok(())
    }

    /// Step 4 — incorporate. The body joins the body map; the entity now
    /// has a limb it can see. A policy must be bound by the caller at the
    /// same moment (KIRA refuses through a body with no policy), which is
    /// why this returns the regions rather than doing the binding itself:
    /// policy binding lives in the gate, and the gate is not this crate.
    pub fn incorporate(
        &mut self,
        map: &mut BodyMap,
        mut next_region_id: impl FnMut() -> RegionId,
    ) -> core::result::Result<Vec<RegionId>, StepError> {
        if self.step != Step::Described {
            return self.fail(StepError::OutOfOrder { at: self.step });
        }
        let (Some(body), Some(schema)) = (self.body, self.schema.clone()) else {
            return self.fail(StepError::OutOfOrder { at: self.step });
        };
        if map.knows_body(&body) {
            return self.fail(StepError::AlreadyBound);
        }

        // one region per class the schema mentions; every verb of that
        // class is advertised on it
        let mut regions: Vec<Region> = Vec::new();
        let mut ids: Vec<RegionId> = Vec::new();
        for class in schema.regions() {
            let id = next_region_id();
            let mut label = String::new();
            label.push_str(&schema.label);
            label.push('/');
            label.push_str(class.name());
            let mut r = Region::new(id, body, class, &label);
            for v in schema.verbs().iter().filter(|v| v.region == class) {
                // a verb the body would not let us use is not advertised:
                // an unbounded parameter means every call fails validate,
                // and advertising it would make the entity believe it has
                // a capability it does not
                if v.is_actionable() {
                    r.advertise(v.verb);
                }
            }
            if r.capabilities().is_empty() {
                continue;
            }
            ids.push(id);
            regions.push(r);
        }

        if regions.is_empty() {
            return self.fail(StepError::NothingToDo);
        }

        let label = schema.label.clone();
        if map.incorporate(body, &label, schema, regions).is_err() {
            return self.fail(StepError::AlreadyBound);
        }
        self.step = Step::Incorporated;
        Ok(ids)
    }

    /// Step 5 — calibrate. The calibration comes from the entity's state
    /// graph if this body has been inhabited before, or from probing if
    /// not. Either way it is the *entity's* record, pushed down.
    ///
    /// The `push` closure is what sends it to the body and returns
    /// whether the body acknowledged. Until it does, the body stays at
    /// step 4 and consequential verbs stay refused.
    pub fn calibrate(
        &mut self,
        calibration: Calibration,
        expected_joints: usize,
        push: impl FnOnce(&Calibration) -> bool,
    ) -> core::result::Result<(), StepError> {
        if self.step != Step::Incorporated {
            return self.fail(StepError::OutOfOrder { at: self.step });
        }
        let Some(body) = self.body else {
            return self.fail(StepError::OutOfOrder { at: self.step });
        };
        if calibration.body != body {
            return self.fail(StepError::IdentityShifted);
        }
        if !calibration.is_complete(expected_joints) {
            return self.fail(StepError::CalibrationInvalid);
        }
        if calibration.joints.iter().any(|j| !j.is_sane()) {
            return self.fail(StepError::CalibrationInvalid);
        }
        if !push(&calibration) {
            return self.fail(StepError::CalibrationInvalid);
        }
        self.absorb("calibration", &(calibration.joints.len() as u64).to_le_bytes());
        self.calibration = Some(calibration);
        self.step = Step::Calibrated;
        Ok(())
    }

    /// The line written to the state graph when a body joins.
    pub fn render_join(&self, at: Tick) -> String {
        let mut s = String::new();
        s.push_str("body ");
        match self.body {
            Some(b) => s.push_str(b.fingerprint().as_str()),
            None => s.push_str("unidentified"),
        }
        s.push(' ');
        s.push_str(self.step.name());
        if let Some(sc) = &self.schema {
            s.push_str(" via ");
            s.push_str(&sc.adapter);
            s.push_str(" verbs=");
            let mut n = sc.len();
            let mut buf = [0u8; 8];
            let mut i = 0;
            if n == 0 {
                buf[0] = b'0';
                i = 1;
            }
            while n > 0 {
                buf[i] = b'0' + (n % 10) as u8;
                n /= 10;
                i += 1;
            }
            while i > 0 {
                i -= 1;
                s.push(buf[i] as char);
            }
        }
        s.push_str(" @t");
        let mut t = at.0;
        let mut buf = [0u8; 20];
        let mut i = 0;
        if t == 0 {
            buf[0] = b'0';
            i = 1;
        }
        while t > 0 {
            buf[i] = b'0' + (t % 10) as u8;
            t /= 10;
            i += 1;
        }
        while i > 0 {
            i -= 1;
            s.push(buf[i] as char);
        }
        s
    }
}

/// The digest a part's identity is taken over: its capability schema
/// and its facets together. Public so that anything holding a part's
/// attestation key and its full description can recompute its id
/// without running an acquisition.
pub fn part_digest(schema_digest: &Hash, facets: &Hash) -> Hash {
    let mut h = Sha256::tagged(Tag::Schema);
    h.field(b"part");
    h.field(schema_digest.as_bytes());
    h.field(facets.as_bytes());
    h.finish()
}

/// Build the default envelope for a region class from a schema.
///
/// Every consequential region gets an envelope whether or not the body
/// offered numbers, and a body that offered none gets one that permits
/// nothing. That is a body the entity can see and cannot move, which is
/// the correct state for a device that would not say what it can
/// survive.
///
/// **The envelope is the widest any verb declared, not the tightest.**
/// This was the other way round until an end-to-end run caught it. The
/// reasoning that produced the intersection — "a grant wide enough for
/// the loosest verb is wider than the strictest verb was ever checked
/// against" — sounds right and is wrong, because it confuses two
/// different jobs:
///
/// * **Validate** enforces what *this verb* may do, against the verb's
///   own declared parameter bounds. That check already exists and runs
///   on every proposal.
/// * **The envelope** bounds what *this region* may ever physically do.
///   It is what Domain 2 runs inside at kilohertz, long after any
///   particular verb was authorized.
///
/// Take the intersection and a region with a fast verb and a slow one
/// can never perform the fast one: an arm whose `home` is limited to
/// 60 °/s would cap `move_joint` at 60 °/s too, even though the
/// manufacturer declared 180. Take the union and the fast verb works,
/// the slow verb is still held to its own limit at validate, and the
/// body's own `native_envelope` narrows the whole thing again on the
/// other side of the wire. Three checks, each doing one job.
pub fn default_envelope(
    schema: &CapabilitySchema,
    class: RegionClass,
    id: u16,
    period: brainos_abi::time::Duration,
    link_grace: brainos_abi::time::Duration,
) -> brainos_rt::envelope::Envelope {
    use brainos_rt::envelope::{Envelope, Limit};
    let mut env = Envelope::new(id, period, link_grace);
    for v in schema.verbs().iter().filter(|v| v.region == class) {
        for p in &v.params {
            if let (Some(ch), true) = (p.channel, p.bounded) {
                let l = Limit::new(p.min as f32, p.max as f32);
                if l.is_sane() {
                    // several verbs may bound the same channel; the
                    // envelope keeps the *union* (see the note above:
                    // each verb is still held to its own bounds at
                    // validate)
                    match env.limit(ch) {
                        None => {
                            env.bound(ch, l);
                        }
                        Some(existing) => {
                            let wider =
                                Limit::new(existing.min.min(l.min), existing.max.max(l.max));
                            if wider.is_sane() {
                                env.bound(ch, wider);
                            }
                        }
                    }
                }
            }
        }
    }
    env
}

/// A convenience for adapters: the whole protocol when the device is
/// already trusted and local (our own firmware over a wired link).
pub fn acquire_native(
    entity: EntityId,
    session: SessionId,
    key: &BrainKey,
    rng: &mut Drbg,
    device: &BrainKey,
    schema: CapabilitySchema,
    map: &mut BodyMap,
    calibration: Calibration,
    expected_joints: usize,
    next_region_id: impl FnMut() -> RegionId,
) -> core::result::Result<(BodyId, Vec<RegionId>), StepError> {
    let mut a = Acquisition::discover(entity, session);
    a.handshake(
        key,
        rng,
        device.public(),
        ABI,
        |c| device.answer(c),
        |_, _| true,
    )?;
    a.describe(schema)?;
    let regions = a.incorporate(map, next_region_id)?;
    let body = a.body.expect("set at describe");
    let mut cal = calibration;
    cal.body = body;
    a.calibrate(cal, expected_joints, |_| true)?;
    Ok((body, regions))
}

/// Result alias for adapters that also do I/O.
pub type AcquireResult<T> = Result<T>;
