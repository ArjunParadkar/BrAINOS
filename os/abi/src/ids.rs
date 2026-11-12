//! BFS  LEVEL    Foundation - shared vocabulary (ids, time, verbs, codec, errors)  [crate brainos-abi]
//! BFS  PURPOSE  Fixed 32-byte ids (entity, body, session, ...) as separate types so one kind cannot be passed as another and names are never strings
//! BFS  DEFINES  Id32, Fingerprint, NodeId
//! BFS  USES     this crate: codec, error
//! BFS  USED BY  abi/lib, abi/tests/abi_suite, adapters/boundary, adapters/tests/adapters_suite, blur/acquire, blur/calibrate, blur/policy, blur/session, +80 more
//!
//! Identities — every name in the system, as bytes.
//!
//! The UNIX habit is to name things with strings and then decide
//! authority by comparing them. That works while every name is issued by
//! one trusted administrator. It stops working the moment a body the
//! entity did not build introduces itself, because then "who you are" is
//! whatever you claimed to be.
//!
//! So: an `EntityId` is a public key and nothing else. A `BodyId` is the
//! hash of a body's schema and its attestation key, so two bodies with
//! the same friendly name are different bodies and the type system knows
//! it. Everything is fixed-size, `Copy`, and comparable in constant time
//! where comparison is a security decision.

use crate::codec::{Decode, Decoder, Encode, Encoder};
use crate::error::{Error, Result};
use core::fmt;

/// A 32-byte identifier. Every id in BrAInOS is one of these underneath;
/// the newtypes exist so a body id can never be passed where an entity
/// id belongs, which is a mistake that would otherwise compile.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Id32(pub [u8; 32]);

impl Id32 {
    pub const ZERO: Id32 = Id32([0u8; 32]);

    pub const fn from_bytes(b: [u8; 32]) -> Id32 {
        Id32(b)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn is_zero(&self) -> bool {
        // not constant-time on purpose: "is this unset" is not a secret
        self.0 == [0u8; 32]
    }

    /// Constant-time equality. Used wherever a mismatch would otherwise
    /// leak how much of a secret an attacker guessed correctly — key
    /// comparison, token comparison, attestation nonces.
    pub fn ct_eq(&self, other: &Id32) -> bool {
        let mut diff = 0u8;
        for i in 0..32 {
            diff |= self.0[i] ^ other.0[i];
        }
        diff == 0
    }

    /// Parse 64 hex characters. Rejects anything else — a short id is not
    /// a prefix match, it is an error.
    pub fn parse_hex(s: &str) -> Result<Id32> {
        let b = s.as_bytes();
        if b.len() != 64 {
            return Err(Error::Malformed("id must be 64 hex characters"));
        }
        let mut out = [0u8; 32];
        for i in 0..32 {
            let hi = hex_val(b[2 * i]).ok_or(Error::Malformed("id has a non-hex character"))?;
            let lo = hex_val(b[2 * i + 1]).ok_or(Error::Malformed("id has a non-hex character"))?;
            out[i] = (hi << 4) | lo;
        }
        Ok(Id32(out))
    }

    /// Lowercase hex, written into a caller-provided buffer so this works
    /// with no allocator at all.
    pub fn write_hex(&self, out: &mut [u8; 64]) {
        for i in 0..32 {
            out[2 * i] = hex_char(self.0[i] >> 4);
            out[2 * i + 1] = hex_char(self.0[i] & 0x0F);
        }
    }

    /// The short display form used in logs and on screen: first eight and
    /// last four hex characters. Never used for comparison.
    pub fn fingerprint(&self) -> Fingerprint {
        let mut full = [0u8; 64];
        self.write_hex(&mut full);
        let mut f = [0u8; 14];
        f[..8].copy_from_slice(&full[..8]);
        f[8] = b'.';
        f[9] = b'.';
        f[10..].copy_from_slice(&full[60..]);
        Fingerprint(f)
    }
}

const fn hex_char(n: u8) -> u8 {
    if n < 10 {
        b'0' + n
    } else {
        b'a' + n - 10
    }
}

const fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

impl fmt::Debug for Id32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.fingerprint())
    }
}

