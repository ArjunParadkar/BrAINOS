//! The capability marketplace.
//!
//! Third parties publish skills; entities acquire them; every
//! acquisition passes KIRA. The economics are in the master document
//! (15% of GMV); the engineering constraint is one sentence:
//!
//! > **There is no push.**
//!
//! A publisher cannot install anything. A package is offered; an entity
//! acquires it; and acquisition requires *two* signatures — the
//! publisher's, so the package is authentic, and the owner's, so
//! "the vendor added a capability to your robot overnight" is not a
//! sentence that can be true. The second signature is the one that
//! makes this a marketplace rather than a fleet-management backdoor.
//!
//! A package declares what it needs, in the same vocabulary a body uses
//! to declare what it offers: a namespace it will propose within, and a
//! maximum consequence. An entity can therefore decide whether to
//! install something *before* running it, which is the only point at
//! which deciding is cheap.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::ids::{CapabilityId, EntityId};
use brainos_abi::text::Text;
use brainos_abi::time::Tick;
use brainos_abi::verb::{Namespace, Verb};
use brainos_crypto::domain::Tag;
use brainos_crypto::key::{BrainKey, PublicKey, Signature};
use brainos_crypto::sha256::{Hash, Sha256};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarketError {
    /// the publisher's signature did not verify
    Unsigned,
    /// the owner has not countersigned
    NotCountersigned,
    /// the owner's countersignature is for a different package
    WrongPackage,
    /// the package asks for authority wider than the owner granted
    AsksTooMuch,
    /// the package's declared verbs are outside its declared namespace
    Inconsistent,
    /// the package was withdrawn or revoked
    Withdrawn,
    /// this entity already has it
    AlreadyInstalled,
}

/// Who published something.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Publisher {
    pub key: PublicKey,
}

impl Publisher {
    pub fn id(&self) -> EntityId {
        self.key.entity_id()
    }
}

/// What a package says about itself, before anyone runs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub version: u32,
    /// the widest authority the package will ever propose within
    pub scope: Namespace,
    /// the specific verbs it declares
    pub verbs: Vec<Verb>,
    /// the largest consequence it claims to have, 0-100. Advisory — the
    /// gate does not believe it — but a package that claims to be
    /// harmless and asks for `flight.*` is one an owner can refuse
    /// without running it.
    pub claimed_consequence: u8,
    /// whether anything it does is irreversible
    pub irreversible: bool,
    /// one line for a human
    pub summary: String,
    /// **Where this was learned.**
    ///
    /// The fleet-transfer experiment measured what a marketplace without
    /// this field costs. A skill adopted from a deployment unlike your
    /// own left the entity *permanently worse* than one that started
    /// from nothing — 80% against 87% — because what an entity learns is
    /// never "walking needs carpet", it is "walking needs carpet
    /// **here**", and nothing in the claim says where here is. Two
    /// robots in different places learn opposite rules and both are
    /// right.
    ///
    /// So a package says where it came from, and the buyer can tell a
    /// matched skill from a blind one before installing it rather than
    /// six hundred episodes afterwards. It is a free-text label, not an
    /// ontology: an honest short description a human recognises beats a
    /// taxonomy nobody fills in correctly.
    pub learned_in: String,
    /// How much evidence is behind it: attempts the publisher made
    /// *without* each condition, which is the only number that says
    /// anything about necessity.
    ///
    /// Advisory in exactly the way `claimed_consequence` is — a buyer
    /// cannot check it and the gate does not believe it. What makes it
    /// safe to act on anyway is that the buyer holds the claim on trust
    /// and probes it: its own five tests overrule whatever the seller
    /// said.
    pub tests_behind_it: u32,
}

impl Manifest {
    /// Is the manifest internally consistent? Every declared verb must
    /// fall inside the declared scope.
    ///
    /// This is checked before a signature is even looked at, because a
    /// manifest that contradicts itself is one nobody should be asked to
    /// countersign.
    pub fn is_consistent(&self) -> bool {
        !self.verbs.is_empty()
            && self.verbs.iter().all(|v| v.matches(&self.scope))
            // a package that will not say where it was learned is one an
            // owner cannot judge, and the experiment says an unjudgeable
            // package is worse than no package
            && !self.learned_in.is_empty()
    }

    /// Does this package come from somewhere like here?
    ///
    /// Deliberately a string comparison and deliberately the buyer's
    /// call. There is no authority that knows whether two workshops are
    /// alike, and a system that pretended to would be confidently wrong
    /// in exactly the cases that matter.
    pub fn learned_somewhere_like(&self, here: &str) -> bool {
        self.learned_in.eq_ignore_ascii_case(here)
    }

