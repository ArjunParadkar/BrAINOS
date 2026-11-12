//! BFS  LEVEL    Parts and nerves - every piece of hardware as a body part  [crate brainos-part]
//! BFS  PURPOSE  EnvelopePack: a signed profile of limits (from a human or a vendor datasheet) attached to a part at acquisition, so a part that states none can be used
//! BFS  DEFINES  AppliesTo, Source, Limit, PackError, EnvelopePack
//! BFS  USES     this crate: class, schema | brainos: abi, body, crypto
//! BFS  USED BY  entity/entity/parts, host/nerves/mod, host/nerves/packfile, host/tests/nerves_live, part/lib
//!
//! Envelope packs — limits a part does not state for itself.
//!
//! The live survey found that most hardware says nothing about what it
//! can survive: a fan reports its speed and no maximum, a CPU sensor
//! reports a temperature and no threshold. The rule that a part with
//! no limits gets no verbs is right, and it leaves the entity able to
//! feel those parts and never use them.
//!
//! The limits exist. They are in a datasheet, or in the head of
//! whoever built the machine. An `EnvelopePack` is how they get in: a
//! small **signed** document that says, for one part or one model of
//! part, "this sense is degraded at X and critical at Y".
//!
//! ```text
//!   EnvelopePack
//!     applies to   this exact part (by its key)  |  every part with this label
//!     limits       sense, kind, warn, critical, which direction is bad
//!     source       a human said so  |  a vendor's datasheet
//!     reference    free text: "HP Omen 16 service manual, p. 41"
//!     issuer       whose statement this is (a public key)
//!     signature    by the issuer
//!     approval     by the entity's own Key, when the issuer is someone else
//! ```
//!
//! # Who may say what a part's limits are
//!
//! A pack is accepted only if the entity's Key has signed it — directly
//! (the owner wrote it) or as an approval over a vendor's signature
//! (the owner accepted the vendor's). A vendor's signature alone is
//! not enough. Limits decide what the gate will permit; nobody but the
//! owner gets to widen what an entity may do to its own body. This is
//! the same rule the skill marketplace uses for a package.
//!
//! # What a pack cannot do
//!
//! * **Loosen a limit the hardware stated.** If a drive says it is
//!   critical at 85 °C, a pack may say 80 and may not say 90. A pack
//!   fills silence and tightens; it never overrules the part.
//! * **Invent a sense.** A limit must name a sense the part reports.
//! * **Go unnoticed.** The limits become part of the part's schema,
//!   and the schema is hashed into the part's identity. A part acquired
//!   with a pack and the same part acquired without it are different
//!   parts, and swapping the pack means acquiring again. The pack's
//!   issuer and reference are written into the entity's memory when
//!   the part joins.
//!
//! A pack does not make a number true. It makes it *attributable*: the
//! gate enforces the limit, and the memory says who vouched for it.

use crate::schema::{PartSchema, VitalKind, VitalSpec};
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::text::Text;
use brainos_body::vital::{Band, SENSE_NAME_MAX};
use brainos_crypto::domain::Tag;
use brainos_crypto::key::{BrainKey, PublicKey, Signature};
use brainos_crypto::sha256::{Hash, Sha256};

/// Which parts a pack speaks for.
#[derive(Clone, Copy, Debug)]
pub enum AppliesTo {
    /// exactly one part, by the key it attests with
    Part(PublicKey),
    /// every part that describes itself with this label — a model
    Label(Text<48>),
}

/// Where the numbers came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Source {
    /// a person who knows the machine
    Human = 0,
    /// a manufacturer's published figures
    Vendor = 1,
}

impl Source {
    pub const fn name(self) -> &'static str {
        match self {
            Source::Human => "a human",
            Source::Vendor => "a vendor datasheet",
        }
    }
}

/// One limit.
#[derive(Clone, Copy, Debug)]
pub struct Limit {
    pub sense: Text<SENSE_NAME_MAX>,
    pub kind: VitalKind,
    pub band: Band,
}

