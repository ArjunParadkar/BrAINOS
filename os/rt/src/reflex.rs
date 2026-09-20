//! Reflexes — Tier 0 cognition, which is to say none.
//!
//! A reflex is a pure function from a sensor frame to a command, running
//! inside an `Envelope` that KIRA granted once. It has no memory beyond a
//! small fixed state, no access to the state graph, and no way to reach
//! cognition. It cannot propose. It can only act inside its envelope, or
//! stop.
//!
//! **Stopping is the escalation path.** When a reflex cannot stay inside
//! its envelope it returns a `Stop`. The body goes to its safe pose, the
//! stop is recorded, and the *seat* notices — through the ordinary
//! telemetry path, at whatever latency that has — that the body stopped
//! and why. Cognition then decides what to do about it, with all the time
//! in the world, because the body is already safe. This is the opposite
//! of asking permission mid-fall.
//!
//! The consequence worth stating plainly: there is no code path in this
//! crate by which a reflex does something a human would have to approve.
//! Everything a reflex can do was approved before it ran.

use crate::envelope::{Channel, Envelope, Violation};
use brainos_abi::text::Text;
use brainos_abi::time::{Duration, Instant};

/// Why a reflex stopped. Closed list: every one of these is a specific
/// physical situation, and the seat's response differs per case.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum StopReason {
    /// a commanded or measured value left the envelope
    EnvelopeViolation = 1,
    /// the seat has not been heard from within the link grace period
    LinkLost = 2,
    /// the reflex itself did not run within its period
    LoopOverran = 3,
    /// a sensor returned a value that cannot be true (NaN, out of physics)
    SensorImplausible = 4,
    /// the body's own stability invariant predicts a fall
    StabilityLost = 5,
    /// power is below the floor needed to finish a motion safely
    PowerFloor = 6,
    /// the seat commanded a stop
    Commanded = 7,
    /// the reflex was asked to command a channel its envelope never bounded
    UnstatedChannel = 8,
}

impl StopReason {
    pub const fn name(self) -> &'static str {
        match self {
            StopReason::EnvelopeViolation => "envelope-violation",
            StopReason::LinkLost => "link-lost",
            StopReason::LoopOverran => "loop-overran",
            StopReason::SensorImplausible => "sensor-implausible",
            StopReason::StabilityLost => "stability-lost",
            StopReason::PowerFloor => "power-floor",
            StopReason::Commanded => "commanded",
            StopReason::UnstatedChannel => "unstated-channel",
        }
    }

    /// Can the body resume on its own once the condition clears, or does
    /// resuming require a decision from above? The conservative answer is
    /// the default: only a commanded stop and a recovered link resume
    /// without cognition.
    pub fn self_clearing(self) -> bool {
        matches!(self, StopReason::Commanded | StopReason::LinkLost)
    }
}

/// A stop, with enough detail to explain itself later.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Stop {
    pub reason: StopReason,
    pub at: Instant,
    pub violation: Option<Violation>,
}

impl Stop {
    pub fn new(reason: StopReason, at: Instant) -> Stop {
        Stop {
            reason,
            at,
            violation: None,
        }
    }

    pub fn from_violation(v: Violation, at: Instant) -> Stop {
        Stop {
            reason: if v.unstated {
                StopReason::UnstatedChannel
            } else {
                StopReason::EnvelopeViolation
            },
            at,
            violation: Some(v),
        }
    }

    pub fn describe(&self) -> Text<192> {
        let mut t = Text::new();
        t.push_str("stopped: ");
        t.push_str(self.reason.name());
        if let Some(v) = self.violation {
            t.push_str(" — ");
            t.push_str(v.describe().as_str());
        }
        t
    }
}

/// What one pass of a reflex produced.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ReflexOutcome {
    /// a command, already clamped into the envelope
    Command { channel: Channel, value: f32 },
    /// nothing to do this pass
    Hold,
    /// the body must stop
    Stop(Stop),
}

/// A sensor frame handed to a reflex. Fixed size, no allocation, and
/// every field is `Option` because a body may not have that sense — a
/// reflex that requires a missing sense stops rather than assuming.
#[derive(Clone, Copy, Default, Debug)]
pub struct Frame {
    pub at: Instant,
    pub angle: Option<f32>,
    pub rate: Option<f32>,
    pub current: Option<f32>,
    pub voltage: Option<f32>,
    pub tilt: Option<f32>,
    pub temperature: Option<f32>,
    pub speed: Option<f32>,
    pub clearance: Option<f32>,
    /// when the seat was last heard from
    pub last_seat_contact: Instant,
}

