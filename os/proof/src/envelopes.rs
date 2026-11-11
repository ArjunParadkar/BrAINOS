//! BFS  LEVEL    Stage 6 - property domains (bounded checking, not a proof)  [crate brainos-proof]
//! BFS  PURPOSE  Enumerates seat-limit and body-limit combinations and checks the real body runtime never applies a command outside their intersection
//! BFS  DEFINES  SeatBound, Calibrated, Geometry, Posture, Ask, Power, EnvelopeCase, EnvelopeDomain, EnvelopeOutcome, run_envelope_case, EnvelopeReport, verify_envelopes
//! BFS  USES     brainos: abi, bodyd, miniblur-core, rt
//! BFS  USED BY  proof/lib, proof/report, proof/tests/proof_suite
//!
//! The second domain: two envelopes, exhaustively.
//!
//! The KIRA domain checks the gate. This one checks the property the
//! *body* path rests on, which is a different claim and needs its own
//! enumeration:
//!
//! > **A body never applies a command outside the intersection of what
//! > the seat granted and what the body's own mechanism permits.**
//!
//! That sentence has more corners in it than it looks. The seat may
//! bound a channel the body says nothing about, or the reverse. The two
//! may overlap partially, not at all, or exactly. A channel neither
//! mentions is unstated, which must refuse rather than permit. And the
//! body may be uncalibrated, stopped, out of power, or disagreeing about
//! the gait — each of which refuses at a different place and for a
//! different reason.
//!
//! Enumerating it is worth more than testing it because the interesting
//! failures are combinations: an empty intersection *and* an unstated
//! channel, a seat limit inside a body limit *and* a command on the
//! boundary. A hand-written suite gets the six cases somebody thought
//! of.
//!
//! The harness runs the real `BodyRuntime` against the real `Hexapod`
//! against the real `miniblur-core` geometry. Nothing is modelled.

use alloc::vec::Vec;
use brainos_abi::time::{Duration, Instant};
use brainos_abi::verb::Verb;
use brainos_bodyd::hexapod::{Hexapod, HexapodConfig, MIN_GAIT_SPEED, TOP_SPEED_MPS};
use brainos_bodyd::mechanism::{Mechanism, Refusal};
use brainos_bodyd::protocol::{Command, Report};
use brainos_bodyd::runtime::{BodyRuntime, Measured, State};
use brainos_rt::envelope::{Channel, Envelope, Limit};
use miniblur_core::geometry::{LegGeom, Vec2};
use miniblur_core::servo::ServoTable;

/// How the seat bounds the joint angle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SeatBound {
    /// wider than the body's own reach
    Wider,
    /// exactly the body's reach
    Exact,
    /// narrower than the body needs for a gait
    Narrower,
    /// disjoint from what the body can reach at all
    Disjoint,
    /// the seat says nothing about this channel
    Unstated,
}

/// Whether the entity pushed a calibration down.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Calibrated {
    Yes,
    No,
}

/// What the body is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Geometry {
    /// the default Mini-Blur
    Balanced,
    /// battery moved aft; the gait would tip it
    TailHeavy,
}

/// What state the body is in when the command arrives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Posture {
    Ready,
    Stopped,
}

/// What the seat asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ask {
    /// a walk at a speed the gait supports
    GoodWalk,
    /// a walk slower than the gait's floor
    Creep,
    /// a walk faster than the body's top speed
    TooFast,
    /// stand still
    Stand,
    /// a verb this body does not have
    Unknown,
}

/// Whether the rail can feed it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Power {
    Good,
    Flat,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EnvelopeCase {
    pub seat: SeatBound,
    pub calibrated: Calibrated,
    pub geometry: Geometry,
    pub posture: Posture,
    pub ask: Ask,
    pub power: Power,
}

impl EnvelopeCase {
    /// Should the body accept it?
    ///
    /// Written independently of the runtime, as an oracle. The order
    /// mirrors the order the body checks in, which is itself a claim
    /// worth having written down twice.
    pub fn should_accept(&self) -> bool {
        // stopped bodies need a decision, not a retry
        if self.posture == Posture::Stopped {
            return false;
        }
        if self.calibrated == Calibrated::No {
            return false;
        }
        if self.ask == Ask::Unknown {
            return false;
        }
        // a seat envelope disjoint from the body's reach leaves an empty
        // intersection, which is unstated, which refuses
        if self.seat == SeatBound::Disjoint {
            return false;
        }
        match self.ask {
            Ask::Stand => {
                // standing commands the joint at zero, which is inside
                // any non-disjoint bound; power does not gate a hold
                self.seat != SeatBound::Disjoint
            }
            Ask::Creep | Ask::TooFast => false,
            Ask::GoodWalk => {
                if self.power == Power::Flat {
                    return false;
                }
                if self.geometry == Geometry::TailHeavy {
                    return false;
                }
                // a seat bound narrower than the gait's own swing refuses
                // at the envelope check
                self.seat != SeatBound::Narrower
            }
            Ask::Unknown => false,
        }
    }
}

