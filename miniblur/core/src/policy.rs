//! The hexapod's invariants as KIRA sees them (Execution Plan §4.5) — and as
//! the board's reflex loop sees them. One set of functions, called from two
//! places: `validate` at stage 4, `simulate` at stage 5, `policy` at stage 6.
//! The seat wraps them in `brainos_mind::kira::BodyPolicy`; the firmware
//! calls them before it plays a plan. Neither side trusts the other to have
//! done it.
//!
//! Targets are the KIRA string form: `gait.walk` → `"<dir> <steps> <speed>"`,
//! `servo.set` → `"<ch> <deg>"`, everything else → `""`.

use crate::gait::{Plan, Walk};
use crate::power;
use crate::schema::Model;
use crate::stability;
use alloc::format;
use alloc::string::String;

/// What the body is doing right now, as far as the policy is concerned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// standing or sitting, servos holding
    Idle,
    /// a plan is being played
    Walking,
    /// `servo.set` is allowed; gaits are not
    Calibrating,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Idle => "idle",
            Mode::Walking => "walking",
            Mode::Calibrating => "calibrating",
        }
    }
}

/// The live facts the POLICY stage consults. The body reports them in every
/// telemetry digest; the seat mirrors them; the board owns them.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BodyState {
    pub mode: Mode,
    pub vbat: f32,
    /// milliseconds since the body last heard the seat (or vice versa)
    pub link_age_ms: u64,
    /// true while sitting (a walk from a sit must stand first)
    pub sitting: bool,
}

impl Default for BodyState {
    fn default() -> BodyState {
        BodyState { mode: Mode::Idle, vbat: f32::NAN, link_age_ms: 0, sitting: true }
    }
}

/// Parse `"<ch> <deg>"`.
pub fn parse_servo_target(t: &str) -> Result<(u8, f32), String> {
    let mut it = t.split_whitespace();
    let ch: u8 = it
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| String::from("servo.set wants `<channel> <degrees>`"))?;
    let deg: f32 = it
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| String::from("servo.set wants `<channel> <degrees>`"))?;
    Ok((ch, deg))
}

/// Stage 4 — is the request well-formed and inside the body's envelope?
pub fn validate(m: &Model, verb: &str, target: &str) -> Result<(), String> {
    match verb {
        "gait.walk" => {
            let w = Walk::parse_target(target)?;
            if w.steps == 0 || w.steps > m.limits.max_steps {
                return Err(format!(
                    "{} steps is outside the envelope (1..={})",
                    w.steps, m.limits.max_steps
                ));
            }
            if !(0.1..=1.0).contains(&w.speed) {
                return Err(format!("speed {:.2} is outside the envelope (0.1..=1.0)", w.speed));
            }
            // every pose of the plan must be reachable by every servo
            let plan = Plan::new(m.gait, w);
            for pose in plan.iter() {
                m.table.targets(&pose)?;
            }
            Ok(())
        }
        "servo.set" => {
            let (ch, deg) = parse_servo_target(target)?;
            m.table.check_raw(ch, deg)
        }
        "gait.stand" | "gait.sit" | "gait.stop" | "body.telemetry" | "body.calibrate" | "body.schema" => Ok(()),
        other => Err(format!("this body has no verb '{other}'")),
    }
}

/// Stage 5 — forward-model the consequence. Ok(prediction) or Err(invariant).
pub fn simulate(m: &Model, verb: &str, target: &str) -> Result<String, String> {
    match verb {
        "gait.walk" => {
            let w = Walk::parse_target(target)?;
            let plan = Plan::new(m.gait, w);
            let margin = stability::plan_min_margin_mm(&m.geom, &plan);
            if margin < m.limits.tipover_margin_mm {
                return Err(format!(
                    "forward model: the CoM comes within {margin:.1} mm of the support polygon edge (floor {:.0} mm) — that step tips",
                    m.limits.tipover_margin_mm
                ));
            }
            let (peak, n) = m.limits.power.plan_peak_amps(&m.table, &plan)?;
            if !m.limits.power.rail_ok(peak) {
                return Err(format!(
                    "forward model: {n} servos moving at once draw {peak:.2} A, rail is {:.1} A — that step browns out",
                    m.limits.power.rail_amps
                ));
            }
            Ok(format!(
                "{} step(s) {} at {:.0}% in ~{:.1} s; worst CoM margin {margin:.0} mm; peak {peak:.2} A",
                w.steps, w.dir.name(), w.speed * 100.0, plan.total_ms() as f32 / 1000.0
            ))
        }
        "gait.stand" => {
            let margin = stability::pose_margin_mm(&m.geom, &crate::Pose::STAND);
            Ok(format!("six feet down, CoM margin {margin:.0} mm"))
        }
        "gait.sit" => Ok(String::from("body lowers onto the ground; servos unload")),
        "gait.stop" => Ok(String::from("the current plan ends at the next sample; body stands")),
        "servo.set" => {
            let (ch, deg) = parse_servo_target(target)?;
            Ok(format!("channel {ch} moves to {deg:.1}°; no gait running"))
        }
        _ => Ok(String::from("no motion")),
    }
}

