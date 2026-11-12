//! BFS  LEVEL    Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)  [crate brainos-motor]
//! BFS  PURPOSE  Tests the three reflex arcs: each answers in the same tick it feels the event, the grip reflex never exceeds the granted force, and each fades or releases afterwards
//! BFS  DEFINES  -
//! BFS  USES     brainos: motor
//! BFS  USED BY  (entry point or test)
//!

use brainos_motor::reflex::{Arcs, Feel};

const HELD: Feel = Feel { holding: true, slip_mps: 0.0, touch_n: 0.0, touch_expected: true, off_path_m: 0.0 };

#[test]
fn slip_tightens_the_grip_in_the_tick_it_is_felt() {
    let mut a = Arcs::default();
    for _ in 0..100 {
        let r = a.step(&HELD, 6.0, 20.0, 1.0);
        assert_eq!(r.extra_squeeze_n, 0.0);
        assert!(!r.slip_fired);
    }
    let r = a.step(&Feel { slip_mps: 0.02, ..HELD }, 6.0, 20.0, 1.0);
    assert!(r.slip_fired);
    assert!(r.extra_squeeze_n >= 1.0, "the very tick that feels the slip squeezes harder: {}", r.extra_squeeze_n);
}

#[test]
fn the_grip_reflex_stops_at_the_granted_force() {
    let mut a = Arcs::default();
    let mut most = 0.0f32;
    for _ in 0..500 {
        let r = a.step(&Feel { slip_mps: 0.05, ..HELD }, 6.0, 9.0, 1.0);
        most = most.max(6.0 + r.extra_squeeze_n);
    }
    assert!(most <= 9.0 + 1e-6, "squeezed {most} N under a 9 N grant");
    assert!(most > 8.9, "and used what was granted");
    // already at the cap: nothing to add
    let mut a = Arcs::default();
    assert_eq!(a.step(&Feel { slip_mps: 0.05, ..HELD }, 9.0, 9.0, 1.0).extra_squeeze_n, 0.0);
}

#[test]
fn the_extra_grip_fades_once_nothing_slips_and_goes_when_the_hand_is_empty() {
    let mut a = Arcs::default();
    for _ in 0..10 {
        a.step(&Feel { slip_mps: 0.05, ..HELD }, 6.0, 20.0, 1.0);
    }
    let high = a.step(&HELD, 6.0, 20.0, 1.0).extra_squeeze_n;
    let mut later = high;
    for _ in 0..1000 {
        later = a.step(&HELD, 6.0, 20.0, 1.0).extra_squeeze_n;
    }
    assert!(later < high, "{high} then {later}");
    assert!(later > 0.0, "it fades over seconds, not at once");
    assert_eq!(a.step(&Feel { holding: false, ..HELD }, 0.0, 20.0, 1.0).extra_squeeze_n, 0.0);
}

#[test]
fn slip_means_nothing_when_nothing_is_held() {
    let mut a = Arcs::default();
    let r = a.step(&Feel { holding: false, slip_mps: 1.0, ..HELD }, 0.0, 20.0, 1.0);
    assert!(!r.slip_fired);
    assert_eq!(r.extra_squeeze_n, 0.0);
}

#[test]
fn an_unexpected_touch_softens_at_once_and_an_expected_one_does_not() {
    let free = Feel { holding: false, touch_expected: false, ..HELD };
    let mut a = Arcs::default();
    assert_eq!(a.step(&free, 0.0, 20.0, 1.0).stiff_scale, 1.0);
    let r = a.step(&Feel { touch_n: 4.0, ..free }, 0.0, 20.0, 1.0);
    assert!(r.yield_fired);
    assert!(r.stiff_scale <= 0.2, "soft in the same tick: {}", r.stiff_scale);
    assert_eq!(a.yields, 1);

    // the same force during a stroke that is meant to touch
    let mut a = Arcs::default();
    let r = a.step(&Feel { touch_n: 4.0, touch_expected: true, holding: false, ..HELD }, 0.0, 20.0, 1.0);
    assert!(!r.yield_fired);
    assert_eq!(r.stiff_scale, 1.0);
}

#[test]
fn after_yielding_the_body_firms_up_again_and_one_touch_is_counted_once() {
    let free = Feel { holding: false, touch_expected: false, ..HELD };
    let mut a = Arcs::default();
    // a touch that lasts 50 ms
    for _ in 0..50 {
        a.step(&Feel { touch_n: 4.0, ..free }, 0.0, 20.0, 1.0);
    }
    assert_eq!(a.yields, 1, "one touch, however long");
    let mut scale = 0.0;
    let mut back_at = None;
    for ms in 0..1000 {
        scale = a.step(&free, 0.0, 20.0, 1.0).stiff_scale;
        if scale >= 1.0 && back_at.is_none() {
            back_at = Some(ms);
        }
    }
    assert_eq!(scale, 1.0);
    let back = back_at.unwrap();
    assert!((200..=260).contains(&back), "firm again {back} ms after the touch ended");
}

#[test]
fn a_shove_off_the_path_holds_the_paths_clock_until_the_body_is_back() {
    let mut a = Arcs::default();
    assert!(!a.step(&Feel { off_path_m: 0.019, ..HELD }, 6.0, 20.0, 1.0).hold_path);
    let r = a.step(&Feel { off_path_m: 0.021, ..HELD }, 6.0, 20.0, 1.0);
    assert!(r.hold_path && r.wait_fired);
    assert!(!a.step(&Feel { off_path_m: 0.01, ..HELD }, 6.0, 20.0, 1.0).hold_path);
}