pub struct EnvelopeDomain;

impl EnvelopeDomain {
    pub const SEATS: [SeatBound; 5] = [
        SeatBound::Wider,
        SeatBound::Exact,
        SeatBound::Narrower,
        SeatBound::Disjoint,
        SeatBound::Unstated,
    ];
    pub const CALIBRATED: [Calibrated; 2] = [Calibrated::Yes, Calibrated::No];
    pub const GEOMETRIES: [Geometry; 2] = [Geometry::Balanced, Geometry::TailHeavy];
    pub const POSTURES: [Posture; 2] = [Posture::Ready, Posture::Stopped];
    pub const ASKS: [Ask; 5] = [
        Ask::GoodWalk,
        Ask::Creep,
        Ask::TooFast,
        Ask::Stand,
        Ask::Unknown,
    ];
    pub const POWERS: [Power; 2] = [Power::Good, Power::Flat];

    pub const SIZE: usize = 5 * 2 * 2 * 2 * 5 * 2;

    pub fn enumerate() -> Vec<EnvelopeCase> {
        let mut out = Vec::with_capacity(EnvelopeDomain::SIZE);
        for seat in EnvelopeDomain::SEATS {
            for calibrated in EnvelopeDomain::CALIBRATED {
                for geometry in EnvelopeDomain::GEOMETRIES {
                    for posture in EnvelopeDomain::POSTURES {
                        for ask in EnvelopeDomain::ASKS {
                            for power in EnvelopeDomain::POWERS {
                                out.push(EnvelopeCase {
                                    seat,
                                    calibrated,
                                    geometry,
                                    posture,
                                    ask,
                                    power,
                                });
                            }
                        }
                    }
                }
            }
        }
        out
    }
}

fn verb(a: Ask) -> Verb {
    match a {
        Ask::Stand => Verb::literal("legs.stand"),
        Ask::Unknown => Verb::literal("arm.grasp"),
        _ => Verb::literal("legs.walk"),
    }
}

fn command(a: Ask) -> Command {
    match a {
        Ask::GoodWalk => Command::new(verb(a))
            .arg("speed", TOP_SPEED_MPS * 0.7)
            .arg("heading", 0.0),
        Ask::Creep => Command::new(verb(a))
            .arg("speed", TOP_SPEED_MPS * MIN_GAIT_SPEED * 0.5)
            .arg("heading", 0.0),
        Ask::TooFast => Command::new(verb(a))
            .arg("speed", TOP_SPEED_MPS * 5.0)
            .arg("heading", 0.0),
        Ask::Stand => Command::new(verb(a)).arg("height", 0.0),
        Ask::Unknown => Command::new(verb(a)),
    }
}

fn seat_envelope(s: SeatBound, native_reach: f32) -> Envelope {
    let mut e = Envelope::new(
        1,
        Duration::from_millis(20),
        Duration::from_millis(200),
    );
    match s {
        SeatBound::Wider => {
            e.bound(Channel::Angle, Limit::new(-180.0, 180.0));
        }
        SeatBound::Exact => {
            e.bound(Channel::Angle, Limit::new(-native_reach, native_reach));
        }
        SeatBound::Narrower => {
            e.bound(Channel::Angle, Limit::new(-1.0, 1.0));
        }
        SeatBound::Disjoint => {
            e.bound(Channel::Angle, Limit::new(120.0, 180.0));
        }
        SeatBound::Unstated => {}
    }
    // the other channels are always generous, so the case is about the
    // angle rather than about something else quietly refusing
    e.bound(Channel::Current, Limit::new(0.0, 10.0));
    e.bound(Channel::Tilt, Limit::new(-45.0, 45.0));
    e.bound(Channel::Voltage, Limit::new(5.0, 9.0));
    e
}

/// What happened when a case was run.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvelopeOutcome {
    pub report: Report,
    pub accepted: bool,
    /// every command the body actually applied
    pub applied: Vec<(Channel, f32)>,
    /// the intersection the body computed
    pub effective: Option<Envelope>,
    pub final_state: State,
}

