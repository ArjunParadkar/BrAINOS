//! BFS  LEVEL    Domain 2 Autonomic - the runtime on the body's own computer  [crate brainos-bodyd]
//! BFS  PURPOSE  The desk arm's tick: plays a primitive's goals as one path, solves the joints, lets the reflex arcs answer what is felt, has the runner check the tick against the mandate, and only then computes efforts
//! BFS  DEFINES  FREE_FINGER_N, DeskArm, Sense, Drive, Event, GAINS, ArmBody, HOLD, HOLD_MAX_MS, part_schema
//! BFS  USES     this crate: deskarm | brainos: abi, body, motor, part, rt | external: libm
//! BFS  USED BY  host/motor_mind, host/motor_seat, host/tests/motor_mandate
//!
//! The desk arm's loop: one tick, a thousand times a second.
//!
//! Order matters and is the whole design:
//!
//! 1. the reflex arcs read what is felt
//! 2. the path says where the tool should be now
//! 3. the arm's own kinematics turn that into joint targets
//! 4. **the runner checks the tick against the mandate**
//! 5. only a tick that passed becomes efforts
//!
//! Nothing after step 4 can make a tick larger. With no mandate there is
//! no step 5: the arm floats, holding only its own weight, and yields to
//! anything that pushes it.
//!
//! Two runs can be alive: a **task** (one primitive, short) and a
//! **standing** run (hold a posture, long). The standing run is what
//! keeps the arm where it is between tasks, and it needs its own grant
//! like anything else.

use crate::deskarm::{DeskArmConfig, Pose, Tool, DOF, ELBOW, GRIP, ROLL, SHOULDER, WRIST};
use alloc::vec::Vec;
use brainos_abi::ids::BodyId;
use brainos_abi::time::Instant;
use brainos_motor::control::{impedance, Gain, Goal, Played, Player, Style, Touch, Want, DIMS};
use brainos_motor::mandate::{Ceiling, RunMandate, Workspace};
use brainos_motor::model::JointKind;
use brainos_motor::primitive::Contact;
use brainos_motor::reflex::{Arcs, Feel};
use brainos_motor::run::{Breach, Felt, Kinematics, Proposal, Runner, Summary};
use libm::cosf;

const DEG: f32 = core::f32::consts::PI / 180.0;
const G: f32 = 9.81;

/// The force the fingers may use just to move when they hold nothing.
pub const FREE_FINGER_N: f32 = 3.0;

/// The arm as the runner sees it.
#[derive(Clone, Copy, Debug)]
pub struct DeskArm {
    pub config: DeskArmConfig,
    pub id: BodyId,
}

impl DeskArm {
    /// The most this arm will accept in a mandate.
    pub fn ceiling(&self) -> Ceiling {
        let c = &self.config;
        let reach = c.upper_m + c.fore_m + c.tool_m();
        Ceiling {
            max_duration_ms: 60_000,
            max_force_n: c.grip_force_n,
            max_rate_dps: c.rate_dps,
            reach: Some(Workspace {
                min: [-reach, -reach, -0.01],
                max: [reach, reach, c.shoulder_h_m + reach],
            }),
            max_tilt_deg: None,
        }
    }

    /// What the arm must push with at each pitch joint to hold itself
    /// (and `payload_kg` at the tool) still. From the arm's own masses.
    pub fn holding_torque(&self, q: &Pose, payload_kg: f32) -> [f32; DOF] {
        let c = &self.config;
        let a1 = q[SHOULDER] * DEG;
        let a12 = a1 + q[ELBOW] * DEG;
        let a123 = a12 + q[WRIST] * DEG;
        // horizontal distance of each joint and each mass from the shoulder
        let r_elbow = c.upper_m * cosf(a1);
        let r_wrist = r_elbow + c.fore_m * cosf(a12);
        let along = |d: f32| r_wrist + d * cosf(a123);
        let masses = [
            (c.link_mass_kg, 0.5 * r_elbow),
            (c.link_mass_kg, r_elbow + 0.5 * c.fore_m * cosf(a12)),
            (c.link_mass_kg * 0.5, along(0.5 * c.wrist_m)),
            (c.hand_mass_kg, along(c.wrist_m + 0.5 * c.palm_m)),
            (0.03, along(c.wrist_m + c.palm_m + 0.5 * c.finger_m)),
            (payload_kg, along(c.tool_m())),
        ];
        let mut out = [0.0f32; DOF];
        for (joint, r_joint, first) in [(SHOULDER, 0.0, 0usize), (ELBOW, r_elbow, 1), (WRIST, r_wrist, 2)] {
            out[joint] = masses[first..]
                .iter()
                .map(|(m, r)| m * G * (r - r_joint))
                .sum();
        }
        out
    }
}

