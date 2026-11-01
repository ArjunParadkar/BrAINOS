//! Domain 2 on the Teensy 4.1: the part of BLUR that never waits.
//!
//! `Autonomic::step` is called once per millisecond with what the pins
//! read (`NerveUp`) and fills in what the pins write (`NerveDown`). On
//! the robot that call is the body of the 1 kHz timer interrupt and the
//! structs are filled by the FlexCAN / LPSPI drivers; in the twin they
//! come and go in a datagram. The logic is the same code either way.
//!
//! This file holds the acquisition-time procedures (§8 steps 1–5 need
//! the body to *do* things: answer a census, twitch a joint, find its
//! stops, take a calibration). The run-time supervisor — envelope,
//! reflexes, e-stop — is `supervisor.rs`.
//!
//! **Before calibration is committed, a node may move only under a
//! probe current cap.** Every probe here runs under it, so an
//! uncalibrated BLUR can twitch and home, and cannot stand up, kick, or
//! hit anyone hard. The node enforces the cap itself (`CONFIG Commit`
//! is what lifts it), so a Teensy bug cannot lift it either.

use crate::calibrate;
use crate::fit;
use crate::census::{Census, CensusBuilder};
use crate::descriptor::{Bus, EncoderSide, ACTUATORS, N_ACT};
use crate::wire::{
    self, Bmi270Burst, CanFrame, Config, ConfigKey, Feedback, NerveDown, NerveUp, Raw, Setpoint,
    FC_CONFIG_ACK, FC_FEEDBACK, FC_IDENTITY, FC_RAW, SP_ENABLE, ST_ENC_OK,
};
use alloc::vec::Vec;

/// Ticks a census listens for IDENTITY pages after the broadcast.
pub const CENSUS_TICKS: u32 = 60;
/// Homing: target ramp speed (rad/s, scaled for winches), stiffness, and
/// the tracking error that means "this is a stop".
pub const HOME_RAMP: f32 = 0.6;
pub const HOME_KP_FRAC: f32 = 2.0;
pub const HOME_STALL_ERR: f32 = 0.12;
pub const HOME_TIMEOUT_TICKS: u32 = 6_000;
/// How often (ticks) a probe refreshes its setpoint. Matches the bus
/// budget: 200 Hz.
pub const SP_EVERY: u32 = 5;

#[derive(Clone, Copy, Debug, Default)]
pub struct JointSeen {
    pub fb: Option<Feedback>,
    pub fb_tick: u32,
    pub raw: Option<Raw>,
    pub raw_tick: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Census,
    /// a ± torque pulse on one joint, to see that torque and encoder agree
    Probe { joint: usize, tau: f32 },
    /// find both stops at a capped current
    Home { joint: usize, current_cap: f32 },
    /// push configuration values and wait for every ack
    Push { values: Vec<(usize, ConfigKey, f32)> },
    /// average the IMU while the body is held still
    ImuLevel { samples: u32 },
    /// hold every calibrated joint at its stand angle, let the winches
    /// hang at zero force, and read the cables once they settle
    Rest { settle_ms: u32 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Census(Census),
    Probe {
        joint: usize,
        /// the doublet fit: sign of `b` is the direction, `t_stat` its confidence
        fit: Option<fit::FitResult>,
        enc_ok: bool,
        /// |current| seen while the pulse was on, A
        current: f32,
        fresh: bool,
    },
    Home {
        joint: usize,
        /// the stop found on the home side (node frame)
        stop: Option<f32>,
        /// the farthest the reach check got toward the other side
        reached: Option<f32>,
        enc_ok: bool,
    },
    Pushed { acked: usize, failed: Vec<(usize, ConfigKey)> },
    Rest {
        cables: Vec<(usize, f32)>,
        /// the worst stand-pose hold error at the moment of reading (joint, rad)
        hold_err: (usize, f32),
    },
    Imu {
        n: u32,
        missing: u32,
        acc: [f32; 3],
        gyro: [f32; 3],
        acc_norm_sd: f32,
    },
}

#[derive(Clone, Copy, Debug)]
struct PushItem {
    j: usize,
    key: ConfigKey,
    value: f32,
    seq: u8,
    sent_at: u32,
    attempts: u8,
}

#[derive(Clone, Debug)]
enum Proc {
    Idle,
    Census { until: u32, b: CensusBuilder },
    Probe { j: usize, tau: f32, t0: u32, fit: fit::Fit, last_fb: u32, i_peak: f32, enc_ok: bool, stage: u8 },
    Home { j: usize, cap: f32, t0: u32, target: f32, dir: f32, lo: Option<f32>, hi: Option<f32>, enc_ok: bool, stage: u8, mark: f32, integ: f32 },
    Push { pending: Vec<PushItem>, acked: usize, failed: Vec<(usize, ConfigKey)> },
    Imu { want: u32, n: u32, missing: u32, acc: [f32; 3], gyro: [f32; 3], norm_sum: f32, norm_sq: f32, until: u32 },
    Rest { t0: u32, settle: u32, from: [f32; N_ACT], sum: [f32; N_ACT], n: u32, integ: [f32; N_ACT] },
}

pub struct Autonomic {
    pub tick: u32,
    pub seen: [JointSeen; N_ACT],
    proc: Proc,
    done: Option<Outcome>,
    seq: u8,
    pub last_imu: Option<Bmi270Burst>,
    pub imu_missing_run: u32,
    pub rail_on: bool,
    pub estop_latched: bool,
    pub vbat_mv: u16,
    /// frames that did not fit this tick's TX mailboxes
    pub tx_dropped: u32,
    /// the run-time supervisor, installed once the body is calibrated
    pub sup: Option<crate::supervisor::Supervisor>,
}

impl Default for Autonomic {
    fn default() -> Self {
        Autonomic::new()
    }
}

fn act(j: usize) -> &'static crate::descriptor::ActuatorSpec {
    &ACTUATORS[j]
}