    pub fn digest(&self) -> Hash {
        let mut h = Sha256::tagged(Tag::Package);
        h.field(self.name.as_bytes());
        h.field(&self.version.to_le_bytes());
        let mut e = brainos_abi::codec::Encoder::new();
        brainos_abi::codec::Encode::encode(&self.scope, &mut e);
        h.field(&e.finish_bounded());
        // verbs are sorted so the digest does not depend on declaration
        // order
        let mut verbs: Vec<&Verb> = self.verbs.iter().collect();
        verbs.sort_by_key(|v| v.as_str());
        h.field(&(verbs.len() as u64).to_le_bytes());
        for v in verbs {
            h.field(v.as_str().as_bytes());
        }
        h.field(&[self.claimed_consequence, self.irreversible as u8]);
        h.field(self.learned_in.as_bytes());
        h.field(&self.tests_behind_it.to_le_bytes());
        h.finish()
    }
}

/// What a skill package actually delivers.
///
/// The marketplace's other types are about *authority* — may this run,
/// did the owner agree, does it ask for more than they granted. This is
/// the content: the conditions the publisher found mattered.
///
/// Note what is *not* here. The publisher's counts are not delivered.
/// What transfers is the claim; the buyer's own numbers start at zero
/// and its own five tests overrule whatever the seller believed. That
/// is not politeness about data formats, it is the conclusion of the
/// fleet-transfer measurement: delivering the seller's evidence made a
/// mismatched skill permanent, and delivering the claim alone made it
/// temporary. `Manifest::tests_behind_it` carries the seller's count as
/// advisory text for a human deciding whether to buy, and nothing reads
/// it.
#[derive(Clone, Debug, PartialEq)]
pub struct SkillContents {
    pub verb: Verb,
    pub name: String,
    /// the tags the publisher found were required, in sorted order so
    /// the digest does not depend on how they were listed
    pub conditions: Vec<String>,
}

impl SkillContents {
    pub fn new(verb: Verb, name: &str, conditions: &[&str]) -> SkillContents {
        let mut conditions: Vec<String> = conditions
            .iter()
            .map(|c| alloc::string::ToString::to_string(*c))
            .collect();
        conditions.sort();
        conditions.dedup();
        SkillContents {
            verb,
            name: alloc::string::ToString::to_string(name),
            conditions,
        }
    }

    pub fn digest(&self) -> Hash {
        let mut h = Sha256::tagged(Tag::Package);
        h.field(self.verb.as_str().as_bytes());
        h.field(self.name.as_bytes());
        h.field(&(self.conditions.len() as u64).to_le_bytes());
        for c in &self.conditions {
            h.field(c.as_bytes());
        }
        h.finish()
    }

    /// Does this content match what the manifest promised?
    ///
    /// A package whose manifest declares `legs.climb` and whose contents
    /// teach something about `arm.grasp` is one where the thing the
    /// owner countersigned is not the thing being installed.
    pub fn matches(&self, manifest: &Manifest) -> bool {
        manifest.verbs.contains(&self.verb) && self.verb.matches(&manifest.scope)
    }
}

/// What the publisher's signature covers: the manifest and whatever the
/// package delivers, bound together.
fn signed_digest(manifest: &Manifest, contents: Option<&SkillContents>) -> Hash {
    match contents {
        // a package with no contents signs its manifest digest exactly
        // as it always did, so nothing already published changes meaning
        None => manifest.digest(),
        Some(c) => {
            let mut h = Sha256::tagged(Tag::Package);
            h.field(manifest.digest().as_bytes());
            h.field(c.digest().as_bytes());
            h.finish()
        }
    }
}

/// A published package.
#[derive(Clone, Debug)]
pub struct Package {
    pub id: CapabilityId,
    pub manifest: Manifest,
    pub publisher: Publisher,
    pub signature: Signature,
    pub published_at: Tick,
    pub withdrawn: bool,
    /// what it delivers, and its digest is covered by the signature —
    /// so a reseller cannot keep the manifest and swap the contents
    pub contents: Option<SkillContents>,
}

impl Package {
    /// Publish. The publisher signs the manifest digest.
    pub fn publish(key: &BrainKey, manifest: Manifest, at: Tick) -> Option<Package> {
        Package::publish_with(key, manifest, None, at)
    }