impl Kinematics for DeskArm {
    fn id(&self) -> BodyId {
        self.id
    }
    fn joints(&self) -> usize {
        DOF
    }
    fn kind(&self, joint: usize) -> JointKind {
        if joint == GRIP {
            JointKind::Slide
        } else {
            JointKind::Hinge
        }
    }
    fn limits(&self, joint: usize) -> (f32, f32) {
        if joint == GRIP {
            (0.0, self.config.grip_max_m)
        } else {
            self.config.limits[joint]
        }
    }
    fn max_rate(&self, joint: usize) -> f32 {
        if joint == GRIP {
            self.config.grip_rate_mps
        } else {
            self.config.rate_dps
        }
    }
    fn max_effort(&self, joint: usize) -> f32 {
        if joint == GRIP {
            self.config.grip_force_n
        } else {
            self.config.torque_nm
        }
    }
    fn tool(&self, q: &[f32]) -> Option<[f32; 3]> {
        let pose: Pose = q.try_into().ok()?;
        Some(self.config.tool(&pose).pos)
    }
}

/// What the arm feels in one tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sense {
    pub q: Pose,
    pub dq: Pose,
    /// force on the tool (or what it holds) from the world, newtons
    pub tool_n: f32,
    /// force between the fingers and what they hold, newtons
    pub grip_n: f32,
    /// how fast what is held slides against the fingers, m/s
    pub slip_mps: f32,
}

/// What the arm does in one tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct Drive {
    /// N·m for the five joints, N for the fingers
    pub effort: Pose,
    pub running: bool,
    pub standing: bool,
    pub goal: u16,
    pub stiff: f32,
    pub slip_fired: bool,
    pub yield_fired: bool,
    pub wait_fired: bool,
    pub squeeze_n: f32,
}

/// Something worth telling the entity about.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Event {
    /// a run ended, for whatever reason is in the summary
    Ended(Summary),
    /// the path asked for somewhere the arm cannot reach
    Unreachable(Summary),
    /// the fingers closed on nothing
    NothingGripped(Summary),
    /// a queued run was joined onto the one before it
    Joined,
    /// the standing run ended
    StandingEnded(Summary),
}

/// Full-stiffness gains: N·m per radian, N·m·s per radian; N/m for the
/// fingers. Plausible, like every number about this arm.
pub const GAINS: [Gain; DOF] = [
    Gain { kp: 30.0, kd: 1.2 },
    Gain { kp: 60.0, kd: 2.4 },
    Gain { kp: 40.0, kd: 1.6 },
    Gain { kp: 12.0, kd: 0.35 },
    Gain { kp: 4.0, kd: 0.08 },
    Gain { kp: 2500.0, kd: 25.0 },
];

/// The loop.
pub struct ArmBody {
    pub arm: DeskArm,
    pub style: Style,
    task: Runner,
    standing: Runner,
    player: Option<Player>,
    queued: Option<(RunMandate, Vec<Goal>)>,
    arcs: Arcs,
    stand_at: Pose,
    last_target: Pose,
    last_want: Option<Want>,
    last_at: Instant,
    /// what is assumed to be in the hand, for the feed-forward only
    pub payload_kg: f32,
    pub ticks: u64,
    pub ticks_refused: u64,
}

fn goal_of(tool: &Tool, twist: f32, open: f32) -> [f32; DIMS] {
    [
        tool.pos[0],
        tool.pos[1],
        tool.pos[2],
        tool.finger_yaw_deg,
        tool.pitch_deg,
        twist,
        open,
    ]
}

/// Make a list of goals fit this arm and this grant before it is
/// played: the fingers are symmetric, so each heading is taken on the
/// half-turn nearest the one before it; and a stroke that turns the
/// wrist is given at least the time that turn needs at 80 % of the
/// granted rate. Strokes are only ever made slower.
fn fit(from: &[f32; DIMS], mut goals: Vec<Goal>, rate_dps: f32) -> Vec<Goal> {
    let mut prev = *from;
    for g in goals.iter_mut() {
        while g.at[3] - prev[3] > 90.0 {
            g.at[3] -= 180.0;
        }
        while g.at[3] - prev[3] < -90.0 {
            g.at[3] += 180.0;
        }
        let turn = (3..6)
            .map(|k| libm::fabsf(g.at[k] - prev[k]))
            .fold(0.0f32, f32::max);
        // a minimum-jerk move peaks at 1.875 times its mean rate
        let need_ms = 1.875 * turn / (0.8 * rate_dps.max(1.0)) * 1000.0;
        if (g.ms as f32) < need_ms {
            g.ms = need_ms as u32 + 1;
        }
        prev = g.at;
    }
    goals
}