/// Probes and homing work in node units; a winch moves metres, not
/// radians, so its ramp and stall threshold scale with its range.
fn unit_scale(j: usize) -> f32 {
    let a = act(j);
    if a.tendon {
        (a.q_max - a.q_min) / 1.5
    } else {
        1.0
    }
}

impl Autonomic {
    pub fn new() -> Autonomic {
        Autonomic {
            tick: 0,
            seen: [JointSeen::default(); N_ACT],
            proc: Proc::Idle,
            done: None,
            seq: 0,
            last_imu: None,
            imu_missing_run: 0,
            rail_on: false,
            estop_latched: false,
            vbat_mv: 0,
            tx_dropped: 0,
            sup: None,
        }
    }

    pub fn busy(&self) -> bool {
        !matches!(self.proc, Proc::Idle)
    }

    pub fn request(&mut self, r: Request) -> bool {
        if self.busy() {
            return false;
        }
        let now = self.tick;
        self.proc = match r {
            Request::Census => Proc::Census {
                until: now + CENSUS_TICKS,
                b: CensusBuilder::new(),
            },
            Request::Probe { joint, tau } => Proc::Probe {
                j: joint,
                tau,
                t0: now,
                fit: fit::Fit::new(),
                last_fb: u32::MAX,
                i_peak: 0.0,
                enc_ok: true,
                stage: 0,
            },
            Request::Home { joint, current_cap } => Proc::Home {
                j: joint,
                cap: current_cap,
                t0: now,
                target: f32::NAN,
                dir: -1.0,
                lo: None,
                hi: None,
                enc_ok: true,
                stage: 0,
                mark: f32::NAN,
                integ: 0.0,
            },
            Request::Push { values } => Proc::Push {
                pending: values
                    .into_iter()
                    .map(|(j, key, value)| PushItem { j, key, value, seq: 0, sent_at: 0, attempts: 0 })
                    .collect(),
                acked: 0,
                failed: Vec::new(),
            },
            Request::Rest { settle_ms } => {
                let mut from = [f32::NAN; N_ACT];
                for (j, f) in from.iter_mut().enumerate() {
                    *f = self.seen[j].fb.map(|fb| fb.q).unwrap_or(f32::NAN);
                }
                Proc::Rest { t0: now, settle: settle_ms, from, sum: [0.0; N_ACT], n: 0, integ: [0.0; N_ACT] }
            }
            Request::ImuLevel { samples } => Proc::Imu {
                want: samples,
                n: 0,
                missing: 0,
                acc: [0.0; 3],
                gyro: [0.0; 3],
                norm_sum: 0.0,
                norm_sq: 0.0,
                until: now + samples * 3 + 50,
            },
        };
        true
    }

