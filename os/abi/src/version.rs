//! BFS  LEVEL    Foundation - shared vocabulary (ids, time, verbs, codec, errors)  [crate brainos-abi]
//! BFS  PURPOSE  The major.minor.patch version of this shared vocabulary and the rule for accepting a peer: same major, peer minor not newer than ours
//! BFS  DEFINES  AbiVersion, ABI
//! BFS  USES     this crate: codec, error
//! BFS  USED BY  abi/lib, abi/tests/abi_suite, blur/acquire, blur/tests/blur_suite, body/acquire, body/tests/body_suite, entity/entity, entity/entity/parts, +2 more
//!
//! ABI versioning.
//!
//! An entity outlives its bodies, and it will meet bodies built against
//! an older substrate than the one it is running. The compatibility rule
//! is the conservative one, on purpose:
//!
//! * A **major** mismatch is incompatible. Refuse the handshake.
//! * A **minor** difference is compatible only when the *peer* is at or
//!   below us, because a newer peer may use fields we would silently
//!   ignore, and silently ignoring a field in a capability schema means
//!   acting on a body whose limits we did not read.
//! * **Patch** never affects compatibility.
//!
//! The asymmetry is the point. Most protocols say "newer is fine, ignore
//! what you don't know". That is correct for a document format and wrong
//! for an actuator envelope.

use crate::codec::{Decode, Decoder, Encode, Encoder};
use crate::error::Result;
use core::fmt;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct AbiVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl AbiVersion {
    pub const fn new(major: u16, minor: u16, patch: u16) -> AbiVersion {
        AbiVersion {
            major,
            minor,
            patch,
        }
    }

    /// Can we safely speak to a peer announcing `peer`?
    pub fn accepts(&self, peer: &AbiVersion) -> bool {
        self.major == peer.major && peer.minor <= self.minor
    }

    /// Why not, for the handshake's refusal message.
    pub fn why_not(&self, peer: &AbiVersion) -> Option<&'static str> {
        if self.major != peer.major {
            Some("major version mismatch")
        } else if peer.minor > self.minor {
            Some("peer speaks a newer minor version; fields would be ignored")
        } else {
            None
        }
    }
}

impl fmt::Display for AbiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl Encode for AbiVersion {
    fn encode(&self, e: &mut Encoder) {
        e.u16(self.major);
        e.u16(self.minor);
        e.u16(self.patch);
    }
}

impl Decode for AbiVersion {
    fn decode(d: &mut Decoder) -> Result<AbiVersion> {
        Ok(AbiVersion {
            major: d.u16()?,
            minor: d.u16()?,
            patch: d.u16()?,
        })
    }
}

/// The ABI this build speaks.
pub const ABI: AbiVersion = AbiVersion::new(1, 0, 0);
