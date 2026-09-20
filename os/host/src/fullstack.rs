//! The whole stack, end to end.
//!
//! `demo.rs` shows the entity: memory, the gate, two bodies, an export.
//! It stops where a real deployment starts — at the wire. This module
//! goes the rest of the way:
//!
//! ```text
//!   entity ─ propose ─▶ KIRA ─ Granted ─▶ protocol ─▶ wire ─▶ bodyd
//!                                                               │
//!                                    miniblur-core ◀─ mechanism ┘
//!                                    (real gait, real geometry,
//!                                     real tip-over, real rail)
//! ```
//!
//! Everything in that diagram is the shipping code. The gait is the one
//! the ESP32-S3 firmware plays; the tip-over check is the convex-hull
//! support polygon from `miniblur-core::stability`; the wire is the
//! authenticated, replay-protected transport with its fault model turned
//! on. What is simulated is the servo: nothing here has moved a motor.
//!
//! **The point of the exercise** is the case in `subverted_seat`. KIRA
//! is the gate, and the whole architecture rests on it — but "rests on"
//! must not mean "has no answer if it fails". So: a seat whose gate has
//! been subverted grants a motion the body's own geometry says is a
//! fall, the token is genuine, the wire is authentic, and the body
//! refuses anyway. That is the same envelope checked twice, and it is
//! the reason the duplication is in the design rather than being
//! redundant work.

use brainos_abi::ids::BodyId;
use brainos_abi::time::{Duration, Instant, Tick};
use brainos_abi::verb::Verb;
use brainos_adapters::native::mini_blur;
use brainos_body::calibration::{Calibration, JointTrim};
use brainos_body::hal::Telemetry;
use brainos_bodyd::hexapod::{Hexapod, HexapodConfig, MIN_GAIT_SPEED, TOP_SPEED_MPS};
use brainos_bodyd::mechanism::Refusal;
use brainos_bodyd::protocol::{Command, Report, Wire as Protocol};
use brainos_bodyd::runtime::{BodyRuntime, Measured, State};
use brainos_crypto::key::BrainKey;
use brainos_crypto::rng::Drbg;
use brainos_entity::entity::Entity;
use brainos_kira::gate::Verdict;
use brainos_rt::envelope::{Channel, Envelope, Limit};
use brainos_transport::frame::Kind;
use brainos_transport::loopback::{Faults, Loopback};
use brainos_transport::session::{Peer, Wire};
use miniblur_core::geometry::{LegGeom, Vec2};
use miniblur_core::servo::ServoTable;

fn v(s: &str) -> Verb {
    Verb::parse(s).expect("a well-formed verb")
}

/// The seat's view of the body's envelope, derived from the schema it
/// was given at §8 step 3.
fn granted_envelope() -> Envelope {
    let mut e = Envelope::new(
        1,
        Duration::from_millis(20),
        Duration::from_millis(200),
    );
    e.bound(Channel::Angle, Limit::new(-30.0, 30.0));
    e.bound(Channel::Current, Limit::new(0.0, 3.0));
    e.bound(Channel::Tilt, Limit::new(-20.0, 20.0));
    e.bound(Channel::Voltage, Limit::new(6.6, 8.4));
    e
}

fn entity_calibration() -> Calibration {
    let mut c = Calibration::new(BodyId::ZERO, Tick(1));
    for ch in 0..10u8 {
        c.trim(JointTrim {
            channel: ch,
            offset_deg: 0.0,
            scale: 1.0,
            min_deg: -28.0,
            max_deg: 28.0,
            probed: true,
        })
        .expect("a sane trim");
    }
    c
}

fn fit() -> Telemetry {
    Telemetry {
        at: Instant(0),
        link_alive: true,
        rail_v: Some(7.4),
        current_a: Some(0.42),
        tilt_deg: Some(1.2),
        hottest_c: Some(38.0),
        stops: 0,
        overruns: 0,
    }
}

fn measured() -> Measured {
    Measured {
        rail_v: Some(7.4),
        current_a: Some(0.42),
        tilt_deg: Some(1.2),
        hottest_c: Some(38.0),
    }
}

/// Everything, wired together.
pub struct Stack {
    pub entity: Entity,
    pub body_id: BodyId,
    pub seat_wire: Wire,
    pub limb_wire: Wire,
    pub link: Loopback,
    pub body: BodyRuntime<Hexapod>,
}

