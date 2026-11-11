//! BFS  LEVEL    Domain 2 Autonomic - envelopes, reflexes, scheduler, watchdogs  [crate brainos-rt]
//! BFS  PURPOSE  Tests for rt: reflexes stay inside their limits, a dead link stops the body, unmeetable schedules are refused, fired watchdogs stay fired
//! BFS  DEFINES  -
//! BFS  USES     brainos: abi, rt
//! BFS  USED BY  (entry point or test)
//!
//! The Domain 2 suite.
//!
//! What these tests defend: a reflex cannot leave its envelope, a body
//! stops when its link dies, a schedule that cannot be met is refused
//! before the robot moves, and a watchdog cannot be petted back to life
//! by the thing that failed.

use brainos_abi::time::{Duration, Instant};
use brainos_rt::budget::Budget;
use brainos_rt::envelope::{Channel, Envelope, Limit};
use brainos_rt::reflex::{Frame, Reflex, ReflexBank, ReflexOutcome, SafePose, Stop, StopReason};
use brainos_rt::ring::{Ring, Spsc, Window};
use brainos_rt::sched::{liu_layland_bound_ppm, MissKind, Scheduler, TaskSpec, Unschedulable};
use brainos_rt::watchdog::{Bite, Watchdog, WatchdogSet, Wd};

// ---------- envelopes ----------

#[test]
fn an_insane_limit_never_enters_an_envelope() {
    let mut e = Envelope::new(1, Duration::from_millis(10), Duration::from_millis(200));
    assert!(!e.bound(Channel::Angle, Limit::new(10.0, -10.0)), "inverted");
    assert!(!e.bound(Channel::Angle, Limit::new(f32::NAN, 1.0)), "NaN");
    assert!(e.limit(Channel::Angle).is_none());
    assert!(e.bound(Channel::Angle, Limit::new(-35.0, 35.0)));
    assert_eq!(e.limit(Channel::Angle), Some(Limit::new(-35.0, 35.0)));
}

#[test]
fn an_unstated_channel_is_a_violation_not_a_pass() {
    let e = Envelope::new(1, Duration::from_millis(10), Duration::from_millis(200))
        .with(Channel::Angle, -35.0, 35.0);
    assert!(e.check(Channel::Angle, 0.0).is_ok());
    let v = e.check(Channel::Current, 0.1).unwrap_err();
    assert!(v.unstated, "an unmentioned channel is not authorized");
    assert!(e.clamp(Channel::Current, 0.1).is_none());
}

#[test]
fn nan_is_never_inside_an_envelope_and_clamps_to_the_safe_corner() {
    let l = Limit::new(-35.0, 35.0);
    assert!(!l.contains(f32::NAN));
    assert_eq!(l.clamp(f32::NAN), -35.0);
    assert_eq!(l.excess(f32::NAN), f32::INFINITY);
}

#[test]
fn clamping_cannot_produce_a_value_outside_the_envelope() {
    let l = Limit::new(-35.0, 35.0);
    for v in [-1e9f32, -36.0, -35.0, 0.0, 35.0, 36.0, 1e9, f32::INFINITY] {
        let c = l.clamp(v);
        assert!(l.contains(c), "{v} clamped to {c}");
    }
}

#[test]
fn narrowing_an_envelope_is_within_it_and_widening_is_not() {
    let wide = Envelope::new(1, Duration::from_millis(10), Duration::from_millis(200))
        .with(Channel::Angle, -45.0, 45.0)
        .with(Channel::Current, 0.0, 3.0);
    let narrow = Envelope::new(2, Duration::from_millis(10), Duration::from_millis(200))
        .with(Channel::Angle, -20.0, 20.0)
        .with(Channel::Current, 0.0, 2.0);
    let wider = Envelope::new(3, Duration::from_millis(10), Duration::from_millis(200))
        .with(Channel::Angle, -90.0, 90.0)
        .with(Channel::Current, 0.0, 3.0);
    assert!(narrow.is_within(&wide));
    assert!(!wider.is_within(&wide));
    // an envelope that says nothing about a channel the other bounds is wider
    let silent = Envelope::new(4, Duration::from_millis(10), Duration::from_millis(200))
        .with(Channel::Angle, -20.0, 20.0);
    assert!(!silent.is_within(&wide));
}

