//! BFS  LEVEL    Host - the only crate with an operating system (CLI, scenarios, OS nerves)  [crate brainos-host]
//! BFS  PURPOSE  Wires entity, gate, wire protocol and body runtime with real gait and tip-over checks; shows the body refuses a fall even if the gate is subverted
//! BFS  DEFINES  Stack, SeatVerdict, PlayResult, subverted_seat, SubversionResult, Line, run, MANUFACTURER_DESCRIPTION, AcquiredBody, acquire_from_manufacturer, manufacturer_demo, TwoBodies, +3 more
//! BFS  USES     brainos: abi, adapters, body, bodyd, crypto, entity, kira, memory, miniblur-core, rt, +1 more
//! BFS  USED BY  host/life, host/migrate, host/tests/fullstack_suite
//!
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

// =====================================================================
// Phase 1's deliverable: a body we did not build
// =====================================================================

/// An MHS device description, as a manufacturer would publish it.
///
/// This is the shape Anthropic's Model Hardware Standard uses: tools
/// with JSON Schema inputs, including `minimum` and `maximum`. It is why
/// MHS is the easier of the two adapters — a ROS topic says "this is a
/// Twist" and an MHS tool says "this takes an angle between −360 and
/// 360 degrees".
///
/// Note the `speed` bound. The manufacturer says the arm will move at up
/// to 180 °/s. Our own mechanism's static torque model is only trusted
/// to 90 °/s, so the seat will grant a fast motion and the body will
/// refuse it. That is not a disagreement about the hardware; it is a
/// disagreement about what we are willing to model, and the body is the
/// one that gets the last word.
pub const MANUFACTURER_DESCRIPTION: &str = r#"{
  "name": "M0609 collaborative arm",
  "device_type": "arm",
  "tools": [
    {
      "name": "move_joint",
      "description": "move one joint to an absolute angle",
      "irreversible": true,
      "timeout_ms": 12000,
      "input_schema": {
        "type": "object",
        "properties": {
          "joint":  {"type": "integer", "minimum": 0,    "maximum": 5},
          "angle":  {"type": "number",  "unit": "deg",   "minimum": -360, "maximum": 360},
          "speed":  {"type": "number",  "unit": "deg/s", "minimum": 0,    "maximum": 180}
        },
        "required": ["joint", "angle"]
      }
    },
    {
      "name": "home",
      "description": "return to the home pose",
      "input_schema": {
        "type": "object",
        "properties": {
          "speed": {"type": "number", "unit": "deg/s", "minimum": 0, "maximum": 60}
        }
      }
    }
  ]
}"#;

/// What happened when the entity inhabited a body it did not build.
#[derive(Clone, Debug)]
pub struct AcquiredBody {
    /// what the adapter read out of the manufacturer's description
    pub verbs: Vec<String>,
    /// how much of it is actionable — the number a design partner asks
    /// for first
    pub actionable_pct: u8,
    pub body_id: BodyId,
    /// the seat's view of the joint rate, from the manufacturer
    pub seat_rate_limit: Option<f32>,
    /// what the body's own model will actually accept
    pub body_rate_limit: Option<f32>,
    /// a legal motion: what the seat said and what the body said
    pub legal: (bool, Option<Refusal>),
    /// a motion inside the manufacturer's numbers but outside ours
    pub too_fast: (bool, Option<Refusal>),
    /// a motion the arm's own torque budget refuses
    pub too_heavy: (bool, Option<Refusal>),
}

