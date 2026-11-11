//! BFS  LEVEL    Domain 2 Autonomic - envelopes, reflexes, scheduler, watchdogs  [crate brainos-rt]
//! BFS  PURPOSE  Times every pass of a task into a fixed histogram and reports when the measured worst case exceeds the run time the task declared
//! BFS  DEFINES  Budget, BudgetReport
//! BFS  USES     brainos: abi
//! BFS  USED BY  rt/lib, rt/tests/rt_suite
//!
//! Execution budgets — turning a declared WCET into a measured one.
//!
//! `TaskSpec::wcet` is a promise. This is how the promise is kept honest:
//! every pass is timed, the distribution is kept in a fixed-size
//! histogram, and the scheduler's declared bound is compared against what
//! actually happened. A task whose measured worst case has crept above
//! its declared one is a scheduling analysis that is now wrong, and the
//! system says so before the deadline it silently invalidated is missed
//! on a body.
//!
//! The histogram is logarithmic and fixed at 16 buckets. Percentiles from
//! it are approximate, and the code says so rather than reporting a
//! precise-looking number: a p99 from a log histogram is "somewhere in
//! this bucket", and the bucket edge is what gets reported.

use brainos_abi::text::Text;
use brainos_abi::time::Duration;

const BUCKETS: usize = 16;

/// A log2 histogram of durations, in microseconds.
///
/// Bucket `i` holds durations in `[2^i, 2^(i+1))` microseconds, so bucket
/// 0 is 1–2 µs and bucket 15 is 32.7 ms and up. That range covers
/// everything from an interrupt handler to a deliberation pass.
#[derive(Clone, Copy, Default)]
pub struct Budget {
    pub name: &'static str,
    declared: Duration,
    buckets: [u32; BUCKETS],
    samples: u64,
    total_micros: u64,
    worst: Duration,
    best: Duration,
    /// passes that exceeded the declared budget
    pub exceeded: u64,
}

impl Budget {
    pub fn new(name: &'static str, declared: Duration) -> Budget {
        Budget {
            name,
            declared,
            buckets: [0; BUCKETS],
            samples: 0,
            total_micros: 0,
            worst: Duration::ZERO,
            best: Duration::FOREVER,
            exceeded: 0,
        }
    }

    pub fn declared(&self) -> Duration {
        self.declared
    }

    pub fn samples(&self) -> u64 {
        self.samples
    }

    pub fn record(&mut self, d: Duration) {
        let us = d.as_micros();
        let b = bucket_of(us);
        self.buckets[b] = self.buckets[b].saturating_add(1);
        self.samples = self.samples.saturating_add(1);
        self.total_micros = self.total_micros.saturating_add(us);
        if d > self.worst {
            self.worst = d;
        }
        if d < self.best {
            self.best = d;
        }
        if d > self.declared {
            self.exceeded = self.exceeded.saturating_add(1);
        }
    }

    pub fn worst(&self) -> Duration {
        self.worst
    }

    pub fn best(&self) -> Duration {
        if self.samples == 0 {
            Duration::ZERO
        } else {
            self.best
        }
    }

    pub fn mean(&self) -> Duration {
        if self.samples == 0 {
            Duration::ZERO
        } else {
            Duration::from_micros(self.total_micros / self.samples)
        }
    }

    /// The upper edge of the bucket containing the given percentile.
    /// Reported as an upper bound because that is what a log histogram
    /// actually knows.
    pub fn percentile_upper(&self, pct: u8) -> Duration {
        if self.samples == 0 {
            return Duration::ZERO;
        }
        let target = (self.samples.saturating_mul(pct.min(100) as u64)) / 100;
        let mut seen = 0u64;
        for (i, count) in self.buckets.iter().enumerate() {
            seen = seen.saturating_add(*count as u64);
            if seen >= target.max(1) {
                return Duration::from_micros(1u64 << (i + 1));
            }
        }
        self.worst
    }

    /// Does reality still support the declared bound? A single overrun in
    /// a million passes is a glitch; a measured worst case above the
    /// declaration means the schedulability analysis is stale.
    pub fn is_honest(&self) -> bool {
        self.samples == 0 || self.worst <= self.declared
    }

    pub fn report(&self) -> BudgetReport {
        BudgetReport {
            name: self.name,
            declared: self.declared,
            measured_worst: self.worst,
            mean: self.mean(),
            p99_upper: self.percentile_upper(99),
            samples: self.samples,
            exceeded: self.exceeded,
            honest: self.is_honest(),
        }
    }

    pub fn reset(&mut self) {
        let (name, declared) = (self.name, self.declared);
        *self = Budget::new(name, declared);
    }
}

fn bucket_of(micros: u64) -> usize {
    if micros < 2 {
        return 0;
    }
    let mut b = 0usize;
    let mut v = micros;
    while v > 1 && b < BUCKETS - 1 {
        v >>= 1;
        b += 1;
    }
    b.min(BUCKETS - 1)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BudgetReport {
    pub name: &'static str,
    pub declared: Duration,
    pub measured_worst: Duration,
    pub mean: Duration,
    pub p99_upper: Duration,
    pub samples: u64,
    pub exceeded: u64,
    pub honest: bool,
}

impl BudgetReport {
    pub fn describe(&self) -> Text<160> {
        let mut t = Text::new();
        t.push_str(self.name);
        t.push_str(": declared ");
        t.push_u64(self.declared.as_micros());
        t.push_str("us worst ");
        t.push_u64(self.measured_worst.as_micros());
        t.push_str("us mean ");
        t.push_u64(self.mean.as_micros());
        t.push_str("us n=");
        t.push_u64(self.samples);
        if !self.honest {
            t.push_str(" BUDGET STALE");
        }
        t
    }
}
