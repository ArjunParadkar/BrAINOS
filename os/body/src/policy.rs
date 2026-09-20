//! Per-body policy — the invariants that are the body's, not the gate's.
//!
//! §3.3: KIRA is one pipeline; the invariants are the body's. A hexapod's
//! simulate is a tip-over check; an aircraft's is the flight envelope;
//! its policy is geofence, altitude floor, bank-angle limit. The pipeline
//! is fixed and the same everywhere, which is what makes it provable; the
//! numbers differ per body, which is what makes it useful.
//!
//! A `BodyPolicy` supplies stages 4, 5 and 6 for verbs authorized through
//! its body. Two rules govern it and both are enforced here rather than
//! trusted:
//!
//! * **It can only tighten.** The generic floors run first. A body policy
//!   is asked afterwards and may refuse; it is never asked whether to
//!   permit something the floors already refused.
//! * **It is stateless about memory.** A policy observes telemetry and
//!   holds a view of the body *right now*. It does not remember, because
//!   remembering is the state graph's job, and a policy that accumulated
//!   history would be memory living in a device.

use crate::calibration::Calibration;
use crate::hal::Telemetry;
use crate::schema::{CapabilitySchema, ParamKind};
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::error::DenyCode;
use brainos_abi::ids::BodyId;
use brainos_abi::verb::Verb;
use brainos_rt::envelope::Envelope;

/// An argument to a verb, already parsed out of whatever the cognitive
/// layer produced. The gate never sees free text for a bounded
/// parameter — parsing happens at the proposal boundary, and a value
/// that would not parse never becomes an argument at all.
#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Scalar(f64),
    Count(i64),
    Choice(String),
    Text(String),
    Flag(bool),
}

impl Arg {
    pub fn as_scalar(&self) -> Option<f64> {
        match self {
            Arg::Scalar(v) => Some(*v),
            Arg::Count(v) => Some(*v as f64),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Arg::Text(s) | Arg::Choice(s) => Some(s),
            _ => None,
        }
    }

    pub fn kind(&self) -> ParamKind {
        match self {
            Arg::Scalar(_) => ParamKind::Scalar,
            Arg::Count(_) => ParamKind::Count,
            Arg::Choice(_) => ParamKind::Enum,
            Arg::Text(_) => ParamKind::Text,
            Arg::Flag(_) => ParamKind::Flag,
        }
    }
}

/// A named argument list.
pub type Args = Vec<(String, Arg)>;

/// A body's refusal: a code plus one line of detail.
#[derive(Clone, Debug, PartialEq)]
pub struct Refusal {
    pub code: DenyCode,
    pub detail: String,
}

impl Refusal {
    pub fn new(code: DenyCode, detail: &str) -> Refusal {
        Refusal {
            code,
            detail: alloc::string::ToString::to_string(detail),
        }
    }
}

/// What the body is doing right now. A policy's answers depend on it, and
/// it is an enum rather than a set of booleans because the states are
/// mutually exclusive and treating them otherwise produces a body that is
/// both walking and stowed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Posture {
    /// powered but not holding itself up
    Stowed,
    /// holding a pose, not moving
    Standing,
    /// executing a motion
    Moving,
    /// stopped by a reflex, awaiting a decision
    Stopped,
    /// link down; the body is on its own
    Isolated,
}

impl Posture {
    /// Can a new motion start from here? Not from `Stopped` — that needs
    /// a decision from cognition, which is the whole point of a reflex
    /// stop being an escalation rather than a retry.
    pub fn accepts_motion(self) -> bool {
        matches!(self, Posture::Standing | Posture::Moving)
    }
}

/// The three stages a body supplies.
pub trait BodyPolicy: Send {
    fn body(&self) -> BodyId;

    /// A line for `body.list` and the audit trail.
    fn describe(&self) -> String;

    /// Stage 4 — validate. Is this request well-formed and inside the
    /// body's declared envelope? Schema checking has already happened;
    /// this is for constraints the schema cannot express, such as
    /// relationships *between* parameters.
    fn validate(&self, verb: &Verb, args: &Args) -> core::result::Result<(), Refusal>;

    /// Stage 5 — simulate. Forward-model the consequence. `Ok` carries
    /// the prediction, for the audit record; `Err` is a predicted
    /// invariant break.
    ///
    /// A policy that cannot predict must return `Unpredictable` rather
    /// than `Ok`. Refusing to act beats acting on no model.
    fn simulate(&self, verb: &Verb, args: &Args) -> core::result::Result<String, Refusal>;

