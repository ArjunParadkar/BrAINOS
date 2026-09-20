//! The Domain 2 loop.
//!
//! One pass does five things, in this order, and the order is the safety
//! argument:
//!
//! ```text
//!   1  read the wire           — has the seat said anything?
//!   2  check the watchdogs     — is anything overdue?
//!   3  check the envelope      — are the measurements still inside it?
//!   4  play one sample         — if there is a motion and nothing objects
//!   5  report                  — telemetry up the wire
//! ```
//!
//! Reading the wire first means a stop command arrives before the next
//! sample is played. Checking the watchdogs before the envelope means a
//! loop that has stopped running is caught even when its last readings
//! looked fine. Playing the sample last means everything that could
//! refuse has already had its turn.
//!
//! **The runtime never decides what to do.** It plays a motion the
//! mechanism planned and the envelope permits, or it stops. Stopping is
//! the only escalation available and it is enough — the seat sees the
//! stop in ordinary telemetry and cognition decides, with the body
//! already safe.
//!
//! **Two envelopes, intersected.** The seat grants one; the mechanism
//! has its own. The runtime commands only inside both, which is how a
//! compromised seat cannot push the body past what its own reflexes
//! allow. `granted_envelope` can only ever narrow what the mechanism
//! would permit, never widen it.

use crate::mechanism::{Mechanism, Motion, Refusal, Sample};
use crate::protocol::{Command, Report};
use alloc::vec::Vec;
use brainos_abi::text::Text;
use brainos_abi::time::{Duration, Instant};
use brainos_rt::envelope::{Channel, Envelope, Limit};
use brainos_rt::reflex::{Stop, StopReason};
use brainos_rt::watchdog::{Bite, Watchdog, WatchdogSet};

/// What the body is doing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// powered, calibrated, nothing in progress
    Idle,
    /// playing a motion
    Playing,
    /// stopped by a reflex; needs a fresh envelope from above
    Stopped,
    /// no calibration; the entity has not pushed it down yet
    Uncalibrated,
}

/// What one pass produced.
#[derive(Clone, Debug, PartialEq)]
pub struct Pass {
    pub state: State,
    /// the sample applied this pass, if any
    pub applied: Option<Sample>,
    /// anything to send up the wire
    pub reports: Vec<Report>,
    /// set the moment the body stops
    pub stop: Option<Stop>,
}

/// Measured reality, from the body's own sensors.
#[derive(Clone, Copy, Debug, Default)]
pub struct Measured {
    pub rail_v: Option<f32>,
    pub current_a: Option<f32>,
    pub tilt_deg: Option<f32>,
    pub hottest_c: Option<f32>,
}

/// The body side.
pub struct BodyRuntime<M: Mechanism> {
    pub mechanism: M,
    /// what the seat granted. `None` means nothing is authorized, which
    /// means nothing moves.
    granted: Option<Envelope>,
    /// the intersection of the granted envelope and the mechanism's own
    effective: Option<Envelope>,
    motion: Option<Motion>,
    next_sample: usize,
    next_due: Instant,
    state: State,
    stop: Option<Stop>,
    dogs: WatchdogSet<4>,
    last_seat: Instant,
    /// how long the body will act with no word from the seat
    pub link_grace: Duration,
    pub stops: u32,
    pub overruns: u32,
    pub samples_played: u32,
    last_measured: Measured,
}

