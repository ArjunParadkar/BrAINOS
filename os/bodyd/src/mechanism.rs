//! BFS  LEVEL    Domain 2 Autonomic - the runtime on the body's own computer  [crate brainos-bodyd]
//! BFS  PURPOSE  Defines the Mechanism trait each body type implements (plan a motion, check a pose, give a safe pose) plus the Motion, Sample and Refusal types
//! BFS  DEFINES  Refusal, Sample, Motion, Mechanism
//! BFS  USES     brainos: abi, rt
//! BFS  USED BY  bodyd/arm, bodyd/hexapod, bodyd/lib, bodyd/protocol, bodyd/runtime, bodyd/tests/bodyd_suite, host/bench, host/fullstack, +2 more
//!
//! What a body physically is.
//!
//! A `Mechanism` is the part that differs between a hexapod, an arm and
//! a drive base. Everything else in this crate — the loop, the
//! watchdogs, the envelope enforcement, the wire — is the same for all
//! of them, which is the point: adding a body is implementing four
//! methods, not writing a runtime.
//!
//! The four methods split along one line: `plan` and `invariant` are
//! *judgement about physics*, and `sample` and `safe` are *motion*. A
//! mechanism may refuse in the first two and must not fail in the last
//! two — `safe()` in particular is called when something has already
//! gone wrong, and a safe pose that can fail is not one.

use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::text::Text;
use brainos_abi::time::Duration;
use brainos_abi::verb::Verb;
use brainos_rt::envelope::{Channel, Envelope};

/// Why the body refused something the seat granted.
///
/// This is a short closed list because it travels up the wire in a
/// fixed-size frame and because each one is a different thing for the
/// entity to think about. "The seat asked for something my own numbers
/// say is a fall" and "I do not have that verb" are different
/// situations.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Refusal {
    /// this body does not do that
    UnknownVerb = 1,
    /// the arguments did not parse into a motion
    Malformed = 2,
    /// a commanded value is outside the granted envelope
    OutsideEnvelope = 3,
    /// the body's own forward model says this would break an invariant
    WouldBreakInvariant = 4,
    /// the body is not calibrated, so its numbers are guesses
    Uncalibrated = 5,
    /// the body is stopped and needs a fresh envelope, not a retry
    Stopped = 6,
    /// the motion is longer than the body will run without hearing again
    TooLong = 7,
    /// power is below what finishing this safely needs
    PowerFloor = 8,
}

impl Refusal {
    pub const fn name(self) -> &'static str {
        match self {
            Refusal::UnknownVerb => "unknown-verb",
            Refusal::Malformed => "malformed",
            Refusal::OutsideEnvelope => "outside-envelope",
            Refusal::WouldBreakInvariant => "would-break-invariant",
            Refusal::Uncalibrated => "uncalibrated",
            Refusal::Stopped => "stopped",
            Refusal::TooLong => "too-long",
            Refusal::PowerFloor => "power-floor",
        }
    }

    pub fn from_u8(v: u8) -> Option<Refusal> {
        Some(match v {
            1 => Refusal::UnknownVerb,
            2 => Refusal::Malformed,
            3 => Refusal::OutsideEnvelope,
            4 => Refusal::WouldBreakInvariant,
            5 => Refusal::Uncalibrated,
            6 => Refusal::Stopped,
            7 => Refusal::TooLong,
            8 => Refusal::PowerFloor,
            _ => return None,
        })
    }

    /// Is this the body disagreeing with the seat about physics?
    ///
    /// Worth distinguishing in telemetry: a body that refuses an
    /// unknown verb has a schema mismatch, which is a configuration
    /// problem. A body that refuses because its own forward model says
    /// the seat's granted motion would tip it over is the duplication
    /// working, and the entity should learn from it.
    pub const fn is_a_disagreement(self) -> bool {
        matches!(
            self,
            Refusal::OutsideEnvelope | Refusal::WouldBreakInvariant
        )
    }
}

/// One step of a motion: what to command, and when.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Sample {
    /// index within the motion
    pub index: usize,
    /// how long to hold this before the next one
    pub hold: Duration,
    /// per-channel commands. Fixed width; unused entries are `None`.
    pub commands: [Option<(Channel, f32)>; 8],
    /// the body's own prediction of how close to an invariant this
    /// sample is, for telemetry. Units are the mechanism's.
    pub margin: f32,
}

impl Sample {
    pub fn hold_for(index: usize, hold: Duration) -> Sample {
        Sample {
            index,
            hold,
            commands: [None; 8],
            margin: f32::INFINITY,
        }
    }

    pub fn with(mut self, ch: Channel, v: f32) -> Sample {
        for slot in self.commands.iter_mut() {
            if slot.is_none() {
                *slot = Some((ch, v));
                return self;
            }
        }
        self
    }

    pub fn commanded(&self) -> impl Iterator<Item = (Channel, f32)> + '_ {
        self.commands.iter().flatten().copied()
    }
}

/// A motion the body has agreed to perform.
///
/// Note that it is a *plan*, produced by the mechanism and checked by
/// the runtime, rather than a stream the seat pushes. A body playing a
/// plan it verified once is a body that keeps moving correctly when the
/// link hiccups, and one that stops cleanly when the link dies.
#[derive(Clone, Debug, PartialEq)]
pub struct Motion {
    pub verb: Verb,
    pub samples: Vec<Sample>,
    /// total duration, for the watchdog and the seat's expectations
    pub duration: Duration,
    /// the worst margin the mechanism predicts across the whole motion
    pub worst_margin: f32,
    /// a line for the telemetry
    pub summary: String,
}

impl Motion {
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn describe(&self) -> Text<120> {
        let mut t = Text::new();
        t.push_str(self.verb.as_str());
        t.push(' ');
        t.push_u64(self.samples.len() as u64);
        t.push_str(" samples over ");
        t.push_u64(self.duration.as_millis());
        t.push_str("ms, worst margin ");
        t.push_i64(self.worst_margin as i64);
        t
    }
}

/// What a body physically is.
pub trait Mechanism {
    /// A stable name, for telemetry.
    fn name(&self) -> &'static str;

    /// Which channels this body commands.
    fn channels(&self) -> &[Channel];

    /// Turn a verb and its arguments into a motion, or refuse.
    ///
    /// **This is where the body re-derives what the seat granted.** It
    /// does not trust the seat's arguments to be sensible; it plans from
    /// them and then checks the plan against its own model. A seat that
    /// granted a walk the body's geometry says would tip it over gets a
    /// refusal, not a fall.
    fn plan(&self, verb: &Verb, args: &[(&str, f32)]) -> Result<Motion, Refusal>;

    /// The body's own invariant, evaluated on one sample. Called before
    /// every sample is applied, not only at planning time, because
    /// between planning and playing the world may have moved.
    fn invariant(&self, sample: &Sample) -> Result<(), Refusal>;

    /// Where to go when something has gone wrong. Must not fail.
    fn safe(&self) -> Sample;

    /// Does this body hold itself up? A legged body holds torque when it
    /// stops; an arm releases. Getting this backwards is how a robot
    /// falls over while stopping safely.
    fn holds_itself_up(&self) -> bool;

    /// The envelope this mechanism's own numbers imply, independently of
    /// anything the seat said. The runtime intersects this with the
    /// granted envelope, so the body can only ever be more restrictive.
    fn native_envelope(&self, id: u16) -> Envelope;
}