    /// Stage 6 — the body's hard invariants *right now*: link, power,
    /// posture, temperature.
    fn policy(&self, verb: &Verb, args: &Args) -> core::result::Result<(), Refusal>;

    /// Telemetry updates the policy's view of the body.
    fn observe(&mut self, _t: &Telemetry) {}

    /// The envelope this body's reflexes run inside, for the audit trail
    /// and for re-binding after a reconnect.
    fn envelope(&self) -> Option<Envelope> {
        None
    }
}

/// A policy that refuses everything, used for a body whose schema
/// declared verbs but which has no policy bound yet.
///
/// This exists so that "incorporated but not yet policed" is a
/// *representable* state that fails closed, rather than a gap where the
/// gate finds no policy and has to decide what no-policy means. It means
/// no.
pub struct NullPolicy {
    pub body: BodyId,
}

impl BodyPolicy for NullPolicy {
    fn body(&self) -> BodyId {
        self.body
    }

    fn describe(&self) -> String {
        alloc::string::ToString::to_string("no policy bound; refuses everything")
    }

    fn validate(&self, _v: &Verb, _a: &Args) -> core::result::Result<(), Refusal> {
        Err(Refusal::new(
            DenyCode::OutsideBodyEnvelope,
            "no policy is bound to this body",
        ))
    }

    fn simulate(&self, _v: &Verb, _a: &Args) -> core::result::Result<String, Refusal> {
        Err(Refusal::new(
            DenyCode::Unpredictable,
            "no policy is bound to this body",
        ))
    }

    fn policy(&self, _v: &Verb, _a: &Args) -> core::result::Result<(), Refusal> {
        Err(Refusal::new(
            DenyCode::BodyInvariant,
            "no policy is bound to this body",
        ))
    }
}

/// A general-purpose policy built from a schema, a calibration and live
/// telemetry.
///
/// Most bodies do not need bespoke logic; they need their schema enforced
/// and their telemetry respected. This does both, and a body with genuine
/// dynamics — a hexapod's tip-over, an aircraft's envelope — implements
/// `BodyPolicy` itself and uses this as the floor.
pub struct SchemaPolicy {
    pub body: BodyId,
    pub schema: CapabilitySchema,
    pub calibration: Option<Calibration>,
    pub telemetry: Telemetry,
    pub posture: Posture,
    pub envelope: Option<Envelope>,
}

impl SchemaPolicy {
    pub fn new(body: BodyId, schema: CapabilitySchema) -> SchemaPolicy {
        SchemaPolicy {
            body,
            schema,
            calibration: None,
            telemetry: Telemetry::default(),
            posture: Posture::Stowed,
            envelope: None,
        }
    }

    pub fn with_envelope(mut self, e: Envelope) -> SchemaPolicy {
        self.envelope = Some(e);
        self
    }

    pub fn calibrated(mut self, c: Calibration) -> SchemaPolicy {
        self.calibration = Some(c);
        self
    }
}

impl BodyPolicy for SchemaPolicy {
    fn body(&self) -> BodyId {
        self.body
    }

    fn describe(&self) -> String {
        let mut s = String::new();
        s.push_str(&self.schema.label);
        s.push_str(" (");
        s.push_str(self.schema.adapter.as_str());
        s.push_str(") ");
        s.push_str(match self.posture {
            Posture::Stowed => "stowed",
            Posture::Standing => "standing",
            Posture::Moving => "moving",
            Posture::Stopped => "STOPPED",
            Posture::Isolated => "ISOLATED",
        });
        if let Some(c) = &self.calibration {
            s.push_str(" calib ");
            s.push_str(if c.probed_fraction_pct() == 100 {
                "probed"
            } else {
                "assumed"
            });
        } else {
            s.push_str(" uncalibrated");
        }
        s
    }

