//! BFS  LEVEL    Domain 2 Autonomic - the BLUR quadruped body (wire, acquisition, supervisor)  [crate brainos-blur]
//! BFS  PURPOSE  Runs the five acquisition steps (discover, handshake, schema, policy, calibrate) against a BLUR body and reports the step where any fault was caught
//! BFS  DEFINES  Hello, Port, Stage, Finding, Report, Record, Acquirer, current_limits, explain
//! BFS  USES     this crate: autonomic, calibrate, census, descriptor, firmware, fit, schema, wire | brainos: abi, body, crypto
//! BFS  USED BY  host/blur_gate2, host/blur_run, host/blur_sil
//!
//! §8 for BLUR, end to end, over the protocol state machine in
//! `brainos-body`.
//!
//! This module does not re-implement acquisition. `Acquisition` owns the
//! five steps and their ordering; this file is the adapter that knows
//! what a BLUR is, asks the body to do what each step needs, and calls
//! `Acquisition::reject` with the reason when the body is not what it
//! should be. The `Report` says at which step a fault was caught, which
//! is the thing the adversarial tests assert.

use crate::autonomic::{Outcome, Request};
use crate::calibrate::{self, CalFault, HomeResult, ImuResult, JointCal, ProbeResult};
use crate::census::{check_census, BuildSheet, Census, CensusFault};
use crate::descriptor::{ACTUATORS, N_ACT};
use crate::firmware::{absorb, census_payload};
use crate::schema::{check_schema, SchemaFault};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::ids::{BodyId, EntityId, RegionId, SessionId};
use brainos_abi::time::Tick;
use brainos_abi::version::AbiVersion;
use brainos_body::acquire::{Acquisition, StepError};
use brainos_body::calibration::Calibration;
use brainos_body::region::BodyMap;
use brainos_body::schema::CapabilitySchema;
use brainos_crypto::domain::Tag;
use brainos_crypto::key::{BrainKey, Challenge, PublicKey, Signature};
use brainos_crypto::rng::Drbg;
use core::cell::RefCell;

/// What the link reports when it first comes up.
#[derive(Clone, Copy, Debug)]
pub struct Hello {
    pub device: PublicKey,
    pub abi: AbiVersion,
    pub rail_on: bool,
    pub estop_latched: bool,
}

