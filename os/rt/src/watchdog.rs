//! BFS  LEVEL    Domain 2 Autonomic - envelopes, reflexes, scheduler, watchdogs  [crate brainos-rt]
//! BFS  PURPOSE  Timers that fire when a loop hangs, runs too fast, or stops making progress, and stay fired until something above clears them
//! BFS  DEFINES  Wd, Bite, Watchdog, WatchdogSet
//! BFS  USES     brainos: abi
//! BFS  USED BY  bodyd/runtime, rt/lib, rt/tests/rt_suite
//!
//! Watchdogs — the layer that assumes everything above it will fail.
//!
//! The §3.12 audit kept watchdogs and fail-safe defaults under "physics,
//! kept and strengthened". This module is that strengthening.
//!
//! A single watchdog is a timer something must pet. That is necessary and
//! not sufficient, because the common real failure is not "the loop
//! stopped" but "the loop is running and doing the wrong thing" — and a
//! loop that is wrong is perfectly capable of petting a timer. So there
//! are three kinds here, and a body uses all three:
//!
//! * **Liveness** — pet within a deadline or fire. Catches a hung loop.
//! * **Windowed** — pet within a deadline *and not before* a floor.
//!   Catches a loop that has started spinning far too fast, which on a
//!   servo means a controller that has gone unstable.
//! * **Progress** — a monotonically increasing counter must actually
//!   increase. Catches a loop that is running and petting and producing
//!   nothing, which is what a deadlocked state machine looks like from
//!   outside.
//!
//! Firing is not recoverable from inside. A fired watchdog stays fired
//! until something above it — which by construction is not the thing
//! that failed — clears it.

