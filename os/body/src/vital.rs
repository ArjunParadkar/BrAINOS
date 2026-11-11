//! BFS  LEVEL    Section 8 Acquisition - a device becomes a region of the body  [crate brainos-body]
//! BFS  PURPOSE  One reading of one sense of one part, and the health bands a vital sign is judged against
//! BFS  DEFINES  SENSE_NAME_MAX, Felt, Health, Band
//! BFS  USES     brainos: abi
//! BFS  USED BY  body/lib, body/policy, entity/entity/parts, entity/tests/parts_suite, host/nerves/hwmon, host/nerves/machine, host/nerves/mod, host/nerves/nvml, +11 more
//!
//! Vitals — what a part feels, right now.
//!
//! `Telemetry` (in `hal`) is a fixed report with six fields, written for
//! a body that walks. It cannot say "the GPU is at 91 °C" or "this flash
//! has used 80 % of its write budget", and a policy that cannot hear
//! those cannot refuse on their account.
//!
//! `Felt` is the general form: one named sense of one part, one value,
//! one time. It is deliberately a *reading* and not a record. A policy
//! holds the latest `Felt` for the senses it guards — a view of the
//! body right now, exactly as it holds the latest `Telemetry` — and
//! never a history. History is memory, and memory is the state graph's
//! (the Memory-Integrity Law).
//!
//! `Health` is the four-band judgement a vital is reduced to. It is an
//! enum rather than a number so that "worse" is totally ordered and two
//! pieces of code cannot disagree about which side of a threshold a
//! reading is on.

use brainos_abi::ids::BodyId;
use brainos_abi::text::Text;
use brainos_abi::time::Instant;

/// The longest sense name. Names are short on purpose: they are
/// compared on every reading.
pub const SENSE_NAME_MAX: usize = 32;

/// One reading of one sense of one part.
#[derive(Clone, Copy, Debug)]
pub struct Felt {
    /// the part that reported it. A part is a body in the §8 sense:
    /// its id is `H(attestation key ‖ schema digest)`.
    pub part: BodyId,
    /// which sense, by the name the part declared
    pub sense: Text<SENSE_NAME_MAX>,
    pub value: f64,
    pub at: Instant,
    /// false when the nerve knows the value is stale or estimated
    pub fresh: bool,
}

impl Felt {
    pub fn new(part: BodyId, sense: &str, value: f64, at: Instant) -> Felt {
        Felt {
            part,
            sense: Text::from_str_truncating(sense),
            value,
            at,
            fresh: true,
        }
    }

    pub fn stale(mut self) -> Felt {
        self.fresh = false;
        self
    }

    pub fn is(&self, part: &BodyId, sense: &str) -> bool {
        self.part == *part && self.sense.as_str() == sense
    }
}

/// How a vital sign stands. Ordered: later is worse.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum Health {
    /// inside its normal range
    Nominal = 0,
    /// past the warning threshold: still working, worth knowing about
    Degraded = 1,
    /// past the critical threshold: must not be made worse
    Critical = 2,
    /// the part reported a fault, or stopped reporting at all
    Failed = 3,
}

impl Health {
    pub const ALL: [Health; 4] = [
        Health::Nominal,
        Health::Degraded,
        Health::Critical,
        Health::Failed,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Health::Nominal => "nominal",
            Health::Degraded => "degraded",
            Health::Critical => "critical",
            Health::Failed => "failed",
        }
    }

    /// May a part in this state be relied on for new work?
    pub const fn is_usable(self) -> bool {
        matches!(self, Health::Nominal | Health::Degraded)
    }
}

/// The two thresholds a vital is judged against, and which direction
/// is bad. Temperature is bad when it rises; a write budget or a
/// battery charge is bad when it falls.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Band {
    pub warn: f64,
    pub critical: f64,
    pub rising_is_bad: bool,
}

impl Band {
    /// Bad when the value rises (temperature, error counts, wear used).
    pub fn rising(warn: f64, critical: f64) -> Band {
        Band {
            warn,
            critical,
            rising_is_bad: true,
        }
    }

    /// Bad when the value falls (charge, budget remaining, fan speed).
    pub fn falling(warn: f64, critical: f64) -> Band {
        Band {
            warn,
            critical,
            rising_is_bad: false,
        }
    }

    /// A band is sane when warning comes before critical in the bad
    /// direction and both are real numbers. An insane band judges
    /// everything `Failed`: a part that cannot say where its limits are
    /// has no limits anyone can respect.
    pub fn is_sane(&self) -> bool {
        self.warn.is_finite()
            && self.critical.is_finite()
            && if self.rising_is_bad {
                self.warn <= self.critical
            } else {
                self.warn >= self.critical
            }
    }

    pub fn judge(&self, value: f64) -> Health {
        if !self.is_sane() || !value.is_finite() {
            return Health::Failed;
        }
        let (past_warn, past_crit) = if self.rising_is_bad {
            (value >= self.warn, value >= self.critical)
        } else {
            (value <= self.warn, value <= self.critical)
        };
        if past_crit {
            Health::Critical
        } else if past_warn {
            Health::Degraded
        } else {
            Health::Nominal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rising_band_gets_worse_as_the_value_rises() {
        let b = Band::rising(80.0, 95.0);
        assert_eq!(b.judge(40.0), Health::Nominal);
        assert_eq!(b.judge(80.0), Health::Degraded);
        assert_eq!(b.judge(94.9), Health::Degraded);
        assert_eq!(b.judge(95.0), Health::Critical);
    }

    #[test]
    fn a_falling_band_gets_worse_as_the_value_falls() {
        let b = Band::falling(0.2, 0.05);
        assert_eq!(b.judge(0.9), Health::Nominal);
        assert_eq!(b.judge(0.2), Health::Degraded);
        assert_eq!(b.judge(0.05), Health::Critical);
    }

    #[test]
    fn a_band_that_makes_no_sense_and_a_value_that_is_not_a_number_both_fail() {
        assert_eq!(Band::rising(95.0, 80.0).judge(10.0), Health::Failed);
        assert_eq!(Band::falling(0.05, 0.2).judge(0.9), Health::Failed);
        assert_eq!(Band::rising(80.0, 95.0).judge(f64::NAN), Health::Failed);
        assert_eq!(Band::rising(f64::NAN, 95.0).judge(10.0), Health::Failed);
    }

    #[test]
    fn health_is_ordered_worst_last() {
        let mut sorted = Health::ALL;
        sorted.sort();
        assert_eq!(sorted, Health::ALL);
        assert!(Health::Degraded.is_usable());
        assert!(!Health::Critical.is_usable());
    }

    #[test]
    fn a_reading_is_matched_by_part_and_sense_together() {
        let a = BodyId::from_bytes([1; 32]);
        let b = BodyId::from_bytes([2; 32]);
        let f = Felt::new(a, "temp.c", 61.0, Instant(5));
        assert!(f.is(&a, "temp.c"));
        assert!(!f.is(&b, "temp.c"));
        assert!(!f.is(&a, "fan.rpm"));
        assert!(f.fresh && !f.stale().fresh);
    }
}
