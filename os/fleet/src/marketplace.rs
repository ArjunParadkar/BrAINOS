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
}

impl Manifest {
    /// Is the manifest internally consistent? Every declared verb must
    /// fall inside the declared scope.
    ///
    /// This is checked before a signature is even looked at, because a
    /// manifest that contradicts itself is one nobody should be asked to
    /// countersign.
    pub fn is_consistent(&self) -> bool {
        !self.verbs.is_empty() && self.verbs.iter().all(|v| v.matches(&self.scope))
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
        h.finish()
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
}

impl Package {
    /// Publish. The publisher signs the manifest digest.
    pub fn publish(key: &BrainKey, manifest: Manifest, at: Tick) -> Option<Package> {
        if !manifest.is_consistent() {
            return None;
        }
        let digest = manifest.digest();
        Some(Package {
            id: CapabilityId(digest.to_id()),
            signature: key.sign(Tag::Package, digest.as_bytes()),
            publisher: Publisher { key: key.public() },
            manifest,
            published_at: at,
            withdrawn: false,
        })
    }

    pub fn verify(&self) -> bool {
        self.manifest.is_consistent()
            && self.publisher.key.verify(
                Tag::Package,
                self.manifest.digest().as_bytes(),
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