/// How the entity reaches the body. In the twin this is in-process over
/// the nerve socket; on the robot, the USB link to the Teensy. Nothing in
/// this file knows which.
pub trait Port {
    fn hello(&mut self) -> Option<Hello>;
    fn answer(&mut self, c: &Challenge) -> Signature;
    fn offer(&mut self, entity: &PublicKey, sig: &Signature, transcript: &[u8; 32]) -> bool;
    fn census(&mut self, transcript: &[u8; 32]) -> Option<(Census, Signature)>;
    fn schema(&mut self) -> CapabilitySchema;
    /// Run one Autonomic procedure to completion (or `None` on timeout).
    fn run(&mut self, r: Request) -> Option<Outcome>;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Stage {
    Discover,
    Handshake,
    Schema,
    Incorporate,
    Calibrate,
    Done,
}

impl Stage {
    pub const fn name(self) -> &'static str {
        match self {
            Stage::Discover => "DISCOVER",
            Stage::Handshake => "HANDSHAKE",
            Stage::Schema => "SCHEMA",
            Stage::Incorporate => "INCORPORATE",
            Stage::Calibrate => "CALIBRATE",
            Stage::Done => "DONE",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Finding {
    NoAnswer,
    Step(StepError),
    CensusUnsigned,
    Census(CensusFault),
    Schema(SchemaFault),
    Cal(CalFault),
    RailOff,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub reached: Stage,
    /// where acquisition stopped, if it did
    pub caught_at: Option<Stage>,
    pub finding: Option<Finding>,
    pub detail: String,
    pub body: Option<BodyId>,
    pub regions: Vec<RegionId>,
    pub census: Option<Census>,
    /// on a first inhabitation, the build sheet to record (with the
    /// human's confirmation) — the entity did not have one
    pub new_sheet: Option<BuildSheet>,
    pub joints: Option<[JointCal; N_ACT]>,
    pub calibration: Option<Calibration>,
    /// the line for the state graph's Body node
    pub join: String,
    pub log: Vec<String>,
}

impl Report {
    fn new() -> Report {
        Report {
            reached: Stage::Discover,
            caught_at: None,
            finding: None,
            detail: String::new(),
            body: None,
            regions: Vec::new(),
            census: None,
            new_sheet: None,
            joints: None,
            calibration: None,
            join: String::new(),
            log: Vec::new(),
        }
    }
    pub fn ok(&self) -> bool {
        self.reached == Stage::Done && self.caught_at.is_none()
    }
    fn stop(mut self, at: Stage, f: Finding, detail: String) -> Report {
        self.caught_at = Some(at);
        self.finding = Some(f);
        self.detail = detail;
        self
    }
}

/// The entity's memory of this body, from the state graph.
#[derive(Clone, Debug, Default)]
pub struct Record {
    pub sheet: Option<BuildSheet>,
    /// recorded zero offsets, for the drift check
    pub offsets: Option<[f32; N_ACT]>,
}

pub struct Acquirer<'k> {
    pub key: &'k BrainKey,
    pub entity: EntityId,
    pub session: SessionId,
    pub imu_samples: u32,
}

impl<'k> Acquirer<'k> {
    pub fn run<P: Port>(
        &self,
        port: &mut P,
        rng: &mut Drbg,
        map: &mut BodyMap,
        record: &Record,
        at: Tick,
        next_region_id: impl FnMut() -> RegionId,
    ) -> Report {
        let mut r = Report::new();
        let mut a = Acquisition::discover(self.entity, self.session);

        // ---- 1 DISCOVER
        let Some(hello) = port.hello() else {
            let e = a.reject(StepError::DeviceUnattested);
            return r.stop(Stage::Discover, Finding::Step(e), "nothing answered on the body link".into());
        };
        r.log.push(format!(
            "discover: body answered, rail {}, e-stop {}",
            if hello.rail_on { "on" } else { "off" },
            if hello.estop_latched { "PRESSED" } else { "released" }
        ));

        // ---- 2 HANDSHAKE (keys, then parts)
        r.reached = Stage::Handshake;
        let t_after_device = absorb(&a.transcript(), "device", &hello.device.to_bytes());
        let cell = RefCell::new(&mut *port);
        let hs = a.handshake(
            self.key,
            rng,
            hello.device,
            hello.abi,
            |c| cell.borrow_mut().answer(c),
            |pk, sig| cell.borrow_mut().offer(pk, sig, &t_after_device),
        );
        if let Err(e) = hs {
            return r.stop(Stage::Handshake, Finding::Step(e), format!("attestation failed: {e:?}"));
        }
        let tr = a.transcript();
        let Some((census, sig)) = port.census(&tr) else {
            let e = a.reject(StepError::PartsUnrecognised);
            return r.stop(Stage::Handshake, Finding::Step(e), "the body did not report its parts".into());
        };
        if !hello.device.verify(Tag::Attestation, &census_payload(&tr, &census), &sig) {
            a.reject(StepError::PartsUnrecognised);
            return r.stop(Stage::Handshake, Finding::CensusUnsigned, "census signature does not verify against the attested key".into());
        }
        r.log.push(format!(
            "handshake: attested; census {} nodes, {} conflicts, imu {}",
            census.nodes.len(),
            census.conflicts.len(),
            if census.imu_present { "present" } else { "ABSENT" }
        ));
        if let Err(f) = check_census(&census, record.sheet.as_ref()) {
            a.reject(StepError::PartsUnrecognised);
            r.census = Some(census);
            return r.stop(Stage::Handshake, Finding::Census(f), format!("parts do not match: {f:?}"));
        }
        if record.sheet.is_none() {
            r.new_sheet = Some(BuildSheet::from_census(&census));
            r.log.push("handshake: no build sheet on record — first inhabitation, sheet proposed for the human to confirm".into());
        }
        a.witness("census", census.digest().as_bytes());

        // ---- 3 SCHEMA
        r.reached = Stage::Schema;
        let schema = port.schema();
        if let Err(f) = check_schema(&schema, &census) {
            a.reject(StepError::SchemaContradictsBody);
            r.census = Some(census);
            return r.stop(Stage::Schema, Finding::Schema(f.clone()), format!("schema does not describe this body: {f:?}"));
        }
        if let Err(e) = a.describe(schema) {
            return r.stop(Stage::Schema, Finding::Step(e), format!("schema refused: {e:?}"));
        }
        r.body = a.body;
        r.census = Some(census);
        r.log.push(format!("schema: {} verbs, body {}", a.schema.as_ref().map(|s| s.len()).unwrap_or(0),
            a.body.map(|b| alloc::string::ToString::to_string(b.fingerprint().as_str())).unwrap_or_default()));

        // ---- 4 INCORPORATE
        r.reached = Stage::Incorporate;
        match a.incorporate(map, next_region_id) {
            Ok(ids) => {
                r.log.push(format!("incorporate: {} regions on the body map", ids.len()));
                r.regions = ids;
            }
            Err(e) => return r.stop(Stage::Incorporate, Finding::Step(e), format!("incorporation refused: {e:?}")),
        }

        // ---- 5 CALIBRATE
        r.reached = Stage::Calibrate;
        if !hello.rail_on || hello.estop_latched {
            a.reject(StepError::CalibrationInvalid);
            return r.stop(Stage::Calibrate, Finding::RailOff, "motor rail is off or the e-stop is pressed; nothing can be probed".into());
        }
        match self.calibrate(port, &mut r, record) {
            Ok((joints, cal_imu)) => {
                let body = a.body.unwrap_or_default();
                let limits = current_limits();
                let push = calibrate::push_values(&joints, &limits);
                let pushed = match port.run(Request::Push { values: push.clone() }) {
                    Some(Outcome::Pushed { acked, failed }) => {
                        r.log.push(format!("calibrate: pushed {} values, {} acknowledged, {} not", push.len(), acked, failed.len()));
                        failed.first().map(|(j, _)| *j)
                    }
                    _ => Some(0),
                };
                if let Some(j) = pushed {
                    a.reject(StepError::CalibrationInvalid);
                    let f = CalFault::NotAcknowledged { joint: j };
                    return r.stop(Stage::Calibrate, Finding::Cal(f), format!("{} did not acknowledge its calibration", ACTUATORS[j].name));
                }
                // the tail cables, from the force-free rest at the stand pose
                let mut joints = joints;
                let cables = match port.run(Request::Rest { settle_ms: 2500 }) {
                    Some(Outcome::Rest { cables, hold_err }) => {
                        r.log.push(format!("  rest  stand hold: worst {} off by {:+.4} rad while the cables were read",
                            ACTUATORS[hold_err.0].name, hold_err.1));
                        cables
                    }
                    _ => Vec::new(),
                };
                let mut fix = Vec::new();
                for (j, reading) in cables {
                    let a_ = &ACTUATORS[j];
                    joints[j].offset = a_.q_rest - reading;
                    fix.push((j, crate::wire::ConfigKey::Offset, joints[j].offset));
                    r.log.push(format!("  rest  {:<17} cable {:>8.4} m at zero force → offset {:>8.4} (design rest {:.4})",
                        a_.name, reading, joints[j].offset, a_.q_rest));
                }
                let n_fix = fix.len();
                // a committed node will not take a new zero (its envelope is
                // locked; nerves.py `Node.config`): reopen, write, recommit --
                // three pushes in turn, since frames sent together can arrive
                // out of order
                let reopen: Vec<_> = fix.iter().map(|&(j, _, _)| (j, crate::wire::ConfigKey::Commit, 0.0)).collect();
                let recommit: Vec<_> = fix.iter().map(|&(j, _, _)| (j, crate::wire::ConfigKey::Commit, 1.0)).collect();
                let ok = |o: Option<Outcome>| matches!(o, Some(Outcome::Pushed { failed, .. }) if failed.is_empty());
                let done = n_fix > 0
                    && ok(port.run(Request::Push { values: reopen }))
                    && ok(port.run(Request::Push { values: fix }))
                    && ok(port.run(Request::Push { values: recommit }));
                match done {
                    true => {}
                    false => {
                        a.reject(StepError::CalibrationInvalid);
                        let f = CalFault::NotAcknowledged { joint: 19 };
                        return r.stop(Stage::Calibrate, Finding::Cal(f), "the tail cables did not take their rest calibration".into());
                    }
                }
                let cal = calibrate::to_calibration(body, at, &joints, &cal_imu);
                if let Err(e) = a.calibrate(cal.clone(), N_ACT, |_| true) {
                    return r.stop(Stage::Calibrate, Finding::Step(e), format!("calibration refused by the protocol: {e:?}"));
                }
                r.joints = Some(joints);
                r.calibration = Some(cal);
                r.join = a.render_join(at);
                r.reached = Stage::Done;
                r
            }
            Err(f) => {
                a.reject(StepError::CalibrationInvalid);
                let who = f.joint().map(|j| ACTUATORS[j].name).unwrap_or("imu");
                let d = format!("{who}: {}", explain(&f));
                r.stop(Stage::Calibrate, Finding::Cal(f), d)
            }
        }
    }