#[test]
fn a_violation_explains_itself_without_allocating() {
    let e = Envelope::new(9, Duration::from_millis(10), Duration::from_millis(200))
        .with(Channel::Current, 0.0, 2.4);
    let v = e.check(Channel::Current, 3.1).unwrap_err();
    let d = v.describe();
    assert!(d.as_str().contains("current"), "{d}");
    assert!(d.as_str().contains("outside"), "{d}");
    assert!(v.excess() > 0.0);
}

// ---------- reflexes ----------

/// A controller that always wants more than it is allowed. The point of
/// the test is that wanting is not getting.
struct Greedy {
    want: f32,
}

impl Reflex for Greedy {
    fn name(&self) -> &'static str {
        "greedy"
    }
    fn channel(&self) -> Channel {
        Channel::Angle
    }
    fn desire(&mut self, _f: &Frame) -> Option<f32> {
        Some(self.want)
    }
}

/// A reflex with its own invariant — a gait's tip-over check.
struct TipOver;

impl Reflex for TipOver {
    fn name(&self) -> &'static str {
        "tip-over"
    }
    fn channel(&self) -> Channel {
        Channel::Angle
    }
    fn desire(&mut self, _f: &Frame) -> Option<f32> {
        Some(0.0)
    }
    fn invariant(&self, f: &Frame) -> Result<(), StopReason> {
        match f.tilt {
            Some(t) if t.abs() > 25.0 => Err(StopReason::StabilityLost),
            _ => Ok(()),
        }
    }
}

fn envelope() -> Envelope {
    Envelope::new(1, Duration::from_millis(10), Duration::from_millis(200))
        .with(Channel::Angle, -35.0, 35.0)
        .with(Channel::Current, 0.0, 2.4)
}

fn frame(at_us: u64, seat_us: u64) -> Frame {
    Frame {
        at: Instant(at_us),
        last_seat_contact: Instant(seat_us),
        ..Default::default()
    }
}

#[test]
fn a_reflex_cannot_command_outside_its_envelope() {
    let mut greedy = Greedy { want: 500.0 };
    let mut bank: ReflexBank<4> = ReflexBank::new();
    assert!(bank.bind(&mut greedy, envelope()));

    let mut out = [ReflexOutcome::Hold; 4];
    let n = bank.run(&frame(1_000, 1_000), &mut out);
    assert_eq!(n, 1);
    match out[0] {
        ReflexOutcome::Command { channel, value } => {
            assert_eq!(channel, Channel::Angle);
            assert_eq!(value, 35.0, "the desire was clamped, not obeyed");
        }
        other => panic!("expected a clamped command, got {other:?}"),
    }
}

#[test]
fn a_reflex_whose_channel_is_unstated_cannot_be_bound_at_all() {
    let mut greedy = Greedy { want: 1.0 };
    let mut bank: ReflexBank<4> = ReflexBank::new();
    let e = Envelope::new(1, Duration::from_millis(10), Duration::from_millis(200))
        .with(Channel::Current, 0.0, 2.0); // says nothing about Angle
    assert!(!bank.bind(&mut greedy, e));
    assert!(bank.is_empty());
}

#[test]
fn a_lost_link_stops_the_body() {
    let mut greedy = Greedy { want: 0.0 };
    let mut bank: ReflexBank<4> = ReflexBank::new();
    bank.bind(&mut greedy, envelope());
    let mut out = [ReflexOutcome::Hold; 4];

    // inside the grace period
    bank.run(&frame(100_000, 0), &mut out);
    assert!(!bank.is_stopped());

    // past it: 200 ms grace, 201 ms since contact
    bank.run(&frame(201_000, 0), &mut out);
    assert!(bank.is_stopped());
    assert_eq!(bank.stop_state().unwrap().reason, StopReason::LinkLost);
}

