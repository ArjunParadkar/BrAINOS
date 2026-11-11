//! BFS  LEVEL    Domain 3 KIRA - the eight-stage gate; the only path to action  [crate brainos-kira]
//! BFS  PURPOSE  The typed request the gate accepts (verb, which body, arguments, tick) and its signed form, so free text never reaches the gate
//! BFS  DEFINES  Action, Request, Proposal
//! BFS  USES     brainos: abi, body, crypto
//! BFS  USED BY  adapters/boundary, adapters/tests/adapters_suite, blur/session, entity/entity, entity/tests/entity_suite, entity/tests/parts_suite, host/bench, host/blur_gate2, +12 more
//!
//! What can be asked for.
//!
//! A proposal is not text. By the time anything reaches the gate it is a
//! typed `Request`: a verb that parsed, a body that exists as an id, and
//! arguments that are already the right *kind* for their parameters. That
//! boundary is the whole of stage 1, and it is deliberately dumb — it
//! extracts structure and nothing else, so there is no clever path by
//! which injected text becomes authority.
//!
//! Note what a request carries that a syscall does not: which body, and
//! a tick. The body is there because "walk" with two hexapods bound is
//! ambiguous and resolving ambiguity silently moves the wrong robot. The
//! tick is there because it is signed, which is what stops an old
//! request from being replayed later.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::codec::Encoder;
use brainos_abi::error::{Denied, DenyCode};
use brainos_abi::ids::BodyId;
use brainos_abi::time::Tick;
use brainos_abi::verb::Verb;
use brainos_body::policy::{Arg, Args};
use brainos_crypto::domain::Tag;
use brainos_crypto::key::{BrainKey, Signature};
use brainos_crypto::sha256::{Hash, Sha256};

/// What kind of thing is being asked for. The distinction matters at
/// stage 5: an intrinsic act has no body to model, and a body act does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// act through a limb of a body
    Body,
    /// something about the entity itself — write memory, consolidate,
    /// release a body, escalate to a cognitive resource
    Intrinsic,
    /// reach into an untrusted world (§3.8). Never enabled by default.
    World,
}

/// A typed action request.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub verb: Verb,
    pub action: Action,
    /// which body to act through. `None` only for intrinsic acts.
    pub body: Option<BodyId>,
    pub args: Args,
    pub tick: Tick,
    /// a one-line statement of why, for the audit record. Never branched
    /// on — a reason that affected the decision would be a reason worth
    /// lying about.
    pub reason: String,
}

impl Request {
    pub fn on_body(verb: Verb, body: BodyId, tick: Tick) -> Request {
        Request {
            verb,
            action: Action::Body,
            body: Some(body),
            args: Vec::new(),
            tick,
            reason: String::new(),
        }
    }

    pub fn intrinsic(verb: Verb, tick: Tick) -> Request {
        Request {
            verb,
            action: Action::Intrinsic,
            body: None,
            args: Vec::new(),
            tick,
            reason: String::new(),
        }
    }

    pub fn world(verb: Verb, tick: Tick) -> Request {
        Request {
            verb,
            action: Action::World,
            body: None,
            args: Vec::new(),
            tick,
            reason: String::new(),
        }
    }

    pub fn arg(mut self, name: &str, a: Arg) -> Request {
        self.args
            .push((alloc::string::ToString::to_string(name), a));
        // sorted so the signed digest does not depend on argument order
        self.args.sort_by(|a, b| a.0.cmp(&b.0));
        self
    }

    pub fn because(mut self, why: &str) -> Request {
        self.reason = alloc::string::ToString::to_string(why);
        self
    }

    pub fn get(&self, name: &str) -> Option<&Arg> {
        self.args.iter().find(|(n, _)| n == name).map(|(_, a)| a)
    }

    /// Everything about this request that the signature covers.
    /// Canonical, so the same request always digests the same way.
    pub fn digest(&self) -> Hash {
        let mut h = Sha256::tagged(Tag::Request);
        h.field(self.verb.as_str().as_bytes());
        h.field(&[match self.action {
            Action::Body => 0,
            Action::Intrinsic => 1,
            Action::World => 2,
        }]);
        match &self.body {
            Some(b) => h.field(b.as_bytes()),
            None => h.field(&[]),
        }
        h.field(&self.tick.0.to_le_bytes());
        h.field(&(self.args.len() as u64).to_le_bytes());
        for (name, arg) in &self.args {
            h.field(name.as_bytes());
            let mut e = Encoder::new();
            encode_arg(arg, &mut e);
            h.field(&e.finish_bounded());
        }
        // the reason is deliberately outside the digest: it is narration,
        // it does not affect the decision, and including it would let a
        // caller change the digest by rewording
        h.finish()
    }

    /// Sign it. This is what the entity does before handing a request to
    /// the gate; the gate verifies it at stage 2.
    pub fn sign(&self, key: &BrainKey) -> Signature {
        key.sign(Tag::Request, self.digest().as_bytes())
    }

    /// A proposal ready for the gate.
    pub fn proposed_by(self, key: &BrainKey) -> Proposal {
        let signature = self.sign(key);
        Proposal {
            request: self,
            signature,
        }
    }
}

fn encode_arg(a: &Arg, e: &mut Encoder) {
    match a {
        Arg::Scalar(v) => {
            e.u8(0);
            e.f64(*v);
        }
        Arg::Count(v) => {
            e.u8(1);
            e.i64(*v);
        }
        Arg::Choice(s) => {
            e.u8(2);
            e.str(s);
        }
        Arg::Text(s) => {
            e.u8(3);
            e.str(s);
        }
        Arg::Flag(b) => {
            e.u8(4);
            e.bool(*b);
        }
    }
}

/// A signed request. This is the only thing the gate accepts.
#[derive(Clone, Debug)]
pub struct Proposal {
    pub request: Request,
    pub signature: Signature,
}

impl Proposal {
    /// Stage 1, in full. There is very little to do here because the
    /// typing happened at the boundary where a model's text became a
    /// `Request` — which is the point. What remains is the handful of
    /// structural contradictions a well-typed request can still contain.
    pub fn parse_check(&self) -> Result<(), Denied> {
        let r = &self.request;
        match (r.action, r.body) {
            (Action::Body, None) => Err(Denied::new(
                DenyCode::Unparseable,
                "a body action names no body",
            )),
            (Action::Intrinsic, Some(_)) | (Action::World, Some(_)) => Err(Denied::new(
                DenyCode::Unparseable,
                "a non-body action names a body",
            )),
            _ => {
                // duplicate argument names would make "which one did the
                // gate check" unanswerable
                for i in 1..r.args.len() {
                    if r.args[i].0 == r.args[i - 1].0 {
                        return Err(Denied::new(
                            DenyCode::Unparseable,
                            "an argument is supplied twice",
                        ));
                    }
                }
                Ok(())
            }
        }
    }
}