    fn validate(&self, verb: &Verb, args: &Args) -> core::result::Result<(), Refusal> {
        let Some(spec) = self.schema.get(verb) else {
            return Err(Refusal::new(
                DenyCode::BodyLacksVerb,
                "this body does not advertise that verb",
            ));
        };

        // every argument must correspond to a declared parameter and be
        // inside its bounds. An argument the schema does not mention is a
        // refusal, not an ignore: ignoring it means acting on a request
        // that is not the request that was checked.
        for (name, arg) in args {
            let Some(p) = spec.param(name) else {
                return Err(Refusal::new(
                    DenyCode::Malformed,
                    "argument is not declared by the schema",
                ));
            };
            if !p.bounded {
                return Err(Refusal::new(
                    DenyCode::OutsideBodyEnvelope,
                    "the body did not declare bounds for this parameter",
                ));
            }
            let ok = match arg {
                Arg::Scalar(v) => p.admits_scalar(*v),
                Arg::Count(v) => p.admits_scalar(*v as f64),
                Arg::Choice(s) => p.admits_choice(s),
                Arg::Text(s) => p.admits_text(s),
                Arg::Flag(_) => p.kind == ParamKind::Flag,
            };
            if !ok {
                return Err(Refusal::new(
                    DenyCode::OutOfBounds,
                    "argument is outside the declared bounds",
                ));
            }
        }

        // every declared parameter must be supplied. A missing parameter
        // means the body substitutes a default the gate never checked.
        for p in &spec.params {
            if !args.iter().any(|(n, _)| n == &p.name) {
                return Err(Refusal::new(
                    DenyCode::Malformed,
                    "a declared parameter was not supplied",
                ));
            }
        }

        // a consequential region with no calibration is a body acting on
        // numbers nobody measured
        if spec.region.is_consequential() && self.calibration.is_none() {
            return Err(Refusal::new(
                DenyCode::OutsideBodyEnvelope,
                "the body is not calibrated",
            ));
        }
        Ok(())
    }

    fn simulate(&self, verb: &Verb, args: &Args) -> core::result::Result<String, Refusal> {
        let Some(spec) = self.schema.get(verb) else {
            return Err(Refusal::new(
                DenyCode::Unpredictable,
                "no schema entry to predict from",
            ));
        };

        // an irreversible act on a body that is not fit is refused at
        // simulate rather than at policy, because the distinction the
        // audit record should carry is "we predicted harm", not "a rule
        // said no"
        if !spec.reversible && !self.telemetry.is_fit() {
            return Err(Refusal::new(
                DenyCode::IrreversibleBeyondBudget,
                self.telemetry
                    .unfitness()
                    .unwrap_or("the body is not fit for an irreversible act"),
            ));
        }

        // the envelope is the forward model for a consequential region:
        // if a scalar argument would leave it, the consequence is known
        if let Some(env) = &self.envelope {
            for (name, arg) in args {
                let Some(p) = spec.param(name) else { continue };
                let (Some(ch), Some(v)) = (p.channel, arg.as_scalar()) else {
                    continue;
                };
                if env.check(ch, v as f32).is_err() {
                    return Err(Refusal::new(
                        DenyCode::PredictedInvariantBreak,
                        "the commanded value leaves the granted envelope",
                    ));
                }
            }
        } else if spec.region.is_consequential() {
            return Err(Refusal::new(
                DenyCode::Unpredictable,
                "no envelope bound for a consequential region",
            ));
        }

        let mut s = String::new();
        s.push_str("one bounded ");
        s.push_str(spec.region.name());
        s.push_str(" act, inside the envelope");
        Ok(s)
    }

    fn policy(&self, verb: &Verb, _args: &Args) -> core::result::Result<(), Refusal> {
        let Some(spec) = self.schema.get(verb) else {
            return Err(Refusal::new(DenyCode::BodyLacksVerb, "not advertised"));
        };
        if let Some(why) = self.telemetry.unfitness() {
            return Err(Refusal::new(DenyCode::BodyInvariant, why));
        }
        if spec.region.is_consequential() && !self.posture.accepts_motion() {
            return Err(Refusal::new(
                DenyCode::BodyInvariant,
                match self.posture {
                    Posture::Stopped => "the body is stopped and needs a decision, not a retry",
                    Posture::Isolated => "the body is isolated",
                    _ => "the body is stowed",
                },
            ));
        }
        Ok(())
    }

    fn observe(&mut self, t: &Telemetry) {
        self.telemetry = *t;
        if !t.link_alive {
            self.posture = Posture::Isolated;
        } else if t.stops > 0 {
            self.posture = Posture::Stopped;
        } else if self.posture == Posture::Isolated {
            // a recovered link returns the body to standing, not to
            // whatever it was doing when the link dropped
            self.posture = Posture::Standing;
        }
    }

    fn envelope(&self) -> Option<Envelope> {
        self.envelope
    }
}
