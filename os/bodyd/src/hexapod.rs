//! BFS  LEVEL    Domain 2 Autonomic - the runtime on the body's own computer  [crate brainos-bodyd]
//! BFS  PURPOSE  The Mini-Blur hexapod as a Mechanism: refuses any motion that would tip it over, pass a servo limit or overdraw the power rail, via miniblur-core
//! BFS  DEFINES  TOP_SPEED_MPS, MARGIN_FLOOR_MM, MAX_MOTION, MIN_GAIT_SPEED, HexapodConfig, Hexapod
//! BFS  USES     this crate: mechanism | brainos: abi, miniblur-core, rt
//! BFS  USED BY  bodyd/lib, bodyd/tests/bodyd_suite, host/bench, host/fullstack, host/tests/fullstack_suite, proof/envelopes
//!
//! Mini-Blur as a `Mechanism`.
//!
//! This is the Phase 0 body, and every number in it comes from
//! `miniblur-core` — the same crate the ESP32-S3 firmware links. The
//! geometry, the tripod gait, the convex-hull support polygon, the
//! servo table with its linked pairs, and the rail current budget are
//! not reimplemented here; they are called.
//!
//! **What the body checks that the seat also checked.** Three things,
//! all of them before a single servo moves:
//!
//! 1. **Tip-over.** `plan_min_margin_mm` forward-models every pose in
//!    the whole motion and reports the worst distance from the centre of
//!    mass to the edge of the support polygon. Below the margin, the
//!    motion is refused. This is the same function KIRA's simulate stage
//!    runs at the seat.
//! 2. **Servo envelope.** `ServoTable::targets` maps each pose to
//!    absolute servo degrees and refuses a pose that would drive a
//!    channel past its calibrated limit, or tear a linked pair.
//! 3. **Rail current.** `plan_peak_amps` finds the sample with the most
//!    servos moving at once. A gait that browns out the rail is refused,
//!    because a brown-out mid-write is exactly the failure the two-slot
//!    journal exists for and it is better not to have it.
//!
//! **The speed mapping is a scaling, and is labelled as one.** The gait
//! has a cycle time, not a ground speed; turning metres per second into
//! a cycle scalar needs the stride length, and the stride length needs
//! the printed frame measured. Until the wiring doc has that, `speed` is
//! a fraction of the fastest cycle the body will run, and this comment
//! is where that is admitted rather than buried in a constant.

use crate::mechanism::{Mechanism, Motion, Refusal, Sample};
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::time::Duration;
use brainos_abi::verb::Verb;
use brainos_rt::envelope::{Channel, Envelope, Limit};
use miniblur_core::gait::{Dir, GaitParams, Plan, Walk};
use miniblur_core::geometry::LegGeom;
use miniblur_core::power::PowerLimits;
use miniblur_core::servo::ServoTable;
use miniblur_core::stability;

/// The fastest cycle the body will run, expressed as the ground speed
/// the seat's schema advertises. A scaling, not a kinematic model: see
/// the module note.
pub const TOP_SPEED_MPS: f32 = 0.12;

/// How much margin the body insists on, in millimetres. `miniblur-core`
/// measures a default Mini-Blur's tripod gait at about 8 mm worst case,
/// so 4 mm is half of what the body actually achieves — tight enough to
/// catch a bad plan, loose enough not to refuse a good one on a body
/// whose centre of mass has shifted a little.
pub const MARGIN_FLOOR_MM: f32 = 4.0;

/// The largest motion the body will accept in one go.
///
/// Not about link loss — the link watchdog already stops the body after
/// 200 ms of silence, whatever it was doing. This is about the *forward
/// model*: the tip-over margin was computed once, from the geometry as
/// it was at planning time, and a body still playing that plan twelve
/// seconds later is acting on a prediction that has had twelve seconds
/// to stop being true.
///
/// Four seconds is a little over one gait cycle at the slowest speed the
/// body will walk, which is what makes `MIN_GAIT_SPEED` and this number
/// a pair rather than two independent constants.
pub const MAX_MOTION: Duration = Duration::from_millis(4_000);

