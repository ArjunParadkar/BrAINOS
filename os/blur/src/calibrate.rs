//! §8 step 5 for BLUR: turning probe results into a calibration, or into
//! a refusal that says which part is wrong.
//!
//! The procedure runs with BLUR held in the calibration jig (chest
//! clamped level, legs, spine, neck and tail free to hang):
//!
//! 1. **Torque/encoder agreement.** A ± torque pulse on every joint.
//!    Positive torque must move the encoder positive. If it moves it
//!    negative, closing the impedance loop on that joint drives it *away*
//!    from every target, i.e. the first stand-up would be a kick at full
//!    current. Refused, with the remedy. Gravity cancels out of the
//!    difference of the two pulses.
//! 2. **Encoder alive.** The MT6701's field-status bit, and motion under
//!    the probe torque. A joint that drew current and did not move has an
//!    encoder that is not seeing its magnet.
//! 3. **Stop and reach.** Each joint is driven slowly, under a capped
//!    current, to the stop its passive load pushes it toward (computed
//!    from the MJCF), which fixes the zero; then across its travel toward
//!    the other stop, which must neither stall early (a jam) nor pass the
//!    design travel (the wrong stop). For the 4015 joints, whose encoder
//!    sits ahead of the 16:1 gearbox, this also resolves the 22.5°
//!    ambiguity, and is needed at every power-on. The tail cables have no
//!    hard stop; they are zeroed from the force-free rest at the stand
//!    pose, after the rest of the body is calibrated.
//! 4. **Inner ear.** With the chest level, the mean accelerometer vector
//!    gives the IMU's mounting tilt, the mean gyro its bias, and the
//!    spread says whether the body really was still.
//!
//! **What this cannot see.** An encoder mounted reversed *and* a motor
//! whose FOC alignment followed it agree with each other, so step 1
//! passes; step 3 then homes on the opposite stop, and the reach check
//! sees the same travel. A mirrored joint is undetected here, on every
//! joint. `brainos blur gate1` says so in its report.

use crate::descriptor::{ActuatorSpec, EncoderSide, ACTUATORS, N_ACT};
use crate::wire::ConfigKey;
use alloc::vec::Vec;
use brainos_abi::ids::BodyId;
use brainos_abi::time::Tick;
use brainos_body::calibration::{Calibration, JointTrim};

/// Fraction of deliverable torque used for the direction pulse.
pub const PROBE_FRAC: f32 = 0.12;
/// The largest pulse the escalation will try.
pub const PROBE_MAX_FRAC: f32 = 0.8;
/// Below this much differential motion a pulse "did not move" the joint.
pub const MIN_MOTION: f32 = 0.004;
/// Travel tolerance: measured stop-to-stop vs the MJCF range.
pub const TRAVEL_TOL_FRAC: f32 = 0.06;
pub const TRAVEL_TOL_ABS: f32 = 0.03;
/// The tail is a passive sprung chain on a cable; its cable travel is a
/// loose number.
pub const WINCH_TOL_FRAC: f32 = 0.35;
/// Homing current cap as a fraction of the driver's peak.
pub fn home_cap_frac(a: &ActuatorSpec) -> f32 {
    match a.motor {
        // the spine lifts the hanging rear half (≈1.5 N·m) to reach its
        // upper stop, so it needs most of what the driver has
        crate::descriptor::MotorKind::Gimbal4015 => 0.9,
        // ~37 N on the cable: the tail is nine printed segments whose stops
        // are what is being found; at 125 N the twin pushes them through
        // their limits, and the real ones would break first
        crate::descriptor::MotorKind::Winch2204 => 0.15,
        _ => 0.35,
    }
}