    pub fn take(&mut self) -> Option<Outcome> {
        self.done.take()
    }

    fn next_seq(&mut self) -> u8 {
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }

    fn push(&mut self, down: &mut NerveDown, f: CanFrame) {
        if !down.frames.push(f) {
            self.tx_dropped += 1;
        }
    }

    fn fresh_q(&self, j: usize) -> Option<f32> {
        let s = &self.seen[j];
        match s.fb {
            Some(fb) if self.tick.wrapping_sub(s.fb_tick) <= 20 => Some(fb.q),
            _ => None,
        }
    }

    fn enc_ok(&self, j: usize) -> bool {
        match (self.seen[j].fb, act(j).encoder) {
            (_, EncoderSide::None) => true,
            (Some(fb), _) => fb.status & ST_ENC_OK != 0,
            (None, _) => false,
        }
    }

    fn setpoint(&mut self, down: &mut NerveDown, j: usize, sp: Setpoint) {
        let a = act(j);
        let f = sp.encode(a.bus, a.node, a.motor);
        self.push(down, f);
    }

    fn config(&mut self, down: &mut NerveDown, j: usize, key: ConfigKey, value: f32, seq: u8) {
        let a = act(j);
        let f = Config { slot: a.slot, key, value, seq }.encode(a.bus, a.node, false);
        self.push(down, f);
    }

