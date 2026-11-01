//! Domain 2 at run time: the envelope, the reflexes, the e-stop.
//!
//! Called from `Autonomic::step` every millisecond once the body is
//! calibrated. Order of authority inside one tick, highest first:
//!
//! 1. **E-stop.** The latching switch is in series with the motor rail;
//!    when it is pressed the rail is already dead. The supervisor also
//!    broadcasts ESTOP on all three buses, so every node latches itself
//!    disabled and stays so after the rail returns. There is no frame that
//!    clears it and no code path here that tries: a node comes back only
//!    through a fresh §8 calibration push (`Commit`), i.e. the entity
//!    re-acquiring the body. That is the e-stop path no layer can
//!    override.
//! 2. **Reflexes** — they fire without asking cognition. Standing, the
//!    balance reflex (a learned push-recovery policy around the ready
//!    stance) holds the body. A fall braces it (a soft tuck until it comes
//!    to rest), then hands it to the learned righting reflex if it lies on
//!    its side or back, and to the learned stand-up reflex once it is belly
//!    down; standing still for 2 s returns it to balance. A sagging pack or
//!    a lost link to the seat crouches it. Every reflex's targets go through
//!    the same envelope clamp as a session's.
//! 3. **The granted session.** The motor cortex's targets are clamped into
//!    the KIRA-granted envelope before they reach the bus, and the nodes
//!    clamp current again inside their own loop (the second, independent
//!    check). With no grant, or an expired one, the body holds.

use crate::attitude::{ImuMount, Mahony};
use crate::autonomic::JointSeen;
use crate::descriptor::{Bus, EncoderSide, ACTUATORS, N_ACT};
use crate::mlp::Mlp;
use crate::wire::{self, CanFrame, Config, ConfigKey, NerveDown, NerveUp, Setpoint, SP_ENABLE};

/// What a session may do, per joint. Granted by KIRA, enforced here and
/// (current) on the nodes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Envelope {
    pub q_min: [f32; N_ACT],
    pub q_max: [f32; N_ACT],
    /// phase current, A — what the nodes clamp to
    pub i_max: [f32; N_ACT],
    pub kp_max: [f32; N_ACT],
    /// beyond this tilt the session is over and the reflexes own the body
    pub tilt_max_deg: f32,
}

impl Envelope {
    /// The widest envelope this body physically has: the design ranges and
    /// what each driver can deliver. A session is always a narrowing of it.
    pub fn physical() -> Envelope {
        let mut e = Envelope {
            q_min: [0.0; N_ACT],
            q_max: [0.0; N_ACT],
            i_max: [0.0; N_ACT],
            kp_max: [0.0; N_ACT],
            tilt_max_deg: 60.0,
        };
        for (j, a) in ACTUATORS.iter().enumerate() {
            e.q_min[j] = a.q_min;
            e.q_max[j] = a.q_max;
            e.i_max[j] = a.drv_peak_a;
            e.kp_max[j] = if a.tendon { 3000.0 } else { 12.0 * a.tau_deliverable() };
        }
        e
    }

    /// Is `self` entirely inside `outer`?
    pub fn is_within(&self, outer: &Envelope) -> bool {
        (0..N_ACT).all(|j| {
            self.q_min[j] >= outer.q_min[j]
                && self.q_max[j] <= outer.q_max[j]
                && self.q_min[j] <= self.q_max[j]
                && self.i_max[j] <= outer.i_max[j]
                && self.kp_max[j] <= outer.kp_max[j]
        }) && self.tilt_max_deg <= outer.tilt_max_deg
    }

    /// Scale the current limit of every joint (a gentle session).
    pub fn with_current_fraction(mut self, f: f32) -> Envelope {
        for j in 0..N_ACT {
            self.i_max[j] *= f.clamp(0.0, 1.0);
        }
        self
    }
}

/// A KIRA grant as the Teensy holds it: which skill, inside what, until when.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grant {
    pub id: [u8; 32],
    pub skill: u8,
    pub env: Envelope,
    pub until_tick: u32,
}

