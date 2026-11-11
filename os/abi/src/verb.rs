//! BFS  LEVEL    Foundation - shared vocabulary (ids, time, verbs, codec, errors)  [crate brainos-abi]
//! BFS  PURPOSE  Parses and validates action names of the form `namespace.action` (e.g. `legs.walk`); the namespace is what a permission is scoped by
//! BFS  DEFINES  VERB_MAX, HALF_MAX, Verb, Namespace
//! BFS  USES     this crate: codec, error, text
//! BFS  USED BY  abi/lib, abi/tests/abi_suite, adapters/boundary, adapters/mhs, adapters/native, adapters/ros2, adapters/tests/adapters_suite, blur/policy, +66 more
//!
//! Verbs — the names of things that can be done.
//!
//! A verb is `namespace.action`, both lowercase ASCII, both bounded.
//! `legs.walk`. `fs.read`. `arm.grasp`. The namespace is not decoration:
//! it is how a capability is attenuated. A delegated capability for
//! `legs.*` covers `legs.walk` and `legs.turn` and can never be widened
//! to cover `arm.grasp`, and that check is a prefix comparison on a
//! validated string rather than a pattern language with a parser in it.
//!
//! The charset is deliberately small — `[a-z0-9_]` in each half, one dot
//! between. No wildcards inside a verb (only in a *pattern*), no spaces,
//! no unicode. A verb arriving from a body's capability schema is
//! untrusted input, and the narrowest possible grammar is the cheapest
//! way to make "the verb was weird" impossible rather than unlikely.

use crate::codec::{Decode, Decoder, Encode, Encoder};
use crate::error::{Error, Result};
use crate::text::Text;
use core::fmt;

/// Longest a whole verb may be, including the dot.
pub const VERB_MAX: usize = 48;
/// Longest either half may be.
pub const HALF_MAX: usize = 23;

/// A validated `namespace.action`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Verb {
    text: Text<VERB_MAX>,
    /// byte index of the dot, so `namespace()` and `action()` are free
    dot: u8,
}

impl Verb {
    /// Parse and validate. Everything that is not exactly the grammar is
    /// an error — there is no normalization step, because a verb that
    /// means the same thing after cleanup is a second spelling, and two
    /// spellings of one authority is how policy gets bypassed.
    pub fn parse(s: &str) -> Result<Verb> {
        if s.is_empty() || s.len() > VERB_MAX {
            return Err(Error::Malformed("verb length"));
        }
        let b = s.as_bytes();
        let dot = match s.find('.') {
            Some(i) => i,
            None => return Err(Error::Malformed("verb has no namespace")),
        };
        if s[dot + 1..].contains('.') {
            return Err(Error::Malformed("verb has more than one dot"));
        }
        let (ns, act) = (&b[..dot], &b[dot + 1..]);
        if ns.is_empty() || ns.len() > HALF_MAX {
            return Err(Error::Malformed("verb namespace length"));
        }
        if act.is_empty() || act.len() > HALF_MAX {
            return Err(Error::Malformed("verb action length"));
        }
        if !is_ident(ns) || !is_ident(act) {
            return Err(Error::Malformed("verb charset is [a-z0-9_]"));
        }
        Ok(Verb {
            text: Text::from_str_exact(s)?,
            dot: dot as u8,
        })
    }

    /// Build at compile time from a literal known to be valid. Panics on
    /// an invalid literal, which is the correct behaviour for a constant
    /// the programmer wrote: it fails in the test suite, not in a body.
    pub fn literal(s: &'static str) -> Verb {
        match Verb::parse(s) {
            Ok(v) => v,
            Err(_) => panic!("invalid verb literal"),
        }
    }

    pub fn as_str(&self) -> &str {
        self.text.as_str()
    }

    pub fn namespace(&self) -> &str {
        &self.text.as_str()[..self.dot as usize]
    }

    pub fn action(&self) -> &str {
        &self.text.as_str()[self.dot as usize + 1..]
    }

    /// Does this verb fall under a pattern? Patterns are exactly two
    /// shapes and no more: `*` (everything) and `ns.*` (one namespace).
    /// A general glob would need a matcher, and a matcher in the
    /// authorize stage is a place for a bug to live.
    pub fn matches(&self, pattern: &Namespace) -> bool {
        match pattern {
            Namespace::Any => true,
            Namespace::Only(ns) => ns.as_str() == self.namespace(),
            Namespace::Exact(v) => v == self,
        }
    }
}

fn is_ident(b: &[u8]) -> bool {
    !b.is_empty()
        && b.iter()
            .all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'_'))
}

impl fmt::Display for Verb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Verb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl Encode for Verb {
    fn encode(&self, e: &mut Encoder) {
        e.str(self.as_str());
    }
}

impl Decode for Verb {
    fn decode(d: &mut Decoder) -> Result<Verb> {
        Verb::parse(d.str()?)
    }
}

/// What a capability is allowed to cover. Ordered from widest to
/// narrowest so that `<=` on the enum is a real "is at most as wide as"
/// relation — the proof harness relies on that to check that attenuation
/// never widens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Namespace {
    /// every verb — only ever held by the entity's own root authority
    Any,
    /// every verb in one namespace
    Only(Text<HALF_MAX>),
    /// exactly one verb
    Exact(Verb),
}

impl Namespace {
    pub fn only(ns: &str) -> Result<Namespace> {
        if !is_ident(ns.as_bytes()) || ns.len() > HALF_MAX {
            return Err(Error::Malformed("namespace charset is [a-z0-9_]"));
        }
        Ok(Namespace::Only(Text::from_str_exact(ns)?))
    }

    /// Width rank: 0 is widest. Used for the attenuation check.
    pub fn width(&self) -> u8 {
        match self {
            Namespace::Any => 0,
            Namespace::Only(_) => 1,
            Namespace::Exact(_) => 2,
        }
    }

    /// Is `self` at most as wide as `other`? Attenuating a capability
    /// must always produce something that satisfies this against the
    /// original; a delegation that does not is refused at authorize.
    pub fn is_within(&self, other: &Namespace) -> bool {
        match (self, other) {
            (_, Namespace::Any) => true,
            (Namespace::Any, _) => false,
            (Namespace::Only(a), Namespace::Only(b)) => a == b,
            (Namespace::Exact(v), Namespace::Only(b)) => v.namespace() == b.as_str(),
            (Namespace::Exact(a), Namespace::Exact(b)) => a == b,
            (Namespace::Only(_), Namespace::Exact(_)) => false,
        }
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Namespace::Any => f.write_str("*"),
            Namespace::Only(ns) => write!(f, "{ns}.*"),
            Namespace::Exact(v) => write!(f, "{v}"),
        }
    }
}

impl Encode for Namespace {
    fn encode(&self, e: &mut Encoder) {
        match self {
            Namespace::Any => e.u8(0),
            Namespace::Only(ns) => {
                e.u8(1);
                ns.encode(e);
            }
            Namespace::Exact(v) => {
                e.u8(2);
                v.encode(e);
            }
        }
    }
}

impl Decode for Namespace {
    fn decode(d: &mut Decoder) -> Result<Namespace> {
        match d.u8()? {
            0 => Ok(Namespace::Any),
            1 => Ok(Namespace::Only(Text::decode(d)?)),
            2 => Ok(Namespace::Exact(Verb::decode(d)?)),
            _ => Err(Error::Malformed("namespace tag")),
        }
    }
}
