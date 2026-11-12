//! BFS  LEVEL    Domain 2 Autonomic - envelopes, reflexes, scheduler, watchdogs  [crate brainos-rt]
//! BFS  PURPOSE  Numeric limits (angle, speed, current, ...) approved once by the gate; fast loops may command anything inside them and are clamped or stopped
//! BFS  DEFINES  EnvelopeId, Limit, Channel, Envelope, Violation
//! BFS  USES     brainos: abi
//! BFS  USED BY  adapters/mhs, adapters/native, adapters/ros2, adapters/tests/adapters_suite, blur/schema, body/acquire, body/hal, body/policy, +26 more
//!
//! Envelopes — the pre-authorized region a reflex operates inside.
//!
//! This is the object that makes "Tier 0 never proposes into KIRA"
//! coherent rather than a hole in the gate. At incorporation (§8 step 4),
//! KIRA evaluates *the envelope itself* — one decision, about a whole
//! region of behaviour: "this hexapod's femur servos may be commanded
//! between −35° and +35°, at up to 240°/s, drawing at most 2.4 A, while
//! the link is alive". If that is granted, the reflex loop may then issue
//! any command inside it, thousands of times a second, without asking
//! again.
//!
//! So an envelope is a *capability expressed in units*, and the reflex
//! loop's only job is to be certain it never leaves it. Leaving is not
//! an error to be handled; it is a `Violation`, and a violation stops the
//! body. The refusal is structural: `Envelope::clamp` cannot return a
//! value outside the envelope, so code downstream of it does not need to
//! be trusted to check.
//!
//! The same numbers are checked twice — here in Domain 2, and again at
//! the seat before the grant. That duplication is the design (see
//! `miniblur-core`'s module note): a compromised seat cannot push a body
//! past what its own reflexes allow.

use brainos_abi::text::Text;
use brainos_abi::time::Duration;

/// Names an envelope so a violation can say which one was left.
pub type EnvelopeId = u16;

/// One bound. Kept as a plain struct rather than a trait so that an
/// envelope is `Copy`, comparable, and hashable — an envelope is data
/// that was signed at incorporation, not behaviour.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Limit {
    pub min: f32,
    pub max: f32,
}

impl Limit {
    pub const fn new(min: f32, max: f32) -> Limit {
        Limit { min, max }
    }

    pub const UNBOUNDED: Limit = Limit {
        min: f32::NEG_INFINITY,
        max: f32::INFINITY,
    };

    /// Is this limit well-formed? An inverted or NaN limit would admit
    /// everything or nothing depending on comparison order, so it is
    /// rejected when the envelope is built rather than reasoned about
    /// later.
    pub fn is_sane(&self) -> bool {
        !self.min.is_nan() && !self.max.is_nan() && self.min <= self.max
    }

    pub fn contains(&self, v: f32) -> bool {
        // NaN is never inside anything; written this way so that it
        // falls out of the comparison rather than needing a special case
        v >= self.min && v <= self.max
    }

    /// Clamp into the limit. A NaN input clamps to `min`, because a
    /// reflex that received NaN has a broken sensor and the safe corner
    /// of an envelope is always the low one for the quantities that
    /// matter (angle from center, current, speed).
    pub fn clamp(&self, v: f32) -> f32 {
        if v.is_nan() {
            return self.min;
        }
        if v < self.min {
            self.min
        } else if v > self.max {
            self.max
        } else {
            v
        }
    }

    /// Is `self` entirely inside `other`? Narrowing an envelope is always
    /// allowed; widening one is a new authorization.
    pub fn is_within(&self, other: &Limit) -> bool {
        self.min >= other.min && self.max <= other.max
    }

    /// How far outside, as a positive magnitude. Zero when inside.
    pub fn excess(&self, v: f32) -> f32 {
        if v.is_nan() {
            return f32::INFINITY;
        }
        if v < self.min {
            self.min - v
        } else if v > self.max {
            v - self.max
        } else {
            0.0
        }
    }
}

/// The channels an envelope can bound. A closed list, because each one
/// is a physical quantity the body reports and the seat understands;
/// "some other float" is not a safety property.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
#[repr(u8)]
pub enum Channel {
    /// commanded joint angle, degrees from calibrated center
    Angle = 0,
    /// commanded joint rate, degrees per second
    Rate = 1,
    /// measured current on the actuator rail, amperes
    Current = 2,
    /// measured rail voltage, volts
    Voltage = 3,
    /// body tilt from vertical, degrees
    Tilt = 4,
    /// measured temperature, degrees Celsius
    Temperature = 5,
    /// commanded linear speed, metres per second
    Speed = 6,
    /// distance to the nearest obstacle, metres
    Clearance = 7,
    /// force pushed, squeezed or felt at a contact, newtons
    Force = 8,
}

impl Channel {
    pub const ALL: [Channel; 9] = [
        Channel::Angle,
        Channel::Rate,
        Channel::Current,
        Channel::Voltage,
        Channel::Tilt,
        Channel::Temperature,
        Channel::Speed,
        Channel::Clearance,
        Channel::Force,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Channel::Angle => "angle",
            Channel::Rate => "rate",
            Channel::Current => "current",
            Channel::Voltage => "voltage",
            Channel::Tilt => "tilt",
            Channel::Temperature => "temperature",
            Channel::Speed => "speed",
            Channel::Clearance => "clearance",
            Channel::Force => "force",
        }
    }

    pub const fn unit(self) -> &'static str {
        match self {
            Channel::Angle => "deg",
            Channel::Rate => "deg/s",
            Channel::Current => "A",
            Channel::Voltage => "V",
            Channel::Tilt => "deg",
            Channel::Temperature => "C",
            Channel::Speed => "m/s",
            Channel::Clearance => "m",
            Channel::Force => "N",
        }
    }
}