/// One frame of joint targets from the motor cortex (the Pi, 50 Hz).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Command {
    pub tick: u32,
    pub q: [f32; N_ACT],
    pub kp: [f32; N_ACT],
    pub kd: [f32; N_ACT],
    pub tau: [f32; N_ACT],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reflex {
    /// hold the stand pose
    Hold,
    /// the learned get-up policy (one policy for the whole recovery; the
    /// first Gate 2 attempt, kept for comparison)
    GetUp,
    /// fallen: a soft tuck until the body comes to rest, then classify
    Brace,
    /// learned: side/back -> belly down, legs tucked (sternal)
    Righting,
    /// learned: sternal -> standing
    StandUp,
    /// fold down to lie and stop (battery, lost seat)
    Crouch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// calibrated, nothing granted: hold
    Idle,
    Session,
    Reflex(Reflex),
    EStopped,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub setpoints: u32,
    /// setpoint values the supervisor had to clamp into the envelope
    pub clamped: u32,
    pub estop_frames: u32,
    pub falls: u32,
    pub getups_ok: u32,
    pub getups_failed: u32,
    pub link_losses: u32,
    pub battery_crouches: u32,
    pub sessions_ended_by_reflex: u32,
}

pub const SEAT_TIMEOUT_MS: u32 = 120;
pub const VBAT_CUTOFF_MV: u16 = 19_800;
pub const VBAT_HOLD_MS: u32 = 500;
pub const FALL_TILT_DEG: f32 = 50.0;
pub const FALL_TILT_FAST_DEG: f32 = 30.0;
pub const FALL_RATE: f32 = 2.5;
pub const FALL_CONFIRM_MS: u32 = 30;
pub const GETUP_TIMEOUT_MS: u32 = 10_000;
pub const UPRIGHT_HOLD_MS: u32 = 800;
/// the chain's clocks (blur/learn/skills.py: horizons and hold times)
pub const BRACE_MAX_MS: u32 = 1500;
pub const BRACE_STILL_MS: u32 = 200;
pub const SKILL_TIMEOUT_MS: u32 = 6000;
pub const STERNAL_HOLD_MS: u32 = 500;
pub const STAND_HOLD_MS: u32 = 2000;
pub const RECOVERY_TRIES: u32 = 3;
pub const STANCE_BLEND_MS: u32 = 600;
/// the policies' command: elapsed skill time over this, capped at 1
pub const COMMAND_T_S: f32 = 6.0;
/// the command filter: each 20 ms the targets move this fraction of the way
/// to the policy's (blur/learn/skills.py `action_filter`)
pub const FILTER_GETUP: f32 = 0.1;
pub const FILTER_BALANCE: f32 = 0.3;

/// A keyframe reference at time t (s): minimum-jerk between keys, held after
/// the last (`blur/learn/ref/keyframes.py` `Reference`).
pub fn reference_at(keys: &[(f32, [f32; N_ACT])], t: f32) -> [f32; N_ACT] {
    if keys.is_empty() {
        return stand_pose();
    }
    if t <= 0.0 {
        return keys[0].1;
    }
    for w in keys.windows(2) {
        let ((t0, q0), (t1, q1)) = (w[0], w[1]);
        if t <= t1 {
            let s = min_jerk((t - t0) / (t1 - t0).max(1e-9));
            let mut q = q0;
            for j in 0..N_ACT {
                q[j] = q0[j] + (q1[j] - q0[j]) * s;
            }
            return q;
        }
    }
    keys[keys.len() - 1].1
}
pub const SP_EVERY: u32 = 5;
pub const POLICY_EVERY: u32 = 20;

pub struct Supervisor {
    pub mode: Mode,
    pub grant: Option<Grant>,
    pub cmd: Option<Command>,
    pub att: Mahony,
    att_started: bool,
    pub getup: Option<Mlp>,
    /// the Gate 2 reflexes: righting, stand-up, balance (blur/learn/skills.py)
    pub righting: Option<Mlp>,
    pub standup: Option<Mlp>,
    pub balance: Option<Mlp>,
    /// the action the running policy last produced (it observes it)
    prev_action: [f32; 32],
    /// consecutive ms the current skill's success predicate has held
    pred_ms: u32,
    /// ms the body has been still while bracing
    still_ms: u32,
    tries: u32,
    hold_t0: u32,
    hold_from: [f32; N_ACT],
    /// the righting reference for the side the body came to rest on
    righting_ref: &'static [(f32, [f32; N_ACT])],
    /// the tick of the latest step (resume() starts its stance blend here)
    now: u32,
    targets: [f32; N_ACT],
    reflex_t0: u32,
    fall_ms: u32,
    upright_ms: u32,
    low_v_ms: u32,
    pub stats: Stats,
    /// current limits most recently pushed to the nodes
    pushed_i: [f32; N_ACT],
    seq: u8,
}

