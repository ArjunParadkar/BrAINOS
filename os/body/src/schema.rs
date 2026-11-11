//! BFS  LEVEL    Section 8 Acquisition - a device becomes a region of the body  [crate brainos-body]
//! BFS  PURPOSE  Defines CapabilitySchema: the verbs a body offers with typed, bounded parameters, used to validate arguments; a parameter with no bounds always fails
//! BFS  DEFINES  ParamKind, ParamSpec, VerbSpec, CapabilitySchema
//! BFS  USES     this crate: region | brainos: abi, crypto, rt
//! BFS  USED BY  adapters/boundary, adapters/mhs, adapters/native, adapters/ros2, adapters/tests/adapters_suite, blur/acquire, blur/firmware, blur/policy, +19 more
//!
//! Capability schemas — what a body says it can do, and within what.
//!
//! This is the object every adapter produces. A ROS 2 node graph becomes
//! one; an MHS device's MCP tool listing becomes one; our own firmware
//! emits one directly. Everything downstream — authorization, argument
//! validation, envelope derivation — reads only this, which is what
//! makes "a body we did not build" a solved problem rather than an
//! integration project each time.
//!
//! **A schema declares limits, not just names.** `legs.walk` with a
//! speed parameter bounded to 0.6 m/s is a different capability from
//! `legs.walk` unbounded, and the gate needs the difference. A verb
//! whose parameters have no declared bounds is accepted, but every
//! argument to it fails validation — refusing to act is the correct
//! response to a body that will not say what it can survive.

use crate::region::RegionClass;
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::codec::{Decode, Decoder, Encode, Encoder};
use brainos_abi::error::{Error, Result};
use brainos_abi::verb::Verb;
use brainos_crypto::domain::Tag;
use brainos_crypto::sha256::{Hash, Sha256};
use brainos_rt::envelope::Channel;

/// What kind of value a parameter takes. Closed list: a parameter type
/// the gate cannot bound is a parameter the gate cannot check.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ParamKind {
    /// a physical quantity, bounded, with a unit from `Channel`
    Scalar = 0,
    /// an integer count, bounded
    Count = 1,
    /// one of a fixed set of names
    Enum = 2,
    /// free text, length-bounded. The most dangerous kind, and the one
    /// the destructive-pattern floor exists for.
    Text = 3,
    /// on or off
    Flag = 4,
}

impl ParamKind {
    pub const ALL: [ParamKind; 5] = [
        ParamKind::Scalar,
        ParamKind::Count,
        ParamKind::Enum,
        ParamKind::Text,
        ParamKind::Flag,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            ParamKind::Scalar => "scalar",
            ParamKind::Count => "count",
            ParamKind::Enum => "enum",
            ParamKind::Text => "text",
            ParamKind::Flag => "flag",
        }
    }
}

/// One parameter of one verb.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamSpec {
    pub name: String,
    pub kind: ParamKind,
    /// for `Scalar`: which physical channel, so the envelope and the
    /// schema agree about what is being bounded
    pub channel: Option<Channel>,
    pub min: f64,
    pub max: f64,
    /// for `Enum`: the permitted names, sorted so the digest is canonical
    pub choices: Vec<String>,
    /// for `Text`: the maximum length
    pub max_len: u32,
    /// did the body actually state bounds, or are these defaults?
    pub bounded: bool,
}

impl ParamSpec {
    pub fn scalar(name: &str, channel: Channel, min: f64, max: f64) -> ParamSpec {
        ParamSpec {
            name: alloc::string::ToString::to_string(name),
            kind: ParamKind::Scalar,
            channel: Some(channel),
            min,
            max,
            choices: Vec::new(),
            max_len: 0,
            bounded: true,
        }
    }

    pub fn count(name: &str, min: i64, max: i64) -> ParamSpec {
        ParamSpec {
            name: alloc::string::ToString::to_string(name),
            kind: ParamKind::Count,
            channel: None,
            min: min as f64,
            max: max as f64,
            choices: Vec::new(),
            max_len: 0,
            bounded: true,
        }
    }

    pub fn choice(name: &str, choices: &[&str]) -> ParamSpec {
        let mut c: Vec<String> = choices
            .iter()
            .map(|s| alloc::string::ToString::to_string(*s))
            .collect();
        c.sort();
        c.dedup();
        ParamSpec {
            name: alloc::string::ToString::to_string(name),
            kind: ParamKind::Enum,
            channel: None,
            min: 0.0,
            max: 0.0,
            choices: c,
            max_len: 0,
            bounded: true,
        }
    }

    pub fn text(name: &str, max_len: u32) -> ParamSpec {
        ParamSpec {
            name: alloc::string::ToString::to_string(name),
            kind: ParamKind::Text,
            channel: None,
            min: 0.0,
            max: 0.0,
            choices: Vec::new(),
            max_len,
            bounded: max_len > 0,
        }
    }

