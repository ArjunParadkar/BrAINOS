//! A monotonic clock from the host.

use brainos_abi::time::{Clock, Instant};
use std::time::Instant as StdInstant;

/// The host's monotonic clock, zeroed at construction.
///
/// Zeroed rather than absolute because `Instant` is documented as "since
/// the body powered on", and an entity that treated a host's uptime as
/// its own would be confused by every reboot in a way that a robot
/// running the same code would not.
pub struct SystemClock {
    origin: StdInstant,
}

impl Default for SystemClock {
    fn default() -> SystemClock {
        SystemClock::new()
    }
}

impl SystemClock {
    pub fn new() -> SystemClock {
        SystemClock {
            origin: StdInstant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant(self.origin.elapsed().as_micros().min(u64::MAX as u128) as u64)
    }
}

/// A clock that only moves when told to. For deterministic runs of the
/// experience loop, where a real clock would make two runs differ.
#[derive(Default)]
pub struct StepClock {
    micros: std::cell::Cell<u64>,
}

impl StepClock {
    pub fn new() -> StepClock {
        StepClock {
            micros: std::cell::Cell::new(0),
        }
    }

    pub fn advance(&self, by: brainos_abi::time::Duration) {
        self.micros.set(self.micros.get().saturating_add(by.as_micros()));
    }
}

impl Clock for StepClock {
    fn now(&self) -> Instant {
        Instant(self.micros.get())
    }
}
