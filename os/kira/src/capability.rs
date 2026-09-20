//! `Granted` — and why it has no public constructor.
//!
//! This is the smallest and most important file in the substrate.
//!
//! A `Granted` is the token that authorizes one action. It holds a field
//! `_seal: Seal`, where `Seal` is a struct with a private unit field
//! declared in this module. Outside `brainos-kira`, `Seal` cannot be
//! named and cannot be constructed, so `Granted { .. }` cannot be
//! written. Inside `brainos-kira`, exactly one expression constructs one:
//! the line in `gate::commit` that runs after all six preceding stages
//! returned `Pass`.
//!
//! That is the whole mechanism. It is not a check, so it cannot be
//! skipped; it is not a flag, so it cannot be set; it is not a runtime
//! invariant, so it cannot have a bug. A cognitive resource — however
//! capable, however manipulated — can write any bytes it likes and none
//! of them are a `Granted`.
//!
//! **Delegation and attenuation.** A body or a peer sometimes needs to
//! act on the entity's behalf without a round trip per action: a gait
//! loop cannot ask permission per servo step. It receives a
//! `Delegation` — a signed, time-limited, *narrower* authority with
//! caveats attached. The one rule, enforced by `attenuate` and checked
//! exhaustively by the proof harness: a delegation can only ever be
//! narrower than what it came from. There is no operation anywhere that
//! widens one.

use alloc::vec::Vec;
use brainos_abi::codec::{Decode, Decoder, Encode, Encoder};
use brainos_abi::error::{Error, Result};
use brainos_abi::ids::{BodyId, CapabilityId, EntityId};
use brainos_abi::time::Tick;
use brainos_abi::verb::{Namespace, Verb};
use brainos_crypto::domain::Tag;
use brainos_crypto::key::{BrainKey, PublicKey, Signature};
use brainos_crypto::sha256::{Hash, Sha256};

/// The seal. Private unit field; unnameable outside this module.
///
/// Do not add a constructor. Do not derive `Default`. Do not make the
/// field public. Every one of those would turn a compile-time guarantee
/// into a convention.
pub struct Seal(());

/// Authority to perform exactly one action, once, before it expires.
///
/// Not `Clone` and not `Copy`: a capability that can be duplicated is a
/// capability that can be used twice, and "the gate granted one step"
/// should mean one step. The executor consumes it.
pub struct Granted {
    pub id: CapabilityId,
    pub verb: Verb,
    pub body: Option<BodyId>,
    /// the digest of the request this was granted for — the executor
    /// checks it, so a token cannot be paired with a different request
    pub request: Hash,
    pub granted_at: Tick,
    pub ttl_ticks: u64,
    /// signature over the grant, by the Key. Anything downstream can
    /// verify authority without trusting whoever handed it the token.
    pub token: Signature,
    _seal: Seal,
}

impl Granted {
    /// The *only* constructor, and it is `pub(crate)`. Call sites outside
    /// `gate::commit` would be visible in review; there are none.
    pub(crate) fn mint(
        key: &BrainKey,
        verb: Verb,
        body: Option<BodyId>,
        request: Hash,
        granted_at: Tick,
        ttl_ticks: u64,
    ) -> Granted {
        let payload = grant_payload(&key.entity_id(), &verb, &body, &request, granted_at, ttl_ticks);
        let id = CapabilityId(Sha256::tagged(Tag::Grant).chain(&payload).to_id());
        Granted {
            id,
            verb,
            body,
            request,
            granted_at,
            ttl_ticks,
            token: key.sign(Tag::Grant, &payload),
            _seal: Seal(()),
        }
    }

    pub fn valid_at(&self, tick: Tick) -> bool {
        tick.0 >= self.granted_at.0 && tick.since(self.granted_at) <= self.ttl_ticks
    }

    /// Verify the token against a public key. The executor does this even
    /// though it received the token from the gate in-process: it costs
    /// nothing, and the day the executor is on another machine it is the
    /// only thing standing between it and a forged instruction.
    pub fn verify(&self, key: &PublicKey) -> bool {
        let payload = grant_payload(
            &key.entity_id(),
            &self.verb,
            &self.body,
            &self.request,
            self.granted_at,
            self.ttl_ticks,
        );
        key.verify(Tag::Grant, &payload, &self.token)
    }