/// The pose the reflexes hold and get up to.
pub fn stand_pose() -> [f32; N_ACT] {
    let mut q = [0.0; N_ACT];
    for (j, a) in ACTUATORS.iter().enumerate() {
        q[j] = if a.tendon { a.q_rest } else { a.q_stand };
    }
    q
}

/// The impedance gains the reflexes use (the same law the policies were
/// trained against: kp = 6 × deliverable torque per rad).
pub fn reflex_gains() -> ([f32; N_ACT], [f32; N_ACT]) {
    let mut kp = [0.0; N_ACT];
    let mut kd = [0.0; N_ACT];
    for (j, a) in ACTUATORS.iter().enumerate() {
        if a.tendon {
            kp[j] = 100.0;
            kd[j] = 5.0;
        } else if a.encoder == EncoderSide::None {
            kp[j] = 0.0;
            kd[j] = 0.0;
        } else {
            kp[j] = 6.0 * a.tau_deliverable();
            kd[j] = 0.04 * kp[j];
        }
    }
    (kp, kd)
}

/// The joints a learned locomotion/recovery policy drives (legs + spine),
/// in descriptor order — the same list as `blur/learn/env.py` POLICY_JOINTS.
pub fn policy_joints() -> impl Iterator<Item = usize> {
    ACTUATORS.iter().enumerate().filter(|(_, a)| {
        let n = a.name.as_bytes();
        (n.len() > 2 && matches!(&n[..2], b"FR" | b"FL" | b"RR" | b"RL")) || a.name.starts_with("spine")
    }).map(|(j, _)| j)
}

pub const ACTION_SCALE: f32 = 0.6;

/// How a policy's action becomes joint targets. `blur/learn/env.py`
/// `targets_from_action` is the same function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionMode {
    /// nominal + 0.6 rad × a: fine skills around a pose
    Offset,
    /// a ∈ [-1, 1] spans each joint's whole range piecewise through the
    /// nominal (−1 → q_min, 0 → nominal, +1 → q_max): recovery
    Range,
}

pub fn targets_from_action(nominal: &[f32; N_ACT], action: &[f32], mode: ActionMode) -> [f32; N_ACT] {
    let mut t = *nominal;
    for (k, j) in policy_joints().enumerate() {
        let Some(&a) = action.get(k) else { break };
        let a = a.clamp(-1.0, 1.0);
        let d = &ACTUATORS[j];
        let n = nominal[j];
        t[j] = match mode {
            ActionMode::Offset => n + ACTION_SCALE * a,
            ActionMode::Range => {
                if a >= 0.0 {
                    n + a * (d.q_max - n)
                } else {
                    n + a * (n - d.q_min)
                }
            }
        };
    }
    for (j, d) in ACTUATORS.iter().enumerate() {
        t[j] = t[j].clamp(d.q_min, d.q_max);
    }
    t
}

impl Supervisor {
    pub fn new(mount: ImuMount) -> Supervisor {
        Supervisor {
            mode: Mode::Idle,
            grant: None,
            cmd: None,
            att: Mahony::new(mount),
            att_started: false,
            getup: None,
            righting: None,
            standup: None,
            balance: None,
            prev_action: [0.0; 32],
            pred_ms: 0,
            still_ms: 0,
            tries: 0,
            hold_t0: 0,
            hold_from: stand_pose(),
            righting_ref: crate::refs_gen::RIGHTING_LEFT,
            now: 0,
            targets: stand_pose(),
            reflex_t0: 0,
            fall_ms: 0,
            upright_ms: 0,
            low_v_ms: 0,
            stats: Stats::default(),
            pushed_i: [0.0; N_ACT],
            seq: 0,
        }
    }

    pub fn install(&mut self, g: Grant) -> bool {
        if self.mode == Mode::EStopped || !g.env.is_within(&Envelope::physical()) {
            return false;
        }
        // a reflex that is protecting the body (falling, getting up,
        // crouching) is not interrupted by a grant; only a holding body
        // takes a new session
        if matches!(self.mode, Mode::Reflex(Reflex::GetUp) | Mode::Reflex(Reflex::Crouch) | Mode::Reflex(Reflex::Brace)
            | Mode::Reflex(Reflex::Righting) | Mode::Reflex(Reflex::StandUp)) {
            return false;
        }
        self.grant = Some(g);
        if self.mode == Mode::Idle || self.mode == Mode::Reflex(Reflex::Hold) {
            self.mode = Mode::Session;
        }
        true
    }

