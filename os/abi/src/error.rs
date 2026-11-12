//! BFS  LEVEL    Foundation - shared vocabulary (ids, time, verbs, codec, errors)  [crate brainos-abi]
//! BFS  PURPOSE  Keeps malfunctions (Error) apart from deliberate refusals (Denied, with the gate stage and a reason code) so code can act on why
//! BFS  DEFINES  Stage, DenyCode, Denied, Error, Result
//! BFS  USES     nothing outside core/alloc
//! BFS  USED BY  abi/codec, abi/ids, abi/lib, abi/tests/abi_suite, abi/text, abi/time, abi/verb, abi/version, +53 more
//!
//! Errors and refusals.
//!
//! There are two different things here and conflating them is how safety
//! layers get routed around.
//!
//! An `Error` is a malfunction: something was malformed, a buffer ran
//! out, a link died. Errors are the system failing to work.
//!
//! A `Denied` is a *decision*: the gate considered an action and refused
//! it, at a named stage, for a reason with a code. Denials are the system
//! working exactly as designed. They are not errors, they are not
//! exceptions, and they must never be collapsed into a string, because
//! the entity has to be able to reason about its own refusals — "I was
//! denied at authorize because no limb advertises this" is something it
//! can act on ("acquire a body that does"); "operation failed" is not.

use core::fmt;

/// The eight stages, in fixed order. The order is the argument: nothing
/// reaches `Commit` without passing everything before it, and `Audit`
/// runs whatever happened.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
#[repr(u8)]
pub enum Stage {
    Parse = 1,
    Authenticate = 2,
    Authorize = 3,
    Validate = 4,
    Simulate = 5,
    Policy = 6,
    Commit = 7,
    Audit = 8,
}

impl Stage {
    pub const ALL: [Stage; 8] = [
        Stage::Parse,
        Stage::Authenticate,
        Stage::Authorize,
        Stage::Validate,
        Stage::Simulate,
        Stage::Policy,
        Stage::Commit,
        Stage::Audit,
    ];

    pub const fn index(self) -> usize {
        self as usize - 1
    }

