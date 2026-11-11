//! BFS  LEVEL    Domain 2 Autonomic - the BLUR quadruped body (wire, acquisition, supervisor)  [crate brainos-blur]
//! BFS  PURPOSE  BLUR's own checks for gate stages 4-6: per-joint argument ranges, a simulated rollout that must not fall or hit anyone, and live state like e-stop
//! BFS  DEFINES  Prediction, Predictor, BodyState, BlurPolicy
//! BFS  USES     this crate: descriptor, schema, session, supervisor | brainos: abi, body
//! BFS  USED BY  blur/tests/blur_suite, host/blur_gate2
//!
//! KIRA stages 4–6 for BLUR.
//!
//! * **VALIDATE** — what the schema cannot say: a `body.pose` angle
//!   inside *that joint's* range (the schema bounds it to ±3.2 rad for
//!   every joint); walking speed and turn rate together inside what the
//!   day-one walk was trained for.
//! * **SIMULATE** — a short rollout of the requested skill from the body's
//!   current state, by a `Predictor` the host supplies (the twin on the
//!   laptop). A predicted fall, a joint-limit violation or a predicted
//!   contact with a person is a refusal. No predictor, no grant:
//!   `Unpredictable`, never a silent pass.
//! * **POLICY** — the body right now: e-stop, rail, a reflex in progress,
//!   a session already running, the pack.

use crate::descriptor::{actuator_index, ACTUATORS};
use crate::schema;
use crate::session::{session_envelope, skill_code, SKILL_POSE, SKILL_WALK};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use brainos_abi::error::DenyCode;
use brainos_abi::ids::BodyId;
use brainos_abi::verb::Verb;
use brainos_body::hal::Telemetry;
use brainos_body::calibration::Calibration;
use brainos_body::policy::{Args, BodyPolicy, Refusal, SchemaPolicy};
use brainos_body::schema::CapabilitySchema;

/// What a rollout predicted.
#[derive(Clone, Debug, PartialEq)]
pub struct Prediction {
    pub falls: bool,
    pub joint_limit: Option<String>,
    pub person_contact: bool,
    /// worst joint torque as a fraction of what its driver delivers
    pub peak_torque_frac: f32,
    pub horizon_s: f32,
}

pub trait Predictor: Send {
    fn predict(&self, verb: &Verb, args: &Args) -> Option<Prediction>;
}

/// The body's state as stage 6 needs it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BodyState {
    pub estop: bool,
    pub rail_on: bool,
    pub reflex_active: bool,
    pub session_active: bool,
    pub vbat_mv: u16,
    pub tilt_deg: f32,
}

pub struct BlurPolicy {
    body: BodyId,
    /// the schema's declared bounds and calibration check — KIRA's generic
    /// validate delegates to the body's policy, so these run here first
    schema: SchemaPolicy,
    pub predictor: Option<Box<dyn Predictor>>,
    pub state: BodyState,
}

impl BlurPolicy {
    pub fn new(body: BodyId, schema: CapabilitySchema, calibration: Calibration, predictor: Option<Box<dyn Predictor>>) -> BlurPolicy {
        BlurPolicy {
            body,
            schema: SchemaPolicy::new(body, schema).calibrated(calibration),
            predictor,
            state: BodyState { rail_on: true, vbat_mv: 24_000, ..Default::default() },
        }
    }
}

fn scalar(args: &Args, name: &str) -> Option<f64> {
    args.iter().find(|(n, _)| n == name).and_then(|(_, a)| a.as_scalar())
}

impl BodyPolicy for BlurPolicy {
    fn body(&self) -> BodyId {
        self.body
    }

    fn describe(&self) -> String {
        format!("BLUR v36: per-joint envelopes, rollout-backed simulate ({})",
            if self.predictor.is_some() { "predictor attached" } else { "NO predictor: refuses consequential verbs" })
    }

