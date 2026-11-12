//! BFS  LEVEL    Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)  [crate brainos-motor]
//! BFS  PURPOSE  Tests the path player (goals reached, overlapping strokes do not stop at corners, a touch ends a stroke where it happened, closing on nothing is reported) and the joint law
//! BFS  DEFINES  -
//! BFS  USES     brainos: motor
//! BFS  USED BY  (entry point or test)
//!

use brainos_motor::control::{impedance, minjerk, Gain, Goal, Played, Player, Style, Touch, DIMS};
use brainos_motor::primitive::{Contact, Until};

fn goal(x: f32, ms: u32, rest: bool) -> Goal {
    let mut at = [0.0; DIMS];
    at[0] = x;
    Goal { at, squeeze_n: 0.0, ms, stiff: 1.0, contact: Contact::Avoid, until: Until::Time, rest }
}

/// Play to the end; returns x at every millisecond.
fn play(goals: Vec<Goal>, style: Style) -> Vec<f32> {
    let mut p = Player::new([0.0; DIMS], goals, style);
    let mut xs = Vec::new();
    for _ in 0..20_000 {
        let (w, played) = p.step(1.0, &Touch::default());
        xs.push(w.at[0]);
        if played == Played::Done {
            return xs;
        }
    }
    panic!("never finished");
}

#[test]
fn minimum_jerk_starts_and_ends_flat() {
    assert_eq!(minjerk(0.0), 0.0);
    assert_eq!(minjerk(1.0), 1.0);
    assert!((minjerk(0.5) - 0.5).abs() < 1e-6);
    // no velocity at either end
    assert!(minjerk(0.001) < 1e-7);
    assert!(1.0 - minjerk(0.999) < 1e-6);
    // and it never goes backwards
    let mut last = 0.0;
    for k in 0..=1000 {
        let s = minjerk(k as f32 / 1000.0);
        assert!(s >= last - 1e-6, "{s} after {last}");
        last = s;
    }
    assert_eq!(minjerk(-3.0), 0.0);
    assert_eq!(minjerk(7.0), 1.0);
}

#[test]
fn every_goal_is_reached_whichever_way_strokes_are_joined() {
    for style in [Style::FLUID, Style::STOP_GO] {
        let xs = play(vec![goal(0.1, 400, false), goal(0.3, 400, false), goal(0.2, 400, true)], style);
        assert!((xs.last().unwrap() - 0.2).abs() < 1e-5, "{style:?} ended at {}", xs.last().unwrap());
        assert!(xs.iter().cloned().fold(0.0, f32::max) <= 0.3 + 1e-5, "{style:?} overshot");
    }
}

#[test]
fn overlapping_strokes_do_not_stop_at_a_corner_and_the_baseline_does() {
    let goals = || vec![goal(0.1, 500, false), goal(0.2, 500, false), goal(0.3, 500, true)];
    let slowest_between = |xs: &[f32]| {
        // the slowest millisecond while the path is between 0.05 and 0.25
        xs.windows(2)
            .filter(|w| w[0] > 0.05 && w[0] < 0.25)
            .map(|w| w[1] - w[0])
            .fold(f32::INFINITY, f32::min)
    };
    let fluid = play(goals(), Style::FLUID);
    let stop_go = play(goals(), Style::STOP_GO);
    assert!(slowest_between(&fluid) > 1e-5, "the fluid path stopped mid-way");
    assert!(slowest_between(&stop_go) < 1e-7, "the baseline did not stop");
    assert!(fluid.len() + 300 < stop_go.len(), "fluid {} ms, stop-go {} ms", fluid.len(), stop_go.len());
}

#[test]
fn a_touch_ends_the_stroke_where_it_happened() {
    let mut down = goal(0.10, 1000, true);
    down.until = Until::Touch(2.0);
    let mut p = Player::new([0.0; DIMS], vec![down, goal(0.0, 300, true)], Style::FLUID);
    let mut touched_at = None;
    let mut furthest = 0.0f32;
    for ms in 0..3000 {
        // the table is at x = 0.04
        let (w, played) = p.step(1.0, &Touch { tool_n: if furthest >= 0.04 { 3.0 } else { 0.0 }, grip_n: 0.0 });
        furthest = furthest.max(w.at[0]);
        if furthest >= 0.04 && touched_at.is_none() {
            touched_at = Some(ms);
        }
        if played == Played::Done {
            assert!(w.at[0].abs() < 1e-5, "came back to {}", w.at[0]);
            assert!(furthest < 0.0405, "kept pushing to {furthest} after the touch");
            assert!(touched_at.is_some());
            return;
        }
    }
    panic!("never finished");
}

