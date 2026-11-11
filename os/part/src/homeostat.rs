//! BFS  LEVEL    Parts and nerves - every piece of hardware as a body part  [crate brainos-part]
//! BFS  PURPOSE  Homeostat: a Domain 2 regulator (e.g. temperature to fan duty) that runs with no model call inside a range KIRA granted once
//! BFS  DEFINES  ReflexAct, InstallError, Homeostat
//! BFS  USES     this crate: nerve | brainos: abi, body
//! BFS  USED BY  entity/tests/parts_suite, host/nerves/hwmon, host/tests/autonomic_survives, part/lib, part/nerve, part/sim
//!
//! Homeostasis — Domain 2 for parts.
//!
//! Some things must keep happening whether or not anything is thinking:
//! a fan must follow a temperature, a load must back off as a battery
//! empties. Architecture v1.3 puts these in Domain 2: they run inside a
//! *pre-authorized envelope*, with no model call, and they keep running
//! if cognition dies.
//!
//! A `Homeostat` is one such loop. It is a curve from one sense to one
//! actuated value, clamped into a range.
//!
//! **Pre-authorized, not unauthorized.** `install` takes a `Mandate` —
//! the gate granted a regulate verb with a `min` and a `max` — and from
//! then on the homeostat acts only inside `[min, max]`. So every value
//! it ever commands traces back to one `Granted`, but no step of the
//! loop asks anyone anything.
//!
//! **It owns no cognition.** Nothing here imports the entity, the state
//! graph or a model. `step` is a pure function of the last reading and
//! the clock. That is what "survives a killed cognition process" means
//! in code: there is nothing for the loop to wait on.
//!
//! **If it cannot feel, it protects.** With no reading, a stale
//! reading, or an implausible one, it commands the failsafe value. For
//! cooling that is the top of the granted range.

use crate::nerve::Mandate;
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::ids::{BodyId, CapabilityId};
use brainos_abi::text::Text;
use brainos_abi::time::{Duration, Instant};
use brainos_body::vital::{Felt, SENSE_NAME_MAX};

/// One value a homeostat commands. The fields are private and the only
/// constructor is `Homeostat::step`, so a nerve's `reflex` cannot be
/// called with a value no homeostat produced.
#[derive(Clone, Debug)]
pub struct ReflexAct {
    part: BodyId,
    param: String,
    value: f64,
    grant: CapabilityId,
    failsafe: bool,
}

impl ReflexAct {
    pub fn part(&self) -> BodyId {
        self.part
    }
    pub fn param(&self) -> &str {
        &self.param
    }
    pub fn value(&self) -> f64 {
        self.value
    }
    /// The grant this loop was installed under.
    pub fn grant(&self) -> CapabilityId {
        self.grant
    }
    /// Was this the failsafe value, commanded because the loop could
    /// not trust its reading?
    pub fn is_failsafe(&self) -> bool {
        self.failsafe
    }
}

/// Why a homeostat could not be installed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InstallError {
    /// the mandate carries no numeric `min` and `max`
    NoRange,
    /// `min` is above `max`, or one is not a number
    BadRange,
    /// the curve is empty, unsorted, or not numbers
    BadCurve,
}

/// A regulator: one sense in, one value out, inside a granted range.
pub struct Homeostat {
    actuated: BodyId,
    param: String,
    watched: BodyId,
    sense: Text<SENSE_NAME_MAX>,
    /// (reading, output) points, sorted by reading; linear between them
    curve: Vec<(f64, f64)>,
    min: f64,
    max: f64,
    failsafe: f64,
    max_age: Duration,
    grant: CapabilityId,
    last: Option<Felt>,
    /// how many steps have run, and how many of them were failsafe
    steps: u64,
    failsafes: u64,
}

impl Homeostat {
    /// Install a regulator under a mandate.
    ///
    /// The mandate's part is the actuated part. Its `min` and `max`
    /// arguments are the range. `param` is the name of the value the
    /// nerve will be asked to set. `(watched, sense)` is what it
    /// follows — usually a different part. `failsafe` is what to
    /// command when the reading cannot be trusted; it is clamped into
    /// the range like everything else.
    pub fn install(
        mandate: &Mandate,
        param: &str,
        watched: BodyId,
        sense: &str,
        curve: &[(f64, f64)],
        failsafe: f64,
    ) -> Result<Homeostat, InstallError> {
        let (Some(min), Some(max)) = (mandate.scalar("min"), mandate.scalar("max")) else {
            return Err(InstallError::NoRange);
        };
        if !(min.is_finite() && max.is_finite() && min <= max) {
            return Err(InstallError::BadRange);
        }
        if curve.is_empty()
            || curve.iter().any(|(x, y)| !x.is_finite() || !y.is_finite())
            || curve.windows(2).any(|w| w[0].0 >= w[1].0)
            || !failsafe.is_finite()
        {
            return Err(InstallError::BadCurve);
        }
        Ok(Homeostat {
            actuated: mandate.part(),
            param: alloc::string::ToString::to_string(param),
            watched,
            sense: Text::from_str_truncating(sense),
            curve: curve.to_vec(),
            min,
            max,
            failsafe: failsafe.clamp(min, max),
            max_age: Duration::from_secs(5),
            grant: mandate.grant(),
            last: None,
            steps: 0,
            failsafes: 0,
        })
    }

    pub fn with_max_age(mut self, d: Duration) -> Homeostat {
        self.max_age = d;
        self
    }

    pub fn range(&self) -> (f64, f64) {
        (self.min, self.max)
    }

    pub fn steps(&self) -> u64 {
        self.steps
    }

    pub fn failsafes(&self) -> u64 {
        self.failsafes
    }

    /// Hear a reading. Ignores everything except the one sense it
    /// follows, and keeps only the newest.
    pub fn feel(&mut self, f: &Felt) {
        if f.is(&self.watched, self.sense.as_str()) {
            self.last = Some(*f);
        }
    }

    fn curve_at(&self, x: f64) -> f64 {
        let first = self.curve[0];
        let last = self.curve[self.curve.len() - 1];
        if x <= first.0 {
            return first.1;
        }
        if x >= last.0 {
            return last.1;
        }
        for w in self.curve.windows(2) {
            let (a, b) = (w[0], w[1]);
            if x >= a.0 && x <= b.0 {
                return a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0);
            }
        }
        last.1
    }

    /// One pass of the loop. Always produces a command.
    pub fn step(&mut self, now: Instant) -> ReflexAct {
        self.steps += 1;
        let trusted = self.last.and_then(|f| {
            let age = now.checked_since(f.at).unwrap_or(Duration::ZERO);
            (f.fresh && f.value.is_finite() && age <= self.max_age).then_some(f.value)
        });
        let (value, failsafe) = match trusted {
            Some(x) => (self.curve_at(x).clamp(self.min, self.max), false),
            None => {
                self.failsafes += 1;
                (self.failsafe, true)
            }
        };
        ReflexAct {
            part: self.actuated,
            param: self.param.clone(),
            value,
            grant: self.grant,
            failsafe,
        }
    }
}
