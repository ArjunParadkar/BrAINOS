//! BFS  LEVEL    Parts and nerves - every piece of hardware as a body part  [crate brainos-part]
//! BFS  PURPOSE  The Nerve trait every hardware protocol implements, and Mandate: proof of a Granted token, the only thing a nerve acts on
//! BFS  DEFINES  Found, Sensed, NerveFault, MandateError, Mandate, Nerve
//! BFS  USES     this crate: homeostat, schema | brainos: abi, body, crypto, kira
//! BFS  USED BY  bpu/device, bpu/mock, bpu/nerve, bpu/tests/seam, entity/entity/parts, entity/tests/parts_suite, host/nerves/hwmon, host/nerves/machine, +9 more
//!
//! Nerves — the protocols that connect the entity to its parts.
//!
//! A nerve is one way of talking to hardware: sysfs, NVML, CAN, ROS 2.
//! Every nerve implements the same trait, and the trait is the five
//! things the entity ever needs from hardware:
//!
//! ```text
//!   enumerate   what parts are on the other end of this nerve?
//!   describe    what does this part say it is?            -> PartSchema
//!   read        what does it feel right now?              -> Sensed
//!   apply       do this, here is the authority            <- Mandate
//!   safe        stop, now, without being asked twice
//! ```
//!
//! plus two that exist for §8 and Domain 2: `answer` (the part proves
//! it holds its key) and `reflex` (a pre-authorized regulator acts).
//!
//! A nerve knows nothing about verbs being *allowed*. It carries
//! readings up and commands down. Which commands exist is the schema's
//! business; whether one may happen is KIRA's.
//!
//! # A nerve cannot act without a `Granted`
//!
//! `apply` does not take a verb and arguments. It takes a `Mandate`,
//! and a `Mandate` has private fields and exactly one constructor,
//! `Mandate::check`, which consumes a `Granted` and verifies it. So
//! there is no expression a nerve's caller can write that reaches a
//! part's verb without a token the gate minted:
//!
//! ```compile_fail
//! // a Mandate cannot be written down
//! use brainos_part::nerve::Mandate;
//! let m = Mandate { verb: todo!(), part: todo!(), args: todo!(), id: todo!() };
//! ```
//!
//! ```compile_fail
//! // and neither can the token it is made from
//! use brainos_kira::capability::Granted;
//! fn forge(g: &Granted) -> Granted {
//!     Granted { id: g.id, verb: g.verb, body: g.body, request: g.request,
//!               granted_at: g.granted_at, ttl_ticks: g.ttl_ticks, token: g.token }
//! }
//! ```
//!
//! ```compile_fail
//! // a token is spent by the check: it cannot be used for a second command
//! use brainos_part::nerve::Mandate;
//! fn twice(g: brainos_kira::capability::Granted, r: &brainos_kira::action::Request,
//!          k: &brainos_crypto::key::PublicKey) {
//!     let _a = Mandate::check(g, r, k, brainos_abi::time::Tick(1));
//!     let _b = Mandate::check(g, r, k, brainos_abi::time::Tick(1));
//! }
//! ```
//!
//! # A nerve cannot write memory
//!
//! Nothing in this trait hands a nerve a `MemoryHandle`, and every
//! mutating method on the state graph requires one. This crate does not
//! even depend on `brainos-memory` outside its tests. Readings go up as
//! plain values; the *entity* decides which become memory.
//!
//! ```compile_fail
//! // writing to the state graph without the entity's handle
//! use brainos_memory::graph::{NodeKind, StateGraph};
//! use brainos_memory::provenance::Provenance;
//! fn cache(g: &mut StateGraph) {
//!     g.add(NodeKind::Episode, "gpu was 71 C", 50, Provenance::own(brainos_abi::time::Tick(1)));
//! }
//! ```
//!
//! ```compile_fail
//! // keeping a copy of the handle for later
//! fn keep(h: &brainos_memory::law::MemoryHandle) -> brainos_memory::law::MemoryHandle {
//!     h.clone()
//! }
//! ```
//!
//! What the types do **not** stop: `MemoryHandle::issue` is public
//! (the entity lives in another crate), so code that deliberately
//! issues its own handle would compile. `nostd/tests/discipline.rs`
//! closes that at the source level: no nerve file may name it.

use crate::homeostat::ReflexAct;
use crate::schema::PartSchema;
use alloc::vec::Vec;
use brainos_abi::ids::{BodyId, CapabilityId};
use brainos_abi::text::Text;
use brainos_abi::time::{Instant, Tick};
use brainos_abi::verb::Verb;
use brainos_body::policy::{Arg, Args};
use brainos_body::vital::SENSE_NAME_MAX;
use brainos_crypto::key::{Challenge, PublicKey, Signature};
use brainos_kira::action::Request;
use brainos_kira::capability::Granted;