impl ArmBody {
    pub fn new(config: DeskArmConfig, id: BodyId, now: Instant) -> ArmBody {
        ArmBody {
            arm: DeskArm { config, id },
            style: Style::FLUID,
            task: Runner::idle(),
            standing: Runner::idle(),
            player: None,
            queued: None,
            arcs: Arcs::default(),
            stand_at: [0.0; DOF],
            last_target: [0.0; DOF],
            last_want: None,
            last_at: now,
            payload_kg: 0.0,
            ticks: 0,
            ticks_refused: 0,
        }
    }

    pub fn is_running(&self) -> bool {
        self.task.is_running()
    }

    pub fn is_standing(&self) -> bool {
        self.standing.is_running()
    }

    pub fn has_queued(&self) -> bool {
        self.queued.is_some()
    }

    /// The joint targets of the last tick that was applied.
    pub fn last_target(&self) -> &Pose {
        &self.last_target
    }

    pub fn arcs(&self) -> &Arcs {
        &self.arcs
    }

    /// Start a primitive, or queue it to be joined onto the one running.
    /// The mandate is consumed either way. One run can be queued; a
    /// second is refused, so nothing piles up unexamined.
    pub fn start(
        &mut self,
        mandate: RunMandate,
        goals: Vec<Goal>,
        q: &Pose,
        now: Instant,
    ) -> Result<(), Breach> {
        if mandate.part() != self.arm.id {
            return Err(Breach::WrongBody);
        }
        if self.task.is_running() {
            if self.queued.is_some() {
                return Err(Breach::Malformed);
            }
            self.queued = Some((mandate, goals));
            return Ok(());
        }
        let rate = mandate.bounds().rate_dps.min(self.arm.config.rate_dps);
        self.task.begin(mandate, &self.arm, q, now)?;
        let here = goal_of(&self.arm.config.tool(q), 0.0, q[GRIP]);
        let goals = fit(&here, goals, rate);
        self.player = Some(Player::new(here, goals, self.style));
        self.last_target = *q;
        self.last_want = None;
        self.arcs = Arcs::new(self.arcs.tuning);
        Ok(())
    }

    /// Hold this posture under a standing grant until it runs out.
    pub fn stand(&mut self, mandate: RunMandate, q: &Pose, now: Instant) -> Result<(), Breach> {
        self.standing.begin(mandate, &self.arm, q, now)?;
        self.stand_at = *q;
        if !self.task.is_running() {
            self.last_target = *q;
        }
        Ok(())
    }

    /// Give up whatever is running. The arm floats.
    pub fn release(&mut self, now: Instant) -> Option<Summary> {
        self.player = None;
        self.queued = None;
        self.task.finish(now)
    }

    fn solve(&self, at: &[f32; DIMS]) -> Option<Pose> {
        let mut q = self.arm.config.solve(
            &Tool {
                pos: [at[0], at[1], at[2]],
                pitch_deg: at[4],
                finger_yaw_deg: at[3],
            },
            at[6].clamp(0.0, self.arm.config.grip_max_m),
        )?;
        // stay on the same half-turn of the roll as the last tick, then
        // add the twist the stroke asked for
        let mut roll = q[ROLL];
        while roll - self.last_target[ROLL] + at[5] > 90.0 {
            roll -= 180.0;
        }
        while roll - self.last_target[ROLL] + at[5] < -90.0 {
            roll += 180.0;
        }
        q[ROLL] = roll + at[5];
        if self.arm.config.admits(&q) {
            Some(q)
        } else {
            None
        }
    }

    fn efforts(&self, s: &Sense, target: &Pose, stiff: f32, squeeze: f32, dt_s: f32) -> Pose {
        let ff = self.arm.holding_torque(&s.q, self.payload_kg);
        let mut out = [0.0f32; DOF];
        for i in 0..5 {
            let rate = if dt_s > 0.0 {
                (target[i] - self.last_target[i]) * DEG / dt_s
            } else {
                0.0
            };
            out[i] = impedance(
                GAINS[i],
                stiff,
                target[i] * DEG,
                rate,
                s.q[i] * DEG,
                s.dq[i] * DEG,
                ff[i],
                self.arm.config.torque_nm,
            );
        }
        out[GRIP] = impedance(GAINS[GRIP], 1.0, target[GRIP], 0.0, s.q[GRIP], s.dq[GRIP], 0.0, squeeze);
        out
    }