    fn calibrate<P: Port>(&self, port: &mut P, r: &mut Report, record: &Record) -> Result<([JointCal; N_ACT], calibrate::ImuCal), CalFault> {
        // the inner ear first: it is the cheapest check and the one every
        // later reflex depends on
        let want = self.imu_samples;
        let imu = match port.run(Request::ImuLevel { samples: want }) {
            Some(Outcome::Imu { n, missing, acc, gyro, acc_norm_sd }) => ImuResult { n, missing, acc, gyro, acc_norm_sd },
            _ => ImuResult { n: 0, missing: want, acc: [0.0; 3], gyro: [0.0; 3], acc_norm_sd: 0.0 },
        };
        let cal_imu = calibrate::solve_imu(&imu, want)?;
        r.log.push(format!(
            "calibrate: imu level — mount roll {:.2}° pitch {:.2}°, gyro bias ({:.3}, {:.3}, {:.3}) rad/s, {} samples",
            cal_imu.roll.to_degrees(), cal_imu.pitch.to_degrees(),
            cal_imu.gyro_bias[0], cal_imu.gyro_bias[1], cal_imu.gyro_bias[2], imu.n
        ));

        let mut r_lines: Vec<String> = Vec::new();
        for (j, a) in ACTUATORS.iter().enumerate() {
            // a joint resting on a stop under gravity (the hanging spine)
            // does not move under a small pulse; double it, up to 16×,
            // before concluding the encoder is blind
            let mut tau = calibrate::probe_tau(a);
            let mut last = None;
            for _ in 0..5 {
                // both directions' evidence, combined: a direction blocked by
                // a stop contributes ~0, a free one its full weight, and a
                // joint that really is flipped is negative in both
                let mut fits = Vec::new();
                let mut enc_ok = true;
                let mut fresh = true;
                let mut current = 0.0f32;
                for sgn in [1.0f32, -1.0] {
                    match port.run(Request::Probe { joint: j, tau: sgn * tau }) {
                        Some(Outcome::Probe { fit, enc_ok: e, current: i, fresh: f, .. }) => {
                            enc_ok &= e;
                            fresh &= f;
                            current = current.max(i);
                            if let Some(f) = fit {
                                r_log_probe(&mut r_lines, a.name, sgn * tau, &f, i);
                                fits.push(f);
                            }
                        }
                        _ => fresh = false,
                    }
                }
                let p = ProbeResult { fit: crate::fit::combine(&fits), enc_ok, current, fresh };
                let r = calibrate::check_probe(j, &p);
                let retry = matches!(r, Err(CalFault::DeadEncoder { .. })) && p.enc_ok && p.fresh;
                last = Some(r);
                if !retry || tau * 2.0 > calibrate::PROBE_MAX_FRAC * a.tau_deliverable() {
                    break;
                }
                tau *= 2.0;
            }
            if let Some(res) = last {
                if res.is_err() {
                    r.log.append(&mut r_lines);
                }
                res?;
            }
        }
        r.log.append(&mut r_lines);
        r.log.push("calibrate: torque and encoder agree on all 26 joints".into());

        // the IMU must still be there after the probes
        let again = match port.run(Request::ImuLevel { samples: want / 4 }) {
            Some(Outcome::Imu { n, missing, acc, gyro, acc_norm_sd }) => ImuResult { n, missing, acc, gyro, acc_norm_sd },
            _ => ImuResult { n: 0, missing: want / 4, acc: [0.0; 3], gyro: [0.0; 3], acc_norm_sd: 0.0 },
        };
        if again.n < (want / 4) * 9 / 10 {
            return Err(CalFault::ImuLost { got: again.n, missing: again.missing });
        }

        let mut joints = [JointCal { offset: 0.0, q_min: 0.0, q_max: 0.0 }; N_ACT];
        for (j, a) in ACTUATORS.iter().enumerate() {
            if a.tendon {
                // the tail has no hard stop (nine sprung segments: cable
                // length is a smooth function of force); its zero comes from
                // the force-free rest, after the body is calibrated and held
                joints[j] = JointCal { offset: 0.0, q_min: a.q_min, q_max: a.q_max };
                continue;
            }
            let cap = calibrate::home_cap_frac(a) * a.drv_peak_a;
            let h = match port.run(Request::Home { joint: j, current_cap: cap }) {
                Some(Outcome::Home { stop, reached, enc_ok, .. }) => HomeResult { stop, reached, enc_ok },
                _ => HomeResult { stop: None, reached: None, enc_ok: false },
            };
            r.log.push(format!(
                "  home  {:<17} {} stop at {:>8.4} (node)  reach verified {:>4.0}% of design travel",
                a.name,
                if a.home_high { "high" } else { "low " },
                h.stop.unwrap_or(f32::NAN),
                100.0 * calibrate::reach_fraction(j, &h)
            ));
            joints[j] = calibrate::solve_home(j, &h)?;
            if let Some(off) = record.offsets {
                calibrate::check_drift(j, &joints[j], off[j])?;
            }
        }
        r.log.push("calibrate: every joint homed on its stop and moved freely across its travel".into());
        Ok((joints, cal_imu))
    }
}

fn r_log_probe(out: &mut Vec<String>, name: &str, tau: f32, f: &crate::fit::FitResult, i: f32) {
    out.push(format!(
        "  probe {name:<17} τ {tau:>6.3}  motion {:>8.4}  t {:>7.1}  n {}  |i| {:.2} A",
        f.motion,
        f.t_stat(),
        f.n,
        i
    ));
}

/// The current clamp each node enforces once committed: the driver peak,
/// i.e. the widest envelope. KIRA narrows it per session (Phase 2).
pub fn current_limits() -> [f32; N_ACT] {
    let mut l = [0.0; N_ACT];
    for (j, a) in ACTUATORS.iter().enumerate() {
        l[j] = a.drv_peak_a;
    }
    l
}

pub fn explain(f: &CalFault) -> String {
    match f {
        CalFault::Flipped { dq, .. } => format!(
            "positive torque moved the encoder negative (Δ{dq:.4}). Closing the loop would drive it away from every target. \
             Re-run the node's FOC alignment or swap two motor phase leads; do not fix this with a sign in software."
        ),
        CalFault::DeadEncoder { .. } => "current flowed and the encoder did not see motion (or reports no magnetic field). Check the MT6701 magnet and wiring.".into(),
        CalFault::Silent { .. } => "no feedback frames from this joint while probing".into(),
        CalFault::NoStop { low, .. } => format!("did not find the {} stop under the homing current", if *low { "low" } else { "high" }),
        CalFault::Travel { measured, design, .. } => format!("stop-to-stop travel {measured:.3} vs design {design:.3}: the joint is not built as specified, or slips"),
        CalFault::Drift { by, .. } => format!("zero moved {by:.3} rad since the entity last calibrated this body: the encoder magnet or the horn has slipped"),
        CalFault::ImuLost { got, missing } => format!("IMU stopped answering ({got} samples, {missing} missed)"),
        CalFault::ImuImplausible { norm, sd } => format!("IMU reads |a| = {norm:.2} m/s² with spread {sd:.2}: not a still body in gravity"),
        CalFault::NotAcknowledged { .. } => "node did not acknowledge the pushed calibration".into(),
    }
}