impl<M: Mechanism> BodyRuntime<M> {
    pub fn new(mechanism: M, now: Instant) -> BodyRuntime<M> {
        let mut dogs = WatchdogSet::new();
        // the loop itself must keep running
        dogs.add(Watchdog::liveness(
            "loop",
            Duration::from_millis(100),
            now,
        ));
        // and it must keep getting somewhere: a loop that runs and pets
        // and produces nothing is what a deadlocked state machine looks
        // like from outside
        dogs.add(Watchdog::progress(
            "motion",
            Duration::from_secs(5),
            now,
        ));

        BodyRuntime {
            state: if mechanism.name().is_empty() {
                State::Uncalibrated
            } else {
                State::Uncalibrated
            },
            mechanism,
            granted: None,
            effective: None,
            motion: None,
            next_sample: 0,
            next_due: now,
            stop: None,
            dogs,
            last_seat: now,
            link_grace: Duration::from_millis(200),
            stops: 0,
            overruns: 0,
            samples_played: 0,
            last_measured: Measured::default(),
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn is_stopped(&self) -> bool {
        self.state == State::Stopped
    }

    pub fn motion(&self) -> Option<&Motion> {
        self.motion.as_ref()
    }

    pub fn effective_envelope(&self) -> Option<Envelope> {
        self.effective
    }

    /// The body is calibrated and may accept motions.
    pub fn ready(&mut self) {
        if self.state == State::Uncalibrated {
            self.state = State::Idle;
        }
    }

    /// The seat grants an envelope. The body intersects it with its own
    /// and keeps the tighter of the two, per channel.
    ///
    /// `narrow_to` rather than `set`: there is no operation here that
    /// widens what the body will do. A seat that sent a wider envelope
    /// than the mechanism's gets the mechanism's.
    pub fn narrow_to(&mut self, granted: Envelope) {
        let native = self.mechanism.native_envelope(granted.id);
        let mut effective = Envelope::new(granted.id, granted.period, granted.link_grace);
        for ch in Channel::ALL {
            match (granted.limit(ch), native.limit(ch)) {
                (Some(g), Some(n)) => {
                    let tight = Limit::new(g.min.max(n.min), g.max.min(n.max));
                    if tight.is_sane() {
                        effective.bound(ch, tight);
                    }
                    // an empty intersection leaves the channel unstated,
                    // which refuses — the correct answer when the seat
                    // and the body disagree about what is possible
                }
                // a channel only one of them bounds is bounded by that one
                (Some(g), None) => {
                    effective.bound(ch, g);
                }
                (None, Some(n)) => {
                    effective.bound(ch, n);
                }
                (None, None) => {}
            }
        }
        self.granted = Some(granted);
        self.effective = Some(effective);
        self.link_grace = granted.link_grace;
    }

    /// Clear a stop. Only from above, and only with a fresh envelope —
    /// which is what makes a stop an escalation rather than a retry.
    pub fn resume(&mut self, granted: Envelope, now: Instant) {
        self.narrow_to(granted);
        self.stop = None;
        self.motion = None;
        self.next_sample = 0;
        self.state = State::Idle;
        self.dogs.clear_all(now);
        self.last_seat = now;
    }

    /// The seat said something.
    pub fn heard_from_seat(&mut self, now: Instant) {
        self.last_seat = now;
    }

    /// Accept a command — or refuse it.
    ///
    /// This is where the duplication lives. The seat has already gated
    /// this and minted a token; the body plans it again from its own
    /// numbers and may still say no.
    pub fn accept(&mut self, cmd: &Command, now: Instant) -> Report {
        self.heard_from_seat(now);

        if self.state == State::Stopped {
            return Report::Refused {
                verb: cmd.verb,
                refusal: Refusal::Stopped,
                disagreement: false,
            };
        }
        if self.state == State::Uncalibrated {
            return Report::Refused {
                verb: cmd.verb,
                refusal: Refusal::Uncalibrated,
                disagreement: false,
            };
        }
        let Some(effective) = self.effective else {
            return Report::Refused {
                verb: cmd.verb,
                refusal: Refusal::OutsideEnvelope,
                disagreement: false,
            };
        };

        let args = cmd.as_args();
        let motion = match self.mechanism.plan(&cmd.verb, &args) {
            Ok(m) => m,
            Err(r) => {
                return Report::Refused {
                    verb: cmd.verb,
                    refusal: r,
                    disagreement: r.is_a_disagreement(),
                }
            }
        };

        // every sample the mechanism planned must also be inside the
        // effective envelope. The mechanism checked its own physics; this
        // checks the authority.
        for s in &motion.samples {
            for (ch, v) in s.commanded() {
                if effective.check(ch, v).is_err() {
                    return Report::Refused {
                        verb: cmd.verb,
                        refusal: Refusal::OutsideEnvelope,
                        disagreement: true,
                    };
                }
            }
        }

        let report = Report::Accepted {
            verb: motion.verb,
            samples: motion.samples.len() as u32,
            duration_ms: motion.duration.as_millis() as u32,
            worst_margin: motion.worst_margin,
        };
        self.motion = Some(motion);
        self.next_sample = 0;
        self.next_due = now;
        self.state = State::Playing;
        report
    }

    /// The seat commanded a stop.
    pub fn commanded_stop(&mut self, now: Instant) {
        self.enter_stop(Stop::new(StopReason::Commanded, now));
    }

    fn enter_stop(&mut self, stop: Stop) {
        if self.stop.is_none() {
            self.stop = Some(stop);
            self.stops = self.stops.saturating_add(1);
        }
        self.state = State::Stopped;
        self.motion = None;
    }

    /// One pass of the loop.
    pub fn tick(&mut self, now: Instant, measured: Measured) -> Pass {
        self.last_measured = measured;
        let mut reports = Vec::new();

        // --- 1. the watchdogs ---
        self.dogs.pet("loop", now, 0);
        self.dogs.pet("motion", now, self.samples_played as u64);
        if let Some((name, bite)) = self.dogs.check(now) {
            let reason = match bite {
                Bite::Late | Bite::Early => StopReason::LoopOverran,
                Bite::Stalled => StopReason::LoopOverran,
            };
            let _ = name;
            self.overruns = self.overruns.saturating_add(1);
            self.enter_stop(Stop::new(reason, now));
        }

        // --- 2. the link ---
        if self.state != State::Stopped && now.since(self.last_seat) > self.link_grace {
            self.enter_stop(Stop::new(StopReason::LinkLost, now));
        }

        // --- 3. the envelope, against what is actually measured ---
        if self.state != State::Stopped {
            if let Some(env) = self.effective {
                for (ch, value) in [
                    (Channel::Current, measured.current_a),
                    (Channel::Voltage, measured.rail_v),
                    (Channel::Tilt, measured.tilt_deg),
                    (Channel::Temperature, measured.hottest_c),
                ] {
                    let Some(v) = value else { continue };
                    if !v.is_finite() {
                        self.enter_stop(Stop::new(StopReason::SensorImplausible, now));
                        break;
                    }
                    if let Some(limit) = env.limit(ch) {
                        if !limit.contains(v) {
                            self.enter_stop(Stop::from_violation(
                                brainos_rt::envelope::Violation {
                                    envelope: env.id,
                                    channel: ch,
                                    value: v,
                                    limit,
                                    unstated: false,
                                },
                                now,
                            ));
                            break;
                        }
                    }
                }
            }
        }

        // --- 4. play a sample ---
        let mut applied = None;
        if self.state == State::Playing && now.has_passed(self.next_due) {
            let step = self
                .motion
                .as_ref()
                .and_then(|m| m.samples.get(self.next_sample).copied());
            match step {
                None => {
                    let verb = self.motion.as_ref().map(|m| m.verb);
                    if let Some(v) = verb {
                        reports.push(Report::Finished {
                            verb: v,
                            samples_played: self.next_sample as u32,
                        });
                    }
                    self.motion = None;
                    self.next_sample = 0;
                    self.state = State::Idle;
                }
                Some(sample) => {
                    // the mechanism's own invariant, evaluated now rather
                    // than only at planning time: between planning and
                    // playing, the world may have moved
                    if let Err(r) = self.mechanism.invariant(&sample) {
                        let _ = r;
                        self.enter_stop(Stop::new(StopReason::StabilityLost, now));
                    } else {
                        let mut inside = true;
                        if let Some(env) = self.effective {
                            for (ch, v) in sample.commanded() {
                                if env.check(ch, v).is_err() {
                                    inside = false;
                                    self.enter_stop(Stop::from_violation(
                                        brainos_rt::envelope::Violation {
                                            envelope: env.id,
                                            channel: ch,
                                            value: v,
                                            limit: env
                                                .limit(ch)
                                                .unwrap_or(Limit::UNBOUNDED),
                                            unstated: env.limit(ch).is_none(),
                                        },
                                        now,
                                    ));
                                    break;
                                }
                            }
                        } else {
                            inside = false;
                            self.enter_stop(Stop::new(StopReason::UnstatedChannel, now));
                        }
                        if inside {
                            applied = Some(sample);
                            self.samples_played = self.samples_played.saturating_add(1);
                            self.next_sample += 1;
                            self.next_due = now + sample.hold;
                        }
                    }
                }
            }
        }

        // --- 5. report ---
        if let Some(stop) = self.stop {
            if reports.is_empty() {
                reports.push(Report::Stopped {
                    reason: stop.reason as u8,
                    samples_played: self.samples_played,
                });
            }
        }
        reports.push(Report::Telemetry {
            rail_v: measured.rail_v.unwrap_or(0.0),
            current_a: measured.current_a.unwrap_or(0.0),
            tilt_deg: measured.tilt_deg.unwrap_or(0.0),
            hottest_c: measured.hottest_c.unwrap_or(0.0),
            margin_mm: applied.map(|s| s.margin).unwrap_or(f32::INFINITY),
            stops: self.stops,
            overruns: self.overruns,
        });

        Pass {
            state: self.state,
            applied,
            reports,
            stop: self.stop,
        }
    }

    /// Where the body goes when it has stopped. Never fails.
    pub fn safe_pose(&self) -> Sample {
        self.mechanism.safe()
    }

    pub fn describe(&self) -> Text<160> {
        let mut t = Text::new();
        t.push_str(self.mechanism.name());
        t.push(' ');
        t.push_str(match self.state {
            State::Idle => "idle",
            State::Playing => "playing",
            State::Stopped => "STOPPED",
            State::Uncalibrated => "uncalibrated",
        });
        t.push_str(" samples=");
        t.push_u64(self.samples_played as u64);
        if self.stops > 0 {
            t.push_str(" stops=");
            t.push_u64(self.stops as u64);
        }
        t
    }
}