/// Acquire an arm from its manufacturer's description and drive it.
pub fn acquire_from_manufacturer(payload_kg: f32) -> AcquiredBody {
    use brainos_adapters::mhs::from_mhs;
    use brainos_adapters::ros2::actionable_fraction_pct;
    use brainos_bodyd::arm::{Arm, ArmConfig, MAX_JOINTS};

    // --- §8 steps 1–3: the adapter reads the device ---
    let schema = from_mhs(MANUFACTURER_DESCRIPTION).expect("a well-formed device description");
    let verbs: Vec<String> = schema
        .verbs()
        .iter()
        .map(|v| v.verb.as_str().to_string())
        .collect();
    let actionable_pct = actionable_fraction_pct(&schema);
    let seat_rate_limit = schema
        .get(&v("arm.move_joint"))
        .and_then(|s| s.param("speed"))
        .map(|p| p.max as f32);

    // --- §8 steps 4–5: the entity incorporates it ---
    let entity_key = BrainKey::from_seed(&[0x5A; 32]);
    let device_key = BrainKey::from_seed(&[0xC3; 32]);
    let mut entity = Entity::new(BrainKey::from_seed(&[0x5A; 32]), Tick(0));
    entity.wake(None, None, Tick(1)).expect("a fresh entity wakes");

    // an arm has no legs to calibrate; its calibration is its home pose,
    // which lives in the entity's state graph like everything else
    let body_id = entity
        .acquire(&device_key, schema.clone(), Calibration::new(BodyId::ZERO, Tick(1)), 0)
        .expect("the arm joins");
    entity.observe_body(&body_id, &fit());
    let _ = entity_key;

    // --- the body side ---
    let config = ArmConfig {
        payload_kg,
        ..ArmConfig::collaborative_six_axis()
    };
    let body_rate_limit = Some(config.max_rate_dps);
    let mut arm = Arm::new(config);
    arm.calibrate([0.0; MAX_JOINTS]);

    let mut runtime = BodyRuntime::new(arm, Instant(0));
    // the seat's envelope, derived from the manufacturer's own numbers
    let mut granted = Envelope::new(
        2,
        Duration::from_millis(10),
        Duration::from_millis(200),
    );
    granted.bound(Channel::Angle, Limit::new(-360.0, 360.0));
    granted.bound(Channel::Rate, Limit::new(0.0, seat_rate_limit.unwrap_or(180.0)));
    runtime.narrow_to(granted);
    runtime.ready();

    let mut ask = |joint: f64, angle: f64, speed: f64| -> (bool, Option<Refusal>) {
        let out = entity.attempt(
            v("arm.move_joint"),
            body_id,
            &[("joint", joint), ("angle", angle), ("speed", speed)],
        );
        let granted = out.verdict.is_granted();
        if !granted {
            return (false, None);
        }
        let cmd = Command::new(v("arm.move_joint"))
            .arg("joint", joint as f32)
            .arg("angle", angle as f32)
            .arg("speed", speed as f32);
        let report = runtime.accept(&cmd, Instant(0));
        let refusal = match report {
            Report::Refused { refusal, .. } => Some(refusal),
            _ => None,
        };
        (granted, refusal)
    };

    let legal = ask(1.0, 45.0, 30.0);
    let too_fast = ask(1.0, -45.0, 150.0);
    let too_heavy = ask(2.0, 30.0, 20.0);

    AcquiredBody {
        verbs,
        actionable_pct,
        body_id,
        seat_rate_limit,
        body_rate_limit,
        legal,
        too_fast,
        too_heavy,
    }
}

/// The narrated version, for the binary.
pub fn manufacturer_demo() -> Vec<Line> {
    let mut out = Vec::new();

    let light = acquire_from_manufacturer(0.0);
    out.push(line(
        "§8 1-3",
        format!(
            "the MHS adapter read the manufacturer's description: {} verbs ({}), {}% of them actionable because the device declared its bounds",
            light.verbs.len(),
            light.verbs.join(", "),
            light.actionable_pct
        ),
    ));
    out.push(line(
        "§8 4-5",
        format!(
            "incorporated as body {} — a body we did not build, inhabited without anyone ripping out their stack",
            light.body_id.fingerprint()
        ),
    ));
    out.push(line(
        "envelope",
        format!(
            "the manufacturer says the arm moves at up to {} °/s; our own static torque model is trusted to {} °/s, and the intersection is what the body will accept",
            light.seat_rate_limit.unwrap_or(0.0),
            light.body_rate_limit.unwrap_or(0.0)
        ),
    ));
    out.push(line(
        "move",
        format!(
            "45° on joint 1 at 30 °/s — seat: {} · body: {}",
            granted_word(light.legal.0),
            body_word(light.legal.0, light.legal.1)
        ),
    ));
    out.push(line(
        "too fast",
        format!(
            "the same move at 150 °/s, inside the manufacturer's numbers — seat: {} · body: {} — not a disagreement about the hardware, a disagreement about what we are willing to model",
            granted_word(light.too_fast.0),
            body_word(light.too_fast.0, light.too_fast.1)
        ),
    ));

    let heavy = acquire_from_manufacturer(40.0);
    out.push(line(
        "payload",
        format!(
            "the same arm with 40 kg in the gripper, same description, same command — seat: {} · body: {} — forty kilos at nearly a metre is more moment than the shoulder has",
            granted_word(heavy.legal.0),
            body_word(heavy.legal.0, heavy.legal.1)
        ),
    ));

    out
}