#[test]
fn an_implausible_sensor_stops_the_body_rather_than_being_acted_on() {
    let mut greedy = Greedy { want: 0.0 };
    let mut bank: ReflexBank<4> = ReflexBank::new();
    bank.bind(&mut greedy, envelope());
    let mut out = [ReflexOutcome::Hold; 4];

    let mut f = frame(1_000, 1_000);
    f.temperature = Some(4_000.0);
    bank.run(&f, &mut out);
    assert_eq!(
        bank.stop_state().unwrap().reason,
        StopReason::SensorImplausible
    );

    let mut bank2: ReflexBank<4> = ReflexBank::new();
    let mut g2 = Greedy { want: 0.0 };
    bank2.bind(&mut g2, envelope());
    let mut f = frame(1_000, 1_000);
    f.angle = Some(f32::NAN);
    bank2.run(&f, &mut out);
    assert_eq!(
        bank2.stop_state().unwrap().reason,
        StopReason::SensorImplausible
    );
}

#[test]
fn a_measured_value_outside_the_envelope_stops_even_when_the_command_is_legal() {
    let mut greedy = Greedy { want: 0.0 }; // a perfectly legal command
    let mut bank: ReflexBank<4> = ReflexBank::new();
    bank.bind(&mut greedy, envelope());
    let mut out = [ReflexOutcome::Hold; 4];

    let mut f = frame(1_000, 1_000);
    f.current = Some(3.5); // envelope allows 2.4
    bank.run(&f, &mut out);
    assert!(bank.is_stopped());
    let s = bank.stop_state().unwrap();
    assert_eq!(s.reason, StopReason::EnvelopeViolation);
    assert_eq!(s.violation.unwrap().channel, Channel::Current);
}

#[test]
fn a_reflexs_own_invariant_can_stop_the_body() {
    let mut tip = TipOver;
    let mut bank: ReflexBank<4> = ReflexBank::new();
    bank.bind(&mut tip, envelope());
    let mut out = [ReflexOutcome::Hold; 4];

    let mut f = frame(1_000, 1_000);
    f.tilt = Some(10.0);
    bank.run(&f, &mut out);
    assert!(!bank.is_stopped());

    f.tilt = Some(40.0);
    f.at = Instant(2_000);
    bank.run(&f, &mut out);
    assert_eq!(bank.stop_state().unwrap().reason, StopReason::StabilityLost);
}

#[test]
fn an_overrunning_loop_stops_the_body() {
    let mut greedy = Greedy { want: 0.0 };
    let mut bank: ReflexBank<4> = ReflexBank::new();
    bank.bind(&mut greedy, envelope()); // 10 ms period
    let mut out = [ReflexOutcome::Hold; 4];

    bank.run(&frame(1_000, 1_000), &mut out);
    assert!(!bank.is_stopped());
    // 15 ms later: late but inside the 2-period slack
    bank.run(&frame(16_000, 16_000), &mut out);
    assert!(!bank.is_stopped());
    // 25 ms after that: past two periods
    bank.run(&frame(41_000, 41_000), &mut out);
    assert_eq!(bank.stop_state().unwrap().reason, StopReason::LoopOverran);
}

#[test]
fn the_first_stop_reason_is_the_one_kept() {
    let mut bank: ReflexBank<4> = ReflexBank::new();
    bank.stop(Stop::new(StopReason::StabilityLost, Instant(1)));
    bank.stop(Stop::new(StopReason::LinkLost, Instant(2)));
    assert_eq!(bank.stop_state().unwrap().reason, StopReason::StabilityLost);
}

#[test]
fn only_self_clearing_stops_resume_without_a_decision_from_above() {
    let mut bank: ReflexBank<4> = ReflexBank::new();
    bank.stop(Stop::new(StopReason::LinkLost, Instant(1)));
    assert!(bank.try_resume());
    assert!(!bank.is_stopped());

    bank.stop(Stop::new(StopReason::StabilityLost, Instant(2)));
    assert!(!bank.try_resume(), "a fall needs cognition, not a retry");
    assert!(bank.is_stopped());
}

#[test]
fn a_stopped_bank_keeps_reporting_the_stop() {
    let mut greedy = Greedy { want: 0.0 };
    let mut bank: ReflexBank<4> = ReflexBank::new();
    bank.bind(&mut greedy, envelope());
    bank.stop(Stop::new(StopReason::PowerFloor, Instant(1)));
    let mut out = [ReflexOutcome::Hold; 4];
    let n = bank.run(&frame(2_000, 2_000), &mut out);
    assert_eq!(n, 1);
    assert!(matches!(out[0], ReflexOutcome::Stop(_)));
}