/// The slowest cycle the tripod gait will run, as a fraction of the
/// fastest.
///
/// There is a floor because a gait is not a speed dial. Below about a
/// third of nominal the swing phase is slow enough that the body is
/// shuffling rather than walking: the servos spend most of each sample
/// holding against gravity, the rail current goes up rather than down,
/// and the support polygon spends longer near its worst margin. A robot
/// creeping is less stable than a robot walking, which is unintuitive
/// enough to be worth a constant with a comment rather than a clamp
/// buried in an expression.
///
/// Below this the body refuses rather than silently walking faster than
/// it was asked to. Quietly clamping would be the body lying about what
/// it did.
pub const MIN_GAIT_SPEED: f32 = 0.35;

#[derive(Clone, Copy, Debug)]
pub struct HexapodConfig {
    pub geom: LegGeom,
    pub gait: GaitParams,
    pub power: PowerLimits,
    pub margin_floor_mm: f32,
    /// measured rail voltage; below the power model's floor the body
    /// sits down rather than walking
    pub vbat: f32,
}

impl Default for HexapodConfig {
    fn default() -> HexapodConfig {
        HexapodConfig {
            geom: LegGeom::default(),
            gait: GaitParams::default(),
            power: PowerLimits::default(),
            margin_floor_mm: MARGIN_FLOOR_MM,
            vbat: 7.4,
        }
    }
}

/// The Phase 0 body.
pub struct Hexapod {
    pub config: HexapodConfig,
    /// the servo table the *entity* pushed down at §8 step 5. `None`
    /// means uncalibrated, and an uncalibrated hexapod refuses to walk —
    /// its numbers would be guesses.
    table: Option<ServoTable>,
    /// the last sample applied, for the margin the runtime reports
    last_margin: f32,
}

impl Hexapod {
    pub fn new(config: HexapodConfig) -> Hexapod {
        Hexapod {
            config,
            table: None,
            last_margin: f32::INFINITY,
        }
    }

    /// §8 step 5: the entity hands down what it remembers about this
    /// body. The body holds it only while powered.
    pub fn calibrate(&mut self, table: ServoTable) {
        self.table = Some(table);
    }

    pub fn is_calibrated(&self) -> bool {
        self.table.is_some()
    }

    /// Forget it. Called at power-off, and by the tests that check the
    /// body does not quietly keep the entity's memory.
    pub fn forget_calibration(&mut self) {
        self.table = None;
    }

    pub fn last_margin_mm(&self) -> f32 {
        self.last_margin
    }

    fn arg(args: &[(&str, f32)], name: &str) -> Option<f32> {
        args.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
    }

    /// Build a motion from a gait plan, checking everything before
    /// returning it.
    fn motion_from(&self, verb: Verb, plan: Plan, what: &str) -> Result<Motion, Refusal> {
        let Some(table) = self.table.as_ref() else {
            return Err(Refusal::Uncalibrated);
        };

        if plan.total_ms() as u64 > MAX_MOTION.as_millis() {
            return Err(Refusal::TooLong);
        }

        // --- 1. tip-over, over the whole motion ---
        let worst = stability::plan_min_margin_mm(&self.config.geom, &plan);
        if worst < self.config.margin_floor_mm {
            return Err(Refusal::WouldBreakInvariant);
        }

        // --- 2. the servo envelope, pose by pose ---
        let mut samples: Vec<Sample> = Vec::with_capacity(plan.samples());
        let hold = Duration::from_millis(plan.sample_ms() as u64);
        for (i, pose) in plan.iter().enumerate() {
            let targets = table.targets(&pose).map_err(|_| Refusal::OutsideEnvelope)?;
            for (ch, deg) in targets.iter().take(table.len) {
                let cal = table
                    .cal
                    .iter()
                    .take(table.len)
                    .find(|c| c.ch == *ch)
                    .ok_or(Refusal::OutsideEnvelope)?;
                if !cal.in_envelope(*deg) {
                    return Err(Refusal::OutsideEnvelope);
                }
            }
            let margin = stability::pose_margin_mm(&self.config.geom, &pose);
            // the runtime commands in *body* units, not servo units: the
            // envelope the seat granted is about the joint, and the servo
            // mapping is the body's own business
            let max_lift = pose.lift.iter().fold(0.0f32, |a, b| a.max(*b));
            let max_swing = pose
                .swing
                .iter()
                .fold(0.0f32, |a, b| a.max(b.abs()));
            samples.push(
                Sample {
                    index: i,
                    hold,
                    commands: [None; 8],
                    margin,
                }
                .with(Channel::Angle, max_swing.max(max_lift)),
            );
        }

        // --- 3. the rail ---
        if self.config.vbat < self.config.power.vbat_floor {
            return Err(Refusal::PowerFloor);
        }
        let (peak, _moving) = self
            .config
            .power
            .plan_peak_amps(table, &plan)
            .map_err(|_| Refusal::OutsideEnvelope)?;
        if peak > self.config.power.rail_amps {
            return Err(Refusal::PowerFloor);
        }
        for s in samples.iter_mut() {
            *s = s.with(Channel::Current, peak);
        }

        Ok(Motion {
            verb,
            duration: Duration::from_millis(plan.total_ms() as u64),
            worst_margin: worst,
            summary: alloc::format!(
                "{what}: {} samples, worst margin {:.1} mm, peak {:.2} A",
                plan.samples(),
                worst,
                peak
            ),
            samples,
        })
    }