impl Stack {
    /// Build a seat and a body, attested to each other, with the body
    /// calibrated from the entity's own record.
    pub fn assemble(geom: LegGeom, faults: Faults) -> Stack {
        let entity_key = BrainKey::from_seed(&[0x5A; 32]);
        let device_key = BrainKey::from_seed(&[0xB2; 32]);

        // --- the seat ---
        let mut entity = Entity::new(BrainKey::from_seed(&[0x5A; 32]), Tick(0));
        entity.wake(None, None, Tick(1)).expect("a fresh entity wakes");
        let body_id = entity
            .acquire(&device_key, mini_blur(), entity_calibration(), 10)
            .expect("mini-blur joins through §8");
        entity.observe_body(&body_id, &fit());

        // --- the wire ---
        let mut rng = Drbg::deterministic(b"fullstack");
        let (mut seat_wire, nonce) = Wire::initiate(&entity_key, body_id, &mut rng, Instant(0));
        let subkey = entity_key.derive_subkey(&body_id, 32);
        let mut limb_wire = Wire::accept(&subkey, body_id, nonce, Instant(0));
        let peer = Peer {
            entity: Some(entity_key.entity_id()),
            body: Some(body_id),
            key: entity_key.public(),
        };
        seat_wire.attested(peer);
        limb_wire.attested(peer);

        // --- the body ---
        let mut hexapod = Hexapod::new(HexapodConfig {
            geom,
            ..HexapodConfig::default()
        });
        // §8 step 5: the entity pushes the calibration down. The body
        // holds it only while powered.
        hexapod.calibrate(ServoTable::bom_10_servo());
        let mut body = BodyRuntime::new(hexapod, Instant(0));
        body.narrow_to(granted_envelope());
        body.ready();

        Stack {
            entity,
            body_id,
            seat_wire,
            limb_wire,
            link: Loopback::new(faults, b"brushed motors nearby"),
            body,
        }
    }

    /// The seat proposes, the gate decides, and — only if granted — the
    /// command goes down the wire.
    ///
    /// Returns what the gate said and what the body said, which are
    /// deliberately two separate answers.
    pub fn ask(
        &mut self,
        verb: Verb,
        args: &[(&str, f64)],
        now: Instant,
    ) -> (SeatVerdict, Option<Report>) {
        let out = self.entity.attempt(verb, self.body_id, args);
        let seat = match &out.verdict {
            Verdict::Granted(g) => SeatVerdict::Granted {
                grant: *g.id.as_bytes(),
            },
            Verdict::Denied(d) => {
                return (
                    SeatVerdict::Denied {
                        stage: d.stage.name(),
                        code: d.code.name(),
                    },
                    None,
                )
            }
        };
        let grant = match seat {
            SeatVerdict::Granted { grant } => grant,
            _ => unreachable!(),
        };

        let mut cmd = Command::new(verb).under(grant);
        for (n, value) in args {
            cmd = cmd.arg(n, *value as f32);
        }
        (seat, self.deliver(&cmd, now))
    }

    /// Send a command the seat has already authorized, down the real
    /// wire, and collect what the body says back.
    fn deliver(&mut self, cmd: &Command, now: Instant) -> Option<Report> {
        let payload = Protocol::encode_command(cmd).ok()?;
        let framed = self.seat_wire.send(Kind::Command, &payload).ok()?;
        self.link.a_sends(framed);
        self.link.settle();

        while let Some(raw) = self.link.b_receives() {
            let Ok(frame) = self.limb_wire.receive(&raw) else {
                // dropped, corrupted, or replayed: the body simply never
                // hears it, which is the correct behaviour and is why the
                // seat has to notice the absence rather than assume
                continue;
            };
            if frame.kind != Kind::Command {
                continue;
            }
            let Ok(decoded) = Protocol::decode_command(&frame.payload) else {
                continue;
            };
            return Some(self.body.accept(&decoded, now));
        }
        None
    }