impl Encode for Id32 {
    fn encode(&self, e: &mut Encoder) {
        e.raw(&self.0);
    }
}

impl Decode for Id32 {
    fn decode(d: &mut Decoder) -> Result<Id32> {
        let mut b = [0u8; 32];
        d.read_exact(&mut b)?;
        Ok(Id32(b))
    }
}

/// The short human-facing form of an id. Deliberately a distinct type so
/// that a fingerprint can never be passed to something expecting an id —
/// truncated identity is the oldest way to be impersonated.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint([u8; 14]);

impl Fingerprint {
    pub fn as_str(&self) -> &str {
        // every byte was written by hex_char or is '.', so this is ASCII
        core::str::from_utf8(&self.0).unwrap_or("??????..????")
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Declare a newtype over `Id32` with the same behaviour and no implicit
/// conversion to its siblings.
macro_rules! id_newtype {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        pub struct $name(pub Id32);

        impl $name {
            pub const ZERO: $name = $name(Id32::ZERO);
            pub const fn from_bytes(b: [u8; 32]) -> $name {
                $name(Id32(b))
            }
            pub fn as_bytes(&self) -> &[u8; 32] {
                self.0.as_bytes()
            }
            pub fn is_zero(&self) -> bool {
                self.0.is_zero()
            }
            pub fn ct_eq(&self, other: &$name) -> bool {
                self.0.ct_eq(&other.0)
            }
            pub fn parse_hex(s: &str) -> Result<$name> {
                Id32::parse_hex(s).map($name)
            }
            pub fn write_hex(&self, out: &mut [u8; 64]) {
                self.0.write_hex(out)
            }
            pub fn fingerprint(&self) -> Fingerprint {
                self.0.fingerprint()
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.fingerprint())
            }
        }

        impl Encode for $name {
            fn encode(&self, e: &mut Encoder) {
                self.0.encode(e)
            }
        }

        impl Decode for $name {
            fn decode(d: &mut Decoder) -> Result<$name> {
                Id32::decode(d).map($name)
            }
        }
    };
}

id_newtype! {
    /// The entity. This is an ed25519 public key — the BrAIn Key's public
    /// half — and it is the only thing in the system that is permanently
    /// the customer's. Bodies come and go; models are swapped; this does
    /// not change for the life of the entity.
    EntityId
}

id_newtype! {
    /// A body. The hash of (attestation key ‖ capability schema digest),
    /// so a body that reconnects with different capabilities is a
    /// *different* body and must be acquired again through §8. This is
    /// deliberate: a limb that silently changes what it can do is the
    /// exact failure the authorize stage exists to catch.
    BodyId
}

id_newtype! {
    /// A region of a body — a limb, a sense, a surface. Regions are what
    /// KIRA authorizes through; an action with no region is an action
    /// with no limb, and is refused at stage 3.
    RegionId
}

id_newtype! {
    /// A minted capability. The id is the hash of the grant, which makes
    /// the audit log addressable: "show me the decision that permitted
    /// this" is a lookup, not a search through prose.
    CapabilityId
}

id_newtype! {
    /// One connected session with a body or a peer. Sessions are
    /// short-lived and attested; a body that drops and returns gets a new
    /// session id, so replaying an old session's frames fails.
    SessionId
}

/// A node in the state graph. Unlike the ids above this is a small
/// counter, because it names a memory inside one entity rather than an
/// identity across the world — it never crosses a trust boundary alone.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug)]
pub struct NodeId(pub u64);

impl NodeId {
    pub const NONE: NodeId = NodeId(0);
    pub fn is_none(&self) -> bool {
        self.0 == 0
    }
}

impl Encode for NodeId {
    fn encode(&self, e: &mut Encoder) {
        e.u64(self.0);
    }
}

impl Decode for NodeId {
    fn decode(d: &mut Decoder) -> Result<NodeId> {
        d.u64().map(NodeId)
    }
}
