//! The body's side of a command: Domain 2 executing what KIRA granted — and
//! refusing, on its own authority, what its own envelope forbids. The
//! simulator and the firmware both drive a `Body` through this; only the
//! `ServoBus` and the clock differ.
//!
//! A `Body` never remembers (§13.2): it holds the calibration it was handed
//! and the pose it is in, and nothing about what it has done.

use crate::gait::{Plan, Walk};
use crate::json;
use crate::policy::{self, BodyState, Mode};
use crate::schema::{self, Model};
use crate::servo::ServoCal;
use crate::watchdog::LinkWatch;
use crate::{Pose, MAX_SERVOS};
use alloc::format;
use alloc::string::String;

/// What moves the servos. MCPWM on the S3; a struct of floats in the sim.
pub trait ServoBus {
    fn set_deg(&mut self, ch: u8, deg: f32);
    /// Cut the rail (sit + unload). Optional.
    fn relax(&mut self) {}
}

/// A plan in progress.
#[derive(Clone, Copy, Debug)]
pub struct Playing {
    pub plan: Plan,
    pub next_sample: usize,
    pub next_at_ms: u64,
}

pub struct Body<B: ServoBus> {
    pub model: Model,
    pub bus: B,
    pub pose: Pose,
    pub state: BodyState,
    pub link: LinkWatch,
    pub playing: Option<Playing>,
    /// the last servo targets written, for telemetry
    pub last_targets: [(u8, f32); MAX_SERVOS],
    /// running estimate of rail current from the last sample, A
    pub amps: f32,
    pub steps_walked: u32,
}

impl<B: ServoBus> Body<B> {
    pub fn new(model: Model, bus: B, now_ms: u64) -> Body<B> {
        let mut b = Body {
            link: LinkWatch::new(now_ms, model.limits.lost_link_ms),
            model,
            bus,
            pose: Pose::sit(30.0),
            state: BodyState::default(),
            playing: None,
            last_targets: [(255, 0.0); MAX_SERVOS],
            amps: 0.0,
            steps_walked: 0,
        };
        b.write_pose(Pose::sit(30.0));
        b
    }

    /// Any frame from the seat is a heartbeat.
    pub fn heard(&mut self, now_ms: u64) {
        self.link.heard(now_ms);
        self.state.link_age_ms = 0;
    }

    fn write_pose(&mut self, pose: Pose) {
        if let Ok(t) = self.model.table.targets(&pose) {
            let mut moving = 0;
            for i in 0..self.model.table.len {
                if (t[i].1 - self.last_targets[i].1).abs() > self.model.limits.power.moving_threshold_deg {
                    moving += 1;
                }
                self.bus.set_deg(t[i].0, t[i].1);
            }
            self.amps = moving as f32 * self.model.limits.power.servo_moving_amps
                + (self.model.table.len - moving) as f32 * self.model.limits.power.servo_hold_amps;
            self.last_targets = t;
            self.pose = pose;
        }
    }