    /// A still pose held for a moment — stand, sit.
    fn hold(&self, verb: Verb, pose: miniblur_core::Pose, what: &str) -> Result<Motion, Refusal> {
        let Some(table) = self.table.as_ref() else {
            return Err(Refusal::Uncalibrated);
        };
        let targets = table.targets(&pose).map_err(|_| Refusal::OutsideEnvelope)?;
        for (ch, deg) in targets.iter().take(table.len) {
            let cal = table
                .cal
                .iter()
                .take(table.len)
                .find(|c| c.ch == *ch)
                .ok_or(Refusal::OutsideEnvelope)?;
            if !cal.in_envelope(*deg) {
                return Err(Refusal::OutsideEnvelope);
            }
        }
        let margin = stability::pose_margin_mm(&self.config.geom, &pose);
        if margin < self.config.margin_floor_mm {
            return Err(Refusal::WouldBreakInvariant);
        }
        let max_angle = pose
            .lift
            .iter()
            .chain(pose.swing.iter())
            .fold(0.0f32, |a, b| a.max(b.abs()));
        Ok(Motion {
            verb,
            samples: alloc::vec![Sample {
                index: 0,
                hold: Duration::from_millis(250),
                commands: [None; 8],
                margin,
            }
            .with(Channel::Angle, max_angle)
            .with(Channel::Current, self.config.power.servo_hold_amps * 10.0)],
            duration: Duration::from_millis(250),
            worst_margin: margin,
            summary: alloc::format!("{what}: margin {:.1} mm", margin),
        })
    }
}