/// Why a pack was not accepted or could not be applied.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PackError {
    /// it carries no signature
    Unsigned,
    /// the issuer's signature does not verify: the pack was altered,
    /// or was signed by someone else
    BadSignature,
    /// it was issued by someone other than the owner and the owner has
    /// not approved it
    NotApproved,
    /// the approval is not by this entity's Key, or does not verify
    BadApproval,
    /// it is for a different part
    WrongPart,
    /// a limit names a sense the part does not report
    NoSuchSense,
    /// a limit's thresholds are out of order or not numbers
    InsaneBand,
    /// a limit is looser than the one the hardware stated, or points
    /// the other way
    WouldLoosen,
    /// it contains no limits
    Empty,
}

/// A signed profile of limits for a part.
#[derive(Clone, Debug)]
pub struct EnvelopePack {
    pub applies_to: AppliesTo,
    pub source: Source,
    /// where the numbers are from, for a human to check
    pub reference: String,
    limits: Vec<Limit>,
    issuer: Option<PublicKey>,
    signature: Option<Signature>,
    approval: Option<Signature>,
}

impl EnvelopePack {
    pub fn new(applies_to: AppliesTo, source: Source, reference: &str) -> EnvelopePack {
        EnvelopePack {
            applies_to,
            source,
            reference: alloc::string::ToString::to_string(reference),
            limits: Vec::new(),
            issuer: None,
            signature: None,
            approval: None,
        }
    }

    pub fn for_label(label: &str, source: Source, reference: &str) -> EnvelopePack {
        EnvelopePack::new(AppliesTo::Label(Text::from_str_truncating(label)), source, reference)
    }

    /// Add a limit. Kept sorted by sense, one per sense, so the digest
    /// does not depend on the order they were written in. Adding a
    /// limit after signing discards the signatures.
    pub fn limit(mut self, sense: &str, kind: VitalKind, band: Band) -> EnvelopePack {
        self.limits.retain(|l| l.sense.as_str() != sense);
        self.limits.push(Limit {
            sense: Text::from_str_truncating(sense),
            kind,
            band,
        });
        self.limits.sort_by(|a, b| a.sense.as_str().cmp(b.sense.as_str()));
        self.issuer = None;
        self.signature = None;
        self.approval = None;
        self
    }

    pub fn limits(&self) -> &[Limit] {
        &self.limits
    }

    pub fn issuer(&self) -> Option<PublicKey> {
        self.issuer
    }

    /// The digest that is signed: everything the pack says.
    pub fn digest(&self) -> Hash {
        let mut h = Sha256::tagged(Tag::Package);
        h.field(b"envelope-pack/v1");
        match &self.applies_to {
            AppliesTo::Part(k) => {
                h.field(&[0]);
                h.field(&k.to_bytes());
            }
            AppliesTo::Label(l) => {
                h.field(&[1]);
                h.field(l.as_str().as_bytes());
            }
        }
        h.field(&[self.source as u8]);
        h.field(self.reference.as_bytes());
        h.field(&(self.limits.len() as u64).to_le_bytes());
        for l in &self.limits {
            h.field(l.sense.as_str().as_bytes());
            h.field(&[l.kind as u8, l.band.rising_is_bad as u8]);
            h.field(&l.band.warn.to_bits().to_le_bytes());
            h.field(&l.band.critical.to_bits().to_le_bytes());
        }
        h.finish()
    }

    /// The issuer signs what the pack says.
    pub fn signed_by(mut self, issuer: &BrainKey) -> EnvelopePack {
        self.signature = Some(issuer.sign(Tag::Package, self.digest().as_bytes()));
        self.issuer = Some(issuer.public());
        self.approval = None;
        self
    }

    fn approval_payload(&self) -> Option<Vec<u8>> {
        let mut p = Vec::new();
        p.extend_from_slice(b"approve-envelope-pack/v1");
        p.extend_from_slice(self.digest().as_bytes());
        p.extend_from_slice(&self.issuer?.to_bytes());
        Some(p)
    }

