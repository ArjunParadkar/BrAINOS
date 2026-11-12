//! BFS  LEVEL    Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)  [crate brainos-motor]
//! BFS  PURPOSE  Three reflex arcs that need no cognition: tighten the grip when something slips, go soft on an unexpected touch, and wait for the body to come back after a shove
//! BFS  DEFINES  Feel, Answer, Tuning, Arcs
//! BFS  USES     nothing outside core/alloc
//! BFS  USED BY  bodyd/armloop, motor/tests/reflex
//!
//! Reflex arcs: sensor to actuator, with nothing in between that thinks.
//!
//! Each arc reads what the body feels this tick and changes what the
//! body does this tick. None of them asks anything of the entity, so
//! none of them stops working when the entity's thinking does. They do
//! not extend what a run is allowed: a reflex's answer still goes
//! through the `Runner`, and an answer outside the mandate is refused
//! like any other tick. A grip reflex therefore tightens *up to* the
//! granted force and no further.
//!
//! * **slip → grip.** Something held is sliding: squeeze harder.
//! * **touch → yield.** Something touched the body where no touch was
//!   expected: go soft at once, then firm up again.
//! * **shove → wait.** The body has been pushed off its path: stop the
//!   path's clock until the body is back on it, so the path does not
//!   run on ahead of an arm that is somewhere else.

/// What the arcs read.
#[derive(Clone, Copy, Debug, Default)]
pub struct Feel {
    /// is something meant to be in the fingers?
    pub holding: bool,
    /// how fast it is sliding against them, metres per second
    pub slip_mps: f32,
    /// force on the body from the world, newtons
    pub touch_n: f32,
    /// does the stroke in charge expect to touch something?
    pub touch_expected: bool,
    /// distance between where the tool is and where the path says, metres
    pub off_path_m: f32,
}

/// What the arcs change.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Answer {
    /// extra squeeze, newtons, before the mandate's cap is applied
    pub extra_squeeze_n: f32,
    /// multiply the stiffness by this
    pub stiff_scale: f32,
    /// do not advance the path this tick
    pub hold_path: bool,
    pub slip_fired: bool,
    pub yield_fired: bool,
    pub wait_fired: bool,
}

/// The thresholds, in one place. These are the numbers a first hardware
/// test will most want to change.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Tuning {
    pub slip_mps: f32,
    /// each tick of slip adds this fraction of the squeeze in use
    pub slip_step: f32,
    /// how fast the extra squeeze fades once nothing slips, per second
    pub slip_fade_per_s: f32,
    pub touch_n: f32,
    pub yield_to: f32,
    pub yield_ms: f32,
    pub firm_ms: f32,
    pub off_path_m: f32,
}

impl Default for Tuning {
    fn default() -> Tuning {
        Tuning {
            slip_mps: 0.004,
            slip_step: 0.25,
            slip_fade_per_s: 0.5,
            touch_n: 1.5,
            yield_to: 0.15,
            yield_ms: 120.0,
            firm_ms: 120.0,
            off_path_m: 0.02,
        }
    }
}

/// The three arcs and what little they remember between ticks. This is
/// actuator state, not memory: it is gone when the run ends.
#[derive(Clone, Copy, Debug)]
pub struct Arcs {
    pub tuning: Tuning,
    extra_n: f32,
    /// milliseconds since the last unexpected touch
    since_touch_ms: f32,
    pub slips: u32,
    pub yields: u32,
    pub waits: u32,
}

impl Default for Arcs {
    fn default() -> Arcs {
        Arcs::new(Tuning::default())
    }
}

impl Arcs {
    pub fn new(tuning: Tuning) -> Arcs {
        Arcs {
            tuning,
            extra_n: 0.0,
            since_touch_ms: f32::INFINITY,
            slips: 0,
            yields: 0,
            waits: 0,
        }
    }

    /// One tick. `squeeze_n` is what the stroke asked for; `cap_n` is
    /// the most the mandate allows.
    pub fn step(&mut self, feel: &Feel, squeeze_n: f32, cap_n: f32, dt_ms: f32) -> Answer {
        let t = &self.tuning;
        let dt_s = dt_ms * 0.001;

        // slip -> grip
        let slip_fired = feel.holding && feel.slip_mps > t.slip_mps;
        if slip_fired {
            let room = (cap_n - squeeze_n).max(0.0);
            self.extra_n = (self.extra_n + t.slip_step * squeeze_n.max(1.0)).min(room);
            self.slips += 1;
        } else {
            self.extra_n = (self.extra_n - t.slip_fade_per_s * dt_s * squeeze_n.max(1.0)).max(0.0);
        }
        if !feel.holding {
            self.extra_n = 0.0;
        }

        // touch -> yield
        let yield_fired = !feel.touch_expected && feel.touch_n > t.touch_n;
        if yield_fired {
            if self.since_touch_ms > t.yield_ms + t.firm_ms {
                self.yields += 1;
            }
            self.since_touch_ms = 0.0;
        } else {
            self.since_touch_ms += dt_ms;
        }
        let stiff_scale = if self.since_touch_ms <= t.yield_ms {
            t.yield_to
        } else if self.since_touch_ms <= t.yield_ms + t.firm_ms {
            let u = (self.since_touch_ms - t.yield_ms) / t.firm_ms;
            t.yield_to + (1.0 - t.yield_to) * u
        } else {
            1.0
        };

        // shove -> wait
        let wait_fired = feel.off_path_m > t.off_path_m;
        if wait_fired {
            self.waits += 1;
        }

        Answer {
            extra_squeeze_n: self.extra_n,
            stiff_scale,
            hold_path: wait_fired,
            slip_fired,
            yield_fired,
            wait_fired,
        }
    }
}