fn granted_word(b: bool) -> &'static str {
    if b {
        "granted"
    } else {
        "denied"
    }
}

fn refusal_word(r: Option<Refusal>) -> &'static str {
    match r {
        None => "accepted",
        Some(x) => x.name(),
    }
}

/// What the body said, when the seat may not have let it hear anything.
fn body_word(granted: bool, r: Option<Refusal>) -> &'static str {
    if !granted {
        return "never heard it";
    }
    refusal_word(r)
}

// =====================================================================
// Stage 4: one self, two bodies that share nothing but the self
// =====================================================================

/// The entity holding a hexapod and an arm at once, over two wires,
/// with one memory and one gate.
///
/// The Phase 0 demo is a laptop and a hexapod, which is two bodies but
/// only one of them has consequences. This is two *consequential*
/// bodies with entirely different physics, running the same runtime at
/// the same time, arbitrating over something only one of them can have.
///
/// What is the same for both: the Key, the state graph, the gate, the
/// skill graph, the audit log. What is different: everything else.
pub struct TwoBodies {
    pub entity: Entity,
    pub hexapod_id: BodyId,
    pub arm_id: BodyId,
    hexapod: BodyRuntime<Hexapod>,
    arm: BodyRuntime<brainos_bodyd::arm::Arm>,
    hexapod_wire: (Wire, Wire, Loopback),
    arm_wire: (Wire, Wire, Loopback),
}

/// What happened in the two-body run.
#[derive(Clone, Debug, PartialEq)]
pub struct TwoBodyResult {
    pub bodies: usize,
    /// one repertoire spanning both
    pub verbs: Vec<String>,
    /// the hexapod walked
    pub hexapod_moved: bool,
    /// the arm moved
    pub arm_moved: bool,
    /// both wrote into one graph
    pub memories_after: usize,
    /// what the arbiter did when both wanted the workbench
    pub conflict: Option<String>,
    /// which body kept it
    pub winner_is_the_arm: bool,
    /// a body went silent and its claim lapsed
    pub lapsed: usize,
    /// one audit log covering both bodies
    pub audit_entries: usize,
    /// a skill learned on one body, applied to the other
    pub shared_context_worked: bool,
}

impl TwoBodies {
    pub fn assemble() -> TwoBodies {
        use brainos_adapters::mhs::from_mhs;
        use brainos_bodyd::arm::{Arm, ArmConfig, MAX_JOINTS};

        let entity_key = BrainKey::from_seed(&[0x5A; 32]);
        let mut entity = Entity::new(BrainKey::from_seed(&[0x5A; 32]), Tick(0));
        entity.wake(None, None, Tick(1)).expect("a fresh entity wakes");

        // --- the hexapod: a body we built ---
        let hexapod_key = BrainKey::from_seed(&[0xB2; 32]);
        let hexapod_id = entity
            .acquire(&hexapod_key, mini_blur(), entity_calibration(), 10)
            .expect("mini-blur joins");
        entity.observe_body(&hexapod_id, &fit());

        // --- the arm: a body we did not ---
        let arm_key = BrainKey::from_seed(&[0xC3; 32]);
        let arm_schema = from_mhs(MANUFACTURER_DESCRIPTION).expect("a well-formed description");
        let arm_id = entity
            .acquire(
                &arm_key,
                arm_schema,
                Calibration::new(BodyId::ZERO, Tick(1)),
                0,
            )
            .expect("the arm joins");
        entity.observe_body(&arm_id, &fit());

        // --- two wires ---
        let mut rng = Drbg::deterministic(b"two bodies");
        let make_wire = |body: BodyId, rng: &mut Drbg| {
            let (mut seat, nonce) = Wire::initiate(&entity_key, body, rng, Instant(0));
            let subkey = entity_key.derive_subkey(&body, 32);
            let mut limb = Wire::accept(&subkey, body, nonce, Instant(0));
            let peer = Peer {
                entity: Some(entity_key.entity_id()),
                body: Some(body),
                key: entity_key.public(),
            };
            seat.attested(peer);
            limb.attested(peer);
            (seat, limb, Loopback::perfect())
        };
        let hexapod_wire = make_wire(hexapod_id, &mut rng);
        let arm_wire = make_wire(arm_id, &mut rng);

        // --- two runtimes, same code ---
        let mut hex_mech = Hexapod::new(HexapodConfig::default());
        hex_mech.calibrate(ServoTable::bom_10_servo());
        let mut hexapod = BodyRuntime::new(hex_mech, Instant(0));
        hexapod.narrow_to(granted_envelope());
        hexapod.ready();

        let mut arm_mech = Arm::new(ArmConfig::collaborative_six_axis());
        arm_mech.calibrate([0.0; MAX_JOINTS]);
        let mut arm = BodyRuntime::new(arm_mech, Instant(0));
        let mut arm_granted = Envelope::new(
            3,
            Duration::from_millis(10),
            Duration::from_millis(200),
        );
        arm_granted.bound(Channel::Angle, Limit::new(-360.0, 360.0));
        arm_granted.bound(Channel::Rate, Limit::new(0.0, 180.0));
        arm.narrow_to(arm_granted);
        arm.ready();

        TwoBodies {
            entity,
            hexapod_id,
            arm_id,
            hexapod,
            arm,
            hexapod_wire,
            arm_wire,
        }
    }