    pub const fn name(self) -> &'static str {
        match self {
            Stage::Parse => "parse",
            Stage::Authenticate => "authenticate",
            Stage::Authorize => "authorize",
            Stage::Validate => "validate",
            Stage::Simulate => "simulate",
            Stage::Policy => "policy",
            Stage::Commit => "commit",
            Stage::Audit => "audit",
        }
    }

    pub fn from_index(i: usize) -> Option<Stage> {
        Stage::ALL.get(i).copied()
    }

    /// The stage that runs after this one, or `None` for audit.
    pub fn next(self) -> Option<Stage> {
        Stage::from_index(self.index() + 1)
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Why an action was refused. Codes are grouped by stage so that reading
/// a code tells you where the refusal happened even without the stage.
///
/// This list is closed on purpose. A new reason to refuse is a change to
/// the safety surface and gets reviewed as one; it does not arrive as a
/// new string somebody formatted at a call site.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[repr(u16)]
pub enum DenyCode {
    // --- parse ---
    /// the request did not decode into a typed action at all
    Unparseable = 100,
    /// the action names a verb outside the ABI's namespace rules
    UnknownVerb = 101,

    // --- authenticate ---
    /// the signature over the request did not verify against the Key
    BadSignature = 200,
    /// the request's tick is outside the accepted window (replay)
    StaleRequest = 201,
    /// the body presenting this request failed attestation
    UnattestedBody = 202,

    // --- authorize ---
    /// no region of any bound body advertises this capability
    NoSuchLimb = 300,
    /// the named body exists but does not advertise this verb
    BodyLacksVerb = 301,
    /// the named body is not bound to this entity
    NoSuchBody = 302,
    /// a delegated capability was presented that does not cover this act
    CapabilityDoesNotCover = 303,
    /// the presented capability has expired
    CapabilityExpired = 304,
    /// the presented capability was revoked
    CapabilityRevoked = 305,

    // --- validate ---
    /// arguments are outside the schema's declared bounds
    OutOfBounds = 400,
    /// arguments are structurally wrong (missing, wrong type, too long)
    Malformed = 401,
    /// the body's own envelope refuses these arguments
    OutsideBodyEnvelope = 402,

    // --- simulate ---
    /// the forward model predicts an invariant break
    PredictedInvariantBreak = 500,
    /// the forward model could not predict at all; refusing beats guessing
    Unpredictable = 501,
    /// the predicted consequence is irreversible beyond the allowed budget
    IrreversibleBeyondBudget = 502,

    // --- policy ---
    /// a Level-0 drive forbids this, permanently and without appeal
    DriveViolation = 600,
    /// a human-set (Level-1) rule forbids this
    RuleViolation = 601,
    /// a destructive pattern was found in the arguments
    DestructivePattern = 602,
    /// the body's hard invariants right now forbid it (link, power, mode)
    BodyInvariant = 603,
    /// the action would damage something structurally load-bearing
    LoadBearing = 604,
    /// the entity is in a world posture that forbids this
    WorldPosture = 605,

    // --- commit ---
    /// the token could not be minted (key unavailable, quorum missing)
    CannotMint = 700,

    // --- audit ---
    /// the decision could not be recorded, so it is not allowed to stand
    AuditFailed = 800,
}

impl DenyCode {
    /// The stage this code belongs to, derived from its numeric group.
    /// Used by the proof harness to check that no stage ever returns a
    /// code belonging to another stage — a mislabelled refusal is a
    /// refusal that cannot be reasoned about.
    pub fn stage(self) -> Stage {
        match self as u16 / 100 {
            1 => Stage::Parse,
            2 => Stage::Authenticate,
            3 => Stage::Authorize,
            4 => Stage::Validate,
            5 => Stage::Simulate,
            6 => Stage::Policy,
            7 => Stage::Commit,
            _ => Stage::Audit,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            DenyCode::Unparseable => "unparseable",
            DenyCode::UnknownVerb => "unknown-verb",
            DenyCode::BadSignature => "bad-signature",
            DenyCode::StaleRequest => "stale-request",
            DenyCode::UnattestedBody => "unattested-body",
            DenyCode::NoSuchLimb => "no-such-limb",
            DenyCode::BodyLacksVerb => "body-lacks-verb",
            DenyCode::NoSuchBody => "no-such-body",
            DenyCode::CapabilityDoesNotCover => "capability-does-not-cover",
            DenyCode::CapabilityExpired => "capability-expired",
            DenyCode::CapabilityRevoked => "capability-revoked",
            DenyCode::OutOfBounds => "out-of-bounds",
            DenyCode::Malformed => "malformed",
            DenyCode::OutsideBodyEnvelope => "outside-body-envelope",
            DenyCode::PredictedInvariantBreak => "predicted-invariant-break",
            DenyCode::Unpredictable => "unpredictable",
            DenyCode::IrreversibleBeyondBudget => "irreversible-beyond-budget",
            DenyCode::DriveViolation => "drive-violation",
            DenyCode::RuleViolation => "rule-violation",
            DenyCode::DestructivePattern => "destructive-pattern",
            DenyCode::BodyInvariant => "body-invariant",
            DenyCode::LoadBearing => "load-bearing",
            DenyCode::WorldPosture => "world-posture",
            DenyCode::CannotMint => "cannot-mint",
            DenyCode::AuditFailed => "audit-failed",
        }
    }

    /// Every code, so the proof harness can enumerate them exhaustively
    /// rather than trusting that this list and the enum agree.
    pub const ALL: [DenyCode; 25] = [
        DenyCode::Unparseable,
        DenyCode::UnknownVerb,
        DenyCode::BadSignature,
        DenyCode::StaleRequest,
        DenyCode::UnattestedBody,
        DenyCode::NoSuchLimb,
        DenyCode::BodyLacksVerb,
        DenyCode::NoSuchBody,
        DenyCode::CapabilityDoesNotCover,
        DenyCode::CapabilityExpired,
        DenyCode::CapabilityRevoked,
        DenyCode::OutOfBounds,
        DenyCode::Malformed,
        DenyCode::OutsideBodyEnvelope,
        DenyCode::PredictedInvariantBreak,
        DenyCode::Unpredictable,
        DenyCode::IrreversibleBeyondBudget,
        DenyCode::DriveViolation,
        DenyCode::RuleViolation,
        DenyCode::DestructivePattern,
        DenyCode::BodyInvariant,
        DenyCode::LoadBearing,
        DenyCode::WorldPosture,
        DenyCode::CannotMint,
        DenyCode::AuditFailed,
    ];

    /// Is this refusal worth telling the human about, or is it routine?
    /// A drive violation or a predicted invariant break is a thing that
    /// happened to the entity and it should surface; a stale tick is
    /// housekeeping.
    pub fn is_salient(self) -> bool {
        matches!(
            self,
            DenyCode::DriveViolation
                | DenyCode::PredictedInvariantBreak
                | DenyCode::IrreversibleBeyondBudget
                | DenyCode::DestructivePattern
                | DenyCode::LoadBearing
                | DenyCode::UnattestedBody
                | DenyCode::BadSignature
                | DenyCode::AuditFailed
        )
    }
}

impl fmt::Display for DenyCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A refusal: the stage that refused, why, and one short detail for the
/// human. The detail is *additional*; nothing may branch on it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Denied {
    pub stage: Stage,
    pub code: DenyCode,
    pub detail: &'static str,
}