    /// The reflex loop tick. Call as often as you like (≥ 50 Hz on the board).
    /// Returns a message once when the watchdog fires.
    pub fn tick(&mut self, now_ms: u64) -> Option<&'static str> {
        self.state.link_age_ms = self.link.age_ms(now_ms);
        if self.link.poll(now_ms) {
            // lost the mind: stop and stand, on our own authority
            self.playing = None;
            self.state.mode = Mode::Idle;
            self.write_pose(Pose::STAND);
            self.state.sitting = false;
            return Some("link lost: stopped and standing");
        }
        if let Some(mut p) = self.playing {
            if now_ms >= p.next_at_ms {
                if p.next_sample >= p.plan.samples() {
                    self.playing = None;
                    self.state.mode = Mode::Idle;
                    self.write_pose(Pose::STAND);
                    return None;
                }
                let pose = p.plan.sample(p.next_sample);
                // Domain 2 re-checks its own envelope on every sample
                if self.model.table.targets(&pose).is_err() {
                    self.playing = None;
                    self.state.mode = Mode::Idle;
                    self.write_pose(Pose::STAND);
                    return Some("reflex: a sample left the envelope; stopped");
                }
                self.write_pose(pose);
                if p.next_sample % p.plan.params.samples_per_cycle as usize == 0 {
                    self.steps_walked += 1;
                }
                p.next_sample += 1;
                p.next_at_ms = now_ms + p.plan.sample_ms() as u64;
                self.playing = Some(p);
            }
        }
        None
    }

    /// Play a whole plan instantly (the simulator's fast path, and the
    /// forward model). Stops at the first sample the reflex layer refuses.
    pub fn play_all(&mut self, plan: &Plan) -> Result<(), String> {
        for pose in plan.iter() {
            self.model.table.targets(&pose)?;
            self.write_pose(pose);
        }
        self.steps_walked += plan.walk.steps as u32;
        self.write_pose(Pose::STAND);
        Ok(())
    }

    pub fn telemetry(&self) -> String {
        let vbat = if self.state.vbat.is_nan() { String::from("vbat n/a") } else { format!("{:.2} V", self.state.vbat) };
        format!(
            "mode {}; {}; {}; {:.2} A; link {} ms; steps {}; lift {:.0}/{:.0}/{:.0}/{:.0}/{:.0}/{:.0}",
            self.state.mode.name(),
            if self.state.sitting { "sitting" } else { "standing" },
            vbat,
            self.amps,
            self.state.link_age_ms,
            self.steps_walked,
            self.pose.lift[0], self.pose.lift[1], self.pose.lift[2], self.pose.lift[3], self.pose.lift[4], self.pose.lift[5]
        )
    }

    /// Execute one command from the seat. `now_ms` is the body's clock;
    /// `instant` plays gaits to completion before returning (sim / bench),
    /// otherwise the plan is queued for `tick`. Returns (digest, ok).
    pub fn handle(&mut self, verb: &str, input_json: &str, now_ms: u64, instant: bool) -> (String, bool) {
        self.heard(now_ms);
        let target = target_of(verb, input_json);
        // the body's own gate: the same three checks KIRA ran, run again here
        if let Err(e) = policy::validate(&self.model, verb, &target) {
            return (format!("refused by the body (envelope): {e}"), false);
        }
        if let Err(e) = policy::simulate(&self.model, verb, &target) {
            return (format!("refused by the body (forward model): {e}"), false);
        }
        if let Err(e) = policy::policy(&self.model, &self.state, verb, &target) {
            return (format!("refused by the body (invariant): {e}"), false);
        }
        match verb {
            "gait.stand" => {
                self.playing = None;
                self.state.mode = Mode::Idle;
                self.state.sitting = false;
                self.write_pose(Pose::STAND);
                (format!("standing; {}", self.telemetry()), true)
            }
            "gait.sit" => {
                self.playing = None;
                self.state.mode = Mode::Idle;
                self.state.sitting = true;
                self.write_pose(Pose::sit(30.0));
                self.bus.relax();
                (format!("sitting; {}", self.telemetry()), true)
            }
            "gait.stop" => {
                let was = self.playing.is_some();
                self.playing = None;
                self.state.mode = Mode::Idle;
                self.write_pose(Pose::STAND);
                self.state.sitting = false;
                (format!("{}; {}", if was { "stopped" } else { "nothing was moving" }, self.telemetry()), true)
            }
            "gait.walk" => {
                let w = Walk::parse_target(&target).unwrap();
                let plan = Plan::new(self.model.gait, w);
                if self.state.sitting {
                    self.write_pose(Pose::STAND);
                    self.state.sitting = false;
                }
                if instant {
                    match self.play_all(&plan) {
                        Ok(()) => (
                            format!("walked {} step(s) {} at {:.0}%; {}", w.steps, w.dir.name(), w.speed * 100.0, self.telemetry()),
                            true,
                        ),
                        Err(e) => (format!("reflex stopped the walk: {e}"), false),
                    }
                } else {
                    self.state.mode = Mode::Walking;
                    self.playing = Some(Playing { plan, next_sample: 0, next_at_ms: now_ms });
                    (
                        format!("walking {} step(s) {} at {:.0}%, ~{:.1} s", w.steps, w.dir.name(), w.speed * 100.0, plan.total_ms() as f32 / 1000.0),
                        true,
                    )
                }
            }
            "servo.set" => {
                let (ch, deg) = policy::parse_servo_target(&target).unwrap();
                self.bus.set_deg(ch, deg);
                if let Some(i) = self.model.table.channels().iter().position(|c| c.ch == ch) {
                    self.last_targets[i] = (ch, deg);
                }
                (format!("channel {ch} at {deg:.1}°"), true)
            }
            "body.telemetry" => (self.telemetry(), true),
            "body.schema" => (self.model.schema_json(), true),
            "body.calibrate" => {
                // `{"mode":"begin"}` enters calibration; `{"mode":"end"}` leaves it;
                // `{"ch":6,"center":92,"min":30,"max":150}` updates one channel's
                // record — which the seat then writes to the state graph. The body
                // holds it only while powered.
                match json::get_str(input_json, "mode").as_deref() {
                    Some("begin") => {
                        self.playing = None;
                        self.state.mode = Mode::Calibrating;
                        (String::from("calibration mode: servo.set is allowed, gaits are refused"), true)
                    }
                    Some("end") => {
                        self.state.mode = Mode::Idle;
                        self.write_pose(Pose::STAND);
                        self.state.sitting = false;
                        (format!("calibration ended; standing on the new table; {}", schema::calibration_lines("body", &self.model).len()), true)
                    }
                    _ => {
                        let Some(ch) = json::get_f32(input_json, "ch").map(|c| c as u8) else {
                            return (String::from("body.calibrate wants mode begin|end, or a channel record"), false);
                        };
                        let Some(c) = self.model.table.find_mut(ch) else {
                            return (format!("no servo on channel {ch}"), false);
                        };
                        if let Some(v) = json::get_f32(input_json, "center") { c.center_deg = v }
                        if let Some(v) = json::get_f32(input_json, "min") { c.min_deg = v }
                        if let Some(v) = json::get_f32(input_json, "max") { c.max_deg = v }
                        if let Some(v) = json::get_f32(input_json, "dir") { c.dir = v as i8 }
                        let c: ServoCal = *c;
                        (
                            format!("ch{} center {:.1} min {:.1} max {:.1} dir {}", c.ch, c.center_deg, c.min_deg, c.max_deg, c.dir),
                            true,
                        )
                    }
                }
            }
            _ => (format!("this body has no verb '{verb}'"), false),
        }
    }
}