    fn send(
        wire: &mut (Wire, Wire, Loopback),
        runtime_accept: &mut dyn FnMut(&Command) -> Report,
        cmd: &Command,
    ) -> Option<Report> {
        let payload = Protocol::encode_command(cmd).ok()?;
        let framed = wire.0.send(Kind::Command, &payload).ok()?;
        wire.2.a_sends(framed);
        wire.2.settle();
        while let Some(raw) = wire.2.b_receives() {
            let Ok(frame) = wire.1.receive(&raw) else {
                continue;
            };
            let Ok(decoded) = Protocol::decode_command(&frame.payload) else {
                continue;
            };
            return Some(runtime_accept(&decoded));
        }
        None
    }

    /// Ask the hexapod to walk, through the gate and the wire.
    pub fn walk(&mut self, speed: f64) -> (bool, Option<Report>) {
        let out = self.entity.attempt(
            v("legs.walk"),
            self.hexapod_id,
            &[("speed", speed), ("heading", 0.0)],
        );
        if !out.verdict.is_granted() {
            return (false, None);
        }
        let cmd = Command::new(v("legs.walk"))
            .arg("speed", speed as f32)
            .arg("heading", 0.0);
        let body = &mut self.hexapod;
        let report = TwoBodies::send(
            &mut self.hexapod_wire,
            &mut |c| body.accept(c, Instant(0)),
            &cmd,
        );
        (true, report)
    }

    /// Ask the arm to move a joint, through the gate and the wire.
    pub fn move_joint(&mut self, joint: f64, angle: f64, speed: f64) -> (bool, Option<Report>) {
        let out = self.entity.attempt(
            v("arm.move_joint"),
            self.arm_id,
            &[("joint", joint), ("angle", angle), ("speed", speed)],
        );
        if !out.verdict.is_granted() {
            return (false, None);
        }
        let cmd = Command::new(v("arm.move_joint"))
            .arg("joint", joint as f32)
            .arg("angle", angle as f32)
            .arg("speed", speed as f32);
        let body = &mut self.arm;
        let report = TwoBodies::send(
            &mut self.arm_wire,
            &mut |c| body.accept(c, Instant(0)),
            &cmd,
        );
        (true, report)
    }

    /// Run both bodies forward together. This is the part that is
    /// genuinely simultaneous: one loop, two bodies, each on its own
    /// schedule.
    pub fn run_both(&mut self, step: Duration, passes: usize) -> (u32, u32) {
        let mut hex = 0;
        let mut arm = 0;
        let mut t = 0u64;
        for _ in 0..passes {
            self.hexapod.heard_from_seat(Instant(t));
            self.arm.heard_from_seat(Instant(t));
            if self.hexapod.tick(Instant(t), measured()).applied.is_some() {
                hex += 1;
            }
            if self.arm.tick(Instant(t), measured()).applied.is_some() {
                arm += 1;
            }
            t += step.as_micros();
        }
        (hex, arm)
    }
}