/// Stage 6 — the body's hard invariants right now.
pub fn policy(m: &Model, s: &BodyState, verb: &str, _target: &str) -> Result<(), String> {
    let link_ok = s.link_age_ms <= m.limits.lost_link_ms as u64;
    match verb {
        "gait.walk" | "gait.stand" => {
            if !link_ok {
                return Err(format!(
                    "link to the body is {} ms stale (limit {} ms); it is standing still on its own",
                    s.link_age_ms, m.limits.lost_link_ms
                ));
            }
            if !power::PowerLimits::battery_ok(&m.limits.power, s.vbat) {
                return Err(format!(
                    "battery {:.2} V is below the floor {:.2} V; the body sits and will not walk",
                    s.vbat, m.limits.power.vbat_floor
                ));
            }
            if s.mode == Mode::Calibrating {
                return Err(String::from("body is in calibration mode; finish calibrating before walking"));
            }
            Ok(())
        }
        "servo.set" => {
            if s.mode == Mode::Walking {
                return Err(String::from("a gait is running; raw servo moves are refused until it stops"));
            }
            if s.mode != Mode::Calibrating {
                return Err(String::from("raw servo moves are only allowed in calibration mode (body.calibrate first)"));
            }
            Ok(())
        }
        // stopping and sitting are always allowed — they are the safe direction
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> Model {
        Model::default()
    }

    #[test]
    fn a_normal_walk_passes_all_three() {
        let m = model();
        assert!(validate(&m, "gait.walk", "fwd 3 0.5").is_ok());
        let pred = simulate(&m, "gait.walk", "fwd 3 0.5").unwrap();
        assert!(pred.contains("margin"), "{pred}");
        assert!(policy(&m, &BodyState::default(), "gait.walk", "fwd 3 0.5").is_ok());
    }

    #[test]
    fn too_many_steps_fails_validate() {
        let m = model();
        let e = validate(&m, "gait.walk", "fwd 40").unwrap_err();
        assert!(e.contains("envelope"), "{e}");
        assert!(validate(&m, "gait.walk", "fwd 0").is_err());
        assert!(validate(&m, "gait.walk", "fwd 3 5.0").is_err());
    }

    #[test]
    fn an_unknown_verb_fails_validate() {
        let e = validate(&model(), "fly.away", "").unwrap_err();
        assert!(e.contains("no verb"));
    }

    #[test]
    fn a_tippy_body_fails_simulate() {
        let mut m = model();
        // a battery hung far off the back: the CoM leaves the tripod's polygon mid-swing
        m.geom.com = crate::geometry::Vec2::new(-60.0, 0.0);
        assert!(validate(&m, "gait.walk", "fwd 1 0.5").is_ok(), "shape is fine");
        let e = simulate(&m, "gait.walk", "fwd 1 0.5").unwrap_err();
        assert!(e.contains("tips"), "{e}");
    }

    #[test]
    fn a_weak_rail_fails_simulate() {
        let mut m = model();
        m.limits.power.rail_amps = 1.0;
        let e = simulate(&m, "gait.walk", "fwd 1 1.0").unwrap_err();
        assert!(e.contains("browns out"), "{e}");
    }

    #[test]
    fn a_stale_link_fails_policy_and_stop_is_still_allowed() {
        let m = model();
        let s = BodyState { link_age_ms: 900, ..BodyState::default() };
        let e = policy(&m, &s, "gait.walk", "fwd 1").unwrap_err();
        assert!(e.contains("stale"), "{e}");
        assert!(policy(&m, &s, "gait.stop", "").is_ok());
        assert!(policy(&m, &s, "gait.sit", "").is_ok());
    }

    #[test]
    fn a_flat_battery_fails_policy() {
        let m = model();
        let s = BodyState { vbat: 6.2, ..BodyState::default() };
        assert!(policy(&m, &s, "gait.walk", "fwd 1").unwrap_err().contains("battery"));
    }

    #[test]
    fn raw_servo_moves_need_calibration_mode_and_no_gait() {
        let m = model();
        assert!(validate(&m, "servo.set", "3 90").is_ok());
        assert!(validate(&m, "servo.set", "3 5").is_err());
        assert!(validate(&m, "servo.set", "three ninety").is_err());
        let idle = BodyState::default();
        assert!(policy(&m, &idle, "servo.set", "3 90").unwrap_err().contains("calibration mode"));
        let walking = BodyState { mode: Mode::Walking, ..idle };
        assert!(policy(&m, &walking, "servo.set", "3 90").unwrap_err().contains("gait is running"));
        let cal = BodyState { mode: Mode::Calibrating, ..idle };
        assert!(policy(&m, &cal, "servo.set", "3 90").is_ok());
        assert!(policy(&m, &cal, "gait.walk", "fwd 1").unwrap_err().contains("calibration"));
    }
}