impl Mechanism for Hexapod {
    fn name(&self) -> &'static str {
        "mini-blur"
    }

    fn channels(&self) -> &[Channel] {
        &[Channel::Angle, Channel::Current]
    }

    fn plan(&self, verb: &Verb, args: &[(&str, f32)]) -> Result<Motion, Refusal> {
        match verb.as_str() {
            "legs.walk" => {
                let speed_mps = Hexapod::arg(args, "speed").ok_or(Refusal::Malformed)?;
                let heading = Hexapod::arg(args, "heading").unwrap_or(0.0);
                if !speed_mps.is_finite() || !heading.is_finite() {
                    return Err(Refusal::Malformed);
                }
                if speed_mps < 0.0 || speed_mps > TOP_SPEED_MPS {
                    return Err(Refusal::OutsideEnvelope);
                }
                // a heading well off centre is a turn, not a walk
                let dir = if heading > 15.0 {
                    Dir::Right
                } else if heading < -15.0 {
                    Dir::Left
                } else {
                    Dir::Fwd
                };
                let gait_speed = speed_mps / TOP_SPEED_MPS;
                if gait_speed < MIN_GAIT_SPEED {
                    // refusing rather than clamping: a body that quietly
                    // walked faster than it was asked to is a body whose
                    // reports cannot be believed
                    return Err(Refusal::OutsideEnvelope);
                }
                let gait_speed = gait_speed.min(1.0);
                let steps = Hexapod::arg(args, "steps").unwrap_or(1.0);
                if !(1.0..=4.0).contains(&steps) {
                    return Err(Refusal::Malformed);
                }
                let plan = Plan::new(
                    self.config.gait,
                    Walk {
                        dir,
                        steps: steps as u16,
                        speed: gait_speed,
                    },
                );
                self.motion_from(*verb, plan, "walk")
            }
            "legs.turn" => {
                let rate = Hexapod::arg(args, "rate").ok_or(Refusal::Malformed)?;
                if !rate.is_finite() || rate.abs() > 40.0 {
                    return Err(Refusal::OutsideEnvelope);
                }
                if rate == 0.0 {
                    return Err(Refusal::Malformed);
                }
                let turn_speed = rate.abs() / 40.0;
                if turn_speed < MIN_GAIT_SPEED {
                    return Err(Refusal::OutsideEnvelope);
                }
                let plan = Plan::new(
                    self.config.gait,
                    Walk {
                        dir: if rate > 0.0 { Dir::Right } else { Dir::Left },
                        steps: 1,
                        speed: turn_speed.min(1.0),
                    },
                );
                self.motion_from(*verb, plan, "turn")
            }
            "legs.stand" => {
                let height = Hexapod::arg(args, "height").unwrap_or(0.0);
                if !height.is_finite() || height.abs() > 20.0 {
                    return Err(Refusal::OutsideEnvelope);
                }
                let mut pose = miniblur_core::Pose::STAND;
                for l in pose.lift.iter_mut() {
                    *l = height.max(0.0);
                }
                self.hold(*verb, pose, "stand")
            }
            "legs.sit" => {
                let height = Hexapod::arg(args, "height").unwrap_or(-20.0);
                if !height.is_finite() || !(-35.0..=0.0).contains(&height) {
                    return Err(Refusal::OutsideEnvelope);
                }
                // sitting puts the body on the ground; with every foot at
                // the same place there is no support polygon to speak of,
                // so it is checked as a pose the body is *allowed* to be
                // in rather than one it must be stable in
                let Some(table) = self.table.as_ref() else {
                    return Err(Refusal::Uncalibrated);
                };
                let pose = miniblur_core::Pose::sit(-height);
                table.targets(&pose).map_err(|_| Refusal::OutsideEnvelope)?;
                Ok(Motion {
                    verb: *verb,
                    samples: alloc::vec![Sample::hold_for(0, Duration::from_millis(400))
                        .with(Channel::Angle, height.abs())],
                    duration: Duration::from_millis(400),
                    worst_margin: f32::INFINITY,
                    summary: String::from("sit: body on the ground"),
                })
            }
            _ => Err(Refusal::UnknownVerb),
        }
    }

    fn invariant(&self, sample: &Sample) -> Result<(), Refusal> {
        if sample.margin.is_finite() && sample.margin < self.config.margin_floor_mm {
            return Err(Refusal::WouldBreakInvariant);
        }
        Ok(())
    }

    fn safe(&self) -> Sample {
        // A legged body's safe pose is standing, holding torque.
        // Releasing torque means falling, and a hexapod that fell while
        // stopping safely has not stopped safely.
        Sample::hold_for(0, Duration::from_millis(500))
            .with(Channel::Angle, 0.0)
            .with(Channel::Current, self.config.power.servo_hold_amps * 10.0)
    }

    fn holds_itself_up(&self) -> bool {
        true
    }

    fn native_envelope(&self, id: u16) -> Envelope {
        let mut e = Envelope::new(
            id,
            Duration::from_millis(20),
            Duration::from_millis(200),
        );
        // the joint envelope the gait can possibly command, from the gait
        // parameters themselves rather than from a number somebody typed
        let reach = self
            .config
            .gait
            .swing_amp_deg
            .max(self.config.gait.lift_deg);
        e.bound(Channel::Angle, Limit::new(-reach, reach));
        e.bound(
            Channel::Current,
            Limit::new(0.0, self.config.power.rail_amps),
        );
        e.bound(Channel::Tilt, Limit::new(-20.0, 20.0));
        e.bound(
            Channel::Voltage,
            Limit::new(self.config.power.vbat_floor, 8.4),
        );
        e
    }
}