/// A part a nerve found, before it has been acquired.
#[derive(Clone, Copy, Debug)]
pub struct Found {
    /// the nerve's own name for it; meaningless outside that nerve
    pub handle: u32,
    /// the key this part answers challenges with
    pub key: PublicKey,
    pub label: Text<48>,
}

/// One value a nerve read. It becomes a `Felt` when the entity, which
/// knows the part's id, attaches it.
#[derive(Clone, Copy, Debug)]
pub struct Sensed {
    pub sense: Text<SENSE_NAME_MAX>,
    pub value: f64,
    pub fresh: bool,
}

impl Sensed {
    pub fn new(sense: &str, value: f64) -> Sensed {
        Sensed {
            sense: Text::from_str_truncating(sense),
            value,
            fresh: true,
        }
    }
}

/// Why a nerve did not do what it was asked. Small and physical, like
/// `ActuatorFault`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NerveFault {
    /// no such part on this nerve
    NoSuchPart,
    /// the part does not have that verb, or this nerve cannot carry it
    NotSupported,
    /// the operating system or bus refused (permissions, busy, gone)
    Refused,
    /// the part did not answer
    NoResponse,
    /// the mandate is for a different part
    WrongPart,
}

/// Why a token was not accepted as authority.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MandateError {
    /// the token's signature does not verify under the entity's key
    BadToken,
    /// the token was granted for a different request, or has expired
    NotForThisRequest,
    /// the request is not an action on a body
    NotABodyAction,
}

/// Proof that the gate granted exactly this command.
///
/// Holding a `Mandate` means: a `Granted` existed, its signature
/// verified under the entity's key, it had not expired, and it was
/// granted for this request and no other. The token is consumed in
/// making it, so one grant is one command.
pub struct Mandate {
    verb: Verb,
    part: BodyId,
    args: Args,
    id: CapabilityId,
}

impl Mandate {
    /// The only constructor.
    pub fn check(
        granted: Granted,
        request: &Request,
        authority: &PublicKey,
        now: Tick,
    ) -> Result<Mandate, MandateError> {
        let Some(part) = request.body else {
            return Err(MandateError::NotABodyAction);
        };
        if !granted.verify(authority) {
            return Err(MandateError::BadToken);
        }
        if !granted.authorizes(&request.digest(), now)
            || granted.body != Some(part)
            || granted.verb != request.verb
        {
            return Err(MandateError::NotForThisRequest);
        }
        Ok(Mandate {
            verb: request.verb,
            part,
            args: request.args.clone(),
            id: granted.id,
        })
    }

    pub fn verb(&self) -> &Verb {
        &self.verb
    }

    pub fn part(&self) -> BodyId {
        self.part
    }

    /// Which grant this is, for the audit trail.
    pub fn grant(&self) -> CapabilityId {
        self.id
    }

    pub fn arg(&self, name: &str) -> Option<&Arg> {
        self.args.iter().find(|(n, _)| n == name).map(|(_, a)| a)
    }

    pub fn scalar(&self, name: &str) -> Option<f64> {
        self.arg(name).and_then(|a| a.as_scalar())
    }
}

/// One protocol to hardware.
pub trait Nerve {
    /// A short name: "hwmon", "nvml", "socketcan".
    fn name(&self) -> &'static str;

    /// §8 step 1: what is there. Called again later to notice parts
    /// arriving, leaving, or changing.
    fn enumerate(&mut self) -> Vec<Found>;

    /// §8 step 2: the part proves it holds its key.
    fn answer(&mut self, part: &Found, challenge: &Challenge) -> Option<Signature>;

    /// §8 step 3: what the part says it is. Called again each session:
    /// if the answer changes, the part's identity changes with it.
    fn describe(&mut self, part: &Found) -> Option<PartSchema>;

    /// What the part feels now. Appends to `out`; a nerve with nothing
    /// to say appends nothing, which is different from saying zero.
    fn read(&mut self, part: &Found, at: Instant, out: &mut Vec<Sensed>);

    /// Carry out a granted command.
    fn apply(&mut self, part: &Found, mandate: &Mandate) -> Result<(), NerveFault>;

    /// Carry out one step of a pre-authorized regulator (Domain 2). A
    /// `ReflexAct` can only be produced by a `Homeostat`, and a
    /// `Homeostat` can only be installed with a `Mandate`.
    fn reflex(&mut self, _part: &Found, _act: &ReflexAct) -> Result<(), NerveFault> {
        Err(NerveFault::NotSupported)
    }

    /// Put everything on this nerve in its safe state. Must not fail.
    fn safe(&mut self);
}