    /// After a reflex ended in a crouch (a get-up that failed, a sagging
    /// pack), the body stays down until the entity decides otherwise. This
    /// is that decision. It does not clear an e-stop: nothing does, short
    /// of re-acquisition.
    pub fn resume(&mut self) -> bool {
        if self.mode == Mode::EStopped {
            return false;
        }
        self.enter_hold(self.now);
        self.fall_ms = 0;
        self.low_v_ms = 0;
        true
    }

    fn recovering(&self) -> bool {
        matches!(self.mode, Mode::Reflex(Reflex::GetUp) | Mode::Reflex(Reflex::Brace)
            | Mode::Reflex(Reflex::Righting) | Mode::Reflex(Reflex::StandUp))
    }

    fn enter(&mut self, r: Reflex, tick: u32) {
        self.mode = Mode::Reflex(r);
        self.reflex_t0 = tick;
        self.pred_ms = 0;
        self.still_ms = 0;
        self.prev_action = [0.0; 32];
        // a skill starts from its reference's first pose, as in training
        match r {
            Reflex::StandUp => self.targets = reference_at(crate::refs_gen::STANDUP, 0.0),
            Reflex::Righting => self.targets = reference_at(self.righting_ref, 0.0),
            _ => {}
        }
    }

    /// Standing: blend from wherever the joints are into the ready stance,
    /// then the balance reflex (if one is installed) takes over.
    fn enter_hold(&mut self, tick: u32) {
        self.mode = Mode::Reflex(Reflex::Hold);
        self.hold_t0 = tick;
        self.hold_from = self.targets;
        self.prev_action = [0.0; 32];
    }

    /// Re-initialise the attitude estimate from the next accelerometer
    /// sample (the body was moved by hand, not by its own motion).
    pub fn realign(&mut self) {
        self.att_started = false;
        self.fall_ms = 0;
    }

    pub fn revoke(&mut self) {
        self.grant = None;
        if self.mode == Mode::Session {
            self.mode = Mode::Idle;
        }
    }

    pub fn command(&mut self, c: Command) {
        self.cmd = Some(c);
    }

    fn next_seq(&mut self) -> u8 {
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }

    fn push(&mut self, down: &mut NerveDown, f: CanFrame) {
        let _ = down.frames.push(f);
    }

    fn estop(&mut self, down: &mut NerveDown, tick: u32) {
        self.mode = Mode::EStopped;
        self.grant = None;
        self.cmd = None;
        down.relay_open = true;
        // first 10 ms every tick, then every 50 ms: a node that missed one
        // (bus error, it was rebooting) still hears it
        if tick % 50 == 0 || self.stats.estop_frames < 30 {
            for b in Bus::ALL {
                let f = wire::estop(b, 1, (tick & 0xFF) as u8);
                self.push(down, f);
                self.stats.estop_frames += 1;
            }
        }
    }