impl Denied {
    pub const fn new(code: DenyCode, detail: &'static str) -> Denied {
        // The stage is derived from the code rather than passed in, so a
        // caller physically cannot label a refusal with the wrong stage.
        Denied {
            stage: derive_stage(code),
            code,
            detail,
        }
    }

    pub fn is_salient(&self) -> bool {
        self.code.is_salient()
    }
}

/// `const fn` twin of `DenyCode::stage`, so `Denied::new` can be const.
const fn derive_stage(code: DenyCode) -> Stage {
    match code as u16 / 100 {
        1 => Stage::Parse,
        2 => Stage::Authenticate,
        3 => Stage::Authorize,
        4 => Stage::Validate,
        5 => Stage::Simulate,
        6 => Stage::Policy,
        7 => Stage::Commit,
        _ => Stage::Audit,
    }
}

impl fmt::Display for Denied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "denied at {}: {} ({})", self.stage, self.code, self.detail)
    }
}

/// Machine malfunction, as distinct from a decision.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// bytes did not decode into the expected shape
    Malformed(&'static str),
    /// a decode ran past the end of its input
    Truncated,
    /// an encode or a collection hit a hard capacity limit
    Capacity(&'static str),
    /// a value was outside the range its type promises
    Range(&'static str),
    /// something was asked of a component that is not in a state to do it
    State(&'static str),
    /// the far end of a link stopped answering
    LinkLost,
    /// cryptographic verification failed
    Crypto(&'static str),
    /// the requested thing does not exist
    NotFound(&'static str),
    /// the requested thing already exists and would be silently replaced
    Exists(&'static str),
    /// a deadline was missed by a real-time task
    DeadlineMissed,
    /// storage refused or returned corrupt data
    Storage(&'static str),
    /// the operation is defined but not implemented on this platform
    Unsupported(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Malformed(w) => write!(f, "malformed: {w}"),
            Error::Truncated => f.write_str("truncated"),
            Error::Capacity(w) => write!(f, "out of capacity: {w}"),
            Error::Range(w) => write!(f, "out of range: {w}"),
            Error::State(w) => write!(f, "wrong state: {w}"),
            Error::LinkLost => f.write_str("link lost"),
            Error::Crypto(w) => write!(f, "crypto: {w}"),
            Error::NotFound(w) => write!(f, "not found: {w}"),
            Error::Exists(w) => write!(f, "already exists: {w}"),
            Error::DeadlineMissed => f.write_str("deadline missed"),
            Error::Storage(w) => write!(f, "storage: {w}"),
            Error::Unsupported(w) => write!(f, "unsupported: {w}"),
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;