impl Frame {
    pub fn channel(&self, ch: Channel) -> Option<f32> {
        match ch {
            Channel::Angle => self.angle,
            Channel::Rate => self.rate,
            Channel::Current => self.current,
            Channel::Voltage => self.voltage,
            Channel::Tilt => self.tilt,
            Channel::Temperature => self.temperature,
            Channel::Speed => self.speed,
            Channel::Clearance => self.clearance,
        }
    }

    /// Is every present reading physically possible? A servo reporting
    /// 4000 °C or a NaN angle is a broken sensor, and acting on a broken
    /// sensor is worse than stopping.
    pub fn is_plausible(&self) -> bool {
        fn ok(v: Option<f32>, lo: f32, hi: f32) -> bool {
            match v {
                None => true,
                Some(x) => x.is_finite() && x >= lo && x <= hi,
            }
        }
        ok(self.angle, -720.0, 720.0)
            && ok(self.rate, -10_000.0, 10_000.0)
            && ok(self.current, -1_000.0, 1_000.0)
            && ok(self.voltage, -100.0, 100.0)
            && ok(self.tilt, -180.0, 180.0)
            && ok(self.temperature, -80.0, 300.0)
            && ok(self.speed, -100.0, 100.0)
            && ok(self.clearance, 0.0, 10_000.0)
    }
}

/// The behaviour half of a reflex. Deliberately tiny: a reflex that needs
/// more than this is not a reflex, it is Tier 1 and belongs above KIRA.
pub trait Reflex {
    /// A stable name, for telemetry.
    fn name(&self) -> &'static str;

    /// Which channel this reflex commands. Declared rather than returned
    /// so the envelope check can happen in `ReflexBank`, once, instead of
    /// in every implementation where it could be forgotten.
    fn channel(&self) -> Channel;

    /// The desired value for this pass, before clamping. `None` means
    /// hold. This is the only method an implementation writes, and it
    /// cannot escape its envelope because it does not get to apply its
    /// own output.
    fn desire(&mut self, frame: &Frame) -> Option<f32>;

    /// An optional extra invariant only this reflex knows — a gait's
    /// tip-over check, say. Returning `Err` stops the body.
    fn invariant(&self, _frame: &Frame) -> Result<(), StopReason> {
        Ok(())
    }
}

/// One reflex bound to one envelope, with its timing state.
struct Bound<'a> {
    reflex: &'a mut dyn Reflex,
    envelope: Envelope,
    last_run: Option<Instant>,
}

/// The set of reflexes running on a body, and the single place where the
/// envelope is enforced.
///
/// Enforcement lives here rather than in each `Reflex` on purpose. An
/// implementation returns a *desire*; this bank turns desires into
/// commands, and it is not possible to write a `Reflex` that bypasses it,
/// because a `Reflex` has no way to reach an actuator at all.
pub struct ReflexBank<'a, const N: usize> {
    bound: [Option<Bound<'a>>; N],
    count: usize,
    stopped: Option<Stop>,
}

impl<'a, const N: usize> Default for ReflexBank<'a, N> {
    fn default() -> ReflexBank<'a, N> {
        ReflexBank::new()
    }
}

impl<'a, const N: usize> ReflexBank<'a, N> {
    pub fn new() -> ReflexBank<'a, N> {
        ReflexBank {
            bound: [const { None }; N],
            count: 0,
            stopped: None,
        }
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Bind a reflex to a granted envelope. Fails if the bank is full or
    /// if the envelope does not bound the channel the reflex commands —
    /// the second is the important one: a reflex whose channel is
    /// unstated has no authorization at all.
    pub fn bind(&mut self, reflex: &'a mut dyn Reflex, envelope: Envelope) -> bool {
        if self.count >= N || !envelope.states(reflex.channel()) {
            return false;
        }
        self.bound[self.count] = Some(Bound {
            reflex,
            envelope,
            last_run: None,
        });
        self.count += 1;
        true
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.is_some()
    }

    pub fn stop_state(&self) -> Option<Stop> {
        self.stopped
    }

    /// Stop everything. Idempotent — the *first* stop is the one kept,
    /// because the first reason is the true one and later stops are
    /// consequences of it.
    pub fn stop(&mut self, stop: Stop) {
        if self.stopped.is_none() {
            self.stopped = Some(stop);
        }
    }

    /// Clear a stop. Refuses for reasons that are not self-clearing: those
    /// need a decision from above, which arrives as a fresh envelope.
    pub fn try_resume(&mut self) -> bool {
        match self.stopped {
            Some(s) if s.reason.self_clearing() => {
                self.stopped = None;
                for b in self.bound.iter_mut().flatten() {
                    b.last_run = None;
                }
                true
            }
            _ => false,
        }
    }