    /// One millisecond. `seen` is the Autonomic's latest feedback per joint.
    pub fn step(&mut self, up: &NerveUp, seen: &[JointSeen; N_ACT], down: &mut NerveDown) {
        let tick = up.tick;
        self.now = tick;
        // 1. e-stop: the switch, or the rail gone for any reason
        if up.estop_latched || self.mode == Mode::EStopped {
            self.estop(down, tick);
            return;
        }

        // the inner ear
        if let Some(b) = up.imu {
            if !self.att_started {
                self.att.align(b.accel());
                self.att_started = true;
            } else {
                self.att.update(b.accel(), b.gyro(), 0.001);
            }
        }
        let tilt = self.att.tilt_deg();
        let rate = up.imu.map(|b| {
            let w = self.att.rate(b.gyro());
            libm::sqrtf(w[0] * w[0] + w[1] * w[1] + w[2] * w[2])
        }).unwrap_or(0.0);

        // 2. reflex triggers
        let falling = tilt > FALL_TILT_DEG || (tilt > FALL_TILT_FAST_DEG && rate > FALL_RATE);
        self.fall_ms = if falling { self.fall_ms + 1 } else { 0 };
        let chain = self.righting.is_some() && self.standup.is_some();
        if self.fall_ms >= FALL_CONFIRM_MS && !self.recovering() && (chain || self.getup.is_some()) {
            if self.mode == Mode::Session {
                self.stats.sessions_ended_by_reflex += 1;
            }
            self.stats.falls += 1;
            self.grant = None;
            self.upright_ms = 0;
            self.tries = 0;
            self.enter(if chain { Reflex::Brace } else { Reflex::GetUp }, tick);
        }
        self.low_v_ms = if up.vbat_mv > 0 && up.vbat_mv < VBAT_CUTOFF_MV { self.low_v_ms + 1 } else { 0 };
        if self.low_v_ms >= VBAT_HOLD_MS && self.mode != Mode::Reflex(Reflex::Crouch) {
            self.stats.battery_crouches += 1;
            self.grant = None;
            self.mode = Mode::Reflex(Reflex::Crouch);
            self.reflex_t0 = tick;
        }
        if self.mode == Mode::Session {
            let expired = self.grant.map(|g| tick >= g.until_tick).unwrap_or(true);
            let stale = self.cmd.map(|c| tick.wrapping_sub(c.tick) > SEAT_TIMEOUT_MS).unwrap_or(true);
            if expired {
                self.grant = None;
                self.enter_hold(tick);
            } else if stale {
                self.stats.link_losses += 1;
                self.enter_hold(tick);
                self.reflex_t0 = tick;
            }
        }
        if self.mode == Mode::Idle {
            self.enter_hold(tick);
        }

        // 3. what to command this tick
        let env = self.grant.map(|g| g.env).unwrap_or_else(Envelope::physical);
        self.sync_current_limits(&env, down);
        let (kp_r, kd_r) = reflex_gains();
        let (q, kp, kd, tau) = match self.mode {
            Mode::Session => {
                let c = self.cmd.unwrap();
                (c.q, c.kp, c.kd, c.tau)
            }
            Mode::Reflex(Reflex::GetUp) => {
                if tick.wrapping_sub(self.reflex_t0) % POLICY_EVERY == 0 {
                    self.run_getup(seen);
                }
                let upright = tilt < 15.0 && rate < 0.6;
                self.upright_ms = if upright { self.upright_ms + 1 } else { 0 };
                if self.upright_ms >= UPRIGHT_HOLD_MS {
                    self.stats.getups_ok += 1;
                    self.mode = Mode::Reflex(Reflex::Hold);
                } else if tick.wrapping_sub(self.reflex_t0) > GETUP_TIMEOUT_MS {
                    self.stats.getups_failed += 1;
                    self.mode = Mode::Reflex(Reflex::Crouch);
                    self.reflex_t0 = tick;
                }
                (self.targets, kp_r, kd_r, [0.0; N_ACT])
            }
            Mode::Reflex(Reflex::Brace) => {
                // a soft tuck while the body finishes falling; the learned
                // reflexes were trained from bodies that had come to rest
                let el = tick.wrapping_sub(self.reflex_t0);
                self.still_ms = if rate < 0.5 { self.still_ms + 1 } else { 0 };
                if self.still_ms >= BRACE_STILL_MS || el >= BRACE_MAX_MS {
                    // the trainers' resting_class, from the gravity vector:
                    // chest z-axis up > 0.7 belly, < -0.7 back, else the roll's sign
                    let g = self.att.gravity_body();
                    let up = -g[2];
                    if up > 0.7 {
                        self.enter(Reflex::StandUp, tick);
                    } else {
                        self.righting_ref = if up < -0.7 {
                            crate::refs_gen::RIGHTING_BACK
                        } else if libm::atan2f(-g[1], -g[2]) > 0.0 {
                            crate::refs_gen::RIGHTING_RIGHT
                        } else {
                            crate::refs_gen::RIGHTING_LEFT
                        };
                        self.enter(Reflex::Righting, tick);
                    }
                }
                let mut kp = kp_r;
                let mut kd = kd_r;
                for j in 0..N_ACT {
                    kp[j] *= 0.5;
                    kd[j] *= 0.6;
                }
                self.targets = sternal_pose();
                (self.targets, kp, kd, [0.0; N_ACT])
            }
            Mode::Reflex(Reflex::Righting) => {
                let el = tick.wrapping_sub(self.reflex_t0);
                if el % POLICY_EVERY == 0 {
                    let c = [(el as f32 / 1000.0 / COMMAND_T_S).min(1.0)];
                    let r = reference_at(self.righting_ref, el as f32 / 1000.0);
                    self.run_policy(Which::Righting, seen, &r, &c);
                }
                let ok = tilt < 25.8 && rate < 0.5 && near(seen, &sternal_pose(), 0.5);
                self.pred_ms = if ok { self.pred_ms + 1 } else { 0 };
                if self.pred_ms >= STERNAL_HOLD_MS {
                    self.enter(Reflex::StandUp, tick);
                } else if el > SKILL_TIMEOUT_MS {
                    self.retry(tick);
                }
                (self.targets, kp_r, kd_r, [0.0; N_ACT])
            }
            Mode::Reflex(Reflex::StandUp) => {
                let el = tick.wrapping_sub(self.reflex_t0);
                if el % POLICY_EVERY == 0 {
                    let c = [(el as f32 / 1000.0 / COMMAND_T_S).min(1.0)];
                    let r = reference_at(crate::refs_gen::STANDUP, el as f32 / 1000.0);
                    self.run_policy(Which::StandUp, seen, &r, &c);
                }
                let ok = tilt < 19.9 && rate < 0.5 && near(seen, &stand_pose(), 0.35);
                self.pred_ms = if ok { self.pred_ms + 1 } else { 0 };
                if self.pred_ms >= STAND_HOLD_MS {
                    self.stats.getups_ok += 1;
                    self.enter_hold(tick);
                } else if el > SKILL_TIMEOUT_MS {
                    self.retry(tick);
                }
                (self.targets, kp_r, kd_r, [0.0; N_ACT])
            }
            Mode::Reflex(Reflex::Hold) => {
                let el = tick.wrapping_sub(self.hold_t0);
                let ready = ready_pose();
                if self.balance.is_none() {
                    self.targets = stand_pose();
                } else if el < STANCE_BLEND_MS {
                    let s = min_jerk(el as f32 / STANCE_BLEND_MS as f32);
                    for j in 0..N_ACT {
                        self.targets[j] = self.hold_from[j] + s * (ready[j] - self.hold_from[j]);
                    }
                } else if el % POLICY_EVERY == 0 {
                    self.run_policy(Which::Balance, seen, &ready, &[0.0]);
                }
                (self.targets, kp_r, kd_r, [0.0; N_ACT])
            }
            Mode::Reflex(Reflex::Crouch) => {
                // fold toward lying with half the stiffness, then go soft
                let soft = tick.wrapping_sub(self.reflex_t0) > 3000;
                let mut kp = kp_r;
                let mut kd = kd_r;
                for j in 0..N_ACT {
                    kp[j] *= if soft { 0.15 } else { 0.5 };
                    kd[j] *= if soft { 0.3 } else { 0.6 };
                }
                (crouch_pose(), kp, kd, [0.0; N_ACT])
            }
            _ => (stand_pose(), kp_r, kd_r, [0.0; N_ACT]),
        };
        self.targets = q;
        if tick % SP_EVERY == 0 {
            self.send(down, &env, &q, &kp, &kd, &tau);
        }
    }