    fn float(&self, s: &Sense) -> Pose {
        let ff = self.arm.holding_torque(&s.q, self.payload_kg);
        let mut out = [0.0f32; DOF];
        for i in 0..5 {
            out[i] = (ff[i] - GAINS[i].kd * s.dq[i] * DEG)
                .clamp(-self.arm.config.torque_nm, self.arm.config.torque_nm);
        }
        out
    }

    /// One tick.
    pub fn tick(&mut self, now: Instant, s: &Sense) -> (Drive, Option<Event>) {
        self.ticks += 1;
        let dt_ms = (now.since(self.last_at).as_micros() as f32 * 0.001).clamp(0.0, 10.0);
        self.last_at = now;
        let felt = Felt {
            force_n: Some(s.tool_n),
            tilt_deg: None,
        };
        let mut drive = Drive::default();
        let mut event = None;

        if self.task.is_running() && self.player.is_some() {
            let cap = self.task.bounds().map(|b| b.force_n).unwrap_or(0.0);
            let here = self.arm.config.tool(&s.q).pos;
            let (off_path, expected, asked) = match &self.last_want {
                Some(w) => (
                    libm::sqrtf(
                        (0..3).map(|k| (here[k] - w.at[k]) * (here[k] - w.at[k])).sum::<f32>(),
                    ),
                    w.contact == Contact::Expect,
                    w.squeeze_n,
                ),
                None => (0.0, false, 0.0),
            };
            let holding = asked > 0.0 && s.grip_n > 0.3;
            let answer = self.arcs.step(
                &Feel {
                    holding,
                    slip_mps: s.slip_mps,
                    touch_n: s.tool_n,
                    touch_expected: expected,
                    off_path_m: off_path,
                },
                asked,
                cap,
                dt_ms,
            );
            let touch = Touch {
                tool_n: s.tool_n,
                grip_n: s.grip_n,
            };
            let step_ms = if answer.hold_path { 0.0 } else { dt_ms };
            let (want, played) = match self.player.as_mut() {
                Some(p) => p.step(step_ms, &touch),
                None => return (drive, None),
            };

            // join a queued run onto this one at the seam
            let at_seam = self.player.as_ref().map(|p| p.at_the_seam()).unwrap_or(false);
            if at_seam && played != Played::NothingGripped {
                if let Some((mandate, goals)) = self.queued.take() {
                    let rate = mandate.bounds().rate_dps.min(self.arm.config.rate_dps);
                    match self.task.begin(mandate, &self.arm, &self.last_target, now) {
                        Ok(previous) => {
                            let goals = fit(&want.at, goals, rate);
                            if let Some(p) = self.player.as_mut() {
                                p.append(&goals);
                            }
                            event = previous.map(Event::Ended).or(Some(Event::Joined));
                        }
                        Err(_) => {
                            self.ticks_refused += 1;
                        }
                    }
                }
            }
            let joined = matches!(event, Some(Event::Ended(_)) | Some(Event::Joined));

            match self.solve(&want.at) {
                None => {
                    self.player = None;
                    self.queued = None;
                    event = self.task.finish(now).map(Event::Unreachable);
                    drive.effort = self.float(s);
                }
                Some(target) => {
                    let squeeze = (want.squeeze_n.max(FREE_FINGER_N) + answer.extra_squeeze_n)
                        .min(self.task.bounds().map(|b| b.force_n).unwrap_or(0.0));
                    let mut caps = [self.arm.config.torque_nm; DOF];
                    caps[GRIP] = squeeze;
                    let ok = self.task.admit(
                        &self.arm,
                        now,
                        &Proposal {
                            target: &target,
                            effort: &caps,
                        },
                        &felt,
                    );
                    match ok {
                        Ok(()) => {
                            // a standing run, if there is one, follows
                            // along so that it knows where the arm is
                            // when the task hands back to it; if the
                            // task leaves what the standing grant
                            // allows, the standing run is what ends
                            if self.standing.is_running() {
                                if let Err((_, summary)) = self.standing.admit(
                                    &self.arm,
                                    now,
                                    &Proposal {
                                        target: &target,
                                        effort: &caps,
                                    },
                                    &felt,
                                ) {
                                    event = event.or(summary.map(Event::StandingEnded));
                                }
                            }
                            let stiff = want.stiff * answer.stiff_scale;
                            drive.effort = self.efforts(s, &target, stiff, squeeze, dt_ms * 0.001);
                            drive.running = true;
                            drive.goal = want.goal as u16;
                            drive.stiff = stiff;
                            drive.squeeze_n = squeeze;
                            drive.slip_fired = answer.slip_fired;
                            drive.yield_fired = answer.yield_fired;
                            drive.wait_fired = answer.wait_fired;
                            self.last_target = target;
                            self.last_want = Some(want);
                            match played {
                                Played::Going => {}
                                Played::Done if joined => {}
                                Played::Done => {
                                    self.player = None;
                                    event = self.task.finish(now).map(Event::Ended);
                                    self.stand_at = target;
                                }
                                Played::NothingGripped => {
                                    self.player = None;
                                    self.queued = None;
                                    event = self.task.finish(now).map(Event::NothingGripped);
                                    self.stand_at = target;
                                }
                            }
                        }
                        Err((_, summary)) => {
                            self.ticks_refused += 1;
                            self.player = None;
                            self.queued = None;
                            event = summary.map(Event::Ended);
                            drive.effort = self.float(s);
                        }
                    }
                }
            }
        } else if self.standing.is_running() {
            // hold the posture, keeping whatever squeeze the last task
            // ended with so a held thing stays held
            let squeeze = self
                .last_want
                .map(|w| w.squeeze_n)
                .unwrap_or(0.0)
                .max(FREE_FINGER_N)
                .min(self.standing.bounds().map(|b| b.force_n).unwrap_or(0.0));
            let mut caps = [self.arm.config.torque_nm; DOF];
            caps[GRIP] = squeeze;
            let target = self.stand_at;
            match self.standing.admit(
                &self.arm,
                now,
                &Proposal {
                    target: &target,
                    effort: &caps,
                },
                &felt,
            ) {
                Ok(()) => {
                    drive.effort = self.efforts(s, &target, 1.0, squeeze, dt_ms * 0.001);
                    drive.standing = true;
                    drive.stiff = 1.0;
                    drive.squeeze_n = squeeze;
                    self.last_target = target;
                }
                Err((_, summary)) => {
                    self.ticks_refused += 1;
                    event = summary.map(Event::StandingEnded);
                    drive.effort = self.float(s);
                }
            }
        } else {
            drive.effort = self.float(s);
        }
        (drive, event)
    }
}