pub fn probe_tau(a: &ActuatorSpec) -> f32 {
    let t = a.tau_deliverable() * PROBE_FRAC;
    if a.tendon {
        t
    } else {
        t.min(0.5)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProbeResult {
    pub fit: Option<crate::fit::FitResult>,
    pub enc_ok: bool,
    pub current: f32,
    pub fresh: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HomeResult {
    pub stop: Option<f32>,
    pub reached: Option<f32>,
    pub enc_ok: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImuResult {
    pub n: u32,
    pub missing: u32,
    pub acc: [f32; 3],
    pub gyro: [f32; 3],
    pub acc_norm_sd: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CalFault {
    /// positive torque moved the encoder negative
    Flipped { joint: usize, dq: f32 },
    /// the encoder reports no field, or did not move under current
    DeadEncoder { joint: usize },
    /// no feedback at all from the joint during probing
    Silent { joint: usize },
    /// a stop was not found where one should be
    NoStop { joint: usize, low: bool },
    /// stop-to-stop travel differs from the design
    Travel { joint: usize, measured: f32, design: f32 },
    /// the zero moved relative to the entity's record by more than slip tolerance
    Drift { joint: usize, by: f32 },
    /// the IMU stopped answering during calibration
    ImuLost { got: u32, missing: u32 },
    /// the IMU answered with numbers that are not a still body in gravity
    ImuImplausible { norm: f32, sd: f32 },
    /// the node did not acknowledge the pushed calibration
    NotAcknowledged { joint: usize },
}

impl CalFault {
    pub fn joint(&self) -> Option<usize> {
        match *self {
            CalFault::Flipped { joint, .. }
            | CalFault::DeadEncoder { joint }
            | CalFault::Silent { joint }
            | CalFault::NoStop { joint, .. }
            | CalFault::Travel { joint, .. }
            | CalFault::Drift { joint, .. }
            | CalFault::NotAcknowledged { joint } => Some(joint),
            _ => None,
        }
    }
}

/// The verdict needs the motor's pooled term to be significant (|t| ≥ T_SIG)
/// and large enough to be motion rather than quantisation; then its sign
/// is the direction.
pub const T_SIG: f32 = 4.0;

pub fn check_probe(j: usize, p: &ProbeResult) -> Result<(), CalFault> {
    let a = &ACTUATORS[j];
    if !p.fresh {
        return Err(CalFault::Silent { joint: j });
    }
    if a.encoder == EncoderSide::None {
        return Ok(());
    }
    if !p.enc_ok {
        return Err(CalFault::DeadEncoder { joint: j });
    }
    let Some(f) = p.fit else {
        return Err(CalFault::Silent { joint: j });
    };
    let min = if a.tendon { MIN_MOTION * (a.q_max - a.q_min) / 1.5 } else { MIN_MOTION };
    if f.t_stat().abs() < T_SIG || f.motion.abs() < min {
        return Err(CalFault::DeadEncoder { joint: j });
    }
    if f.b < 0.0 {
        return Err(CalFault::Flipped { joint: j, dq: f.motion });
    }
    Ok(())
}

/// Offset (node frame → design frame) and the measured limits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointCal {
    pub offset: f32,
    pub q_min: f32,
    pub q_max: f32,
}

/// The reach check must verify at least this much of the design travel.
/// Not all of it: a joint whose far end is loaded by a jump spring (the
/// front pitch: ~3 N·m at the end of its 244° travel) cannot be driven
/// there under the homing cap, and that is the spring, not a fault.
pub const REACH_MIN_FRAC: f32 = 0.35;

pub fn solve_home(j: usize, h: &HomeResult) -> Result<JointCal, CalFault> {
    let a = &ACTUATORS[j];
    if a.encoder != EncoderSide::None && !h.enc_ok {
        return Err(CalFault::DeadEncoder { joint: j });
    }
    let stop = h.stop.ok_or(CalFault::NoStop { joint: j, low: !a.home_high })?;
    let offset = if a.home_high { a.q_max - stop } else { a.q_min - stop };
    if a.encoder == EncoderSide::None {
        return Ok(JointCal { offset, q_min: a.q_min, q_max: a.q_max });
    }
    let design = a.q_max - a.q_min;
    let reached = h.reached.map(|r| (r - stop).abs()).unwrap_or(0.0);
    let tol = if a.tendon { WINCH_TOL_FRAC * design } else { TRAVEL_TOL_FRAC * design + TRAVEL_TOL_ABS };
    // past the far stop: the stop we homed on is not the design's stop
    if reached > design + tol {
        return Err(CalFault::Travel { joint: j, measured: reached, design });
    }
    if reached < REACH_MIN_FRAC * design {
        return Err(CalFault::Travel { joint: j, measured: reached, design });
    }
    Ok(JointCal { offset, q_min: a.q_min, q_max: a.q_max })
}

/// What fraction of the design travel the reach check verified — the
/// calibration's margin, reported per joint.
pub fn reach_fraction(j: usize, h: &HomeResult) -> f32 {
    let a = &ACTUATORS[j];
    match (h.stop, h.reached) {
        (Some(s), Some(r)) => (r - s).abs() / (a.q_max - a.q_min),
        _ => 0.0,
    }
}

/// For a joint-side encoder the zero is a property of the assembly (where
/// the magnet was glued), so it should match the entity's record modulo
/// one turn. A motor-side encoder's zero changes every power-on and is
/// not compared.
pub fn check_drift(j: usize, now: &JointCal, recorded_offset: f32) -> Result<(), CalFault> {
    let a = &ACTUATORS[j];
    if a.encoder != EncoderSide::Joint || a.tendon {
        return Ok(());
    }
    let two_pi = 2.0 * core::f32::consts::PI;
    let mut d = (now.offset - recorded_offset) % two_pi;
    if d > core::f32::consts::PI {
        d -= two_pi;
    }
    if d < -core::f32::consts::PI {
        d += two_pi;
    }
    if d.abs() > 0.05 {
        return Err(CalFault::Drift { joint: j, by: d });
    }
    Ok(())
}

/// The IMU's mounting tilt and gyro bias, from a still, level body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImuCal {
    pub roll: f32,
    pub pitch: f32,
    pub gyro_bias: [f32; 3],
}

pub fn solve_imu(r: &ImuResult, want: u32) -> Result<ImuCal, CalFault> {
    if r.n < want * 9 / 10 || r.missing > want / 10 {
        return Err(CalFault::ImuLost { got: r.n, missing: r.missing });
    }
    let [ax, ay, az] = r.acc;
    let norm = libm::sqrtf(ax * ax + ay * ay + az * az);
    if (norm - crate::wire::G).abs() > 0.05 * crate::wire::G || r.acc_norm_sd > 0.25 {
        return Err(CalFault::ImuImplausible { norm, sd: r.acc_norm_sd });
    }
    Ok(ImuCal {
        roll: libm::atan2f(ay, az),
        pitch: libm::atan2f(-ax, libm::sqrtf(ay * ay + az * az)),
        gyro_bias: r.gyro,
    })
}

/// Build the entity's record. Angles are stored in degrees (the
/// `JointTrim` convention); winch cable lengths in millimetres.
pub fn to_calibration(body: BodyId, at: Tick, joints: &[JointCal; N_ACT], imu: &ImuCal) -> Calibration {
    let mut c = Calibration::new(body, at);
    for (j, jc) in joints.iter().enumerate() {
        let a = &ACTUATORS[j];
        let k = if a.tendon { 1000.0 } else { 180.0 / core::f32::consts::PI };
        let t = JointTrim {
            channel: j as u8,
            offset_deg: jc.offset * k,
            scale: 1.0,
            min_deg: jc.q_min * k,
            max_deg: jc.q_max * k,
            probed: true,
        };
        // a trim the type refuses is a solver bug; surfacing it as a
        // missing joint makes is_complete fail rather than panicking
        let _ = c.trim(t);
    }
    let _ = c.bias("imu.mount_roll", imu.roll);
    let _ = c.bias("imu.mount_pitch", imu.pitch);
    let _ = c.bias("imu.gyro_bias_x", imu.gyro_bias[0]);
    let _ = c.bias("imu.gyro_bias_y", imu.gyro_bias[1]);
    let _ = c.bias("imu.gyro_bias_z", imu.gyro_bias[2]);
    c
}

/// The CONFIG frames that put a calibration on the nodes.
pub fn push_values(joints: &[JointCal; N_ACT], current_limits: &[f32; N_ACT]) -> Vec<(usize, ConfigKey, f32)> {
    let mut v = Vec::new();
    for (j, jc) in joints.iter().enumerate() {
        v.push((j, ConfigKey::Direction, 1.0));
        v.push((j, ConfigKey::Offset, jc.offset));
        v.push((j, ConfigKey::QMin, jc.q_min));
        v.push((j, ConfigKey::QMax, jc.q_max));
        v.push((j, ConfigKey::CurrentLimit, current_limits[j]));
    }
    for j in 0..N_ACT {
        v.push((j, ConfigKey::Commit, 1.0));
    }
    v
}