    /// Does this token authorize this specific request?
    pub fn authorizes(&self, request_digest: &Hash, tick: Tick) -> bool {
        self.request.ct_eq(request_digest) && self.valid_at(tick)
    }
}

impl core::fmt::Debug for Granted {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "Granted({} ttl {} @{})",
            self.verb, self.ttl_ticks, self.granted_at
        )
    }
}

fn grant_payload(
    entity: &EntityId,
    verb: &Verb,
    body: &Option<BodyId>,
    request: &Hash,
    at: Tick,
    ttl: u64,
) -> Vec<u8> {
    let mut h = Sha256::tagged(Tag::Grant);
    h.field(entity.as_bytes());
    h.field(verb.as_str().as_bytes());
    match body {
        Some(b) => h.field(b.as_bytes()),
        None => h.field(&[]),
    }
    h.field(request.as_bytes());
    h.field(&at.0.to_le_bytes());
    h.field(&ttl.to_le_bytes());
    h.finish().as_bytes().to_vec()
}

/// Convenience so `Sha256::tagged(..).chain(..)` reads left to right.
trait Chain {
    fn chain(self, data: &[u8]) -> Hash;
}

impl Chain for Sha256 {
    fn chain(mut self, data: &[u8]) -> Hash {
        self.update(data);
        self.finish()
    }
}

/// A condition attached to a delegation. Every caveat *narrows*; there
/// is no caveat that permits something.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Caveat {
    /// only this body
    OnBody(BodyId),
    /// expires at this tick
    Until(Tick),
    /// at most this many uses
    Uses(u32),
    /// only while the body reports it is fit
    WhileFit,
    /// only for reversible actions
    ReversibleOnly,
    /// a scalar argument must stay under this
    ScalarBelow(brainos_rt::envelope::Channel, f32),
}

impl Caveat {
    pub fn tag(&self) -> u8 {
        match self {
            Caveat::OnBody(_) => 0,
            Caveat::Until(_) => 1,
            Caveat::Uses(_) => 2,
            Caveat::WhileFit => 3,
            Caveat::ReversibleOnly => 4,
            Caveat::ScalarBelow(_, _) => 5,
        }
    }
}

impl Encode for Caveat {
    fn encode(&self, e: &mut Encoder) {
        e.u8(self.tag());
        match self {
            Caveat::OnBody(b) => b.encode(e),
            Caveat::Until(t) => t.encode(e),
            Caveat::Uses(n) => e.u32(*n),
            Caveat::WhileFit | Caveat::ReversibleOnly => {}
            Caveat::ScalarBelow(c, v) => {
                e.u8(*c as u8);
                e.f32(*v);
            }
        }
    }
}

impl Decode for Caveat {
    fn decode(d: &mut Decoder) -> Result<Caveat> {
        Ok(match d.u8()? {
            0 => Caveat::OnBody(BodyId::decode(d)?),
            1 => Caveat::Until(Tick::decode(d)?),
            2 => Caveat::Uses(d.u32()?),
            3 => Caveat::WhileFit,
            4 => Caveat::ReversibleOnly,
            5 => {
                let c = d.u8()?;
                let ch = brainos_rt::envelope::Channel::ALL
                    .get(c as usize)
                    .copied()
                    .ok_or(Error::Malformed("unknown channel"))?;
                Caveat::ScalarBelow(ch, d.f32()?)
            }
            _ => return Err(Error::Malformed("unknown caveat")),
        })
    }
}

/// Standing authority handed to something that is not the entity.
///
/// Unlike `Granted`, this one *is* serializable and transferable — that
/// is its purpose. Its safety comes from being narrow, signed, and
/// short-lived, and from the fact that it still has to be presented to
/// the gate: holding a delegation does not let you act, it lets you ask
/// with better standing.
#[derive(Clone, Debug, PartialEq)]
pub struct Delegation {
    pub id: CapabilityId,
    /// the entity whose authority this is
    pub issuer: EntityId,
    /// who may present it
    pub holder: EntityId,
    /// what it covers — never `Any` for a delegation issued to anything
    /// other than the entity's own executor
    pub scope: Namespace,
    pub caveats: Vec<Caveat>,
    pub issued_at: Tick,
    pub expires_at: Tick,
    /// if this was attenuated from another delegation, its id
    pub parent: Option<CapabilityId>,
    pub signature: Signature,
}