/// The verb that holds a posture under a standing grant.
pub const HOLD: &str = "arm.hold";
/// The longest a standing grant may last.
pub const HOLD_MAX_MS: u32 = 600_000;

/// What the arm says it is, in the vocabulary acquisition speaks: one
/// verb per primitive in the library, each with the bounding parameters
/// every run carries, the frame it acts in, and its own parameters with
/// the ranges it was built for.
pub fn part_schema(
    arm: &DeskArm,
    library: &[brainos_motor::primitive::MotorPrimitive],
) -> brainos_part::schema::PartSchema {
    use brainos_body::schema::ParamSpec;
    use brainos_motor::mandate::run_verb;
    use brainos_part::class::PartClass;
    use brainos_part::schema::{PartSchema, Quantity, SenseSpec, VitalKind, VitalSpec};
    use brainos_rt::envelope::Channel;

    let ceiling = arm.ceiling();
    let reach = ceiling.reach.map(|w| w.max[0] as f64).unwrap_or(1.0);
    let mut schema = PartSchema::new(PartClass::Actuator, "desk arm", "sim")
        .sense(SenseSpec::new("motor.load", Quantity::Fraction, 0.0, 1.5))
        .sense(SenseSpec::new("tool.force", Quantity::Count, 0.0, 200.0))
        .vital(VitalSpec::new(
            "motor.load",
            VitalKind::Load,
            brainos_body::vital::Band::rising(0.9, 1.2),
        ));
    for p in library {
        let mut v = run_verb(p.verb, p.kind.name(), &ceiling)
            .with(ParamSpec::scalar("fx", Channel::Clearance, -reach, reach))
            .with(ParamSpec::scalar("fy", Channel::Clearance, -reach, reach))
            .with(ParamSpec::scalar("fz", Channel::Clearance, -0.01, reach))
            .with(ParamSpec::scalar("fyaw", Channel::Angle, -360.0, 360.0));
        for q in &p.params {
            let ch = if q.name.ends_with("_deg") {
                Channel::Angle
            } else if q.name.ends_with("_n") {
                Channel::Force
            } else {
                Channel::Clearance
            };
            v = v.with(ParamSpec::scalar(&q.name, ch, q.min as f64, q.max as f64));
        }
        schema = schema.verb(v);
    }
    let hold = run_verb(
        brainos_abi::verb::Verb::literal(HOLD),
        "hold the present posture",
        &Ceiling {
            max_duration_ms: HOLD_MAX_MS,
            ..ceiling
        },
    );
    schema.verb(hold)
}