#[test]
fn closing_on_nothing_is_reported_and_closing_on_something_is_not() {
    let mut close = goal(0.0, 300, true);
    close.until = Until::Gripped(2.0);
    close.squeeze_n = 6.0;
    let mut p = Player::new([0.0; DIMS], vec![close], Style::FLUID);
    let mut out = Played::Going;
    for _ in 0..2000 {
        out = p.step(1.0, &Touch::default()).1;
        if out != Played::Going {
            break;
        }
    }
    assert_eq!(out, Played::NothingGripped);

    let mut p = Player::new([0.0; DIMS], vec![close], Style::FLUID);
    let mut out = Played::Going;
    for ms in 0..2000 {
        out = p.step(1.0, &Touch { tool_n: 0.0, grip_n: if ms > 200 { 3.0 } else { 0.0 } }).1;
        if out != Played::Going {
            break;
        }
    }
    assert_eq!(out, Played::Done);
}

#[test]
fn the_softer_of_two_overlapping_strokes_governs() {
    let mut soft = goal(0.2, 400, true);
    soft.stiff = 0.3;
    let mut p = Player::new([0.0; DIMS], vec![goal(0.1, 400, false), soft], Style::FLUID);
    let mut seen_soft_while_first_unfinished = false;
    for ms in 0..400 {
        let (w, _) = p.step(1.0, &Touch::default());
        if ms > 260 && ms < 395 {
            seen_soft_while_first_unfinished |= (w.stiff - 0.3).abs() < 1e-6;
        }
    }
    assert!(seen_soft_while_first_unfinished);
    // the baseline ignores per-stroke stiffness: it is always stiff
    let mut p = Player::new([0.0; DIMS], vec![soft], Style::STOP_GO);
    assert_eq!(p.step(1.0, &Touch::default()).0.stiff, 1.0);
}

#[test]
fn goals_added_later_are_played_after_the_ones_before() {
    let mut p = Player::new([0.0; DIMS], vec![goal(0.1, 300, false)], Style::FLUID);
    for _ in 0..200 {
        p.step(1.0, &Touch::default());
    }
    assert!(p.at_the_seam(), "a stroke that need not rest can be joined at 60 % of its time");
    p.append(&[goal(0.3, 300, true)]);
    let mut last = 0.0;
    for _ in 0..2000 {
        let (w, played) = p.step(1.0, &Touch::default());
        last = w.at[0];
        if played == Played::Done {
            break;
        }
    }
    assert!((last - 0.3).abs() < 1e-5);
}

#[test]
fn the_joint_law_is_a_spring_a_damper_and_a_cap() {
    let g = Gain { kp: 10.0, kd: 1.0 };
    // spring only
    assert!((impedance(g, 1.0, 0.1, 0.0, 0.0, 0.0, 0.0, 100.0) - 1.0).abs() < 1e-6);
    // softer by the stiffness, and the damper by its square root
    assert!((impedance(g, 0.25, 0.1, 0.0, 0.0, 0.0, 0.0, 100.0) - 0.25).abs() < 1e-6);
    assert!((impedance(g, 0.25, 0.0, 0.0, 0.0, 1.0, 0.0, 100.0) + 0.5).abs() < 1e-6);
    // what must be held up is added whatever the stiffness
    assert!((impedance(g, 0.0, 0.0, 0.0, 0.0, 0.0, 0.7, 100.0) - 0.7).abs() < 1e-6);
    // and nothing leaves the cap
    assert_eq!(impedance(g, 1.0, 100.0, 0.0, 0.0, 0.0, 0.0, 2.0), 2.0);
    assert_eq!(impedance(g, 1.0, -100.0, 0.0, 0.0, 0.0, 0.0, 2.0), -2.0);
}