    /// Run every bound reflex for one pass.
    ///
    /// The order of checks is the safety argument, so it is written out:
    /// implausible sensors first (nothing downstream can be trusted),
    /// then the link (a body acting on stale intent is the classic
    /// runaway), then loop timing, then the reflex's own invariant, then
    /// the desire, and only then the envelope. A failure at any point
    /// stops the body and no further reflex runs this pass.
    pub fn run(&mut self, frame: &Frame, out: &mut [ReflexOutcome; N]) -> usize {
        if let Some(s) = self.stopped {
            for slot in out.iter_mut().take(self.count) {
                *slot = ReflexOutcome::Stop(s);
            }
            return self.count;
        }

        if !frame.is_plausible() {
            let s = Stop::new(StopReason::SensorImplausible, frame.at);
            self.stop(s);
            for slot in out.iter_mut().take(self.count) {
                *slot = ReflexOutcome::Stop(s);
            }
            return self.count;
        }

        let mut n = 0;
        for i in 0..self.count {
            let Some(b) = self.bound[i].as_mut() else {
                continue;
            };

            // the link: stale intent is the classic runaway
            let since_seat = frame.at.since(frame.last_seat_contact);
            if since_seat > b.envelope.link_grace {
                let s = Stop::new(StopReason::LinkLost, frame.at);
                out[n] = ReflexOutcome::Stop(s);
                n += 1;
                self.stopped = Some(s);
                break;
            }

            // the loop itself: a reflex that did not run on time did not
            // check anything on time either
            if let Some(last) = b.last_run {
                let gap = frame.at.since(last);
                // one whole period of slack before it counts as an overrun
                if gap > b.envelope.period.saturating_mul(2) {
                    let s = Stop::new(StopReason::LoopOverran, frame.at);
                    out[n] = ReflexOutcome::Stop(s);
                    n += 1;
                    self.stopped = Some(s);
                    break;
                }
            }
            b.last_run = Some(frame.at);

            if let Err(reason) = b.reflex.invariant(frame) {
                let s = Stop::new(reason, frame.at);
                out[n] = ReflexOutcome::Stop(s);
                n += 1;
                self.stopped = Some(s);
                break;
            }

            // every *measured* channel the envelope bounds must also be
            // inside it — a reflex commanding a legal angle while the
            // current is over the limit is still a body about to be hurt
            let mut measured_violation = None;
            for ch in Channel::ALL {
                if let (Some(limit), Some(v)) = (b.envelope.limit(ch), frame.channel(ch)) {
                    if !limit.contains(v) {
                        measured_violation = Some(crate::envelope::Violation {
                            envelope: b.envelope.id,
                            channel: ch,
                            value: v,
                            limit,
                            unstated: false,
                        });
                        break;
                    }
                }
            }
            if let Some(v) = measured_violation {
                let s = Stop::from_violation(v, frame.at);
                out[n] = ReflexOutcome::Stop(s);
                n += 1;
                self.stopped = Some(s);
                break;
            }

            match b.reflex.desire(frame) {
                None => {
                    out[n] = ReflexOutcome::Hold;
                    n += 1;
                }
                Some(v) => {
                    let ch = b.reflex.channel();
                    match b.envelope.clamp(ch, v) {
                        // clamped, not refused: a desire slightly past the
                        // limit is a controller doing its job at the edge,
                        // and the edge is where it should end up
                        Some(clamped) => {
                            out[n] = ReflexOutcome::Command {
                                channel: ch,
                                value: clamped,
                            };
                            n += 1;
                        }
                        // unstated channel: `bind` should have caught this,
                        // so reaching here means the envelope was replaced
                        // with a narrower one underneath a running reflex
                        None => {
                            let s = Stop::new(StopReason::UnstatedChannel, frame.at);
                            out[n] = ReflexOutcome::Stop(s);
                            n += 1;
                            self.stopped = Some(s);
                            break;
                        }
                    }
                }
            }
        }
        n
    }
}

/// The safe pose a body goes to when a reflex stops. Not a behaviour —
/// a fixed set of commands the platform layer applies without running any
/// logic, because the logic is what just failed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SafePose {
    /// hold position with torque, or release and let gravity act
    pub hold_torque: bool,
    /// how long to hold before releasing torque, so a held pose does not
    /// cook the servos indefinitely
    pub hold_for: Duration,
}

impl SafePose {
    /// A legged body holds: releasing torque means falling.
    pub const STAND_DOWN: SafePose = SafePose {
        hold_torque: true,
        hold_for: Duration::from_secs(30),
    };
    /// A wheeled or armed body releases: holding means pressing.
    pub const RELEASE: SafePose = SafePose {
        hold_torque: false,
        hold_for: Duration::ZERO,
    };
}