use brainos_abi::text::Text;
use brainos_abi::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wd {
    /// must be petted at least this often
    Liveness,
    /// must be petted inside a window, neither late nor early
    Windowed,
    /// a counter must keep moving
    Progress,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bite {
    /// nothing was heard in time
    Late,
    /// it was petted sooner than physically sensible
    Early,
    /// it is alive but not getting anywhere
    Stalled,
}

#[derive(Clone, Copy)]
pub struct Watchdog {
    pub name: &'static str,
    pub kind: Wd,
    /// fire if not petted within this
    pub deadline: Duration,
    /// for `Windowed`: petting sooner than this also fires
    pub floor: Duration,
    last_pet: Instant,
    last_progress: u64,
    /// for `Progress`: how long the counter may stand still
    pub stall_grace: Duration,
    last_progress_at: Instant,
    fired: Option<Bite>,
    /// how many times this has fired in the body's life — a watchdog that
    /// has fired once is a fault, a watchdog that fires regularly is a
    /// design error, and the difference should be visible
    pub bites: u32,
}

impl Watchdog {
    pub fn liveness(name: &'static str, deadline: Duration, now: Instant) -> Watchdog {
        Watchdog {
            name,
            kind: Wd::Liveness,
            deadline,
            floor: Duration::ZERO,
            last_pet: now,
            last_progress: 0,
            stall_grace: Duration::FOREVER,
            last_progress_at: now,
            fired: None,
            bites: 0,
        }
    }

    pub fn windowed(
        name: &'static str,
        floor: Duration,
        deadline: Duration,
        now: Instant,
    ) -> Watchdog {
        Watchdog {
            kind: Wd::Windowed,
            floor,
            ..Watchdog::liveness(name, deadline, now)
        }
    }

    pub fn progress(name: &'static str, stall_grace: Duration, now: Instant) -> Watchdog {
        Watchdog {
            kind: Wd::Progress,
            stall_grace,
            deadline: Duration::FOREVER,
            ..Watchdog::liveness(name, Duration::FOREVER, now)
        }
    }

    pub fn has_fired(&self) -> bool {
        self.fired.is_some()
    }

    pub fn bite(&self) -> Option<Bite> {
        self.fired
    }

    /// Pet it. For a `Progress` watchdog, pass the counter.
    pub fn pet(&mut self, now: Instant, progress: u64) {
        if self.fired.is_some() {
            // a fired watchdog cannot be petted back to life by the thing
            // that failed; only `clear` from above resets it
            return;
        }
        match self.kind {
            Wd::Windowed => {
                let since = now.since(self.last_pet);
                if since < self.floor {
                    self.fired = Some(Bite::Early);
                    self.bites = self.bites.saturating_add(1);
                    return;
                }
            }
            Wd::Progress => {
                if progress > self.last_progress {
                    self.last_progress = progress;
                    self.last_progress_at = now;
                }
            }
            Wd::Liveness => {}
        }
        self.last_pet = now;
    }

    /// Check without petting. Called from whatever runs regardless of the
    /// thing being watched — on a real board, a hardware timer ISR.
    pub fn check(&mut self, now: Instant) -> Option<Bite> {
        if let Some(b) = self.fired {
            return Some(b);
        }
        let bite = match self.kind {
            Wd::Liveness | Wd::Windowed => {
                if now.since(self.last_pet) > self.deadline {
                    Some(Bite::Late)
                } else {
                    None
                }
            }
            Wd::Progress => {
                if now.since(self.last_progress_at) > self.stall_grace {
                    Some(Bite::Stalled)
                } else {
                    None
                }
            }
        };
        if let Some(b) = bite {
            self.fired = Some(b);
            self.bites = self.bites.saturating_add(1);
        }
        bite
    }

    /// Reset. Only ever called from a layer above the one being watched.
    pub fn clear(&mut self, now: Instant) {
        self.fired = None;
        self.last_pet = now;
        self.last_progress_at = now;
    }

    pub fn describe(&self) -> Text<96> {
        let mut t = Text::new();
        t.push_str(self.name);
        t.push_str(match self.fired {
            None => " ok",
            Some(Bite::Late) => " LATE",
            Some(Bite::Early) => " EARLY",
            Some(Bite::Stalled) => " STALLED",
        });
        if self.bites > 0 {
            t.push_str(" bites=");
            t.push_u64(self.bites as u64);
        }
        t
    }
}

/// Every watchdog on a body, checked together.
///
/// The set is checked as a unit because the response is a unit: any bite
/// stops the body. There is no "this one is less important" — a watchdog
/// nobody acts on is a watchdog that should be deleted, so that the ones
/// that remain mean something.
pub struct WatchdogSet<const N: usize> {
    dogs: [Option<Watchdog>; N],
    count: usize,
}

impl<const N: usize> Default for WatchdogSet<N> {
    fn default() -> WatchdogSet<N> {
        WatchdogSet::new()
    }
}

impl<const N: usize> WatchdogSet<N> {
    pub fn new() -> WatchdogSet<N> {
        WatchdogSet {
            dogs: [None; N],
            count: 0,
        }
    }

    pub fn add(&mut self, wd: Watchdog) -> bool {
        if self.count >= N {
            return false;
        }
        self.dogs[self.count] = Some(wd);
        self.count += 1;
        true
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Watchdog> {
        self.dogs
            .iter_mut()
            .flatten()
            .find(|d| d.name == name)
    }

    pub fn get(&self, name: &str) -> Option<&Watchdog> {
        self.dogs.iter().flatten().find(|d| d.name == name)
    }

    pub fn pet(&mut self, name: &str, now: Instant, progress: u64) -> bool {
        match self.get_mut(name) {
            Some(d) => {
                d.pet(now, progress);
                true
            }
            None => false,
        }
    }

    /// Check every watchdog. Returns the first that has fired — first in
    /// registration order, which is the order the body's designer chose
    /// and therefore the order that best explains a cascade.
    pub fn check(&mut self, now: Instant) -> Option<(&'static str, Bite)> {
        let mut first: Option<(&'static str, Bite)> = None;
        for d in self.dogs.iter_mut().flatten() {
            if let Some(b) = d.check(now) {
                if first.is_none() {
                    first = Some((d.name, b));
                }
            }
        }
        first
    }

    pub fn any_fired(&self) -> bool {
        self.dogs.iter().flatten().any(|d| d.has_fired())
    }

    pub fn clear_all(&mut self, now: Instant) {
        for d in self.dogs.iter_mut().flatten() {
            d.clear(now);
        }
    }
}