    /// The owner approves someone else's pack. Signs the pack's digest
    /// *and* the issuer's key, so the approval cannot be moved to a
    /// different issuer's copy of the same numbers.
    pub fn approved_by(mut self, owner: &BrainKey) -> EnvelopePack {
        if let Some(p) = self.approval_payload() {
            self.approval = Some(owner.sign(Tag::Package, &p));
        }
        self
    }

    /// Is this pack one the entity whose public key is `owner` should
    /// act on?
    pub fn verify(&self, owner: &PublicKey) -> Result<(), PackError> {
        if self.limits.is_empty() {
            return Err(PackError::Empty);
        }
        let (Some(issuer), Some(sig)) = (self.issuer, self.signature) else {
            return Err(PackError::Unsigned);
        };
        if !issuer.verify(Tag::Package, self.digest().as_bytes(), &sig) {
            return Err(PackError::BadSignature);
        }
        if issuer.to_bytes() == owner.to_bytes() {
            return Ok(());
        }
        let Some(approval) = self.approval else {
            return Err(PackError::NotApproved);
        };
        let payload = self.approval_payload().ok_or(PackError::Unsigned)?;
        if !owner.verify(Tag::Package, &payload, &approval) {
            return Err(PackError::BadApproval);
        }
        Ok(())
    }

    fn speaks_for(&self, part: &PublicKey, schema: &PartSchema) -> bool {
        match &self.applies_to {
            AppliesTo::Part(k) => k.to_bytes() == part.to_bytes(),
            AppliesTo::Label(l) => l.as_str() == schema.label(),
        }
    }

    /// Verify the pack and add its limits to what a part declared.
    ///
    /// Returns the schema the part should be acquired with. The part's
    /// own stated limits are kept unless the pack's are tighter.
    pub fn apply(
        &self,
        owner: &PublicKey,
        part: &PublicKey,
        schema: PartSchema,
    ) -> Result<PartSchema, PackError> {
        self.verify(owner)?;
        if !self.speaks_for(part, &schema) {
            return Err(PackError::WrongPart);
        }
        let mut out = schema;
        for l in &self.limits {
            let sense = l.sense.as_str();
            if out.sense_named(sense).is_none() {
                return Err(PackError::NoSuchSense);
            }
            if !l.band.is_sane() {
                return Err(PackError::InsaneBand);
            }
            if let Some(stated) = out.vital_named(sense) {
                let s = stated.band;
                let tighter = if s.rising_is_bad {
                    l.band.rising_is_bad && l.band.warn <= s.warn && l.band.critical <= s.critical
                } else {
                    !l.band.rising_is_bad && l.band.warn >= s.warn && l.band.critical >= s.critical
                };
                if !tighter {
                    return Err(PackError::WouldLoosen);
                }
            }
            out = out.vital(VitalSpec {
                sense: l.sense,
                kind: l.kind,
                band: l.band,
            });
        }
        Ok(out)
    }