    /// A timed-out recovery skill braces and starts over; after
    /// RECOVERY_TRIES it gives up and crouches (the entity decides what next).
    fn retry(&mut self, tick: u32) {
        self.tries += 1;
        if self.tries >= RECOVERY_TRIES {
            self.stats.getups_failed += 1;
            self.enter(Reflex::Crouch, tick);
        } else {
            self.enter(Reflex::Brace, tick);
        }
    }

    fn run_policy(&mut self, which: Which, seen: &[JointSeen; N_ACT], nominal: &[f32; N_ACT], command: &[f32]) {
        let p = match which {
            Which::Righting => self.righting.as_ref(),
            Which::StandUp => self.standup.as_ref(),
            Which::Balance => self.balance.as_ref(),
        };
        let Some(p) = p else { return };
        let mut obs = [0.0f32; 128];
        let n = build_obs(&self.att, seen, nominal, &self.prev_action, command, &mut obs);
        let mut act = [0.0f32; 32];
        if n != p.obs_dim || !p.act(&obs[..n], &mut act[..p.act_dim]) {
            return;
        }
        // residual around the nominal (the skill's reference at its clock, or
        // the ready stance), through the command filter
        let new = targets_from_action(nominal, &act[..p.act_dim], ActionMode::Offset);
        let alpha = match which {
            Which::Balance => FILTER_BALANCE,
            _ => FILTER_GETUP,
        };
        for j in 0..N_ACT {
            self.targets[j] += alpha * (new[j] - self.targets[j]);
        }
        self.prev_action[..p.act_dim].copy_from_slice(&act[..p.act_dim]);
    }