const CHANNELS: usize = 9;

/// A pre-authorized region of behaviour.
///
/// `PartialEq` because an envelope is a value that gets compared —
/// "is the intersection the same as what the body already had" is a real
/// question and comparing field by field at call sites would be a place
/// for a missed channel to hide.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Envelope {
    pub id: EnvelopeId,
    limits: [Limit; CHANNELS],
    /// which channels this envelope actually constrains; an unset channel
    /// is not "unbounded", it is "this envelope makes no statement", and
    /// a reflex that commands an unstated channel is refused
    stated: [bool; CHANNELS],
    /// the reflex must be re-run at least this often or the watchdog
    /// fires — a stale reflex is as dangerous as a wrong one
    pub period: Duration,
    /// how long the body may keep acting on this envelope with no word
    /// from the seat before it stops on its own
    pub link_grace: Duration,
}

impl Envelope {
    pub fn new(id: EnvelopeId, period: Duration, link_grace: Duration) -> Envelope {
        Envelope {
            id,
            limits: [Limit::UNBOUNDED; CHANNELS],
            stated: [false; CHANNELS],
            period,
            link_grace,
        }
    }

    /// Bound a channel. Returns `false` and changes nothing if the limit
    /// is not well-formed — an insane limit never enters an envelope.
    pub fn bound(&mut self, ch: Channel, limit: Limit) -> bool {
        if !limit.is_sane() {
            return false;
        }
        self.limits[ch as usize] = limit;
        self.stated[ch as usize] = true;
        true
    }

    pub fn with(mut self, ch: Channel, min: f32, max: f32) -> Envelope {
        self.bound(ch, Limit::new(min, max));
        self
    }

    pub fn states(&self, ch: Channel) -> bool {
        self.stated[ch as usize]
    }

    pub fn limit(&self, ch: Channel) -> Option<Limit> {
        if self.stated[ch as usize] {
            Some(self.limits[ch as usize])
        } else {
            None
        }
    }

    /// Check one value. An unstated channel is a violation, not a pass:
    /// commanding something the envelope never mentioned means the
    /// envelope KIRA granted was not the envelope being used.
    pub fn check(&self, ch: Channel, v: f32) -> Result<(), Violation> {
        match self.limit(ch) {
            None => Err(Violation {
                envelope: self.id,
                channel: ch,
                value: v,
                limit: Limit::UNBOUNDED,
                unstated: true,
            }),
            Some(l) if l.contains(v) => Ok(()),
            Some(l) => Err(Violation {
                envelope: self.id,
                channel: ch,
                value: v,
                limit: l,
                unstated: false,
            }),
        }
    }

    /// Clamp a value into the envelope. Returns `None` for an unstated
    /// channel: there is nothing to clamp *to*, and inventing a bound
    /// would be the reflex authorizing itself.
    pub fn clamp(&self, ch: Channel, v: f32) -> Option<f32> {
        self.limit(ch).map(|l| l.clamp(v))
    }

    /// Is `self` entirely inside `other`? Used when a body re-handshakes
    /// with a narrower envelope: narrowing needs no new grant.
    pub fn is_within(&self, other: &Envelope) -> bool {
        for i in 0..CHANNELS {
            match (self.stated[i], other.stated[i]) {
                // other constrains it and we do not: we are wider
                (false, true) => return false,
                (true, true) => {
                    if !self.limits[i].is_within(&other.limits[i]) {
                        return false;
                    }
                }
                _ => {}
            }
        }
        self.period >= other.period || self.period == other.period
    }

    /// A one-line description for the audit trail and `body.list`.
    pub fn describe(&self) -> Text<160> {
        let mut t = Text::new();
        t.push_str("envelope#");
        t.push_u64(self.id as u64);
        for ch in Channel::ALL {
            if let Some(l) = self.limit(ch) {
                t.push(' ');
                t.push_str(ch.name());
                t.push('[');
                t.push_i64(l.min as i64);
                t.push(',');
                t.push_i64(l.max as i64);
                t.push(']');
            }
        }
        t
    }
}

/// A value that left its envelope. Carries everything needed to explain
/// the stop without formatting a sentence in a servo loop.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Violation {
    pub envelope: EnvelopeId,
    pub channel: Channel,
    pub value: f32,
    pub limit: Limit,
    /// true when the channel was never bounded at all
    pub unstated: bool,
}

impl Violation {
    pub fn excess(&self) -> f32 {
        if self.unstated {
            f32::INFINITY
        } else {
            self.limit.excess(self.value)
        }
    }

    pub fn describe(&self) -> Text<128> {
        let mut t = Text::new();
        t.push_str(self.channel.name());
        if self.unstated {
            t.push_str(" is not bounded by this envelope");
            return t;
        }
        t.push(' ');
        t.push_i64(self.value as i64);
        t.push_str(self.channel.unit());
        t.push_str(" outside [");
        t.push_i64(self.limit.min as i64);
        t.push(',');
        t.push_i64(self.limit.max as i64);
        t.push(']');
        t
    }
}