    fn validate(&self, verb: &Verb, args: &Args) -> Result<(), Refusal> {
        self.schema.validate(verb, args)?;
        let Some(skill) = skill_code(verb) else {
            return Ok(()); // voice.say and friends: the schema's bounds are the whole story
        };
        if skill == SKILL_POSE {
            let joint = args.iter().find(|(n, _)| n == "joint").and_then(|(_, a)| a.as_text());
            let Some(j) = joint.and_then(actuator_index) else {
                return Err(Refusal::new(DenyCode::OutsideBodyEnvelope, "body.pose names no joint this body has"));
            };
            let a = &ACTUATORS[j];
            let angle = scalar(args, "angle").unwrap_or(f64::NAN) as f32;
            if !(angle >= a.q_min && angle <= a.q_max) {
                return Err(Refusal::new(DenyCode::OutsideBodyEnvelope, "body.pose angle is outside that joint's range"));
            }
        }
        if skill == SKILL_WALK {
            let vx = scalar(args, "vx").unwrap_or(0.0);
            let yaw = scalar(args, "yaw").unwrap_or(0.0);
            // turning hard at full speed is outside what the day-one walk covers
            if vx.abs() / schema::WALK_VX.1 + yaw.abs() / schema::WALK_YAW.1 > 1.5 {
                return Err(Refusal::new(DenyCode::OutsideBodyEnvelope, "speed and turn rate together exceed the walk's envelope"));
            }
        }
        if session_envelope(verb, args).is_none() {
            return Err(Refusal::new(DenyCode::OutsideBodyEnvelope, "no envelope inside this body describes that request"));
        }
        Ok(())
    }

    fn simulate(&self, verb: &Verb, args: &Args) -> Result<String, Refusal> {
        if skill_code(verb).is_none() {
            return Ok(String::from("not a motion"));
        }
        // A rollout predicts what the commanded motion does. With the rail
        // off, or a reflex owning the body, the command would not run at
        // all; rolling it out from a slumped or falling body only
        // "predicts a fall" that is a fact about the state, not the motion,
        // and refuses at the wrong stage for the wrong reason. Stage 6
        // refuses these states unconditionally, so nothing is waved through.
        let s = &self.state;
        if s.estop || !s.rail_on || s.reflex_active {
            return Ok(String::from("rollout not run: the body cannot execute a command now (stage 6 decides)"));
        }
        let Some(p) = self.predictor.as_ref() else {
            return Err(Refusal::new(DenyCode::Unpredictable, "no model of this body is available to predict the motion"));
        };
        let Some(r) = p.predict(verb, args) else {
            return Err(Refusal::new(DenyCode::Unpredictable, "the rollout could not be run from the current state"));
        };
        if r.falls {
            return Err(Refusal::new(DenyCode::PredictedInvariantBreak, "the rollout predicts a fall"));
        }
        if r.person_contact {
            return Err(Refusal::new(DenyCode::PredictedInvariantBreak, "the rollout predicts contact with a person"));
        }
        if let Some(j) = &r.joint_limit {
            let _ = j;
            return Err(Refusal::new(DenyCode::PredictedInvariantBreak, "the rollout predicts a joint driven past its limit"));
        }
        Ok(format!("rollout {:.1} s: stays up, peak torque {:.0}% of driver", r.horizon_s, 100.0 * r.peak_torque_frac))
    }

    fn policy(&self, verb: &Verb, _args: &Args) -> Result<(), Refusal> {
        if skill_code(verb).is_none() {
            return Ok(());
        }
        let s = &self.state;
        if s.estop || !s.rail_on {
            return Err(Refusal::new(DenyCode::BodyInvariant, "the e-stop is pressed or the motor rail is off"));
        }
        if s.reflex_active {
            return Err(Refusal::new(DenyCode::BodyInvariant, "a reflex owns the body (it fell or is protecting itself)"));
        }
        if s.session_active {
            return Err(Refusal::new(DenyCode::BodyInvariant, "a session is already running on this body"));
        }
        if s.vbat_mv > 0 && s.vbat_mv < crate::supervisor::VBAT_CUTOFF_MV {
            return Err(Refusal::new(DenyCode::BodyInvariant, "the pack is below its cutoff"));
        }
        Ok(())
    }

    /// Telemetry, as BLUR's supervisor reports it: `rail_v` is the motor
    /// rail after the e-stop switch (0 when it is pressed); `stops` counts
    /// a reflex that owns the body right now.
    fn observe(&mut self, t: &Telemetry) {
        if let Some(v) = t.rail_v {
            self.state.rail_on = v > 1.0;
            self.state.estop = v <= 1.0;
            if v > 1.0 {
                self.state.vbat_mv = (v * 1000.0) as u16;
            }
        }
        if let Some(tilt) = t.tilt_deg {
            self.state.tilt_deg = tilt;
        }
        self.state.reflex_active = t.stops > 0;
    }
}