    fn run_getup(&mut self, seen: &[JointSeen; N_ACT]) {
        let Some(p) = self.getup.as_ref() else { return };
        let mut obs = [0.0f32; 128];
        let n = build_obs(&self.att, seen, &stand_pose(), &self.prev_action, &[], &mut obs);
        let mut act = [0.0f32; 32];
        if n != p.obs_dim || !p.act(&obs[..n], &mut act[..p.act_dim]) {
            return;
        }
        self.targets = targets_from_action(&stand_pose(), &act[..p.act_dim], ActionMode::Range);
        self.prev_action[..p.act_dim].copy_from_slice(&act[..p.act_dim]);
    }

    /// Push the envelope's current limits to the nodes when they change.
    fn sync_current_limits(&mut self, env: &Envelope, down: &mut NerveDown) {
        for j in 0..N_ACT {
            if (self.pushed_i[j] - env.i_max[j]).abs() > 1e-3 {
                let a = &ACTUATORS[j];
                let seq = self.next_seq();
                let f = Config { slot: a.slot, key: ConfigKey::CurrentLimit, value: env.i_max[j], seq }.encode(a.bus, a.node, false);
                self.push(down, f);
                self.pushed_i[j] = env.i_max[j];
            }
        }
    }

    /// The first of the two checks: clamp into the envelope before the bus.
    fn send(&mut self, down: &mut NerveDown, env: &Envelope, q: &[f32; N_ACT], kp: &[f32; N_ACT], kd: &[f32; N_ACT], tau: &[f32; N_ACT]) {
        for (j, a) in ACTUATORS.iter().enumerate() {
            if a.encoder == EncoderSide::None {
                continue;
            }
            let mut c = 0;
            let qc = clamp_count(q[j], env.q_min[j], env.q_max[j], &mut c);
            let kpc = clamp_count(kp[j], 0.0, env.kp_max[j], &mut c);
            let kdc = clamp_count(kd[j], 0.0, 0.2 * env.kp_max[j], &mut c);
            let tmax = env.i_max[j] * if a.kt_out.is_finite() { a.kt_out } else { 0.0 };
            let tc = clamp_count(tau[j], -tmax, tmax, &mut c);
            self.stats.clamped += c;
            self.stats.setpoints += 1;
            let f = Setpoint { slot: a.slot, flags: SP_ENABLE, q: qc, tau_ff: tc, kp: kpc, kd: kdc }.encode(a.bus, a.node, a.motor);
            self.push(down, f);
        }
    }
}

fn clamp_count(v: f32, lo: f32, hi: f32, c: &mut u32) -> f32 {
    if !v.is_finite() {
        *c += 1;
        return lo.max(0.0).min(hi);
    }
    if v < lo {
        *c += 1;
        lo
    } else if v > hi {
        *c += 1;
        hi
    } else {
        v
    }
}

#[derive(Clone, Copy)]
enum Which {
    Righting,
    StandUp,
    Balance,
}

fn min_jerk(s: f32) -> f32 {
    let s = s.clamp(0.0, 1.0);
    s * s * s * (10.0 - 15.0 * s + 6.0 * s * s)
}

/// Every policy joint within `tol` of `pose` and slower than 1.5 rad/s
/// (the trainers' predicates, from what the encoders report).
fn near(seen: &[JointSeen; N_ACT], pose: &[f32; N_ACT], tol: f32) -> bool {
    policy_joints().all(|j| match seen[j].fb {
        Some(f) => (f.q - pose[j]).abs() < tol && f.dq.abs() < 1.5,
        None => false,
    })
}