    fn ingest(&mut self, up: &NerveUp) {
        self.rail_on = up.rail_on;
        self.estop_latched = up.estop_latched;
        self.vbat_mv = up.vbat_mv;
        match up.imu {
            Some(b) => {
                self.last_imu = Some(b);
                self.imu_missing_run = 0;
            }
            None => self.imu_missing_run = self.imu_missing_run.saturating_add(1),
        }
        for f in up.frames.as_slice() {
            match f.function() {
                FC_FEEDBACK => {
                    if let Some(j) = crate::descriptor::actuator_at(f.node(), f.data[0]) {
                        if act(j).bus == f.bus {
                            if let Some(fb) = Feedback::decode(f, act(j).motor) {
                                self.seen[j].fb = Some(fb);
                                self.seen[j].fb_tick = self.tick;
                            }
                        }
                    }
                }
                FC_RAW => {
                    if let Some(j) = crate::descriptor::actuator_at(f.node(), f.data[0]) {
                        if let Some(r) = Raw::decode(f) {
                            self.seen[j].raw = Some(r);
                            self.seen[j].raw_tick = self.tick;
                        }
                    }
                }
                FC_IDENTITY => {
                    if let Proc::Census { b, .. } = &mut self.proc {
                        b.page(f);
                    }
                }
                FC_CONFIG_ACK => {
                    if let Some((c, true)) = Config::decode(f) {
                        if let Proc::Push { pending, acked, .. } = &mut self.proc {
                            if let Some(pos) = pending.iter().position(|p| {
                                p.attempts > 0
                                    && act(p.j).node == f.node()
                                    && act(p.j).slot == c.slot
                                    && p.key == c.key
                                    && p.seq == c.seq
                                    && (c.value - p.value).abs() <= 1e-6 * (1.0 + p.value.abs())
                            }) {
                                pending.remove(pos);
                                *acked += 1;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// One millisecond.
    pub fn step(&mut self, up: &NerveUp, down: &mut NerveDown) {
        self.tick = up.tick;
        down.tick = up.tick;
        self.ingest(up);
        // the e-stop outranks every procedure: abandon it, stop the body
        if up.estop_latched {
            if !matches!(self.proc, Proc::Idle) {
                self.proc = Proc::Idle;
                self.done = None;
            }
            match self.sup.as_mut() {
                Some(s) => s.step(up, &self.seen, down),
                None => {
                    down.relay_open = true;
                    for b in Bus::ALL {
                        let f = wire::estop(b, 1, (up.tick & 0xFF) as u8);
                        self.push(down, f);
                    }
                }
            }
            return;
        }
        let proc = core::mem::replace(&mut self.proc, Proc::Idle);
        self.proc = self.advance(proc, down);
        if matches!(self.proc, Proc::Idle) {
            if let Some(s) = self.sup.as_mut() {
                s.step(up, &self.seen, down);
            }
        }
    }

    fn advance(&mut self, proc: Proc, down: &mut NerveDown) -> Proc {
        let now = self.tick;
        match proc {
            Proc::Idle => Proc::Idle,

            Proc::Census { until, b } => {
                if b.is_fresh() {
                    for bus in Bus::ALL {
                        self.push(down, wire::census(bus));
                    }
                    let mut b = b;
                    b.started();
                    return Proc::Census { until, b };
                }
                if now >= until {
                    let imu = self.imu_missing_run == 0;
                    self.done = Some(Outcome::Census(b.finish(imu)));
                    Proc::Idle
                } else {
                    Proc::Census { until, b }
                }
            }

            Proc::Probe { j, tau, t0, mut fit, mut last_fb, mut i_peak, mut enc_ok, stage } => {
                let dt = now.wrapping_sub(t0);
                enc_ok &= self.enc_ok(j);
                if let Some(fb) = self.seen[j].fb {
                    let i = if fb.current < 0.0 { -fb.current } else { fb.current };
                    if stage == 1 && i > i_peak {
                        i_peak = i;
                    }
                    // one sample per feedback frame that arrived since the last tick
                    if stage == 1 && self.seen[j].fb_tick != last_fb {
                        last_fb = self.seen[j].fb_tick;
                        fit.push(dt as f32 * 0.001, fb.q);
                    }
                }
                match stage {
                    // let the joint settle a moment, then run the doublet
                    0 if dt >= 30 => {
                        return Proc::Probe { j, tau, t0: now, fit, last_fb, i_peak, enc_ok, stage: 1 };
                    }
                    1 => {
                        // the torque the node should be applying for the
                        // millisecond that starts now; ProbeTorque is a
                        // stream the node drops after 50 ms of silence
                        let u = fit::profile(dt);
                        let prev = if dt == 0 { f32::NAN } else { fit::profile(dt - 1) };
                        if u != prev || dt % 10 == 0 {
                            let seq = self.next_seq();
                            self.config(down, j, ConfigKey::ProbeTorque, u * tau, seq);
                        }
                        if dt >= fit::DOUBLET_MS + 40 {
                            let r = fit.solve(tau);
                            self.done = Some(Outcome::Probe {
                                joint: j,
                                fit: r,
                                enc_ok,
                                current: i_peak,
                                fresh: fit.len() >= 8,
                            });
                            return Proc::Idle;
                        }
                    }
                    _ => {}
                }
                Proc::Probe { j, tau, t0, fit, last_fb, i_peak, enc_ok, stage }
            }

            Proc::Home { j, cap, t0, mut target, mut dir, mut lo, mut hi, mut enc_ok, mut stage, mut mark, mut integ } => {
                // lo = the stop found on the home side; hi = how far the
                // reach check got toward the other side (node frame)
                let a = act(j);
                let us = unit_scale(j);
                let dt = now.wrapping_sub(t0);
                enc_ok &= self.enc_ok(j);
                let q = self.fresh_q(j);
                if stage == 0 {
                    let seq = self.next_seq();
                    self.config(down, j, ConfigKey::CurrentLimit, cap, seq);
                    dir = if a.home_high { 1.0 } else { -1.0 };
                    return Proc::Home { j, cap, t0: now, target, dir, lo, hi, enc_ok, stage: 1, mark: f32::NAN, integ };
                }
                let Some(q) = q else {
                    if dt > 200 {
                        self.done = Some(Outcome::Home { joint: j, stop: lo, reached: hi, enc_ok: false });
                        return Proc::Idle;
                    }
                    return Proc::Home { j, cap, t0, target, dir, lo, hi, enc_ok, stage, mark, integ };
                };
                if target.is_nan() {
                    target = q;
                }
                if dt > HOME_TIMEOUT_TICKS {
                    stage = 9;
                }
                let kp = HOME_KP_FRAC * a.tau_deliverable() / us;
                let kd = 0.02 * kp;
                let travel = a.q_max - a.q_min;
                let err = target - q;
                let e = if err < 0.0 { -err } else { err };
                let mut stalled = false;
                if dt % 150 == 0 {
                    if !mark.is_nan() {
                        let moved = q - mark;
                        let m = if moved < 0.0 { -moved } else { moved };
                        stalled = dt >= 300 && m < 0.015 * us && e > 0.5 * HOME_STALL_ERR * us;
                    }
                    mark = q;
                }
                match stage {
                    // 1: to the home-side stop, until the joint stops following
                    1 => {
                        if e < HOME_STALL_ERR * us {
                            target += dir * HOME_RAMP * us * 0.001;
                        }
                        if stalled {
                            lo = Some(q);
                            dir = -dir;
                            return Proc::Home { j, cap, t0: now, target: q, dir, lo, hi, enc_ok, stage: 2, mark: f32::NAN, integ };
                        }
                    }
                    // 2: the reach check — across the travel, toward (and a
                    // little past) where the design says the other stop is.
                    // Getting past it means the stop we found is not the
                    // design's stop; stalling far short of it is a spring
                    // or a jam, and how far it got is reported either way.
                    2 => {
                        let stop = lo.unwrap_or(q);
                        let goal = stop + dir * (travel + 0.1) * if a.tendon { 1.0 } else { 1.0 };
                        let remaining = (goal - target) * dir;
                        if e < HOME_STALL_ERR * us && remaining > 0.0 {
                            target += dir * HOME_RAMP * us * 0.001;
                        }
                        let far = (q - stop) * dir;
                        let best = hi.map(|h| (h - stop) * dir).unwrap_or(0.0);
                        if far > best {
                            hi = Some(q);
                        }
                        if stalled || (remaining <= 0.0 && e < 0.5 * HOME_STALL_ERR * us) {
                            return Proc::Home { j, cap, t0: now, target, dir, lo, hi, enc_ok, stage: 3, mark: f32::NAN, integ };
                        }
                    }
                    // 3: back to the middle of what was verified, then limp
                    3 => {
                        let mid = match (lo, hi) {
                            (Some(l), Some(h)) => 0.5 * (l + h),
                            _ => q,
                        };
                        let step = HOME_RAMP * us * 0.001;
                        if (target - mid).abs() <= step {
                            target = mid;
                            if dt > 300 {
                                stage = 9;
                            }
                        } else {
                            target += if mid > target { step } else { -step };
                        }
                    }
                    _ => {}
                }
                if stage == 9 {
                    self.setpoint(down, j, Setpoint { slot: a.slot, flags: 0, q, tau_ff: 0.0, kp: 0.0, kd: 0.0 });
                    self.done = Some(Outcome::Home { joint: j, stop: lo, reached: hi, enc_ok });
                    return Proc::Idle;
                }
                let lim = calibrate::home_cap_frac(a) * a.tau_deliverable();
                integ = (integ + 4.0 * kp * (target - q) * 0.001).clamp(-lim, lim);
                if dt % SP_EVERY == 0 {
                    self.setpoint(down, j, Setpoint { slot: a.slot, flags: SP_ENABLE, q: target, tau_ff: integ, kp, kd });
                }
                Proc::Home { j, cap, t0, target, dir, lo, hi, enc_ok, stage, mark, integ }
            }

            Proc::Push { mut pending, acked, mut failed } => {
                // an item is resent when its ack is 30 ms late, at most
                // three times; at most 24 frames go out per millisecond so a
                // push never floods a bus
                let mut i = 0;
                while i < pending.len() {
                    if pending[i].attempts >= 3 && now.wrapping_sub(pending[i].sent_at) > 30 {
                        let p = pending.remove(i);
                        failed.push((p.j, p.key));
                    } else {
                        i += 1;
                    }
                }
                if pending.is_empty() {
                    self.done = Some(Outcome::Pushed { acked, failed });
                    return Proc::Idle;
                }
                let mut out = Vec::new();
                for p in pending.iter_mut() {
                    if out.len() >= 24 {
                        break;
                    }
                    if p.attempts == 0 || (p.attempts < 3 && now.wrapping_sub(p.sent_at) > 30) {
                        self.seq = self.seq.wrapping_add(1);
                        p.seq = self.seq;
                        p.sent_at = now;
                        p.attempts += 1;
                        out.push((p.j, p.key, p.value, p.seq));
                    }
                }
                for (j, k, v, seq) in out {
                    self.config(down, j, k, v, seq);
                }
                Proc::Push { pending, acked, failed }
            }

            Proc::Rest { t0, settle, from, mut sum, mut n, mut integ } => {
                let dt = now.wrapping_sub(t0);
                // ramp from wherever each joint hangs to its stand angle over
                // 0.8 s, then hold; winches and the gripper get no torque
                let ramp = (dt as f32 / 800.0).min(1.0);
                // integral action after the ramp: the spine holding the rear
                // half sags ~0.1 rad under stiffness alone, and the tail's
                // rest length depends on exactly how the pelvis is pitched
                if ramp >= 1.0 {
                    for j in 0..N_ACT {
                        let a = act(j);
                        if a.tendon || a.encoder == EncoderSide::None {
                            continue;
                        }
                        if let Some(fb) = self.seen[j].fb {
                            let lim = 0.6 * a.tau_deliverable();
                            integ[j] = (integ[j] + 8.0 * a.tau_deliverable() * (a.q_stand - fb.q) * 0.001).clamp(-lim, lim);
                        }
                    }
                }
                if dt % SP_EVERY == 0 {
                    for j in 0..N_ACT {
                        let a = act(j);
                        if a.encoder == EncoderSide::None {
                            self.setpoint(down, j, Setpoint { slot: a.slot, flags: 0, q: 0.0, tau_ff: 0.0, kp: 0.0, kd: 0.0 });
                            continue;
                        }
                        if a.tendon {
                            // damping only: it stops the nine-segment chain
                            // ringing (its own damping is tiny) and exerts no
                            // force once the tail is still, so the rest it
                            // settles to is the zero-force rest
                            self.setpoint(down, j, Setpoint { slot: a.slot, flags: SP_ENABLE, q: 0.0, tau_ff: 0.0, kp: 0.0, kd: 40.0 });
                            continue;
                        }
                        let start = if from[j].is_nan() { a.q_stand } else { from[j] };
                        let q = start + (a.q_stand - start) * ramp;
                        let kp = 4.0 * a.tau_deliverable();
                        self.setpoint(down, j, Setpoint { slot: a.slot, flags: SP_ENABLE, q, tau_ff: integ[j], kp, kd: 0.03 * kp });
                    }
                }
                if dt + 300 >= settle {
                    for j in 0..N_ACT {
                        if act(j).tendon {
                            if let Some(fb) = self.seen[j].fb {
                                sum[j] += fb.q;
                            }
                        }
                    }
                    n += 1;
                }
                if dt >= settle {
                    let d = if n == 0 { 1.0 } else { n as f32 };
                    let cables = (0..N_ACT).filter(|j| act(*j).tendon).map(|j| (j, sum[j] / d)).collect();
                    let mut worst = (0usize, 0.0f32);
                    for j in 0..N_ACT {
                        let a = act(j);
                        if a.tendon || a.encoder == EncoderSide::None {
                            continue;
                        }
                        if let Some(fb) = self.seen[j].fb {
                            let e = fb.q - a.q_stand;
                            if e.abs() > worst.1.abs() {
                                worst = (j, e);
                            }
                        }
                    }
                    self.done = Some(Outcome::Rest { cables, hold_err: worst });
                    return Proc::Idle;
                }
                Proc::Rest { t0, settle, from, sum, n, integ }
            }

            Proc::Imu { want, mut n, mut missing, mut acc, mut gyro, mut norm_sum, mut norm_sq, until } => {
                if self.imu_missing_run == 0 {
                    if let Some(b) = self.last_imu {
                        let a3 = b.accel();
                        let g3 = b.gyro();
                        for k in 0..3 {
                            acc[k] += a3[k];
                            gyro[k] += g3[k];
                        }
                        let nn = libm::sqrtf(a3[0] * a3[0] + a3[1] * a3[1] + a3[2] * a3[2]);
                        norm_sum += nn;
                        norm_sq += nn * nn;
                        n += 1;
                    }
                } else {
                    missing += 1;
                }
                if n >= want || now >= until {
                    let d = if n == 0 { 1.0 } else { n as f32 };
                    let mean = norm_sum / d;
                    let var = (norm_sq / d - mean * mean).max(0.0);
                    self.done = Some(Outcome::Imu {
                        n,
                        missing,
                        acc: [acc[0] / d, acc[1] / d, acc[2] / d],
                        gyro: [gyro[0] / d, gyro[1] / d, gyro[2] / d],
                        acc_norm_sd: libm::sqrtf(var),
                    });
                    return Proc::Idle;
                }
                Proc::Imu { want, n, missing, acc, gyro, norm_sum, norm_sq, until }
            }
        }
    }
}