    pub fn flag(name: &str) -> ParamSpec {
        ParamSpec {
            name: alloc::string::ToString::to_string(name),
            kind: ParamKind::Flag,
            channel: None,
            min: 0.0,
            max: 1.0,
            choices: Vec::new(),
            max_len: 0,
            bounded: true,
        }
    }

    /// A parameter an adapter could not find bounds for. Accepted into
    /// the schema so the verb is still visible, but every argument to it
    /// will be refused at validate — the entity knows the body has a
    /// hand, and refuses to move it blind.
    pub fn unbounded(name: &str, kind: ParamKind) -> ParamSpec {
        ParamSpec {
            name: alloc::string::ToString::to_string(name),
            kind,
            channel: None,
            min: f64::NEG_INFINITY,
            max: f64::INFINITY,
            choices: Vec::new(),
            max_len: 0,
            bounded: false,
        }
    }

    /// Does a scalar argument satisfy this spec?
    pub fn admits_scalar(&self, v: f64) -> bool {
        self.bounded
            && matches!(self.kind, ParamKind::Scalar | ParamKind::Count)
            && v.is_finite()
            && v >= self.min
            && v <= self.max
    }

    pub fn admits_choice(&self, v: &str) -> bool {
        self.bounded && self.kind == ParamKind::Enum && self.choices.iter().any(|c| c == v)
    }

    pub fn admits_text(&self, v: &str) -> bool {
        self.bounded && self.kind == ParamKind::Text && v.len() as u32 <= self.max_len
    }
}

impl Encode for ParamSpec {
    fn encode(&self, e: &mut Encoder) {
        e.str(&self.name);
        e.u8(self.kind as u8);
        e.opt(&self.channel.map(|c| c as u8));
        e.f64(self.min);
        e.f64(self.max);
        e.seq(self.choices.len());
        for c in &self.choices {
            e.str(c);
        }
        e.u32(self.max_len);
        e.bool(self.bounded);
    }
}

impl Decode for ParamSpec {
    fn decode(d: &mut Decoder) -> Result<ParamSpec> {
        let name = alloc::string::ToString::to_string(d.str()?);
        let k = d.u8()?;
        let kind = ParamKind::ALL
            .get(k as usize)
            .copied()
            .ok_or(Error::Malformed("unknown param kind"))?;
        let ch: Option<u8> = d.opt()?;
        let channel = match ch {
            None => None,
            Some(c) => Some(
                Channel::ALL
                    .get(c as usize)
                    .copied()
                    .ok_or(Error::Malformed("unknown channel"))?,
            ),
        };
        let min = d.f64()?;
        let max = d.f64()?;
        let n = d.seq()?;
        let mut choices = Vec::new();
        for _ in 0..n {
            choices.push(alloc::string::ToString::to_string(d.str()?));
        }
        Ok(ParamSpec {
            name,
            kind,
            channel,
            min,
            max,
            choices,
            max_len: d.u32()?,
            bounded: d.bool()?,
        })
    }
}

/// One thing a body can do.
#[derive(Clone, Debug, PartialEq)]
pub struct VerbSpec {
    pub verb: Verb,
    /// which region of the body performs it
    pub region: RegionClass,
    pub params: Vec<ParamSpec>,
    /// a human-readable line, for `body.list` and for the audit trail
    pub summary: String,
    /// can the effect of this verb be undone by the entity itself?
    /// Irreversible verbs face a stricter simulate stage.
    pub reversible: bool,
    /// the worst case this verb can take, so the scheduler can plan and
    /// the watchdog knows when it has hung
    pub worst_case_ms: u32,
}

impl VerbSpec {
    pub fn new(verb: Verb, region: RegionClass, summary: &str) -> VerbSpec {
        VerbSpec {
            verb,
            region,
            params: Vec::new(),
            summary: alloc::string::ToString::to_string(summary),
            reversible: true,
            worst_case_ms: 1_000,
        }
    }

    pub fn with(mut self, p: ParamSpec) -> VerbSpec {
        self.params.push(p);
        // parameters are sorted by name so the digest does not depend on
        // the order an adapter happened to discover them in
        self.params.sort_by(|a, b| a.name.cmp(&b.name));
        self
    }

    pub fn irreversible(mut self) -> VerbSpec {
        self.reversible = false;
        self
    }

    pub fn taking_up_to(mut self, ms: u32) -> VerbSpec {
        self.worst_case_ms = ms;
        self
    }

    pub fn param(&self, name: &str) -> Option<&ParamSpec> {
        self.params.iter().find(|p| p.name == name)
    }

    /// Is every parameter bounded? A verb with an unbounded parameter is
    /// visible but unusable, and this is how the entity knows which.
    pub fn is_actionable(&self) -> bool {
        self.params.iter().all(|p| p.bounded)
    }
}