/// Belly down with the legs tucked under: the hand-off pose between the
/// righting and stand-up reflexes (`blur/learn/ref/keyframes.py` STERNAL:
/// the rig's `lay` legs, both sides alike, spine straight).
pub fn sternal_pose() -> [f32; N_ACT] {
    let mut q = stand_pose();
    for (j, a) in ACTUATORS.iter().enumerate() {
        let v: Option<f32> = match (a.name.get(..1), a.name.get(2..)) {
            (Some("F"), Some("_abd")) | (Some("R"), Some("_abd")) => Some(0.0),
            (Some("F"), Some("_pitch")) => Some(0.72),
            (Some("F"), Some("_knee")) => Some(-2.34),
            (Some("F"), Some("_dist")) => Some(0.15),
            (Some("R"), Some("_pitch")) => Some(-1.00),
            (Some("R"), Some("_knee")) => Some(2.06),
            (Some("R"), Some("_dist")) => Some(-2.97),
            _ => None,
        };
        if a.name.starts_with("spine") {
            q[j] = 0.0;
        } else if let Some(v) = v {
            q[j] = v.clamp(a.q_min, a.q_max);
        }
    }
    q
}

/// The balance reflex's ready stance (`blur/learn/ref/stances.py`
/// 'wide+low'): hips 0.15 rad (front) and 0.30 rad (rear) outward, every
/// knee folded 15 % of the way toward short, its hip pitch taking half the
/// fold back.
pub fn ready_pose() -> [f32; N_ACT] {
    let mut q = stand_pose();
    for leg in ["FR", "FL", "RR", "RL"] {
        let idx = |s: &str| ACTUATORS.iter().position(|a| a.name.len() == leg.len() + s.len() && a.name.starts_with(leg) && a.name.ends_with(s));
        let (Some(abd), Some(pitch), Some(knee)) = (idx("_abd"), idx("_pitch"), idx("_knee")) else { continue };
        let out = if leg.starts_with('F') { 0.15 } else { 0.30 };
        q[abd] = if leg.ends_with('R') { out } else { -out };
        let k = &ACTUATORS[knee];
        let folded = if k.q_stand > 0.0 { k.q_max } else { k.q_min };
        let dk = 0.15 * (folded - k.q_stand);
        q[knee] = k.q_stand + dk;
        q[pitch] -= 0.5 * dk;
    }
    for (j, a) in ACTUATORS.iter().enumerate() {
        q[j] = q[j].clamp(a.q_min, a.q_max);
    }
    q
}

/// Folded down: the rig's `lay` pose for the legs, spine straight.
pub fn crouch_pose() -> [f32; N_ACT] {
    let mut q = stand_pose();
    for (j, a) in ACTUATORS.iter().enumerate() {
        let n = a.name;
        if n.ends_with("_knee") {
            // fold each knee most of the way toward the end that shortens the leg
            let folded = if a.q_stand > 0.0 { a.q_max } else { a.q_min };
            q[j] = a.q_stand + 0.7 * (folded - a.q_stand);
        }
    }
    q
}

/// The observation every BLUR policy sees, in the layout of
/// `blur/learn/env.py` `observe()`: gravity (3), 0.25 × rate (3),
/// q − nominal over the policy joints, 0.05 × dq over them, the previous
/// action, the command. Returns the length written.
pub fn build_obs(att: &Mahony, seen: &[JointSeen; N_ACT], nominal: &[f32; N_ACT], prev: &[f32], command: &[f32], out: &mut [f32]) -> usize {
    let g = att.gravity_body();
    let mut p = 0;
    for k in 0..3 {
        out[p] = g[k];
        p += 1;
    }
    let w = att.last_rate();
    for k in 0..3 {
        out[p] = 0.25 * w[k];
        p += 1;
    }
    let pj: [usize; 19] = {
        let mut a = [0usize; 19];
        for (k, j) in policy_joints().enumerate().take(19) {
            a[k] = j;
        }
        a
    };
    for &j in pj.iter() {
        out[p] = seen[j].fb.map(|f| f.q).unwrap_or(nominal[j]) - nominal[j];
        p += 1;
    }
    for &j in pj.iter() {
        out[p] = 0.05 * seen[j].fb.map(|f| f.dq).unwrap_or(0.0);
        p += 1;
    }
    for k in 0..19 {
        out[p] = prev.get(k).copied().unwrap_or(0.0);
        p += 1;
    }
    for &c in command {
        out[p] = c;
        p += 1;
    }
    p
}
