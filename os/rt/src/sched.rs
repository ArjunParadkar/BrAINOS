//! BFS  LEVEL    Domain 2 Autonomic - envelopes, reflexes, scheduler, watchdogs  [crate brainos-rt]
//! BFS  PURPOSE  Fixed-priority scheduler for periodic loops: refuses task sets that cannot meet their deadlines and reports every deadline missed
//! BFS  DEFINES  TaskId, Tick, TaskSpec, Unschedulable, Slot, Scheduler, MissKind, Miss, liu_llayland_bound_ppm_table, liu_layland_bound_ppm
//! BFS  USES     brainos: abi
//! BFS  USED BY  nostd/lib, rt/lib, rt/tests/rt_suite
//!
//! A deterministic real-time scheduler.
//!
//! UNIX schedules processes, fairly, so that forty strangers share a
//! machine. An entity in a body has one self and a handful of loops with
//! hard periods, and fairness is not a property it wants — a gait loop
//! that gets its fair share instead of its deadline produces a limp.
//!
//! So: fixed-priority, rate-monotonic, non-preemptive within a pass.
//! Tasks declare a period and a worst-case execution time; the scheduler
//! runs whatever is due, highest rate first, and reports every missed
//! deadline rather than quietly falling behind. There is no dynamic
//! priority, no ageing, no heuristics — the whole point is that the
//! behaviour is predictable enough to argue about before the robot moves.
//!
//! **Schedulability is checked, not hoped for.** `Scheduler::analyse`
//! runs the Liu–Layland utilisation bound and, when that is
//! inconclusive, exact response-time analysis. A task set that does not
//! pass is refused at construction. This is the difference between "it
//! seemed to keep up on the bench" and "it cannot miss".

use brainos_abi::text::Text;
use brainos_abi::time::{Duration, Instant};

pub type TaskId = u16;
/// A scheduler pass number, distinct from the entity's cognition tick.
pub type Tick = u64;

/// What a task promises about itself.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TaskSpec {
    pub id: TaskId,
    pub name: &'static str,
    /// how often it must run
    pub period: Duration,
    /// the longest it may take. Measured, not guessed: `Budget` in the
    /// sibling module is what turns this from a promise into a fact.
    pub wcet: Duration,
    /// deadline relative to release; defaults to the period
    pub deadline: Duration,
    /// a task whose overrun stops the body rather than being reported
    pub critical: bool,
}

impl TaskSpec {
    pub fn periodic(id: TaskId, name: &'static str, period: Duration, wcet: Duration) -> TaskSpec {
        TaskSpec {
            id,
            name,
            period,
            wcet,
            deadline: period,
            critical: false,
        }
    }

    pub fn critical(mut self) -> TaskSpec {
        self.critical = true;
        self
    }

    pub fn with_deadline(mut self, d: Duration) -> TaskSpec {
        self.deadline = d;
        self
    }

    /// Utilisation contributed to the processor, in parts per million —
    /// integer arithmetic, because a scheduler that depends on float
    /// rounding to decide schedulability is not a guarantee.
    pub fn utilisation_ppm(&self) -> u64 {
        if self.period.as_micros() == 0 {
            return u64::MAX;
        }
        self.wcet
            .as_micros()
            .saturating_mul(1_000_000)
            .saturating_div(self.period.as_micros())
    }
}

/// Why a task set was refused.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Unschedulable {
    /// total utilisation exceeds 100%: no scheduler can do it
    Overloaded { utilisation_ppm: u64 },
    /// response-time analysis says this task can miss its deadline
    DeadlineUnreachable { id: TaskId, response: Duration },
    /// a task declared a period or WCET of zero
    Degenerate { id: TaskId },
    /// more tasks than the scheduler has slots
    TooManyTasks,
}

/// One task's runtime state.
#[derive(Clone, Copy)]
pub struct Slot {
    pub spec: TaskSpec,
    pub next_release: Instant,
    pub last_start: Option<Instant>,
    pub last_duration: Duration,
    pub worst_duration: Duration,
    pub runs: u64,
    pub misses: u64,
    pub overruns: u64,
}

impl Slot {
    fn new(spec: TaskSpec, now: Instant) -> Slot {
        Slot {
            spec,
            next_release: now,
            last_start: None,
            last_duration: Duration::ZERO,
            worst_duration: Duration::ZERO,
            runs: 0,
            misses: 0,
            overruns: 0,
        }
    }
}

/// The scheduler. `N` slots, fixed at compile time.
pub struct Scheduler<const N: usize> {
    slots: [Option<Slot>; N],
    count: usize,
    pass: Tick,
    /// order of `slots` indices, shortest period first (rate-monotonic)
    order: [u8; N],
}