/// Why a delegation does not cover a request.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CoverageFailure {
    OutOfScope,
    Expired,
    WrongHolder,
    WrongBody,
    UsesExhausted,
    BodyNotFit,
    Irreversible,
    ArgumentTooLarge,
    BadSignature,
}

impl Delegation {
    fn payload(
        issuer: &EntityId,
        holder: &EntityId,
        scope: &Namespace,
        caveats: &[Caveat],
        issued_at: Tick,
        expires_at: Tick,
        parent: &Option<CapabilityId>,
    ) -> Vec<u8> {
        let mut h = Sha256::tagged(Tag::Delegation);
        h.field(issuer.as_bytes());
        h.field(holder.as_bytes());
        let mut e = Encoder::new();
        scope.encode(&mut e);
        h.field(&e.finish_bounded());
        h.field(&(caveats.len() as u64).to_le_bytes());
        for c in caveats {
            let mut e = Encoder::new();
            c.encode(&mut e);
            h.field(&e.finish_bounded());
        }
        h.field(&issued_at.0.to_le_bytes());
        h.field(&expires_at.0.to_le_bytes());
        match parent {
            Some(p) => h.field(p.as_bytes()),
            None => h.field(&[]),
        }
        h.finish().as_bytes().to_vec()
    }

    /// Issue a delegation directly from the entity's own authority.
    pub fn issue(
        key: &BrainKey,
        holder: EntityId,
        scope: Namespace,
        caveats: Vec<Caveat>,
        issued_at: Tick,
        expires_at: Tick,
    ) -> Delegation {
        let issuer = key.entity_id();
        let mut caveats = caveats;
        caveats.sort_by_key(|c| c.tag());
        let payload = Delegation::payload(
            &issuer,
            &holder,
            &scope,
            &caveats,
            issued_at,
            expires_at,
            &None,
        );
        Delegation {
            id: CapabilityId(Sha256::tagged(Tag::Delegation).chain(&payload).to_id()),
            issuer,
            holder,
            scope,
            caveats,
            issued_at,
            expires_at,
            parent: None,
            signature: key.sign(Tag::Delegation, &payload),
        }
    }

    /// Narrow this delegation and hand it on.
    ///
    /// Returns `None` if the requested scope is not within the current
    /// one, or if the new expiry is later. **There is no operation in
    /// this crate that widens a delegation**, and the proof harness
    /// checks that claim exhaustively over the namespace lattice.
    pub fn attenuate(
        &self,
        key: &BrainKey,
        holder: EntityId,
        scope: Namespace,
        extra_caveats: Vec<Caveat>,
        expires_at: Tick,
    ) -> Option<Delegation> {
        if !scope.is_within(&self.scope) {
            return None;
        }
        if expires_at.0 > self.expires_at.0 {
            return None;
        }
        let mut caveats = self.caveats.clone();
        caveats.extend(extra_caveats);
        caveats.sort_by_key(|c| c.tag());
        // caveats only ever accumulate, and the tightest of each kind
        // wins — handing on a delegation cannot loosen one
        caveats = tighten(caveats);

        let payload = Delegation::payload(
            &self.issuer,
            &holder,
            &scope,
            &caveats,
            self.issued_at,
            expires_at,
            &Some(self.id),
        );
        Some(Delegation {
            id: CapabilityId(Sha256::tagged(Tag::Delegation).chain(&payload).to_id()),
            issuer: self.issuer,
            holder,
            scope,
            caveats,
            issued_at: self.issued_at,
            expires_at,
            parent: Some(self.id),
            signature: key.sign(Tag::Delegation, &payload),
        })
    }

    pub fn verify(&self, issuer: &PublicKey) -> bool {
        if !self.issuer.ct_eq(&issuer.entity_id()) {
            return false;
        }
        let payload = Delegation::payload(
            &self.issuer,
            &self.holder,
            &self.scope,
            &self.caveats,
            self.issued_at,
            self.expires_at,
            &self.parent,
        );
        issuer.verify(Tag::Delegation, &payload, &self.signature)
    }

