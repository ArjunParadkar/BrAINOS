//! Time, as three different things that are never the same thing.
//!
//! **`Tick`** is the entity's own continuous counter. It never resets
//! while the entity lives, it is monotone across body swaps and power
//! loss (it is journalled), and it is what memory is ordered by. It is
//! not a clock: one tick is one pass of the experience loop, and passes
//! are not uniform in duration.
//!
//! **`Micros`** is a monotonic hardware clock in microseconds since the
//! body powered on. It is what real-time deadlines are measured against.
//! It resets when the body does, which is exactly why memory is not
//! ordered by it.
//!
//! **Wall time** is deliberately absent. Nothing in the trust hierarchy
//! is allowed to depend on it, because wall time arrives from the
//! network, the network is an untrusted world (§3.8), and an invariant
//! that can be moved by lying about the date is not an invariant. Bodies
//! that need a calendar get it as an ordinary sensor reading with
//! provenance, like any other untrusted fact.

use crate::codec::{Decode, Decoder, Encode, Encoder};
use crate::error::Result;
use core::fmt;
use core::ops::{Add, AddAssign, Sub};

/// The entity's continuous cognition counter.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug)]
pub struct Tick(pub u64);

impl Tick {
    pub const ZERO: Tick = Tick(0);

    pub fn next(self) -> Tick {
        // saturating rather than wrapping: at 1 kHz this saturates after
        // 584 million years, and a wrapped tick would reorder memory
        Tick(self.0.saturating_add(1))
    }

    pub fn advance(&mut self) -> Tick {
        *self = self.next();
        *self
    }

    /// Ticks elapsed since an earlier tick; zero if `earlier` is later.
    pub fn since(self, earlier: Tick) -> u64 {
        self.0.saturating_sub(earlier.0)
    }

    pub fn plus(self, n: u64) -> Tick {
        Tick(self.0.saturating_add(n))
    }
}

impl fmt::Display for Tick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t{}", self.0)
    }
}

impl Encode for Tick {
    fn encode(&self, e: &mut Encoder) {
        e.u64(self.0);
    }
}

impl Decode for Tick {
    fn decode(d: &mut Decoder) -> Result<Tick> {
        d.u64().map(Tick)
    }
}

/// Microseconds. A plain scalar; use `Instant` and `Duration` for
/// anything where the distinction between "a point" and "a span"
/// matters, which is everywhere a deadline is involved.
pub type Micros = u64;

/// A point on the body's monotonic clock.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug)]
pub struct Instant(pub Micros);

impl Instant {
    pub const ZERO: Instant = Instant(0);

    pub fn micros(self) -> Micros {
        self.0
    }

    /// Time since an earlier instant. Saturating: a clock that appears to
    /// have gone backwards yields zero rather than a colossal duration
    /// that would look like a satisfied deadline.
    pub fn since(self, earlier: Instant) -> Duration {
        Duration(self.0.saturating_sub(earlier.0))
    }

    pub fn checked_since(self, earlier: Instant) -> Option<Duration> {
        self.0.checked_sub(earlier.0).map(Duration)
    }

    pub fn has_passed(self, deadline: Instant) -> bool {
        self.0 >= deadline.0
    }
}

impl Add<Duration> for Instant {
    type Output = Instant;
    fn add(self, d: Duration) -> Instant {
        Instant(self.0.saturating_add(d.0))
    }
}

impl AddAssign<Duration> for Instant {
    fn add_assign(&mut self, d: Duration) {
        self.0 = self.0.saturating_add(d.0);
    }
}

impl Sub<Instant> for Instant {
    type Output = Duration;
    fn sub(self, other: Instant) -> Duration {
        self.since(other)
    }
}

impl Encode for Instant {
    fn encode(&self, e: &mut Encoder) {
        e.u64(self.0);
    }
}

impl Decode for Instant {
    fn decode(d: &mut Decoder) -> Result<Instant> {
        d.u64().map(Instant)
    }
}

/// A span of monotonic time.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug)]
pub struct Duration(pub Micros);

impl Duration {
    pub const ZERO: Duration = Duration(0);
    /// Longer than any real deadline; used as "no limit" without needing
    /// an `Option` in a hot struct.
    pub const FOREVER: Duration = Duration(u64::MAX);

    pub const fn from_micros(us: u64) -> Duration {
        Duration(us)
    }

    pub const fn from_millis(ms: u64) -> Duration {
        Duration(ms.saturating_mul(1_000))
    }

    pub const fn from_secs(s: u64) -> Duration {
        Duration(s.saturating_mul(1_000_000))
    }

    pub const fn from_hz(hz: u32) -> Duration {
        if hz == 0 {
            Duration::FOREVER
        } else {
            Duration(1_000_000 / hz as u64)
        }
    }

    pub fn as_micros(self) -> u64 {
        self.0
    }

    pub fn as_millis(self) -> u64 {
        self.0 / 1_000
    }

    pub fn as_secs(self) -> u64 {
        self.0 / 1_000_000
    }

    pub fn saturating_sub(self, other: Duration) -> Duration {
        Duration(self.0.saturating_sub(other.0))
    }

    pub fn saturating_mul(self, n: u32) -> Duration {
        Duration(self.0.saturating_mul(n as u64))
    }
}

impl Add<Duration> for Duration {
    type Output = Duration;
    fn add(self, other: Duration) -> Duration {
        Duration(self.0.saturating_add(other.0))
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 >= 1_000_000 {
            write!(f, "{}.{:03}s", self.0 / 1_000_000, (self.0 % 1_000_000) / 1_000)
        } else if self.0 >= 1_000 {
            write!(f, "{}.{:03}ms", self.0 / 1_000, self.0 % 1_000)
        } else {
            write!(f, "{}us", self.0)
        }
    }
}

impl Encode for Duration {
    fn encode(&self, e: &mut Encoder) {
        e.u64(self.0);
    }
}

impl Decode for Duration {
    fn decode(d: &mut Decoder) -> Result<Duration> {
        d.u64().map(Duration)
    }
}

/// A monotonic clock. The one thing every platform must supply: the UEFI
/// core reads the TSC, the ESP32 reads its systimer, a host reads
/// `Instant::now`. Nothing above this trait knows which.
pub trait Clock {
    fn now(&self) -> Instant;
}