impl<const N: usize> Default for Scheduler<N> {
    fn default() -> Scheduler<N> {
        Scheduler::new()
    }
}

impl<const N: usize> Scheduler<N> {
    pub fn new() -> Scheduler<N> {
        Scheduler {
            slots: [const { None }; N],
            count: 0,
            pass: 0,
            order: [0u8; N],
        }
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn pass(&self) -> Tick {
        self.pass
    }

    /// Add a task and re-check schedulability. A task that breaks the set
    /// is rejected and the set is left exactly as it was — partial
    /// acceptance would leave a body running a schedule nobody checked.
    pub fn admit(&mut self, spec: TaskSpec, now: Instant) -> Result<(), Unschedulable> {
        if self.count >= N {
            return Err(Unschedulable::TooManyTasks);
        }
        if spec.period.as_micros() == 0 || spec.wcet.as_micros() == 0 {
            return Err(Unschedulable::Degenerate { id: spec.id });
        }

        self.slots[self.count] = Some(Slot::new(spec, now));
        self.count += 1;
        self.sort_rate_monotonic();

        match self.analyse() {
            Ok(()) => Ok(()),
            Err(e) => {
                // roll back completely
                self.count -= 1;
                self.slots[self.count] = None;
                self.sort_rate_monotonic();
                Err(e)
            }
        }
    }

    fn sort_rate_monotonic(&mut self) {
        // insertion sort over indices: N is small and this must be
        // allocation-free and deterministic
        let mut k = 0usize;
        for i in 0..self.count {
            self.order[k] = i as u8;
            k += 1;
        }
        for i in 1..self.count {
            let cur = self.order[i];
            let cur_period = self.slots[cur as usize]
                .as_ref()
                .map(|s| s.spec.period.as_micros())
                .unwrap_or(u64::MAX);
            let mut j = i;
            while j > 0 {
                let prev = self.order[j - 1];
                let prev_period = self.slots[prev as usize]
                    .as_ref()
                    .map(|s| s.spec.period.as_micros())
                    .unwrap_or(u64::MAX);
                if prev_period <= cur_period {
                    break;
                }
                self.order[j] = prev;
                j -= 1;
            }
            self.order[j] = cur;
        }
    }

    pub fn total_utilisation_ppm(&self) -> u64 {
        let mut u = 0u64;
        for s in self.slots.iter().flatten() {
            u = u.saturating_add(s.spec.utilisation_ppm());
        }
        u
    }

    /// Is this task set schedulable under rate-monotonic priorities?
    ///
    /// Two tests, in order of cost. Liu–Layland is a sufficient condition
    /// — if utilisation is under `n(2^(1/n) − 1)` the set is definitely
    /// schedulable — and it is cheap. When it fails, the set may still be
    /// fine, so exact response-time analysis runs: each task's response
    /// time is the fixed point of `R = C + Σ ceil(R/Tj)·Cj` over all
    /// higher-priority tasks, and the set is schedulable if every `R`
    /// lands inside its deadline.
    pub fn analyse(&self) -> Result<(), Unschedulable> {
        let u = self.total_utilisation_ppm();
        if u > 1_000_000 {
            return Err(Unschedulable::Overloaded {
                utilisation_ppm: u,
            });
        }
        if self.count == 0 {
            return Ok(());
        }
        if u <= liu_layland_bound_ppm(self.count) {
            return Ok(());
        }

        // exact response-time analysis, in priority order
        for i in 0..self.count {
            let idx = self.order[i] as usize;
            let Some(task) = self.slots[idx].as_ref() else {
                continue;
            };
            let c = task.spec.wcet.as_micros();
            let d = task.spec.deadline.as_micros();

            let mut r = c;
            // the recurrence is monotone and bounded by the deadline, so
            // it either converges or exceeds D; the iteration cap is
            // belt-and-braces against a pathological set
            for _ in 0..1_000 {
                let mut interference = 0u64;
                for j in 0..i {
                    let jdx = self.order[j] as usize;
                    if let Some(hp) = self.slots[jdx].as_ref() {
                        let tj = hp.spec.period.as_micros();
                        let cj = hp.spec.wcet.as_micros();
                        let jobs = r.div_ceil(tj);
                        interference = interference.saturating_add(jobs.saturating_mul(cj));
                    }
                }
                let next = c.saturating_add(interference);
                if next > d {
                    return Err(Unschedulable::DeadlineUnreachable {
                        id: task.spec.id,
                        response: Duration::from_micros(next),
                    });
                }
                if next == r {
                    break;
                }
                r = next;
            }
        }
        Ok(())
    }

    /// Which task should run now, if any. Highest rate first among those
    /// whose release time has arrived.
    pub fn due(&self, now: Instant) -> Option<TaskId> {
        for i in 0..self.count {
            let idx = self.order[i] as usize;
            if let Some(s) = self.slots[idx].as_ref() {
                if now.has_passed(s.next_release) {
                    return Some(s.spec.id);
                }
            }
        }
        None
    }

    fn slot_mut(&mut self, id: TaskId) -> Option<&mut Slot> {
        self.slots
            .iter_mut()
            .flatten()
            .find(|s| s.spec.id == id)
    }

    pub fn slot(&self, id: TaskId) -> Option<&Slot> {
        self.slots.iter().flatten().find(|s| s.spec.id == id)
    }

    /// Mark a task as started.
    pub fn started(&mut self, id: TaskId, now: Instant) {
        if let Some(s) = self.slot_mut(id) {
            s.last_start = Some(now);
        }
    }

    /// Mark a task as finished. Returns a report if it missed its
    /// deadline or overran its declared WCET.
    ///
    /// Note what happens to `next_release`: it advances by whole periods
    /// from the previous release, not from *now*. Releasing relative to
    /// completion is the mistake that turns one late pass into permanent
    /// drift — the gait would slow down and stay slow.
    pub fn finished(&mut self, id: TaskId, now: Instant) -> Option<Miss> {
        let pass = self.pass;
        let s = self.slot_mut(id)?;
        let start = s.last_start.unwrap_or(now);
        let dur = now.since(start);
        s.last_duration = dur;
        if dur > s.worst_duration {
            s.worst_duration = dur;
        }
        s.runs += 1;

        let release = s.next_release;
        let deadline = release + s.spec.deadline;
        let mut miss = None;
        if now.has_passed(deadline) && now.since(deadline).as_micros() > 0 {
            s.misses += 1;
            miss = Some(Miss {
                id,
                name: s.spec.name,
                by: now.since(deadline),
                critical: s.spec.critical,
                kind: MissKind::Deadline,
            });
        } else if dur > s.spec.wcet {
            s.overruns += 1;
            miss = Some(Miss {
                id,
                name: s.spec.name,
                by: dur.saturating_sub(s.spec.wcet),
                critical: s.spec.critical,
                kind: MissKind::Wcet,
            });
        }

        // advance by whole periods until the next release is in the
        // future, so a long pass skips a beat rather than dragging
        let period = s.spec.period;
        let mut next = release + period;
        while !next.has_passed(now) && next.micros() <= now.micros() {
            next = next + period;
        }
        s.next_release = next;

        let _ = pass;
        miss
    }

    pub fn end_pass(&mut self) {
        self.pass = self.pass.wrapping_add(1);
    }

    /// A one-line health summary for telemetry.
    pub fn describe(&self) -> Text<192> {
        let mut t = Text::new();
        t.push_str("sched ");
        t.push_u64(self.count as u64);
        t.push_str(" tasks, util ");
        t.push_u64(self.total_utilisation_ppm() / 10_000);
        t.push_str("%, misses ");
        let mut misses = 0u64;
        for s in self.slots.iter().flatten() {
            misses += s.misses;
        }
        t.push_u64(misses);
        t
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MissKind {
    /// finished after its deadline
    Deadline,
    /// finished in time but took longer than it declared
    Wcet,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Miss {
    pub id: TaskId,
    pub name: &'static str,
    pub by: Duration,
    pub critical: bool,
    pub kind: MissKind,
}

/// The Liu–Layland bound `n(2^(1/n) − 1)`, in parts per million.
///
/// Computed by table rather than by `powf`, for three reasons: it is
/// exact at the values that matter, it needs no float math library on a
/// microcontroller, and a schedulability test that varies with the libm
/// build is not a test. Beyond the table the bound converges to ln 2.
pub fn liu_llayland_bound_ppm_table(n: usize) -> u64 {
    const TABLE: [u64; 17] = [
        1_000_000, // n = 0 — vacuous
        1_000_000, // 1
        828_427,   // 2
        779_763,   // 3
        756_828,   // 4
        743_492,   // 5
        734_772,   // 6
        728_627,   // 7
        724_062,   // 8
        720_538,   // 9
        717_735,   // 10
        715_456,   // 11
        713_567,   // 12
        711_974,   // 13
        710_614,   // 14
        709_439,   // 15
        708_413,   // 16
    ];
    match TABLE.get(n) {
        Some(v) => *v,
        // ln 2, the limit as n → ∞; always safe to use as the bound
        None => 693_147,
    }
}

pub fn liu_layland_bound_ppm(n: usize) -> u64 {
    liu_llayland_bound_ppm_table(n)
}