    /// Does this delegation cover a request, right now?
    pub fn covers(
        &self,
        holder: &EntityId,
        verb: &Verb,
        body: &Option<BodyId>,
        tick: Tick,
        uses_so_far: u32,
        body_fit: bool,
        reversible: bool,
        scalars: &[(brainos_rt::envelope::Channel, f32)],
    ) -> core::result::Result<(), CoverageFailure> {
        if !self.holder.ct_eq(holder) {
            return Err(CoverageFailure::WrongHolder);
        }
        if !verb.matches(&self.scope) {
            return Err(CoverageFailure::OutOfScope);
        }
        if tick.0 > self.expires_at.0 {
            return Err(CoverageFailure::Expired);
        }
        for c in &self.caveats {
            match c {
                Caveat::OnBody(b) => {
                    if body.as_ref() != Some(b) {
                        return Err(CoverageFailure::WrongBody);
                    }
                }
                Caveat::Until(t) => {
                    if tick.0 > t.0 {
                        return Err(CoverageFailure::Expired);
                    }
                }
                Caveat::Uses(n) => {
                    if uses_so_far >= *n {
                        return Err(CoverageFailure::UsesExhausted);
                    }
                }
                Caveat::WhileFit => {
                    if !body_fit {
                        return Err(CoverageFailure::BodyNotFit);
                    }
                }
                Caveat::ReversibleOnly => {
                    if !reversible {
                        return Err(CoverageFailure::Irreversible);
                    }
                }
                Caveat::ScalarBelow(ch, limit) => {
                    for (c2, v) in scalars {
                        if c2 == ch && !(v.abs() <= *limit) {
                            return Err(CoverageFailure::ArgumentTooLarge);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Is this delegation at most as wide as `other`? The relation the
    /// proof harness checks attenuation against.
    pub fn is_within(&self, other: &Delegation) -> bool {
        self.scope.is_within(&other.scope)
            && self.expires_at.0 <= other.expires_at.0
            && other.caveats.iter().all(|oc| {
                self.caveats.iter().any(|sc| caveat_is_within(sc, oc))
            })
    }
}

/// Is caveat `a` at least as restrictive as caveat `b`?
fn caveat_is_within(a: &Caveat, b: &Caveat) -> bool {
    match (a, b) {
        (Caveat::OnBody(x), Caveat::OnBody(y)) => x == y,
        (Caveat::Until(x), Caveat::Until(y)) => x.0 <= y.0,
        (Caveat::Uses(x), Caveat::Uses(y)) => x <= y,
        (Caveat::WhileFit, Caveat::WhileFit) => true,
        (Caveat::ReversibleOnly, Caveat::ReversibleOnly) => true,
        (Caveat::ScalarBelow(c1, v1), Caveat::ScalarBelow(c2, v2)) => c1 == c2 && v1 <= v2,
        _ => false,
    }
}

/// Collapse duplicate caveat kinds, keeping the tightest of each.
fn tighten(caveats: Vec<Caveat>) -> Vec<Caveat> {
    let mut out: Vec<Caveat> = Vec::new();
    for c in caveats {
        match out.iter_mut().find(|o| o.tag() == c.tag() && same_subject(o, &c)) {
            None => out.push(c),
            Some(existing) => {
                if caveat_is_within(&c, existing) {
                    *existing = c;
                }
            }
        }
    }
    out.sort_by_key(|c| c.tag());
    out
}

fn same_subject(a: &Caveat, b: &Caveat) -> bool {
    match (a, b) {
        (Caveat::ScalarBelow(c1, _), Caveat::ScalarBelow(c2, _)) => c1 == c2,
        (Caveat::OnBody(_), Caveat::OnBody(_)) => true,
        _ => a.tag() == b.tag(),
    }
}

/// A grant as it appears in the audit record — everything about a
/// `Granted` except the unforgeable part, so it can be logged, exported
/// and shown without being a bearer token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grant {
    pub id: CapabilityId,
    pub verb: Verb,
    pub body: Option<BodyId>,
    pub granted_at: Tick,
    pub ttl_ticks: u64,
}

impl From<&Granted> for Grant {
    fn from(g: &Granted) -> Grant {
        Grant {
            id: g.id,
            verb: g.verb,
            body: g.body,
            granted_at: g.granted_at,
            ttl_ticks: g.ttl_ticks,
        }
    }
}