    /// One line for the join record: who vouched for these limits.
    pub fn describe(&self) -> String {
        let mut s = String::from("limits from ");
        s.push_str(self.source.name());
        if let Some(i) = self.issuer {
            s.push_str(", signed by ");
            s.push_str(i.entity_id().fingerprint().as_str());
        }
        if !self.reference.is_empty() {
            s.push_str(" (");
            s.push_str(&self.reference);
            s.push(')');
        }
        s.push_str(": ");
        for (i, l) in self.limits.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            s.push_str(l.sense.as_str());
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::class::PartClass;
    use crate::schema::{Quantity, SenseSpec};
    use brainos_abi::verb::Verb;
    use brainos_body::region::RegionClass;
    use brainos_body::schema::{ParamSpec, VerbSpec};

    fn owner() -> BrainKey {
        BrainKey::from_seed(&[1; 32])
    }
    fn vendor() -> BrainKey {
        BrainKey::from_seed(&[2; 32])
    }
    fn part() -> BrainKey {
        BrainKey::from_seed(&[3; 32])
    }

    /// A fan the way sysfs presents one: a speed, a verb, no limits.
    fn silent_fan() -> PartSchema {
        PartSchema::new(PartClass::Thermal, "hwmon hp", "hwmon")
            .verb(
                VerbSpec::new(Verb::literal("fan.full"), RegionClass::Effector, "full speed")
                    .with(ParamSpec::count("on", 0, 1)),
            )
            .sense(SenseSpec::new("fan1.rpm", Quantity::Rpm, 0.0, 60_000.0))
    }

    fn pack() -> EnvelopePack {
        EnvelopePack::for_label("hwmon hp", Source::Human, "measured at full speed, 2026-11-12")
            .limit("fan1.rpm", VitalKind::Faults, Band::rising(6000.0, 7000.0))
    }

    #[test]
    fn a_pack_signed_by_the_owner_gives_a_silent_part_its_verb() {
        let fan = silent_fan();
        assert!(fan.actionable().is_empty());
        let with = pack()
            .signed_by(&owner())
            .apply(&owner().public(), &part().public(), fan.clone())
            .unwrap();
        assert_eq!(with.actionable().len(), 1);
        assert_eq!(with.check(), Ok(()));
        // and it is a different part for it
        assert_ne!(with.facets(), fan.facets());
    }

    #[test]
    fn an_unsigned_pack_and_an_altered_one_are_refused() {
        let o = owner().public();
        assert_eq!(pack().verify(&o), Err(PackError::Unsigned));

        // signed, then a limit changed: adding a limit drops the signature
        let altered = pack()
            .signed_by(&owner())
            .limit("fan1.rpm", VitalKind::Faults, Band::rising(60_000.0, 70_000.0));
        assert_eq!(altered.verify(&o), Err(PackError::Unsigned));

        // signed over one reference, presented with another
        let mut moved = pack().signed_by(&owner());
        moved.reference = String::from("trust me");
        assert_eq!(moved.verify(&o), Err(PackError::BadSignature));

        assert_eq!(
            EnvelopePack::for_label("x", Source::Human, "").signed_by(&owner()).verify(&o),
            Err(PackError::Empty)
        );
    }

    #[test]
    fn a_vendors_pack_needs_the_owners_approval_and_only_the_owners() {
        let o = owner().public();
        let vendors = pack().signed_by(&vendor());
        assert_eq!(vendors.verify(&o), Err(PackError::NotApproved));

        let approved = vendors.clone().approved_by(&owner());
        assert_eq!(approved.verify(&o), Ok(()));

        // approved by somebody who is not this entity
        let stranger = BrainKey::from_seed(&[9; 32]);
        assert_eq!(vendors.approved_by(&stranger).verify(&o), Err(PackError::BadApproval));
    }

    #[test]
    fn an_approval_cannot_be_moved_to_another_issuers_copy_of_the_same_numbers() {
        let o = owner().public();
        let approved = pack().signed_by(&vendor()).approved_by(&owner());
        // an impostor signs the same numbers and reuses the approval
        let impostor = BrainKey::from_seed(&[8; 32]);
        let mut stolen = pack().signed_by(&impostor);
        stolen.approval = approved.approval;
        assert_eq!(stolen.verify(&o), Err(PackError::BadApproval));
    }

    #[test]
    fn a_pack_for_another_part_or_another_model_does_not_apply() {
        let o = owner();
        let by_label = EnvelopePack::for_label("hwmon other", Source::Human, "")
            .limit("fan1.rpm", VitalKind::Faults, Band::rising(6000.0, 7000.0))
            .signed_by(&o);
        assert_eq!(
            by_label.apply(&o.public(), &part().public(), silent_fan()).unwrap_err(),
            PackError::WrongPart
        );
        let by_key = EnvelopePack::new(AppliesTo::Part(vendor().public()), Source::Human, "")
            .limit("fan1.rpm", VitalKind::Faults, Band::rising(6000.0, 7000.0))
            .signed_by(&o);
        assert_eq!(
            by_key.apply(&o.public(), &part().public(), silent_fan()).unwrap_err(),
            PackError::WrongPart
        );
        let right = EnvelopePack::new(AppliesTo::Part(part().public()), Source::Human, "")
            .limit("fan1.rpm", VitalKind::Faults, Band::rising(6000.0, 7000.0))
            .signed_by(&o);
        assert!(right.apply(&o.public(), &part().public(), silent_fan()).is_ok());
    }

    #[test]
    fn a_pack_cannot_invent_a_sense_or_state_a_limit_that_makes_no_sense() {
        let o = owner();
        let p = EnvelopePack::for_label("hwmon hp", Source::Human, "")
            .limit("fan9.rpm", VitalKind::Faults, Band::rising(1.0, 2.0))
            .signed_by(&o);
        assert_eq!(
            p.apply(&o.public(), &part().public(), silent_fan()).unwrap_err(),
            PackError::NoSuchSense
        );
        let p = EnvelopePack::for_label("hwmon hp", Source::Human, "")
            .limit("fan1.rpm", VitalKind::Faults, Band::rising(7000.0, 6000.0))
            .signed_by(&o);
        assert_eq!(
            p.apply(&o.public(), &part().public(), silent_fan()).unwrap_err(),
            PackError::InsaneBand
        );
    }

    #[test]
    fn a_pack_may_tighten_what_the_hardware_stated_and_may_not_loosen_it() {
        let o = owner();
        let drive = PartSchema::new(PartClass::Storage, "disk", "machine")
            .sense(SenseSpec::new("temp.c", Quantity::Temperature, -60.0, 250.0))
            .vital(VitalSpec::new("temp.c", VitalKind::Thermal, Band::rising(80.0, 85.0)));
        let with = |warn: f64, crit: f64| {
            EnvelopePack::for_label("disk", Source::Vendor, "datasheet")
                .limit("temp.c", VitalKind::Thermal, Band::rising(warn, crit))
                .signed_by(&o)
                .apply(&o.public(), &part().public(), drive.clone())
        };
        let tight = with(70.0, 80.0).unwrap();
        assert_eq!(tight.vital_named("temp.c").unwrap().band.critical, 80.0);
        assert_eq!(with(80.0, 85.0).map(|_| ()), Ok(()), "restating it is allowed");
        assert_eq!(with(80.0, 90.0).unwrap_err(), PackError::WouldLoosen);
        assert_eq!(with(82.0, 85.0).unwrap_err(), PackError::WouldLoosen);
        // and it may not flip which direction is bad
        let flipped = EnvelopePack::for_label("disk", Source::Vendor, "")
            .limit("temp.c", VitalKind::Thermal, Band::falling(90.0, 80.0))
            .signed_by(&o)
            .apply(&o.public(), &part().public(), drive.clone());
        assert_eq!(flipped.unwrap_err(), PackError::WouldLoosen);
    }

    #[test]
    fn the_digest_does_not_depend_on_the_order_limits_were_written_in() {
        let a = EnvelopePack::for_label("x", Source::Human, "r")
            .limit("a", VitalKind::Thermal, Band::rising(1.0, 2.0))
            .limit("b", VitalKind::Faults, Band::rising(3.0, 4.0));
        let b = EnvelopePack::for_label("x", Source::Human, "r")
            .limit("b", VitalKind::Faults, Band::rising(3.0, 4.0))
            .limit("a", VitalKind::Thermal, Band::rising(1.0, 2.0));
        assert_eq!(a.digest(), b.digest());
        let c = EnvelopePack::for_label("x", Source::Vendor, "r")
            .limit("a", VitalKind::Thermal, Band::rising(1.0, 2.0))
            .limit("b", VitalKind::Faults, Band::rising(3.0, 4.0));
        assert_ne!(a.digest(), c.digest(), "the source is part of what is signed");
    }

    #[test]
    fn the_description_says_who_vouched_and_for_what() {
        let d = pack().signed_by(&owner()).describe();
        assert!(d.contains("a human") && d.contains("fan1.rpm") && d.contains("2026-11-12"), "{d}");
    }
}