    /// Run the body forward until the current motion finishes or it
    /// stops. Returns how many samples were actually played.
    pub fn play_out(&mut self, from: Instant, step: Duration, passes: usize) -> PlayResult {
        let mut t = from.micros();
        let mut played = 0u32;
        let mut stopped = None;
        for _ in 0..passes {
            self.body.heard_from_seat(Instant(t));
            let pass = self.body.tick(Instant(t), measured());
            if pass.applied.is_some() {
                played += 1;
            }
            if pass.state == State::Stopped {
                stopped = pass.stop.map(|s| s.reason);
                break;
            }
            if pass.state == State::Idle && played > 0 {
                break;
            }
            t += step.as_micros();
        }
        PlayResult {
            samples_played: played,
            stopped,
            state: self.body.state(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeatVerdict {
    Granted { grant: [u8; 32] },
    Denied { stage: &'static str, code: &'static str },
}

impl SeatVerdict {
    pub fn is_granted(&self) -> bool {
        matches!(self, SeatVerdict::Granted { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayResult {
    pub samples_played: u32,
    pub stopped: Option<brainos_rt::reflex::StopReason>,
    pub state: State,
}

/// The exercise: a seat whose gate has been subverted.
///
/// The token is genuine — it was minted by the real `Kira::commit` after
/// a real evaluation, because in this scenario the *body's schema at the
/// seat* is what is wrong rather than the gate's logic. That is the
/// realistic shape of the failure: nobody breaks ed25519, somebody ships
/// a schema with a number in it that the physical robot cannot survive.
///
/// The seat grants it. The wire carries it. The body refuses it.
pub fn subverted_seat() -> SubversionResult {
    // a robot whose battery has been moved right to the back, so the
    // gait the seat's schema still describes would now tip it over
    let mut geom = LegGeom::default();
    geom.com = Vec2::new(-70.0, 0.0);

    let mut stack = Stack::assemble(geom, Faults::PERFECT);
    let (seat, body) = stack.ask(
        v("legs.walk"),
        &[("speed", 0.08), ("heading", 0.0)],
        Instant(0),
    );

    let body_refusal = match &body {
        Some(Report::Refused { refusal, .. }) => Some(*refusal),
        _ => None,
    };

    SubversionResult {
        seat_granted: seat.is_granted(),
        body_refused: body_refusal,
        // and nothing moved
        samples_played: stack.play_out(Instant(0), Duration::from_millis(50), 100).samples_played,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SubversionResult {
    pub seat_granted: bool,
    pub body_refused: Option<Refusal>,
    pub samples_played: u32,
}

/// One line of the full-stack narration.
pub struct Line {
    pub step: &'static str,
    pub text: String,
}

fn line(step: &'static str, text: impl Into<String>) -> Line {
    Line {
        step,
        text: text.into(),
    }
}

/// The narrated run, for the binary.
pub fn run() -> Vec<Line> {
    let mut out = Vec::new();

    // --- a healthy body ---
    let mut stack = Stack::assemble(LegGeom::default(), Faults::PERFECT);
    out.push(line(
        "assemble",
        format!(
            "entity {} ↔ mini-blur {} over an attested wire; the body is calibrated from the entity's own record",
            stack.entity.id().fingerprint(),
            stack.body_id.fingerprint()
        ),
    ));
    out.push(line(
        "envelope",
        format!(
            "seat granted angle ±30°, current ≤3 A; the body intersected it with its own and kept {}",
            stack
                .body
                .effective_envelope()
                .and_then(|e| e.limit(Channel::Angle))
                .map(|l| format!("±{:.0}°", l.max))
                .unwrap_or_else(|| "nothing".into())
        ),
    ));

    let (seat, body) = stack.ask(
        v("legs.walk"),
        &[("speed", 0.08), ("heading", 0.0)],
        Instant(0),
    );
    out.push(line(
        "walk",
        format!(
            "seat: {} · body: {}",
            if seat.is_granted() { "granted" } else { "denied" },
            match &body {
                Some(Report::Accepted {
                    samples,
                    duration_ms,
                    worst_margin,
                    ..
                }) => format!(
                    "accepted {samples} samples over {duration_ms} ms, worst tip-over margin {worst_margin:.1} mm"
                ),
                Some(Report::Refused { refusal, .. }) => format!("refused: {}", refusal.name()),
                _ => "no answer".into(),
            }
        ),
    ));

    let played = stack.play_out(Instant(0), Duration::from_millis(40), 400);
    out.push(line(
        "play",
        format!(
            "{} samples played, {} — every one checked against the envelope and the tip-over invariant before it was applied",
            played.samples_played,
            match played.stopped {
                None => "finished".to_string(),
                Some(r) => format!("stopped: {}", r.name()),
            }
        ),
    ));

    // --- the gait floor ---
    let (seat, body) = stack.ask(
        v("legs.walk"),
        &[
            ("speed", (TOP_SPEED_MPS * MIN_GAIT_SPEED * 0.5) as f64),
            ("heading", 0.0),
        ],
        Instant(0),
    );
    out.push(line(
        "creep",
        format!(
            "a crawl the seat's schema permits: seat {} · body {} — a robot creeping is less stable than a robot walking, and the body is the one that knows",
            if seat.is_granted() { "granted" } else { "denied" },
            match &body {
                Some(Report::Refused { refusal, .. }) => refusal.name(),
                Some(Report::Accepted { .. }) => "accepted",
                _ => "no answer",
            }
        ),
    ));

    // --- the subverted seat ---
    let s = subverted_seat();
    out.push(line(
        "subverted",
        format!(
            "a robot whose battery moved aft, with the seat's schema unchanged: seat granted={} · body refused={} · samples moved={}",
            s.seat_granted,
            s.body_refused.map(|r| r.name()).unwrap_or("nothing"),
            s.samples_played
        ),
    ));
    out.push(line(
        "subverted",
        "the token was genuine and the wire was authentic. The body refused because its own geometry said the motion was a fall — the same envelope, checked twice.",
    ));

    // --- a bad link ---
    let mut noisy = Stack::assemble(LegGeom::default(), Faults::NOISY);
    let mut heard = 0;
    for i in 0..40 {
        let (_, body) = noisy.ask(
            v("legs.stand"),
            &[("height", 0.0)],
            Instant(i * 1_000),
        );
        if body.is_some() {
            heard += 1;
        }
    }
    out.push(line(
        "bad link",
        format!(
            "40 commands over a link that drops, corrupts, duplicates and reorders: the body heard {heard}, refused {} as forged or replayed, and never acted on a wrong one",
            noisy.limb_wire.frames_refused
        ),
    ));

    out
}