/// The KIRA target string for a verb + JSON input. Mirrored at the seat in
/// `deliberate::kira_target` — the same strings are gated there and
/// re-checked here.
pub fn target_of(verb: &str, input_json: &str) -> String {
    match verb {
        "gait.walk" => {
            let dir = json::get_str(input_json, "dir").unwrap_or_else(|| String::from("fwd"));
            let steps = json::get_f32(input_json, "steps").unwrap_or(1.0) as i64;
            let speed = json::get_f32(input_json, "speed").unwrap_or(0.5);
            format!("{dir} {steps} {speed:.2}")
        }
        "servo.set" => {
            let ch = json::get_f32(input_json, "ch").map(|c| c as i64).unwrap_or(-1);
            let deg = json::get_f32(input_json, "deg").unwrap_or(-1.0);
            format!("{ch} {deg:.1}")
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        writes: alloc::vec::Vec<(u8, f32)>,
    }
    impl ServoBus for Recorder {
        fn set_deg(&mut self, ch: u8, deg: f32) {
            self.writes.push((ch, deg));
        }
    }

    fn body() -> Body<Recorder> {
        Body::new(Model::default(), Recorder::default(), 0)
    }

    #[test]
    fn a_walk_moves_every_servo_and_ends_standing() {
        let mut b = body();
        let (d, ok) = b.handle("gait.walk", r#"{"dir":"fwd","steps":2,"speed":1.0}"#, 10, true);
        assert!(ok, "{d}");
        assert!(d.starts_with("walked 2 step(s) fwd"), "{d}");
        assert_eq!(b.pose, Pose::STAND);
        assert_eq!(b.steps_walked, 2);
        let touched: alloc::collections::BTreeSet<u8> = b.bus.writes.iter().map(|w| w.0).collect();
        assert_eq!(touched.len(), 10);
    }

    #[test]
    fn the_body_refuses_what_kira_should_never_have_granted() {
        let mut b = body();
        let (d, ok) = b.handle("gait.walk", r#"{"dir":"fwd","steps":99}"#, 10, true);
        assert!(!ok);
        assert!(d.contains("refused by the body (envelope)"), "{d}");
        let (d, ok) = b.handle("servo.set", r#"{"ch":3,"deg":90}"#, 20, true);
        assert!(!ok);
        assert!(d.contains("calibration mode"), "{d}");
    }

    #[test]
    fn calibration_mode_allows_raw_moves_and_records_them() {
        let mut b = body();
        assert!(b.handle("body.calibrate", r#"{"mode":"begin"}"#, 10, true).1);
        let (d, ok) = b.handle("servo.set", r#"{"ch":3,"deg":95}"#, 20, true);
        assert!(ok, "{d}");
        let (d, ok) = b.handle("body.calibrate", r#"{"ch":3,"center":95,"min":25}"#, 30, true);
        assert!(ok, "{d}");
        assert_eq!(b.model.table.find(3).unwrap().center_deg, 95.0);
        assert!(b.handle("gait.walk", r#"{"dir":"fwd","steps":1}"#, 40, true).0.contains("calibration"));
        assert!(b.handle("body.calibrate", r#"{"mode":"end"}"#, 50, true).1);
        assert!(b.handle("gait.walk", r#"{"dir":"fwd","steps":1}"#, 60, true).1);
    }

    #[test]
    fn a_queued_walk_plays_over_ticks_and_the_watchdog_stops_it() {
        let mut b = body();
        let (d, ok) = b.handle("gait.walk", r#"{"dir":"fwd","steps":1,"speed":1.0}"#, 0, false);
        assert!(ok, "{d}");
        assert_eq!(b.state.mode, Mode::Walking);
        let step = b.playing.unwrap().plan.sample_ms() as u64;
        let mut t = 0;
        for _ in 0..10 {
            b.tick(t);
            t += step;
        }
        assert_eq!(b.state.mode, Mode::Walking);
        assert!(b.playing.unwrap().next_sample >= 10);
        // silence past the watchdog: the body stops and stands by itself
        let msg = b.tick(t + 1000);
        assert_eq!(msg, Some("link lost: stopped and standing"));
        assert!(b.playing.is_none());
        assert_eq!(b.pose, Pose::STAND);
        assert_eq!(b.state.mode, Mode::Idle);
        // and a walk is refused until the seat is heard again
        let (d, ok) = b.handle("gait.walk", r#"{"dir":"fwd","steps":1}"#, t + 1000, true);
        // handle() counts as hearing the seat, so this one is allowed — the
        // refusal lives at the seat (KIRA policy) when the link is stale there
        assert!(ok, "{d}");
    }

    #[test]
    fn a_queued_walk_completes_by_itself() {
        let mut b = body();
        b.handle("gait.walk", r#"{"dir":"back","steps":1,"speed":1.0}"#, 0, false);
        let step = b.playing.unwrap().plan.sample_ms() as u64;
        let mut t = 0;
        for _ in 0..60 {
            b.heard(t);
            b.tick(t);
            t += step;
        }
        assert!(b.playing.is_none());
        assert_eq!(b.state.mode, Mode::Idle);
        assert_eq!(b.steps_walked, 1);
    }

    #[test]
    fn targets_mirror_the_seat() {
        assert_eq!(target_of("gait.walk", r#"{"dir":"left","steps":3,"speed":0.25}"#), "left 3 0.25");
        assert_eq!(target_of("gait.walk", "{}"), "fwd 1 0.50");
        assert_eq!(target_of("servo.set", r#"{"ch":6,"deg":92.5}"#), "6 92.5");
        assert_eq!(target_of("gait.stop", "{}"), "");
    }
}