    /// Publish a package that delivers something.
    ///
    /// The signature covers the manifest digest *and* the contents
    /// digest together, so a reseller cannot keep the manifest an owner
    /// countersigned and swap what it installs.
    pub fn publish_with(
        key: &BrainKey,
        manifest: Manifest,
        contents: Option<SkillContents>,
        at: Tick,
    ) -> Option<Package> {
        if !manifest.is_consistent() {
            return None;
        }
        if let Some(c) = &contents {
            if !c.matches(&manifest) {
                return None;
            }
        }
        let digest = signed_digest(&manifest, contents.as_ref());
        Some(Package {
            id: CapabilityId(digest.to_id()),
            signature: key.sign(Tag::Package, digest.as_bytes()),
            publisher: Publisher { key: key.public() },
            manifest,
            contents,
            published_at: at,
            withdrawn: false,
        })
    }

    pub fn verify(&self) -> bool {
        self.manifest.is_consistent()
            && self
                .contents
                .as_ref()
                .map(|c| c.matches(&self.manifest))
                .unwrap_or(true)
            && self.publisher.key.verify(
                Tag::Package,
                signed_digest(&self.manifest, self.contents.as_ref()).as_bytes(),
                &self.signature,
            )
    }

    pub fn describe(&self) -> Text<200> {
        let mut t = Text::new();
        t.push_str(&self.manifest.name);
        t.push_str(" v");
        t.push_u64(self.manifest.version as u64);
        t.push_str(" by ");
        t.push_str(self.publisher.id().fingerprint().as_str());
        t.push_str(" · ");
        t.push_str(&self.manifest.summary);
        if self.manifest.irreversible {
            t.push_str(" [irreversible]");
        }
        t
    }
}

/// An installed package, with the two signatures that put it there.
#[derive(Clone, Debug, PartialEq)]
pub struct Acquisition {
    pub package: CapabilityId,
    pub manifest_digest: Hash,
    pub owner: EntityId,
    /// the owner's countersignature over (package, entity)
    pub countersignature: Signature,
    pub acquired_at: Tick,
}

/// What the owner signs to install something.
///
/// Binding the entity id in is what stops a countersignature for one
/// robot being replayed onto another.
fn countersign_payload(package: &CapabilityId, entity: &EntityId) -> Vec<u8> {
    let mut h = Sha256::tagged(Tag::Package);
    h.field(b"countersign");
    h.field(package.as_bytes());
    h.field(entity.as_bytes());
    h.finish().as_bytes().to_vec()
}

/// Countersign a package for one entity. Only the owner's key can do
/// this, and `install` will not accept anything else.
pub fn countersign(owner: &BrainKey, package: &Package, entity: EntityId) -> Signature {
    owner.sign(
        Tag::Package,
        &countersign_payload(&package.id, &entity),
    )
}

/// Install a package into an entity's capability set.
///
/// Every check is here rather than spread across callers, and the order
/// matters: consistency, then authenticity, then the owner's consent,
/// then whether the owner's standing grant covers what is being asked
/// for. A package that fails the last check is one whose *manifest* is
/// wider than the owner ever agreed to, and refusing it at install is
/// the only cheap moment.
#[allow(clippy::too_many_arguments)]
pub fn install(
    package: &Package,
    entity: EntityId,
    owner: &PublicKey,
    countersignature: &Signature,
    owner_grants: &Namespace,
    already: &[CapabilityId],
    at: Tick,
) -> Result<Acquisition, MarketError> {
    if package.withdrawn {
        return Err(MarketError::Withdrawn);
    }
    if !package.manifest.is_consistent() {
        return Err(MarketError::Inconsistent);
    }
    if !package.verify() {
        return Err(MarketError::Unsigned);
    }
    if already.contains(&package.id) {
        return Err(MarketError::AlreadyInstalled);
    }

    let payload = countersign_payload(&package.id, &entity);
    if !owner.verify(Tag::Package, &payload, countersignature) {
        return Err(MarketError::NotCountersigned);
    }

    // the owner's standing grant is the ceiling. A package asking for
    // `*` when the owner granted `legs.*` is refused here, before it
    // runs, rather than being refused verb by verb forever after.
    if !package.manifest.scope.is_within(owner_grants) {
        return Err(MarketError::AsksTooMuch);
    }

    Ok(Acquisition {
        package: package.id,
        manifest_digest: package.manifest.digest(),
        owner: owner.entity_id(),
        countersignature: *countersignature,
        acquired_at: at,
    })
}

/// A countersignature for one entity does not install on another.
pub fn countersignature_covers(
    sig: &Signature,
    package: &CapabilityId,
    entity: &EntityId,
    owner: &PublicKey,
) -> bool {
    owner.verify(Tag::Package, &countersign_payload(package, entity), sig)
}