/// Run the Stage 4 story.
pub fn two_bodies() -> TwoBodyResult {
    use brainos_entity::simultaneity::{Claim, Resource};
    use brainos_memory::episode::Outcome;

    let mut s = TwoBodies::assemble();
    let verbs: Vec<String> = s
        .entity
        .repertoire()
        .iter()
        .map(|v| v.as_str().to_string())
        .collect();

    // --- both act ---
    let (_, hex_report) = s.walk(0.08);
    let (_, arm_report) = s.move_joint(1.0, 45.0, 30.0);
    let hexapod_accepted = matches!(hex_report, Some(Report::Accepted { .. }));
    let arm_accepted = matches!(arm_report, Some(Report::Accepted { .. }));
    let (hex_samples, arm_samples) = s.run_both(Duration::from_millis(40), 200);

    // --- both remember, into one graph ---
    s.entity.outcome(
        v("legs.walk"),
        Some(s.hexapod_id),
        Outcome::Succeeded,
        "walked to the workbench",
        &["workshop", "carpet"],
    );
    s.entity.outcome(
        v("arm.move_joint"),
        Some(s.arm_id),
        Outcome::Succeeded,
        "reached for the thing on the workbench",
        &["workshop", "carpet"],
    );
    let memories_after = s.entity.graph.len();

    // --- one memory, reachable from either body's context ---
    let shared_context_worked = s
        .entity
        .recall("what happened at the workbench", 5)
        .iter()
        .filter(|(n, _)| n.content.contains("workbench"))
        .count()
        >= 2;

    // --- they want the same thing ---
    let workbench = Resource::named("the workbench");
    let arm_claim = Claim {
        body: s.arm_id,
        resource: workbench.clone(),
        verb: v("arm.move_joint"),
        // mid-lift: interrupting drops whatever is in the gripper
        interruption_cost: 85,
        claimed_at: s.entity.tick,
        last_heard: Instant(0),
    };
    let hexapod_claim = Claim {
        body: s.hexapod_id,
        resource: workbench.clone(),
        verb: v("legs.walk"),
        // mid-stride: interrupting is a stumble
        interruption_cost: 25,
        claimed_at: s.entity.tick,
        last_heard: Instant(0),
    };

    s.entity.arbiter.claim(arm_claim.clone()).ok();
    let conflict = match s.entity.arbiter.claim(hexapod_claim) {
        Ok(()) => None,
        Err((_, resolution)) => Some(resolution.because().to_string()),
    };
    let winner_is_the_arm = s
        .entity
        .arbiter
        .holder_of(&workbench)
        .map(|c| c.body == s.arm_id)
        .unwrap_or(false);

    // --- one of them goes silent ---
    let lapsed = s
        .entity
        .arbiter
        .lapse_silent(Instant(
            brainos_entity::simultaneity::SILENCE_GRACE.as_micros() + 1_000,
        ))
        .len();

    TwoBodyResult {
        bodies: s.entity.bodies.body_count(),
        verbs,
        hexapod_moved: hexapod_accepted && hex_samples > 0,
        arm_moved: arm_accepted && arm_samples > 0,
        memories_after,
        conflict,
        winner_is_the_arm,
        lapsed,
        audit_entries: s.entity.kira.trail.len(),
        shared_context_worked,
    }
}

/// The narrated version.
pub fn two_bodies_demo() -> Vec<Line> {
    let r = two_bodies();
    let mut out = Vec::new();

    out.push(line(
        "stage 4",
        format!(
            "{} consequential bodies at once — a hexapod we built and an arm we did not — with {} verbs in one repertoire",
            r.bodies,
            r.verbs.len()
        ),
    ));
    out.push(line(
        "both act",
        format!(
            "both moved in the same loop: the hexapod {} and the arm {}. Same runtime, same wire, same envelope model; a convex hull of feet and a torque budget",
            if r.hexapod_moved { "walked" } else { "did not move" },
            if r.arm_moved { "moved a joint" } else { "did not move" }
        ),
    ));
    out.push(line(
        "one memory",
        format!(
            "{} memories in one graph, written by both bodies; a query about the workbench finds what each of them did: {}",
            r.memories_after, r.shared_context_worked
        ),
    ));
    out.push(line(
        "one gate",
        format!(
            "{} decisions in one audit log, covering both bodies under one Key",
            r.audit_entries
        ),
    ));
    match &r.conflict {
        None => out.push(line("arbiter", "no conflict arose")),
        Some(why) => out.push(line(
            "arbiter",
            format!(
                "both wanted the workbench. The arm kept it: {why}. The hexapod is mid-stride and interrupting that is a stumble; the arm is mid-lift and interrupting that is a drop"
            ),
        )),
    }
    out.push(line(
        "silence",
        format!(
            "{} claim(s) lapsed when a body stopped answering — not immediately, because a link hiccup must not make a robot let go, but a claim held by something that will never release it is a deadlock with an entity in it",
            r.lapsed
        ),
    ));

    out
}