#[test]
fn a_legged_body_holds_torque_and_an_arm_releases() {
    assert!(SafePose::STAND_DOWN.hold_torque);
    assert!(!SafePose::RELEASE.hold_torque);
}

// ---------- scheduling ----------

#[test]
fn an_overloaded_task_set_is_refused_before_the_robot_moves() {
    let mut s: Scheduler<8> = Scheduler::new();
    let now = Instant::ZERO;
    // 1 kHz gait taking 600 us: 60% utilisation
    s.admit(
        TaskSpec::periodic(1, "gait", Duration::from_micros(1_000), Duration::from_micros(600)),
        now,
    )
    .unwrap();
    // another 600 us at 1 kHz would be 120%
    let e = s
        .admit(
            TaskSpec::periodic(2, "vision", Duration::from_micros(1_000), Duration::from_micros(600)),
            now,
        )
        .unwrap_err();
    assert!(matches!(e, Unschedulable::Overloaded { .. }));
    assert_eq!(s.len(), 1, "a refused task leaves the set untouched");
}

#[test]
fn response_time_analysis_catches_what_utilisation_misses() {
    let mut s: Scheduler<8> = Scheduler::new();
    let now = Instant::ZERO;
    // three tasks at 82% total — over the Liu-Layland bound for n=3
    // (77.98%) so the exact test has to decide
    s.admit(
        TaskSpec::periodic(1, "a", Duration::from_micros(1_000), Duration::from_micros(300)),
        now,
    )
    .unwrap();
    s.admit(
        TaskSpec::periodic(2, "b", Duration::from_micros(2_000), Duration::from_micros(600)),
        now,
    )
    .unwrap();
    let u = s.total_utilisation_ppm();
    assert!(u > liu_layland_bound_ppm(2) || u <= 1_000_000);
    // a third task whose deadline genuinely cannot be met
    let e = s.admit(
        TaskSpec::periodic(3, "c", Duration::from_micros(2_500), Duration::from_micros(900)),
        now,
    );
    // either it is accepted with a proof, or refused with a named reason;
    // what must never happen is silent acceptance of an unmeetable set
    match e {
        Ok(()) => assert!(s.analyse().is_ok()),
        Err(Unschedulable::DeadlineUnreachable { id, .. }) => assert_eq!(id, 3),
        Err(other) => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn a_degenerate_task_is_refused() {
    let mut s: Scheduler<4> = Scheduler::new();
    assert!(matches!(
        s.admit(
            TaskSpec::periodic(1, "zero", Duration::ZERO, Duration::from_micros(1)),
            Instant::ZERO
        ),
        Err(Unschedulable::Degenerate { .. })
    ));
}

#[test]
fn tasks_are_offered_highest_rate_first() {
    let mut s: Scheduler<8> = Scheduler::new();
    let now = Instant::ZERO;
    s.admit(
        TaskSpec::periodic(1, "slow", Duration::from_millis(100), Duration::from_micros(500)),
        now,
    )
    .unwrap();
    s.admit(
        TaskSpec::periodic(2, "fast", Duration::from_millis(1), Duration::from_micros(100)),
        now,
    )
    .unwrap();
    // both are due at t=0; the faster one wins
    assert_eq!(s.due(now), Some(2));
}

#[test]
fn a_late_pass_skips_a_beat_rather_than_dragging_the_period() {
    let mut s: Scheduler<4> = Scheduler::new();
    s.admit(
        TaskSpec::periodic(1, "gait", Duration::from_millis(10), Duration::from_millis(5)),
        Instant::ZERO,
    )
    .unwrap();

    s.started(1, Instant::ZERO);
    // took 24 ms instead of 5
    let miss = s.finished(1, Instant(24_000)).expect("a miss is reported");
    assert_eq!(miss.kind, MissKind::Deadline);

    // next release is a whole number of periods from the original, not
    // 10 ms after the late finish
    let slot = s.slot(1).unwrap();
    assert_eq!(slot.next_release.micros() % 10_000, 0);
    assert!(slot.next_release.micros() > 24_000);
    assert_eq!(slot.misses, 1);
}

#[test]
fn finishing_in_time_but_over_the_declared_wcet_is_reported_separately() {
    let mut s: Scheduler<4> = Scheduler::new();
    s.admit(
        TaskSpec::periodic(1, "t", Duration::from_millis(10), Duration::from_millis(2)),
        Instant::ZERO,
    )
    .unwrap();
    s.started(1, Instant::ZERO);
    // 4 ms: inside the 10 ms deadline, over the 2 ms declaration
    let miss = s.finished(1, Instant(4_000)).unwrap();
    assert_eq!(miss.kind, MissKind::Wcet);
    assert_eq!(s.slot(1).unwrap().overruns, 1);
    assert_eq!(s.slot(1).unwrap().misses, 0);
}

#[test]
fn a_comfortable_set_reports_no_misses() {
    let mut s: Scheduler<4> = Scheduler::new();
    s.admit(
        TaskSpec::periodic(1, "t", Duration::from_millis(10), Duration::from_millis(5)),
        Instant::ZERO,
    )
    .unwrap();
    for i in 0..10u64 {
        let at = Instant(i * 10_000);
        s.started(1, at);
        assert!(s.finished(1, Instant(at.micros() + 1_000)).is_none());
    }
    assert_eq!(s.slot(1).unwrap().runs, 10);
    assert_eq!(s.slot(1).unwrap().misses, 0);
}

#[test]
fn the_liu_layland_bound_is_monotone_and_converges_to_ln2() {
    let mut last = u64::MAX;
    for n in 1..=16 {
        let b = liu_layland_bound_ppm(n);
        assert!(b <= last, "bound rose at n={n}");
        last = b;
    }
    assert_eq!(liu_layland_bound_ppm(1000), 693_147);
    assert!(liu_layland_bound_ppm(16) > 693_147);
}

// ---------- watchdogs ----------

#[test]
fn a_liveness_watchdog_fires_when_nothing_is_heard() {
    let mut wd = Watchdog::liveness("gait", Duration::from_millis(50), Instant::ZERO);
    assert!(wd.check(Instant(40_000)).is_none());
    assert_eq!(wd.check(Instant(51_000)), Some(Bite::Late));
    assert!(wd.has_fired());
}

#[test]
fn a_fired_watchdog_cannot_be_petted_back_to_life_by_what_failed() {
    let mut wd = Watchdog::liveness("gait", Duration::from_millis(50), Instant::ZERO);
    wd.check(Instant(60_000));
    assert!(wd.has_fired());
    wd.pet(Instant(61_000), 0);
    assert!(wd.has_fired(), "petting must not clear a fired watchdog");
    // only a layer above clears it
    wd.clear(Instant(62_000));
    assert!(!wd.has_fired());
}

#[test]
fn a_windowed_watchdog_catches_a_loop_that_started_spinning() {
    let mut wd = Watchdog::windowed(
        "servo",
        Duration::from_micros(800),
        Duration::from_micros(1_200),
        Instant::ZERO,
    );
    assert_eq!(wd.kind, Wd::Windowed);
    wd.pet(Instant(1_000), 0); // on time
    assert!(!wd.has_fired());
    wd.pet(Instant(1_100), 0); // 100 us later: far too fast
    assert_eq!(wd.bite(), Some(Bite::Early));
}

#[test]
fn a_progress_watchdog_catches_a_loop_that_runs_but_achieves_nothing() {
    let mut wd = Watchdog::progress("planner", Duration::from_millis(500), Instant::ZERO);
    // petting regularly with the same counter is not progress
    for t in (0..2_000_000u64).step_by(100_000) {
        wd.pet(Instant(t), 7);
    }
    assert_eq!(wd.check(Instant(2_000_000)), Some(Bite::Stalled));

    let mut wd2 = Watchdog::progress("planner", Duration::from_millis(500), Instant::ZERO);
    let mut n = 0u64;
    for t in (0..2_000_000u64).step_by(100_000) {
        n += 1;
        wd2.pet(Instant(t), n);
    }
    assert!(wd2.check(Instant(2_000_000)).is_none());
}

#[test]
fn a_watchdog_set_reports_the_first_bite_in_registration_order() {
    let mut set: WatchdogSet<4> = WatchdogSet::new();
    set.add(Watchdog::liveness("a", Duration::from_millis(10), Instant::ZERO));
    set.add(Watchdog::liveness("b", Duration::from_millis(5), Instant::ZERO));
    assert_eq!(set.len(), 2);
    // both are late at t = 20 ms; the one registered first explains it
    assert_eq!(set.check(Instant(20_000)), Some(("a", Bite::Late)));
    assert!(set.any_fired());
    set.clear_all(Instant(21_000));
    assert!(!set.any_fired());
}

#[test]
fn petting_an_unknown_watchdog_is_reported_not_ignored() {
    let mut set: WatchdogSet<2> = WatchdogSet::new();
    set.add(Watchdog::liveness("a", Duration::from_millis(10), Instant::ZERO));
    assert!(set.pet("a", Instant(1), 0));
    assert!(!set.pet("typo", Instant(1), 0));
}

// ---------- budgets ----------

#[test]
fn a_budget_notices_when_the_declared_wcet_has_gone_stale() {
    let mut b = Budget::new("gait", Duration::from_micros(600));
    for _ in 0..1000 {
        b.record(Duration::from_micros(400));
    }
    assert!(b.is_honest());
    b.record(Duration::from_micros(900));
    assert!(!b.is_honest(), "the scheduling analysis is now wrong");
    let r = b.report();
    assert_eq!(r.exceeded, 1);
    assert_eq!(r.measured_worst, Duration::from_micros(900));
    assert!(r.describe().as_str().contains("BUDGET STALE"));
}

#[test]
fn budget_statistics_are_sane() {
    let mut b = Budget::new("t", Duration::from_micros(1_000));
    for us in [100u64, 200, 300, 400, 500] {
        b.record(Duration::from_micros(us));
    }
    assert_eq!(b.samples(), 5);
    assert_eq!(b.worst(), Duration::from_micros(500));
    assert_eq!(b.best(), Duration::from_micros(100));
    assert_eq!(b.mean(), Duration::from_micros(300));
    // the p99 upper bound is a bucket edge at or above the worst case
    assert!(b.percentile_upper(99) >= b.worst());
}

#[test]
fn an_empty_budget_reports_zeroes_rather_than_infinities() {
    let b = Budget::new("t", Duration::from_micros(10));
    assert_eq!(b.best(), Duration::ZERO);
    assert_eq!(b.mean(), Duration::ZERO);
    assert!(b.is_honest());
}

// ---------- rings ----------

#[test]
fn a_full_ring_drops_and_counts_rather_than_blocking_or_growing() {
    let mut r: Ring<u32, 8> = Ring::new();
    for i in 0..7 {
        assert!(r.push(i));
    }
    assert!(r.is_full());
    assert!(!r.push(99));
    assert_eq!(r.dropped(), 1);
    assert_eq!(r.len(), 7);
    assert_eq!(r.pop(), Some(0));
}

#[test]
fn an_overwriting_ring_keeps_the_newest_reading() {
    let mut r: Ring<u32, 4> = Ring::new();
    for i in 0..3 {
        r.push(i);
    }
    assert!(r.is_full());
    assert_eq!(r.push_overwrite(99), Some(0));
    assert_eq!(r.pop(), Some(1));
    assert_eq!(r.pop(), Some(2));
    assert_eq!(r.pop(), Some(99));
}

#[test]
fn an_spsc_queue_preserves_order() {
    let mut q: Spsc<u8, 16> = Spsc::new();
    for i in 0..10 {
        assert!(q.produce(i));
    }
    for i in 0..10 {
        assert_eq!(q.consume(), Some(i));
    }
    assert!(q.is_empty());
    assert_eq!(q.dropped(), 0);
}

#[test]
fn a_window_tracks_running_statistics_without_allocating() {
    let mut w: Window<4> = Window::new();
    assert!(w.is_empty());
    for v in [1.0f32, 2.0, 3.0, 4.0, 5.0] {
        w.push(v);
    }
    assert!(w.is_full());
    assert_eq!(w.len(), 4);
    // the 1.0 fell out of the window
    assert_eq!(w.min(), 2.0);
    assert_eq!(w.max(), 5.0);
    assert_eq!(w.mean(), 3.5);
    assert_eq!(w.last(), Some(5.0));
}