/// Run one case against the real runtime.
pub fn run_envelope_case(case: &EnvelopeCase) -> EnvelopeOutcome {
    let geom = match case.geometry {
        Geometry::Balanced => LegGeom::default(),
        Geometry::TailHeavy => LegGeom {
            com: Vec2::new(-70.0, 0.0),
            ..LegGeom::default()
        },
    };
    let mut hexapod = Hexapod::new(HexapodConfig {
        geom,
        vbat: match case.power {
            Power::Good => 7.4,
            Power::Flat => 6.0,
        },
        ..HexapodConfig::default()
    });
    if case.calibrated == Calibrated::Yes {
        hexapod.calibrate(ServoTable::bom_10_servo());
    }
    let native_reach = hexapod
        .native_envelope(1)
        .limit(Channel::Angle)
        .map(|l| l.max)
        .unwrap_or(0.0);

    let mut rt = BodyRuntime::new(hexapod, Instant(0));
    rt.narrow_to(seat_envelope(case.seat, native_reach));
    rt.ready();
    if case.posture == Posture::Stopped {
        rt.commanded_stop(Instant(0));
    }

    let cmd = command(case.ask);
    let report = rt.accept(&cmd, Instant(0));
    let accepted = matches!(report, Report::Accepted { .. });

    // play it out and record everything that was actually applied
    let mut applied = Vec::new();
    let mut t = 0u64;
    for _ in 0..400 {
        rt.heard_from_seat(Instant(t));
        let pass = rt.tick(Instant(t), Measured {
            rail_v: Some(7.4),
            current_a: Some(0.4),
            tilt_deg: Some(1.0),
            hottest_c: Some(38.0),
        });
        if let Some(s) = pass.applied {
            for (ch, v) in s.commanded() {
                applied.push((ch, v));
            }
        }
        if pass.state == State::Stopped || pass.state == State::Idle {
            break;
        }
        t += 50_000;
    }

    EnvelopeOutcome {
        report,
        accepted,
        applied,
        effective: rt.effective_envelope(),
        final_state: rt.state(),
    }
}

/// The properties.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvelopeReport {
    pub cases: usize,
    pub accepted: usize,
    pub failures: Vec<(usize, &'static str)>,
    /// how many distinct refusals were produced, so a vacuous run is
    /// visible
    pub distinct_refusals: usize,
}

impl EnvelopeReport {
    pub fn holds(&self) -> bool {
        self.failures.is_empty() && self.is_meaningful()
    }

    /// Did the domain actually exercise anything?
    pub fn is_meaningful(&self) -> bool {
        self.accepted > 0 && self.accepted < self.cases && self.distinct_refusals >= 4
    }
}

/// Enumerate, run, and check.
pub fn verify_envelopes() -> EnvelopeReport {
    let cases = EnvelopeDomain::enumerate();
    let mut failures: Vec<(usize, &'static str)> = Vec::new();
    let mut accepted = 0usize;
    let mut refusals: Vec<Refusal> = Vec::new();

    for (i, case) in cases.iter().enumerate() {
        let out = run_envelope_case(case);
        if out.accepted {
            accepted += 1;
        }
        if let Report::Refused { refusal, .. } = out.report {
            if !refusals.contains(&refusal) {
                refusals.push(refusal);
            }
        }

        // --- E1: nothing is ever applied outside the effective envelope ---
        //
        // The property the whole body path rests on. Note that it is
        // checked against what was *applied*, not against what was
        // planned: a plan that passed and then a sample that escaped
        // would satisfy a weaker check and still be a fall.
        match out.effective {
            None => {
                if !out.applied.is_empty() {
                    failures.push((i, "a body with no effective envelope applied a command"));
                }
            }
            Some(env) => {
                for (ch, v) in &out.applied {
                    if env.check(*ch, *v).is_err() {
                        failures.push((
                            i,
                            "a command was applied outside the intersection of the two envelopes",
                        ));
                        break;
                    }
                }
            }
        }

        // --- E2: the effective envelope is never wider than either input ---
        if let Some(effective) = out.effective {
            let native_reach = {
                let mut h = Hexapod::new(HexapodConfig::default());
                h.calibrate(ServoTable::bom_10_servo());
                h.native_envelope(1).limit(Channel::Angle).map(|l| l.max)
            };
            let seat = seat_envelope(case.seat, native_reach.unwrap_or(0.0));
            for ch in Channel::ALL {
                let Some(e) = effective.limit(ch) else { continue };
                if let Some(s) = seat.limit(ch) {
                    if !e.is_within(&s) {
                        failures.push((i, "the effective envelope is wider than the seat granted"));
                        break;
                    }
                }
            }
        }

        // --- E3: a refused command moves nothing ---
        if !out.accepted && !out.applied.is_empty() {
            failures.push((i, "a refused command still moved the body"));
        }

        // --- E4: the oracle and the runtime agree ---
        if out.accepted != case.should_accept() {
            failures.push((i, "the runtime and the oracle disagree about acceptance"));
        }

        // --- E5: an uncalibrated body never moves ---
        if case.calibrated == Calibrated::No && !out.applied.is_empty() {
            failures.push((i, "an uncalibrated body moved"));
        }

        // --- E6: a stopped body never moves ---
        if case.posture == Posture::Stopped && !out.applied.is_empty() {
            failures.push((i, "a stopped body moved"));
        }

        // --- E7: a body whose geometry would tip never walks ---
        if case.geometry == Geometry::TailHeavy
            && matches!(case.ask, Ask::GoodWalk)
            && out.accepted
        {
            failures.push((i, "a body that would tip over accepted a walk"));
        }

        if failures.len() >= 8 {
            break;
        }
    }

    EnvelopeReport {
        cases: cases.len(),
        accepted,
        failures,
        distinct_refusals: refusals.len(),
    }
}