impl Encode for VerbSpec {
    fn encode(&self, e: &mut Encoder) {
        self.verb.encode(e);
        e.u8(self.region as u8);
        e.seq(self.params.len());
        for p in &self.params {
            p.encode(e);
        }
        e.str(&self.summary);
        e.bool(self.reversible);
        e.u32(self.worst_case_ms);
    }
}

impl Decode for VerbSpec {
    fn decode(d: &mut Decoder) -> Result<VerbSpec> {
        let verb = Verb::decode(d)?;
        let r = d.u8()?;
        let region = RegionClass::from_u8(r).ok_or(Error::Malformed("unknown region class"))?;
        let n = d.seq()?;
        let mut params = Vec::new();
        for _ in 0..n {
            params.push(ParamSpec::decode(d)?);
        }
        Ok(VerbSpec {
            verb,
            region,
            params,
            summary: alloc::string::ToString::to_string(d.str()?),
            reversible: d.bool()?,
            worst_case_ms: d.u32()?,
        })
    }
}

/// Everything a body says it can do.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CapabilitySchema {
    /// what the body calls itself. Advisory only — never used to decide
    /// anything, because a name is whatever the device claimed.
    pub label: String,
    /// which adapter produced this schema
    pub adapter: String,
    verbs: Vec<VerbSpec>,
}

impl CapabilitySchema {
    pub fn new(label: &str, adapter: &str) -> CapabilitySchema {
        CapabilitySchema {
            label: alloc::string::ToString::to_string(label),
            adapter: alloc::string::ToString::to_string(adapter),
            verbs: Vec::new(),
        }
    }

    /// Add a verb. Duplicates replace, and the list is kept sorted by
    /// verb so the digest does not depend on discovery order — two
    /// adapters reading the same robot must produce the same `BodyId`.
    pub fn declare(&mut self, spec: VerbSpec) -> &mut CapabilitySchema {
        self.verbs.retain(|v| v.verb != spec.verb);
        self.verbs.push(spec);
        self.verbs.sort_by(|a, b| a.verb.as_str().cmp(b.verb.as_str()));
        self
    }

    pub fn with(mut self, spec: VerbSpec) -> CapabilitySchema {
        self.declare(spec);
        self
    }

    pub fn verbs(&self) -> &[VerbSpec] {
        &self.verbs
    }

    pub fn len(&self) -> usize {
        self.verbs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }

    pub fn get(&self, verb: &Verb) -> Option<&VerbSpec> {
        self.verbs
            .binary_search_by(|v| v.verb.as_str().cmp(verb.as_str()))
            .ok()
            .map(|i| &self.verbs[i])
    }

    pub fn advertises(&self, verb: &Verb) -> bool {
        self.get(verb).is_some()
    }

    /// Which region classes this body has.
    pub fn regions(&self) -> Vec<RegionClass> {
        let mut out: Vec<RegionClass> = self.verbs.iter().map(|v| v.region).collect();
        out.sort();
        out.dedup();
        out
    }

    /// The digest that, with the attestation key, *is* the body's
    /// identity. Canonical: same capabilities, same digest, regardless of
    /// which adapter found them or in what order.
    pub fn digest(&self) -> Hash {
        let mut h = Sha256::tagged(Tag::Schema);
        // the label and the adapter are deliberately excluded: a device
        // that renames itself is the same body, and a device read through
        // a different adapter is also the same body
        h.field(&(self.verbs.len() as u64).to_le_bytes());
        for v in &self.verbs {
            let mut e = Encoder::new();
            v.encode(&mut e);
            h.field(&e.finish_bounded());
        }
        h.finish()
    }
}

impl Encode for CapabilitySchema {
    fn encode(&self, e: &mut Encoder) {
        e.str(&self.label);
        e.str(&self.adapter);
        e.seq(self.verbs.len());
        for v in &self.verbs {
            v.encode(e);
        }
    }
}

impl Decode for CapabilitySchema {
    fn decode(d: &mut Decoder) -> Result<CapabilitySchema> {
        let label = alloc::string::ToString::to_string(d.str()?);
        let adapter = alloc::string::ToString::to_string(d.str()?);
        let n = d.seq()?;
        let mut verbs = Vec::new();
        for _ in 0..n {
            verbs.push(VerbSpec::decode(d)?);
        }
        // re-sort on decode: a hostile peer could send them unsorted, and
        // the digest must not depend on what order bytes arrived in
        verbs.sort_by(|a, b| a.verb.as_str().cmp(b.verb.as_str()));
        verbs.dedup_by(|a, b| a.verb == b.verb);
        Ok(CapabilitySchema {
            label,
            adapter,
            verbs,
        })
    }
}
